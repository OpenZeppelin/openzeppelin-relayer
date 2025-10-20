//! Solana transaction implementation
//!
//! This module provides the main SolanaRelayerTransaction struct and
//! implements the Transaction trait for Solana transactions.

use async_trait::async_trait;
use chrono::Utc;
use eyre::Result;
use solana_sdk::{pubkey::Pubkey, transaction::Transaction as SolanaTransaction};
use std::str::FromStr;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

use crate::{
    domain::{
        solana::{
            validate_prepared_transaction, SolanaTransactionValidationError,
            SolanaTransactionValidator,
        },
        transaction::{
            common::is_final_state,
            solana::utils::{
                build_transaction_from_instructions, decode_solana_transaction,
                decode_solana_transaction_from_string, is_resubmitable,
            },
            Transaction,
        },
    },
    jobs::{JobProducer, JobProducerTrait, TransactionSend},
    models::{
        produce_transaction_update_notification_payload, EncodedSerializedTransaction,
        NetworkTransactionData, NetworkTransactionRequest, RelayerRepoModel, SolanaTransactionData,
        TransactionError, TransactionRepoModel, TransactionStatus, TransactionUpdateRequest,
    },
    repositories::{
        RelayerRepository, RelayerRepositoryStorage, Repository, TransactionRepository,
        TransactionRepositoryStorage,
    },
    services::{
        SolanaProvider, SolanaProviderError, SolanaProviderTrait, SolanaSignTrait, SolanaSigner,
    },
    utils::base64_encode,
};

#[allow(dead_code)]
pub struct SolanaRelayerTransaction<P, RR, TR, J, S>
where
    P: SolanaProviderTrait,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    J: JobProducerTrait + Send + Sync + 'static,
    S: SolanaSignTrait + Send + Sync + 'static,
{
    relayer: RelayerRepoModel,
    relayer_repository: Arc<RR>,
    provider: Arc<P>,
    job_producer: Arc<J>,
    transaction_repository: Arc<TR>,
    signer: Arc<S>,
}

pub type DefaultSolanaTransaction = SolanaRelayerTransaction<
    SolanaProvider,
    RelayerRepositoryStorage,
    TransactionRepositoryStorage,
    JobProducer,
    SolanaSigner,
>;

#[allow(dead_code)]
impl<P, RR, TR, J, S> SolanaRelayerTransaction<P, RR, TR, J, S>
where
    P: SolanaProviderTrait,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    J: JobProducerTrait + Send + Sync + 'static,
    S: SolanaSignTrait + Send + Sync + 'static,
{
    pub fn new(
        relayer: RelayerRepoModel,
        relayer_repository: Arc<RR>,
        provider: Arc<P>,
        transaction_repository: Arc<TR>,
        job_producer: Arc<J>,
        signer: Arc<S>,
    ) -> Result<Self, TransactionError> {
        Ok(Self {
            relayer,
            relayer_repository,
            provider,
            transaction_repository,
            job_producer,
            signer,
        })
    }

    // Getter methods for status module access
    pub(super) fn provider(&self) -> &P {
        &self.provider
    }

    pub(super) fn transaction_repository(&self) -> &TR {
        &self.transaction_repository
    }

    pub(super) fn relayer(&self) -> &RelayerRepoModel {
        &self.relayer
    }

    pub(super) fn job_producer(&self) -> &J {
        &self.job_producer
    }

    pub(super) fn signer(&self) -> &S {
        &self.signer
    }

    /// Prepare transaction - validate and sign
    async fn prepare_transaction_impl(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        debug!(tx_id = %tx.id, status = ?tx.status, "preparing Solana transaction");

        // If transaction is not in Pending status, return Ok to avoid wasteful retries
        // (e.g., if it's already Sent, Failed, or in another state)
        if tx.status != TransactionStatus::Pending {
            debug!(
                tx_id = %tx.id,
                status = ?tx.status,
                expected_status = ?TransactionStatus::Pending,
                "transaction not in Pending status, skipping preparation"
            );
            return Ok(tx);
        }

        let solana_data = tx.network_data.get_solana_transaction_data()?;

        // Build or decode transaction based on input mode
        let mut transaction = if let Some(transaction_str) = &solana_data.transaction {
            // Transaction mode: decode pre-built transaction
            // Use the provided blockhash from user - resubmit logic will handle expiration if needed
            debug!(
                tx_id = %tx.id,
                "transaction mode: using pre-built transaction with provided blockhash"
            );
            decode_solana_transaction_from_string(transaction_str)?
        } else if let Some(instructions) = &solana_data.instructions {
            // Instructions mode: build transaction from instructions with fresh blockhash
            debug!(
                tx_id = %tx.id,
                "instructions mode: building transaction with fresh blockhash"
            );

            let payer = Pubkey::from_str(&self.relayer.address).map_err(|e| {
                TransactionError::ValidationError(format!("Invalid relayer address: {}", e))
            })?;

            // Fetch fresh blockhash for instructions mode
            let latest_blockhash = self.provider.get_latest_blockhash().await.map_err(|e| {
                TransactionError::UnexpectedError(format!("Failed to get latest blockhash: {}", e))
            })?;

            build_transaction_from_instructions(instructions, &payer, latest_blockhash)?
        } else {
            // Neither transaction nor instructions provided - permanent validation error
            let validation_error = TransactionError::ValidationError(
                "Must provide either transaction or instructions".to_string(),
            );

            let updated_tx = self
                .fail_transaction_with_notification(&tx, &validation_error)
                .await?;

            // Return Ok since transaction is in final Failed state - no retry needed
            return Ok(updated_tx);
        };

        // Validate transaction before signing
        // Distinguish between transient errors (RPC issues) and permanent errors (policy violations)
        if let Err(validation_error) = self.validate_transaction_impl(&transaction).await {
            // Special case: Expired blockhash for resubmittable transactions
            // For single-signer transactions, expired blockhash can be refreshed during resubmit
            // So we treat it as transient and let the resubmit flow handle it
            // For multi-signer transactions, the logic below will mark as failed (is_transient = false)
            if matches!(
                validation_error,
                TransactionError::SolanaValidation(
                    SolanaTransactionValidationError::ExpiredBlockhash(_)
                )
            ) && is_resubmitable(&transaction)
            {
                info!(
                    tx_id = %tx.id,
                    "expired blockhash detected for single-signer transaction, will retry (resubmit will refresh blockhash)"
                );
                return Err(validation_error);
            }

            // Determine if the error is transient
            let is_transient = validation_error.is_transient();

            if is_transient {
                warn!(
                    tx_id = %tx.id,
                    error = %validation_error,
                    "transient validation error (likely RPC/network issue), will retry"
                );
                return Err(validation_error);
            } else {
                // Permanent validation error (policy violation, insufficient balance, etc.) - mark as failed
                warn!(
                    tx_id = %tx.id,
                    error = %validation_error,
                    "permanent validation error, marking transaction as failed"
                );

                let updated_tx = self
                    .fail_transaction_with_notification(&tx, &validation_error)
                    .await?;

                // Return Ok since transaction is in final Failed state - no retry needed
                return Ok(updated_tx);
            }
        }

        // Sign transaction
        let signature = self
            .signer
            .sign(&transaction.message_data())
            .await
            .map_err(|e| TransactionError::SignerError(e.to_string()))?;

        transaction.signatures[0] = signature;

        // Update transaction with signature
        let update = TransactionUpdateRequest {
            status: Some(TransactionStatus::Sent),
            network_data: Some(NetworkTransactionData::Solana(SolanaTransactionData {
                signature: Some(signature.to_string()),
                transaction: Some(
                    EncodedSerializedTransaction::try_from(&transaction)
                        .map_err(|e| {
                            TransactionError::ValidationError(format!(
                                "Failed to encode transaction: {}",
                                e
                            ))
                        })?
                        .into_inner(),
                ),
                ..Default::default()
            })),
            ..Default::default()
        };

        let updated_tx = self
            .transaction_repository
            .partial_update(tx.id.clone(), update)
            .await?;

        // After preparing the transaction, produce a submit job to send it to the blockchain
        self.job_producer
            .produce_submit_transaction_job(
                TransactionSend::submit(updated_tx.id.clone(), updated_tx.relayer_id.clone()),
                None,
            )
            .await?;

        // Send notification as best-effort (errors logged but not propagated)
        if let Err(e) = self.send_transaction_update_notification(&updated_tx).await {
            error!(
                tx_id = %updated_tx.id,
                status = ?TransactionStatus::Sent,
                "sending transaction update notification failed after prepare: {:?}",
                e
            );
        }

        Ok(updated_tx)
    }

    /// Submit transaction to blockchain
    async fn submit_transaction_impl(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        debug!(tx_id = %tx.id, status = ?tx.status, "submitting Solana transaction to blockchain");

        // If transaction is not in expected status, return Ok to avoid wasteful retries
        // (e.g., if it's already in a final state like Failed, Confirmed, etc.)
        if is_final_state(&tx.status) {
            debug!(
                tx_id = %tx.id,
                status = ?tx.status,
                "transaction already in final state, skipping submission"
            );
            return Ok(tx);
        }

        if tx.status != TransactionStatus::Sent && tx.status != TransactionStatus::Submitted {
            debug!(
                tx_id = %tx.id,
                status = ?tx.status,
                "transaction not in expected status for submission, skipping"
            );
            return Ok(tx);
        }

        // Extract Solana transaction data and decode
        let solana_data = tx.network_data.get_solana_transaction_data()?;
        let transaction = decode_solana_transaction(&tx)?;

        // Send to blockchain
        match self.provider.send_transaction(&transaction).await {
            Ok(sig) => sig,
            Err(provider_error) => {
                // Special case: AlreadyProcessed means transaction is already on-chain
                if matches!(provider_error, SolanaProviderError::AlreadyProcessed(_)) {
                    debug!(
                        tx_id = %tx.id,
                        signature = ?solana_data.signature,
                        "transaction already processed on-chain"
                    );

                    // Transaction is already on-chain with existing signature.
                    // Return as-is - the status check job will query and update to the actual on-chain status.
                    return Ok(tx);
                }

                // Special case: BlockhashNotFound handling depends on signature requirements
                if matches!(provider_error, SolanaProviderError::BlockhashNotFound(_))
                    && is_resubmitable(&transaction)
                {
                    // Single-signer: Can update blockhash via resubmit
                    // Return Ok to allow status check to detect expiration and trigger resubmit
                    // The resubmit logic will fetch fresh blockhash, re-sign, and resubmit
                    debug!(
                        tx_id = %tx.id,
                        error = %provider_error,
                        "blockhash expired for single-signer transaction, status check will trigger resubmit"
                    );
                    return Ok(tx);
                }

                error!(
                    tx_id = %tx.id,
                    error = %provider_error,
                    "failed to send transaction to blockchain"
                );

                // Check if error is transient or permanent
                if provider_error.is_transient() {
                    // Transient error - propagate so job can retry
                    return Err(TransactionError::UnderlyingSolanaProvider(provider_error));
                } else {
                    // Non-transient error - mark as failed and send notification
                    let error = TransactionError::UnderlyingSolanaProvider(provider_error);
                    let updated_tx = self.fail_transaction_with_notification(&tx, &error).await?;

                    // Return Ok with failed transaction since it's in final state
                    return Ok(updated_tx);
                }
            }
        };

        debug!(tx_id = %tx.id, "transaction submitted successfully to blockchain");

        // Transaction is now on-chain - update status and timestamp
        // Append signature to hashes array to track attempts
        let signature_str = transaction.signatures[0].to_string();
        let mut updated_hashes = tx.hashes.clone();
        updated_hashes.push(signature_str.clone());

        let update = TransactionUpdateRequest {
            status: Some(TransactionStatus::Submitted),
            sent_at: Some(Utc::now().to_rfc3339()),
            hashes: Some(updated_hashes),
            ..Default::default()
        };

        let updated_tx = match self
            .transaction_repository
            .partial_update(tx.id.clone(), update)
            .await
        {
            Ok(tx) => tx,
            Err(e) => {
                error!(
                    error = %e,
                    tx_id = %tx.id,
                    "CRITICAL: transaction sent to blockchain but failed to update database - transaction may not be tracked correctly"
                );
                // Transaction is on-chain - don't propagate error to avoid wasteful retries
                // Return the original transaction data
                tx
            }
        };

        // Send notification as best-effort (errors logged but not propagated)
        if let Err(e) = self.send_transaction_update_notification(&updated_tx).await {
            error!(
                tx_id = %updated_tx.id,
                status = ?TransactionStatus::Submitted,
                "sending transaction update notification failed after submit: {:?}",
                e
            );
        }

        Ok(updated_tx)
    }

    /// Resubmit transaction
    async fn resubmit_transaction_impl(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        debug!(tx_id = %tx.id, "resubmitting Solana transaction");

        // Validate transaction is in correct status for resubmission
        if !matches!(
            tx.status,
            TransactionStatus::Sent | TransactionStatus::Submitted
        ) {
            warn!(
                tx_id = %tx.id,
                status = ?tx.status,
                "transaction not in expected status for resubmission, skipping"
            );
            return Ok(tx);
        }

        // Decode current transaction
        let mut transaction = decode_solana_transaction(&tx)?;

        info!(
            tx_id = %tx.id,
            old_blockhash = %transaction.message.recent_blockhash,
            "fetching fresh blockhash for resubmission"
        );

        // Fetch fresh blockhash
        let fresh_blockhash = self.provider.get_latest_blockhash().await.map_err(|e| {
            TransactionError::UnexpectedError(format!(
                "Failed to fetch fresh blockhash for resubmit: {}",
                e
            ))
        })?;

        // Update transaction with fresh blockhash
        transaction.message.recent_blockhash = fresh_blockhash;

        // Re-sign the transaction with the updated message
        let signature = self
            .signer
            .sign(&transaction.message_data())
            .await
            .map_err(|e| {
                TransactionError::UnexpectedError(format!("Failed to re-sign transaction: {}", e))
            })?;

        // Update transaction signature
        transaction.signatures[0] = signature;

        // Serialize transaction back to base64
        let serialized_tx = bincode::serialize(&transaction).map_err(|e| {
            TransactionError::UnexpectedError(format!("Failed to serialize transaction: {}", e))
        })?;

        let tx_base64 = base64_encode(&serialized_tx);
        let signature_str = signature.to_string();

        // Append new signature to hashes array to track resubmission attempts
        let mut updated_hashes = tx.hashes.clone();
        updated_hashes.push(signature_str.clone());

        // Update transaction data
        let solana_data = tx.network_data.get_solana_transaction_data()?;
        let updated_solana_data = SolanaTransactionData {
            transaction: Some(tx_base64),
            signature: Some(signature_str.clone()),
            ..solana_data
        };

        // Update in repository with Submitted status and new sent_at
        let update_request = TransactionUpdateRequest {
            status: Some(TransactionStatus::Submitted),
            network_data: Some(NetworkTransactionData::Solana(updated_solana_data)),
            sent_at: Some(Utc::now().to_rfc3339()),
            hashes: Some(updated_hashes),
            ..Default::default()
        };

        // Send resubmitted transaction to blockchain directly - this is the critical operation
        let was_already_processed = match self.provider.send_transaction(&transaction).await {
            Ok(sig) => {
                info!(
                    tx_id = %tx.id,
                    signature = %sig,
                    new_blockhash = %fresh_blockhash,
                    "transaction resubmitted successfully with fresh blockhash"
                );
                false
            }
            Err(e) => {
                // Special case: AlreadyProcessed means transaction is already on-chain
                if matches!(e, SolanaProviderError::AlreadyProcessed(_)) {
                    warn!(
                        tx_id = %tx.id,
                        error = %e,
                        "resubmission indicates transaction already on-chain - keeping original signature"
                    );
                    // Don't update with new signature - the original transaction is what's on-chain
                    true
                } else if e.is_transient() {
                    // Transient error (network, RPC) - return for retry
                    warn!(
                        tx_id = %tx.id,
                        error = %e,
                        "transient error during resubmission, will retry"
                    );
                    return Err(TransactionError::UnderlyingSolanaProvider(e));
                } else {
                    // Permanent error (invalid tx, insufficient funds) - mark as failed
                    warn!(
                        tx_id = %tx.id,
                        error = %e,
                        "permanent error during resubmission, marking transaction as failed"
                    );
                    let updated_tx = self
                        .fail_transaction_with_notification(
                            &tx,
                            &TransactionError::UnderlyingSolanaProvider(e),
                        )
                        .await?;
                    return Ok(updated_tx);
                }
            }
        };

        // If transaction was already processed, don't update anything - status check will handle it
        let updated_tx = if was_already_processed {
            // Transaction already on-chain - return as-is, status check job will update to Confirmed/Mined
            info!(
                tx_id = %tx.id,
                "transaction already on-chain, no update needed - status check will handle confirmation"
            );
            tx
        } else {
            // Transaction resubmitted successfully - update with new signature and blockhash
            let tx = match self
                .transaction_repository
                .partial_update(tx.id.clone(), update_request)
                .await
            {
                Ok(tx) => tx,
                Err(e) => {
                    error!(
                        error = %e,
                        tx_id = %tx.id,
                        "CRITICAL: resubmitted transaction sent to blockchain but failed to update database"
                    );
                    // Transaction is on-chain - return original tx data to avoid wasteful retries
                    tx
                }
            };

            info!(
                tx_id = %tx.id,
                new_signature = %signature_str,
                new_blockhash = %fresh_blockhash,
                "transaction resubmitted with fresh blockhash"
            );

            tx
        };

        Ok(updated_tx)
    }

    /// Helper method to send transaction update notification.
    ///
    /// This is a best-effort operation that logs errors but does not propagate them,
    /// as notification failures should not affect the transaction lifecycle.
    pub(super) async fn send_transaction_update_notification(
        &self,
        tx: &TransactionRepoModel,
    ) -> Result<(), eyre::Report> {
        if let Some(notification_id) = &self.relayer.notification_id {
            self.job_producer
                .produce_send_notification_job(
                    produce_transaction_update_notification_payload(notification_id, tx),
                    None,
                )
                .await?;
        }
        Ok(())
    }

    /// Marks a transaction as failed, updates the database, and sends notification.
    ///
    /// This is a convenience method that combines:
    /// 1. Marking transaction as Failed
    /// 2. Sending notification (best-effort, errors logged but not propagated)
    async fn fail_transaction_with_notification(
        &self,
        tx: &TransactionRepoModel,
        error: &TransactionError,
    ) -> Result<TransactionRepoModel, TransactionError> {
        let updated_tx = self.mark_transaction_as_failed(tx, error).await?;

        // Send notification as best-effort (errors logged but not propagated)
        if let Err(e) = self.send_transaction_update_notification(&updated_tx).await {
            error!(
                tx_id = %updated_tx.id,
                status = ?TransactionStatus::Failed,
                error = %error,
                notification_error = %e,
                "failed to send notification for failed transaction"
            );
        }

        Ok(updated_tx)
    }

    /// Marks a transaction as failed and updates the database.
    async fn mark_transaction_as_failed(
        &self,
        tx: &TransactionRepoModel,
        error: &TransactionError,
    ) -> Result<TransactionRepoModel, TransactionError> {
        warn!(
            tx_id = %tx.id,
            error = %error,
            "marking transaction as Failed"
        );

        let update = TransactionUpdateRequest {
            status: Some(TransactionStatus::Failed),
            status_reason: Some(error.to_string()),
            ..Default::default()
        };

        let updated_tx = self
            .transaction_repository
            .partial_update(tx.id.clone(), update)
            .await?;

        Ok(updated_tx)
    }

    async fn validate_transaction_impl(
        &self,
        tx: &SolanaTransaction,
    ) -> Result<(), TransactionError> {
        let policy = self.relayer.policies.get_solana_policy();
        let relayer_pubkey = Pubkey::from_str(&self.relayer.address).map_err(|e| {
            TransactionError::ValidationError(format!("Invalid relayer address: {}", e))
        })?;

        // Use comprehensive validation from shared domain module
        // This validates: fee payer, allowed/disallowed accounts, programs,
        // signatures, data size, blockhash, simulation, lamports/token transfers
        // Note: Using ? preserves SolanaTransactionValidationError type via #[from]
        validate_prepared_transaction(tx, &relayer_pubkey, &policy, self.provider.as_ref()).await?;

        // Additional fee and balance checks specific to transaction endpoint
        // (RPC methods handle this differently with user fee payments)
        let fee = self
            .provider
            .calculate_total_fee(&tx.message)
            .await
            .map_err(|e| {
                TransactionError::UnexpectedError(format!("Fee estimation failed: {}", e))
            })?;

        // Validate fee against max allowed fee policy
        // Note: Using ? preserves SolanaTransactionValidationError type via #[from]
        SolanaTransactionValidator::validate_max_fee(fee, &policy)?;

        // Validate relayer has sufficient balance (fee + min_balance)
        // Note: Using ? preserves SolanaTransactionValidationError type via #[from]
        SolanaTransactionValidator::validate_sufficient_relayer_balance(
            fee,
            &self.relayer.address,
            &policy,
            self.provider.as_ref(),
        )
        .await?;

        Ok(())
    }
}

#[async_trait]
impl<P, RR, TR, J, S> Transaction for SolanaRelayerTransaction<P, RR, TR, J, S>
where
    P: SolanaProviderTrait,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    J: JobProducerTrait + Send + Sync + 'static,
    S: SolanaSignTrait + Send + Sync + 'static,
{
    async fn prepare_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        self.prepare_transaction_impl(tx).await
    }

    async fn submit_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        self.submit_transaction_impl(tx).await
    }

    async fn resubmit_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        self.resubmit_transaction_impl(tx).await
    }

    /// Main entry point for transaction status handling
    async fn handle_transaction_status(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        self.handle_transaction_status_impl(tx).await
    }

    async fn cancel_transaction(
        &self,
        _tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        Err(TransactionError::NotSupported(
            "Transaction cancellation is not supported for Solana".to_string(),
        ))
    }

    async fn replace_transaction(
        &self,
        _old_tx: TransactionRepoModel,
        _new_tx_request: NetworkTransactionRequest,
    ) -> Result<TransactionRepoModel, TransactionError> {
        Err(TransactionError::NotSupported(
            "Transaction replacement is not supported for Solana".to_string(),
        ))
    }

    async fn sign_transaction(
        &self,
        _tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        Err(TransactionError::NotSupported(
            "Standalone transaction signing is not supported for Solana - signing happens during prepare_transaction".to_string(),
        ))
    }

    async fn validate_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<bool, TransactionError> {
        debug!(tx_id = %tx.id, "validating Solana transaction");

        // Decode transaction
        let transaction = decode_solana_transaction(&tx)?;

        // Run validation logic
        self.validate_transaction_impl(&transaction).await?;

        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        jobs::MockJobProducerTrait,
        repositories::{MockRelayerRepository, MockTransactionRepository},
        services::{MockSolanaProviderTrait, MockSolanaSignTrait},
        utils::mocks::mockutils::{create_mock_solana_relayer, create_mock_solana_transaction},
    };

    #[tokio::test]
    async fn test_solana_transaction_creation() {
        let relayer = create_mock_solana_relayer("test-solana-relayer".to_string(), false);
        let relayer_repository = Arc::new(MockRelayerRepository::new());
        let provider = Arc::new(MockSolanaProviderTrait::new());
        let transaction_repository = Arc::new(MockTransactionRepository::new());
        let job_producer = Arc::new(MockJobProducerTrait::new());
        let signer = Arc::new(MockSolanaSignTrait::new());

        let transaction = SolanaRelayerTransaction::new(
            relayer,
            relayer_repository,
            provider,
            transaction_repository,
            job_producer,
            signer,
        );

        assert!(transaction.is_ok());
    }

    #[tokio::test]
    async fn test_handle_transaction_status_calls_impl() {
        // Create test data
        let relayer = create_mock_solana_relayer("test-solana-relayer".to_string(), false);
        let relayer_repository = Arc::new(MockRelayerRepository::new());
        let provider = Arc::new(MockSolanaProviderTrait::new());
        let transaction_repository = Arc::new(MockTransactionRepository::new());
        let job_producer = Arc::new(MockJobProducerTrait::new());
        let signer = Arc::new(MockSolanaSignTrait::new());

        // Create test transaction (will be in Pending status by default)
        let test_tx = create_mock_solana_transaction();

        // Create transaction handler
        let transaction_handler = SolanaRelayerTransaction::new(
            relayer,
            relayer_repository,
            provider,
            transaction_repository,
            job_producer,
            signer,
        )
        .unwrap();

        // Call handle_transaction_status - with new implementation,
        // Pending transactions just return Ok without querying provider
        let result = transaction_handler
            .handle_transaction_status(test_tx.clone())
            .await;

        // Verify the result is Ok and transaction is unchanged
        assert!(result.is_ok());
        let returned_tx = result.unwrap();
        assert_eq!(returned_tx.id, test_tx.id);
        assert_eq!(returned_tx.status, test_tx.status);
    }
}
