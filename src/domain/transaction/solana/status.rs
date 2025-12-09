//! Solana transaction status handling implementation
//!
//! This module provides transaction status checking for Solana transactions,
//! including status updates, repository management, and webhook notifications.

use crate::constants::{
    MAXIMUM_SOLANA_TX_ATTEMPTS, SOLANA_DEFAULT_TX_VALID_TIMESPAN,
    SOLANA_MIN_AGE_FOR_RESUBMIT_CHECK_SECONDS, SOLANA_PENDING_RECOVERY_TRIGGER_SECONDS,
    SOLANA_PENDING_TIMEOUT_MINUTES, SOLANA_SENT_TIMEOUT_MINUTES,
};
use crate::models::{NetworkTransactionData, SolanaTransactionData};
use crate::services::provider::SolanaProviderError;
use chrono::{DateTime, Duration, Utc};
use solana_commitment_config::CommitmentConfig;
use solana_sdk::{signature::Signature, transaction::Transaction as SolanaTransaction};
use std::str::FromStr;
use tracing::{debug, error, info, warn};

use super::{utils::decode_solana_transaction, SolanaRelayerTransaction};
use crate::domain::transaction::common::is_final_state;
use crate::domain::transaction::solana::utils::{
    is_resubmitable, map_solana_status_to_transaction_status, too_many_solana_attempts,
};
use crate::{
    jobs::{JobProducerTrait, TransactionRequest, TransactionSend},
    models::{
        RelayerRepoModel, SolanaTransactionStatus, TransactionError, TransactionRepoModel,
        TransactionStatus, TransactionUpdateRequest,
    },
    repositories::{transaction::TransactionRepository, RelayerRepository, Repository},
    services::{provider::SolanaProviderTrait, signer::SolanaSignTrait},
};

impl<P, RR, TR, J, S> SolanaRelayerTransaction<P, RR, TR, J, S>
where
    P: SolanaProviderTrait + Send + Sync + 'static,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    J: JobProducerTrait + Send + Sync + 'static,
    S: SolanaSignTrait + Send + Sync + 'static,
{
    /// Main status handling method with error handling
    ///
    /// 1. Check transaction status (query chain or return current for Pending/Sent)
    /// 2. Reload transaction from DB if status changed (ensures fresh data)
    /// 3. Check if too early for resubmit checks (young transactions just update status)
    /// 4. Handle based on detected status (handlers update DB if needed)
    pub async fn handle_transaction_status_impl(
        &self,
        mut tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        debug!(tx_id = %tx.id, status = ?tx.status, "handling solana transaction status");

        // Early return if transaction is already in a final state
        if is_final_state(&tx.status) {
            debug!(status = ?tx.status, "transaction already in final state");
            return Ok(tx);
        }

        // Step 1: Check transaction status (query chain or return current)
        let detected_status = self.check_onchain_transaction_status(&tx).await?;

        // Reload transaction from DB if status changed
        // This ensures we have fresh data if check_transaction_status triggered a recovery
        // or any other update that modified the transaction in the database.
        if tx.status != detected_status {
            tx = self
                .transaction_repository()
                .get_by_id(tx.id.clone())
                .await?;
        }

        // Step 2: Handle based on detected status (handlers will update if needed)
        match detected_status {
            TransactionStatus::Pending => {
                // Pending transactions haven't been submitted yet - schedule request job if not expired
                self.handle_pending_status(tx).await
            }
            TransactionStatus::Sent | TransactionStatus::Submitted => {
                // Sent/Submitted transactions may need resubmission if blockhash expired
                self.handle_resubmit_or_expiration(tx).await
            }
            TransactionStatus::Mined
            | TransactionStatus::Confirmed
            | TransactionStatus::Failed
            | TransactionStatus::Canceled
            | TransactionStatus::Expired => {
                self.update_transaction_status_if_needed(tx, detected_status)
                    .await
            }
        }
    }

    /// Check transaction status from chain (or return current for Pending/Sent)
    ///
    /// Similar to EVM's check_transaction_status, this method:
    /// - Returns current status for Pending/Sent (no on-chain query needed)
    /// - Queries chain for Submitted/Mined and returns appropriate status
    async fn check_onchain_transaction_status(
        &self,
        tx: &TransactionRepoModel,
    ) -> Result<TransactionStatus, TransactionError> {
        // Early return for Pending/Sent - these are DB-only states
        match tx.status {
            TransactionStatus::Pending | TransactionStatus::Sent => {
                return Ok(tx.status.clone());
            }
            _ => {}
        }

        // For Submitted/Mined, query the chain
        let solana_data = tx.network_data.get_solana_transaction_data()?;
        let signature_str = solana_data.signature.as_ref().ok_or_else(|| {
            TransactionError::ValidationError("Transaction signature is missing".to_string())
        })?;

        let signature = Signature::from_str(signature_str).map_err(|e| {
            TransactionError::ValidationError(format!("Invalid signature format: {e}"))
        })?;

        // Query on-chain status
        match self.provider().get_transaction_status(&signature).await {
            Ok(solana_status) => {
                // Map Solana on-chain status to repository status
                Ok(map_solana_status_to_transaction_status(solana_status))
            }
            Err(e) => {
                // Transaction not found or error querying
                warn!(
                    tx_id = %tx.id,
                    signature = %signature_str,
                    error = %e,
                    "error getting transaction status from chain"
                );
                // Return current status (will be handled later for potential resubmit)
                Ok(tx.status.clone())
            }
        }
    }

    /// Update transaction status in DB and send notification (unconditionally)
    ///
    /// Optionally updates network_data along with status. This is useful when
    /// updating the signature field after finding a transaction on-chain.
    ///
    /// Used internally by update_transaction_status_if_needed and
    /// handle_resubmit_or_expiration
    async fn update_transaction_status_and_send_notification(
        &self,
        tx: TransactionRepoModel,
        new_status: TransactionStatus,
        network_data: Option<crate::models::NetworkTransactionData>,
    ) -> Result<TransactionRepoModel, TransactionError> {
        let update_request = TransactionUpdateRequest {
            status: Some(new_status.clone()),
            network_data,
            confirmed_at: if matches!(new_status, TransactionStatus::Confirmed) {
                Some(Utc::now().to_rfc3339())
            } else {
                None
            },
            ..Default::default()
        };

        // Update transaction in repository
        let updated_tx = self
            .transaction_repository()
            .partial_update(tx.id.clone(), update_request)
            .await
            .map_err(|e| TransactionError::UnexpectedError(e.to_string()))?;

        // Send webhook notification if relayer has notification configured
        // Best-effort operation - errors logged but not propagated
        if let Err(e) = self.send_transaction_update_notification(&updated_tx).await {
            error!(
                tx_id = %updated_tx.id,
                status = ?new_status,
                "sending transaction update notification failed: {:?}",
                e
            );
        }

        Ok(updated_tx)
    }

    /// Update transaction status in DB if status has changed
    ///
    /// Similar to EVM's update_transaction_status_if_needed pattern
    async fn update_transaction_status_if_needed(
        &self,
        tx: TransactionRepoModel,
        new_status: TransactionStatus,
    ) -> Result<TransactionRepoModel, TransactionError> {
        if tx.status != new_status {
            return self
                .update_transaction_status_and_send_notification(tx, new_status, None)
                .await;
        }
        Ok(tx)
    }

    /// Handle Pending status - check for expiration/timeout or schedule transaction request job
    ///
    /// Pending transactions haven't been submitted yet, so we should schedule a transaction
    /// request job to prepare and submit them, not a resubmit job.
    async fn handle_pending_status(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        // Step 1: Check if valid_until has expired
        if self.is_valid_until_expired(&tx) {
            info!(
                tx_id = %tx.id,
                valid_until = ?tx.valid_until,
                "pending transaction valid_until has expired"
            );
            return self
                .mark_as_expired(
                    tx,
                    "Transaction valid_until timestamp has expired".to_string(),
                )
                .await;
        }

        // Step 2: Check if transaction has exceeded pending timeout
        // Only schedule recovery job if transaction is stuck (similar to EVM pattern)
        if self.has_exceeded_timeout(&tx)? {
            warn!(
                tx_id = %tx.id,
                timeout_minutes = SOLANA_PENDING_TIMEOUT_MINUTES,
                "pending transaction has exceeded timeout, marking as failed"
            );
            return self
                .mark_as_failed(
                    tx,
                    format!(
                        "Transaction stuck in Pending status for more than {SOLANA_PENDING_TIMEOUT_MINUTES} minutes"
                    ),
                )
                .await;
        }

        // Step 3: Check if transaction is stuck (prepare job may have failed)
        // Only re-queue job if transaction age indicates it might be stuck
        let age = self.get_time_since_sent_or_created_at(&tx).ok_or_else(|| {
            TransactionError::UnexpectedError(
                "Both sent_at and created_at are missing or invalid".to_string(),
            )
        })?;

        // Use a recovery trigger timeout (e.g., 30 seconds)
        // This prevents scheduling a job on every 5-second status check
        if age.num_seconds() >= SOLANA_PENDING_RECOVERY_TRIGGER_SECONDS {
            info!(
                tx_id = %tx.id,
                age_seconds = age.num_seconds(),
                "pending transaction may be stuck, scheduling recovery job"
            );

            let transaction_request = TransactionRequest::new(tx.id.clone(), tx.relayer_id.clone());

            self.job_producer()
                .produce_transaction_request_job(transaction_request, None)
                .await
                .map_err(|e| {
                    TransactionError::UnexpectedError(format!(
                        "Failed to enqueue transaction request job: {e}"
                    ))
                })?;
        } else {
            debug!(
                tx_id = %tx.id,
                age_seconds = age.num_seconds(),
                "pending transaction too young for recovery check"
            );
        }

        Ok(tx)
    }

    /// Check if enough time has passed since sent_at (or created_at) to check for resubmit/expiration
    ///
    /// Falls back to created_at for Pending transactions where sent_at is not yet set.
    /// Returns None if both timestamps are missing or invalid.
    fn get_time_since_sent_or_created_at(&self, tx: &TransactionRepoModel) -> Option<Duration> {
        // Try sent_at first, fallback to created_at for Pending transactions
        let timestamp = tx.sent_at.as_ref().or(Some(&tx.created_at))?;
        match DateTime::parse_from_rfc3339(timestamp) {
            Ok(dt) => Some(Utc::now().signed_duration_since(dt.with_timezone(&Utc))),
            Err(e) => {
                warn!(tx_id = %tx.id, ts = %timestamp, error = %e, "failed to parse timestamp");
                None
            }
        }
    }

    /// Check if any previous signature from the transaction is already on-chain.
    ///
    /// This prevents double-execution by verifying that none of the previous
    /// submission attempts are already processed before resubmitting with a new blockhash.
    ///
    /// Returns:
    /// - `Ok(Some((signature, status)))` if a signature was found on-chain
    /// - `Ok(None)` if no signature was found on-chain
    ///
    /// Critical for handling race conditions where:
    /// - Transaction was sent but DB update failed
    /// - Transaction is in mempool when resubmit logic runs
    /// - RPC indexing lag causes signature lookup to fail temporarily
    async fn check_any_signature_on_chain(
        &self,
        tx: &TransactionRepoModel,
    ) -> Result<Option<(String, SolanaTransactionStatus)>, TransactionError> {
        // Check all previous signatures stored in hashes
        for (idx, sig_str) in tx.hashes.iter().enumerate() {
            let signature = match Signature::from_str(sig_str) {
                Ok(sig) => sig,
                Err(e) => {
                    warn!(
                        tx_id = %tx.id,
                        signature = %sig_str,
                        error = %e,
                        "invalid signature format in hashes, skipping"
                    );
                    continue;
                }
            };

            match self.provider().get_transaction_status(&signature).await {
                Ok(solana_status) => {
                    // Found on-chain! This signature was processed
                    info!(
                        tx_id = %tx.id,
                        signature = %sig_str,
                        signature_idx = idx,
                        on_chain_status = ?solana_status,
                        "found transaction on-chain with previous signature"
                    );
                    return Ok(Some((sig_str.clone(), solana_status)));
                }
                Err(e) => {
                    // Signature not found or RPC error - continue checking others
                    debug!(
                        tx_id = %tx.id,
                        signature = %sig_str,
                        signature_idx = idx,
                        error = %e,
                        "signature not found on-chain or RPC error"
                    );
                    continue;
                }
            }
        }

        // No signatures found on-chain
        Ok(None)
    }

    /// Check if the blockhash in the transaction is still valid
    ///
    /// Queries the chain to see if the blockhash is still recognized
    async fn is_blockhash_valid(
        &self,
        transaction: &SolanaTransaction,
    ) -> Result<bool, TransactionError> {
        let blockhash = transaction.message.recent_blockhash;

        match self
            .provider()
            .is_blockhash_valid(&blockhash, CommitmentConfig::confirmed())
            .await
        {
            Ok(is_valid) => Ok(is_valid),
            Err(e) => {
                // Check if blockhash not found
                if matches!(e, SolanaProviderError::BlockhashNotFound(_)) {
                    info!("blockhash not found on chain, treating as expired");
                    return Ok(false);
                }

                // Propagate the error so the job system can retry the status check later
                warn!(
                    error = %e,
                    "error checking blockhash validity, propagating error for retry"
                );
                Err(TransactionError::UnderlyingSolanaProvider(e))
            }
        }
    }

    /// Mark transaction as expired with appropriate reason
    async fn mark_as_expired(
        &self,
        tx: TransactionRepoModel,
        reason: String,
    ) -> Result<TransactionRepoModel, TransactionError> {
        warn!(tx_id = %tx.id, reason = %reason, "marking transaction as expired");

        let update_request = TransactionUpdateRequest {
            status: Some(TransactionStatus::Expired),
            status_reason: Some(reason),
            ..Default::default()
        };

        self.transaction_repository()
            .partial_update(tx.id.clone(), update_request)
            .await
            .map_err(|e| TransactionError::UnexpectedError(e.to_string()))
    }

    /// Mark transaction as failed with appropriate reason
    async fn mark_as_failed(
        &self,
        tx: TransactionRepoModel,
        reason: String,
    ) -> Result<TransactionRepoModel, TransactionError> {
        warn!(tx_id = %tx.id, reason = %reason, "marking transaction as failed");

        let update_request = TransactionUpdateRequest {
            status: Some(TransactionStatus::Failed),
            status_reason: Some(reason),
            ..Default::default()
        };

        self.transaction_repository()
            .partial_update(tx.id.clone(), update_request)
            .await
            .map_err(|e| TransactionError::UnexpectedError(e.to_string()))
    }

    /// Check if valid_until has expired
    ///
    /// This checks both:
    /// 1. User-provided valid_until (if present)
    /// 2. Default valid_until based on created_at + DEFAULT_TX_VALID_TIMESPAN
    fn is_valid_until_expired(&self, tx: &TransactionRepoModel) -> bool {
        // Check user-provided valid_until first
        if let Some(valid_until_str) = &tx.valid_until {
            if let Ok(valid_until) = DateTime::parse_from_rfc3339(valid_until_str) {
                return Utc::now() > valid_until.with_timezone(&Utc);
            }
        }

        // Fall back to default valid_until based on created_at
        if let Ok(created_at) = DateTime::parse_from_rfc3339(&tx.created_at) {
            let default_valid_until = created_at.with_timezone(&Utc)
                + Duration::milliseconds(SOLANA_DEFAULT_TX_VALID_TIMESPAN);
            return Utc::now() > default_valid_until;
        }

        // If we can't parse created_at, consider it not expired
        // (will be caught by other safety mechanisms)
        false
    }

    /// Check if transaction has exceeded timeout for its status
    fn has_exceeded_timeout(&self, tx: &TransactionRepoModel) -> Result<bool, TransactionError> {
        let age = self.get_time_since_sent_or_created_at(tx).ok_or_else(|| {
            TransactionError::UnexpectedError(
                "Both sent_at and created_at are missing or invalid".to_string(),
            )
        })?;

        let timeout = match tx.status {
            TransactionStatus::Pending => Duration::minutes(SOLANA_PENDING_TIMEOUT_MINUTES),
            TransactionStatus::Sent => Duration::minutes(SOLANA_SENT_TIMEOUT_MINUTES),
            // Submitted status uses attempt-based limiting, not time-based timeout
            _ => return Ok(false), // No timeout for other statuses
        };

        Ok(age >= timeout)
    }

    /// Handle resubmit or expiration logic based on blockhash validity
    ///
    /// This method performs the following steps:
    /// 1. Checks if the transaction's `valid_until` timestamp has expired.
    /// 2. Verifies if the transaction has exceeded status-based timeouts or attempt limits.
    /// 3. Ensures enough time has passed since `sent_at` or `created_at` for resubmission checks.
    /// 4. Checks if any previous signatures are already on-chain to prevent double-execution.
    /// 5. Validates the blockhash and schedules a resubmit job if expired and resubmitable.
    /// 6. Marks the transaction as expired or failed if resubmission is not possible.
    ///
    /// Returns the updated transaction or an error if the operation fails.
    async fn handle_resubmit_or_expiration(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        // Step 1: Check if valid_until has expired
        if self.is_valid_until_expired(&tx) {
            info!(
                tx_id = %tx.id,
                valid_until = ?tx.valid_until,
                "transaction valid_until has expired"
            );
            return self
                .mark_as_expired(
                    tx,
                    "Transaction valid_until timestamp has expired".to_string(),
                )
                .await;
        }

        // Step 2: Check if transaction has exceeded timeout or attempt limit
        if tx.status == TransactionStatus::Submitted {
            // For Submitted status, use attempt-based limiting instead of timeout
            if too_many_solana_attempts(&tx) {
                let attempt_count = tx.hashes.len();
                warn!(
                    tx_id = %tx.id,
                    attempt_count = attempt_count,
                    max_attempts = MAXIMUM_SOLANA_TX_ATTEMPTS,
                    "transaction has exceeded maximum resubmission attempts"
                );
                return self
                    .mark_as_failed(
                        tx,
                        format!(
                            "Transaction exceeded maximum resubmission attempts ({attempt_count} > {MAXIMUM_SOLANA_TX_ATTEMPTS})"
                        ),
                    )
                    .await;
            }
        } else if self.has_exceeded_timeout(&tx)? {
            // For other statuses (Pending, Sent), use time-based timeout
            let timeout_minutes = match tx.status {
                TransactionStatus::Pending => SOLANA_PENDING_TIMEOUT_MINUTES,
                TransactionStatus::Sent => SOLANA_SENT_TIMEOUT_MINUTES,
                _ => 0,
            };
            let status = tx.status.clone();
            warn!(
                tx_id = %tx.id,
                status = ?status,
                timeout_minutes = timeout_minutes,
                "transaction has exceeded timeout for status"
            );
            return self
                .mark_as_failed(
                    tx,
                    format!(
                        "Transaction stuck in {status:?} status for more than {timeout_minutes} minutes"
                    ),
                )
                .await;
        }

        // Step 3: Check if enough time has passed for blockhash check
        let time_since_sent = match self.get_time_since_sent_or_created_at(&tx) {
            Some(duration) => duration,
            None => {
                debug!(tx_id = %tx.id, "both sent_at and created_at are missing or invalid, skipping resubmit check");
                return Ok(tx);
            }
        };

        if time_since_sent.num_seconds() < SOLANA_MIN_AGE_FOR_RESUBMIT_CHECK_SECONDS {
            debug!(
                tx_id = %tx.id,
                time_since_sent_secs = time_since_sent.num_seconds(),
                min_age = SOLANA_MIN_AGE_FOR_RESUBMIT_CHECK_SECONDS,
                "transaction too young for blockhash expiration check"
            );
            return Ok(tx);
        }

        // Step 4: Check if any previous signature is already on-chain
        // This prevents double-execution if:
        // - Transaction was sent but DB update failed
        // - Transaction is still in mempool/processing
        // - RPC had temporary indexing lag
        // - Jobs timeouts causing double-execution
        if let Some((found_signature, solana_status)) =
            self.check_any_signature_on_chain(&tx).await?
        {
            info!(
                tx_id = %tx.id,
                signature = %found_signature,
                on_chain_status = ?solana_status,
                "transaction found on-chain with previous signature, updating to final state"
            );

            // Map Solana on-chain status to repository status
            let new_status = map_solana_status_to_transaction_status(solana_status);

            // Update transaction with correct signature and status
            let solana_data = tx.network_data.get_solana_transaction_data()?;
            let updated_solana_data = SolanaTransactionData {
                signature: Some(found_signature),
                ..solana_data
            };
            let updated_network_data = NetworkTransactionData::Solana(updated_solana_data);

            // Update status, signature, and send notification using shared method
            return self
                .update_transaction_status_and_send_notification(
                    tx,
                    new_status,
                    Some(updated_network_data),
                )
                .await;
        }

        // Step 5: Decode transaction to extract blockhash
        let transaction = decode_solana_transaction(&tx)?;

        // Step 6: Check if blockhash is expired
        let blockhash_valid = self.is_blockhash_valid(&transaction).await?;

        if blockhash_valid {
            debug!(
                tx_id = %tx.id,
                "blockhash still valid, no action needed"
            );
            return Ok(tx);
        }

        info!(
            tx_id = %tx.id,
            "blockhash has expired, checking if transaction can be resubmitted"
        );

        // Step 7: Check if transaction can be resubmitted
        if is_resubmitable(&transaction) {
            info!(
                tx_id = %tx.id,
                "transaction is resubmitable, enqueuing resubmit job"
            );

            // Schedule resubmit job
            self.job_producer()
                .produce_submit_transaction_job(
                    TransactionSend::resubmit(tx.id.clone(), tx.relayer_id.clone()),
                    None,
                )
                .await
                .map_err(|e| {
                    TransactionError::UnexpectedError(format!(
                        "Failed to enqueue resubmit job: {e}"
                    ))
                })?;

            info!(tx_id = %tx.id, "resubmit job enqueued successfully");
            Ok(tx)
        } else {
            // Multi-signature transaction cannot be resubmitted by relayer alone
            warn!(
                tx_id = %tx.id,
                num_signatures = transaction.message.header.num_required_signatures,
                "transaction has expired blockhash but cannot be resubmitted (multi-sig)"
            );

            self.mark_as_expired(
                tx,
                format!(
                    "Blockhash expired and transaction requires {} signatures (cannot resubmit)",
                    transaction.message.header.num_required_signatures
                ),
            )
            .await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        jobs::{MockJobProducerTrait, TransactionCommand},
        models::{NetworkTransactionData, SolanaTransactionData},
        repositories::{MockRelayerRepository, MockTransactionRepository},
        services::{
            provider::{MockSolanaProviderTrait, SolanaProviderError},
            signer::MockSolanaSignTrait,
        },
        utils::{
            base64_encode,
            mocks::mockutils::{create_mock_solana_relayer, create_mock_solana_transaction},
        },
    };
    use eyre::Result;
    use mockall::predicate::*;
    use solana_sdk::{hash::Hash, message::Message, pubkey::Pubkey};
    use solana_system_interface::instruction as system_instruction;
    use std::sync::Arc;

    // Helper to create a transaction with a specific status and optional signature
    fn create_tx_with_signature(
        status: TransactionStatus,
        signature: Option<&str>,
    ) -> TransactionRepoModel {
        let mut tx = create_mock_solana_transaction();
        tx.status = status;
        if let Some(sig) = signature {
            tx.network_data = NetworkTransactionData::Solana(SolanaTransactionData {
                transaction: Some("test".to_string()),
                instructions: None,
                signature: Some(sig.to_string()),
            });
        }
        tx
    }

    #[tokio::test]
    async fn test_handle_status_already_final() {
        let provider = MockSolanaProviderTrait::new();
        let relayer_repo = Arc::new(MockRelayerRepository::new());
        let tx_repo = Arc::new(MockTransactionRepository::new());
        let job_producer = Arc::new(MockJobProducerTrait::new());
        let relayer = create_mock_solana_relayer("test-relayer".to_string(), false);

        let handler = SolanaRelayerTransaction::new(
            relayer,
            relayer_repo,
            Arc::new(provider),
            tx_repo,
            job_producer,
            Arc::new(MockSolanaSignTrait::new()),
        )
        .unwrap();

        // Test with Confirmed status
        let tx_confirmed = create_tx_with_signature(TransactionStatus::Confirmed, None);
        let result = handler
            .handle_transaction_status_impl(tx_confirmed.clone())
            .await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().id, tx_confirmed.id);

        // Test with Failed status
        let tx_failed = create_tx_with_signature(TransactionStatus::Failed, None);
        let result = handler
            .handle_transaction_status_impl(tx_failed.clone())
            .await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().id, tx_failed.id);

        // Test with Expired status
        let tx_expired = create_tx_with_signature(TransactionStatus::Expired, None);
        let result = handler
            .handle_transaction_status_impl(tx_expired.clone())
            .await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().id, tx_expired.id);
    }

    #[tokio::test]
    async fn test_handle_status_processed() -> Result<()> {
        let mut provider = MockSolanaProviderTrait::new();
        let relayer_repo = Arc::new(MockRelayerRepository::new());
        let mut tx_repo = MockTransactionRepository::new();
        let job_producer = MockJobProducerTrait::new();

        let signature_str =
            "4XFPmbPT4TRchFWNmQD2N8BhjxJQKqYdXWQG7kJJtxCBZ8Y9WtNDoPAwQaHFYnVynCjMVyF9TCMrpPFkEpG7LpZr";
        // Start with Submitted status
        let tx = create_tx_with_signature(TransactionStatus::Submitted, Some(signature_str));

        // check_transaction_status will query the chain
        provider
            .expect_get_transaction_status()
            .with(eq(Signature::from_str(signature_str)?))
            .times(1)
            .returning(|_| Box::pin(async { Ok(SolanaTransactionStatus::Processed) }));

        let tx_id = tx.id.clone();
        let tx_id_clone = tx_id.clone();

        // Expect get_by_id call when status changes (to reload fresh data)
        tx_repo
            .expect_get_by_id()
            .with(eq(tx_id.clone()))
            .times(1)
            .returning(move |_| {
                Ok(create_tx_with_signature(
                    TransactionStatus::Submitted, // Return with original status before update
                    Some(signature_str),
                ))
            });

        // Expect status update from Submitted to Mined (Processed maps to Mined)
        tx_repo
            .expect_partial_update()
            .withf(move |tx_id_param, update_req| {
                tx_id_param == &tx_id_clone && update_req.status == Some(TransactionStatus::Mined)
            })
            .times(1)
            .returning(move |_, _| {
                Ok(create_tx_with_signature(
                    TransactionStatus::Mined,
                    Some(signature_str),
                ))
            });

        let handler = SolanaRelayerTransaction::new(
            create_mock_solana_relayer("test-relayer".to_string(), false),
            relayer_repo,
            Arc::new(provider),
            Arc::new(tx_repo),
            Arc::new(job_producer),
            Arc::new(MockSolanaSignTrait::new()),
        )?;

        let result = handler.handle_transaction_status_impl(tx.clone()).await;

        assert!(result.is_ok());
        let updated_tx = result.unwrap();
        assert_eq!(updated_tx.id, tx.id);
        // Status should be upgraded to Mined
        assert_eq!(updated_tx.status, TransactionStatus::Mined);
        Ok(())
    }

    #[tokio::test]
    async fn test_handle_status_confirmed() -> Result<()> {
        let mut provider = MockSolanaProviderTrait::new();
        let relayer_repo = Arc::new(MockRelayerRepository::new());
        let mut tx_repo = MockTransactionRepository::new();
        let job_producer = MockJobProducerTrait::new();

        let signature_str =
            "4XFPmbPT4TRchFWNmQD2N8BhjxJQKqYdXWQG7kJJtxCBZ8Y9WtNDoPAwQaHFYnVynCjMVyF9TCMrpPFkEpG7LpZr";
        let tx = create_tx_with_signature(TransactionStatus::Submitted, Some(signature_str));

        provider
            .expect_get_transaction_status()
            .with(eq(Signature::from_str(signature_str)?))
            .times(1)
            .returning(|_| Box::pin(async { Ok(SolanaTransactionStatus::Confirmed) }));

        let tx_id = tx.id.clone();
        let tx_id_clone = tx_id.clone();

        // Expect get_by_id call when status changes
        tx_repo
            .expect_get_by_id()
            .with(eq(tx_id.clone()))
            .times(1)
            .returning(move |_| {
                Ok(create_tx_with_signature(
                    TransactionStatus::Submitted,
                    Some(signature_str),
                ))
            });

        tx_repo
            .expect_partial_update()
            .withf(move |tx_id_param, update_req| {
                tx_id_param == &tx_id_clone && update_req.status == Some(TransactionStatus::Mined)
            })
            .times(1)
            .returning(move |_, _| {
                Ok(create_tx_with_signature(
                    TransactionStatus::Mined,
                    Some(signature_str),
                ))
            });

        let handler = SolanaRelayerTransaction::new(
            create_mock_solana_relayer("test-relayer".to_string(), false),
            relayer_repo,
            Arc::new(provider),
            Arc::new(tx_repo),
            Arc::new(job_producer),
            Arc::new(MockSolanaSignTrait::new()),
        )?;

        let result = handler.handle_transaction_status_impl(tx.clone()).await;

        assert!(result.is_ok());
        let updated_tx = result.unwrap();
        assert_eq!(updated_tx.id, tx.id);
        assert_eq!(updated_tx.status, TransactionStatus::Mined);
        Ok(())
    }

    #[tokio::test]
    async fn test_handle_status_finalized() -> Result<()> {
        let mut provider = MockSolanaProviderTrait::new();
        let relayer_repo = Arc::new(MockRelayerRepository::new());
        let mut tx_repo = MockTransactionRepository::new();
        let job_producer = MockJobProducerTrait::new();

        let signature_str =
            "4XFPmbPT4TRchFWNmQD2N8BhjxJQKqYdXWQG7kJJtxCBZ8Y9WtNDoPAwQaHFYnVynCjMVyF9TCMrpPFkEpG7LpZr";
        let tx = create_tx_with_signature(TransactionStatus::Mined, Some(signature_str));

        provider
            .expect_get_transaction_status()
            .with(eq(Signature::from_str(signature_str)?))
            .times(1)
            .returning(|_| Box::pin(async { Ok(SolanaTransactionStatus::Finalized) }));

        let tx_id = tx.id.clone();
        let tx_id_clone = tx_id.clone();

        // Expect get_by_id call when status changes
        tx_repo
            .expect_get_by_id()
            .with(eq(tx_id.clone()))
            .times(1)
            .returning(move |_| {
                Ok(create_tx_with_signature(
                    TransactionStatus::Mined,
                    Some(signature_str),
                ))
            });

        tx_repo
            .expect_partial_update()
            .withf(move |tx_id_param, update_req| {
                tx_id_param == &tx_id_clone
                    && update_req.status == Some(TransactionStatus::Confirmed)
            })
            .times(1)
            .returning(move |_, _| {
                Ok(create_tx_with_signature(
                    TransactionStatus::Confirmed,
                    Some(signature_str),
                ))
            });

        let handler = SolanaRelayerTransaction::new(
            create_mock_solana_relayer("test-relayer".to_string(), false),
            relayer_repo,
            Arc::new(provider),
            Arc::new(tx_repo),
            Arc::new(job_producer),
            Arc::new(MockSolanaSignTrait::new()),
        )?;

        let result = handler.handle_transaction_status_impl(tx.clone()).await;

        assert!(result.is_ok());
        let updated_tx = result.unwrap();
        assert_eq!(updated_tx.id, tx.id);
        assert_eq!(updated_tx.status, TransactionStatus::Confirmed);
        Ok(())
    }

    #[tokio::test]
    async fn test_handle_status_provider_error() -> Result<()> {
        let mut provider = MockSolanaProviderTrait::new();
        let relayer_repo = Arc::new(MockRelayerRepository::new());
        let tx_repo = Arc::new(MockTransactionRepository::new());
        let job_producer = MockJobProducerTrait::new();

        let signature_str = "4XFPmbPT4TRchFWNmQD2N8BhjxJQKqYdXWQG7kJJtxCBZ8Y9WtNDoPAwQaHFYnVynCjMVyF9TCMrpPFkEpG7LpZr";
        // Use Submitted status so check_transaction_status() queries provider
        let tx = create_tx_with_signature(TransactionStatus::Submitted, Some(signature_str));
        let error_message = "Provider is down";

        // check_transaction_status will query the provider and get an error
        // It will return the current status (Submitted)
        provider
            .expect_get_transaction_status()
            .with(eq(Signature::from_str(signature_str)?))
            .times(1)
            .returning(move |_| {
                Box::pin(async { Err(SolanaProviderError::RpcError(error_message.to_string())) })
            });

        // No DB update expected since status doesn't change
        // No need to expect manual rescheduling - the job system handles retries

        let handler = SolanaRelayerTransaction::new(
            create_mock_solana_relayer("test-relayer".to_string(), false),
            relayer_repo,
            Arc::new(provider),
            tx_repo,
            Arc::new(job_producer),
            Arc::new(MockSolanaSignTrait::new()),
        )?;

        let result = handler.handle_transaction_status_impl(tx.clone()).await;

        // Provider error in check_transaction_status returns current status
        // Status unchanged, so no DB update, handler just returns Ok(tx)
        assert!(result.is_ok());
        let updated_tx = result.unwrap();
        assert_eq!(updated_tx.status, TransactionStatus::Submitted); // Status unchanged
        Ok(())
    }

    #[tokio::test]
    async fn test_handle_status_failed() -> Result<()> {
        let mut provider = MockSolanaProviderTrait::new();
        let relayer_repo = Arc::new(MockRelayerRepository::new());
        let mut tx_repo = MockTransactionRepository::new();
        let job_producer = MockJobProducerTrait::new();

        let signature_str =
            "4XFPmbPT4TRchFWNmQD2N8BhjxJQKqYdXWQG7kJJtxCBZ8Y9WtNDoPAwQaHFYnVynCjMVyF9TCMrpPFkEpG7LpZr";
        let tx = create_tx_with_signature(TransactionStatus::Submitted, Some(signature_str));

        provider
            .expect_get_transaction_status()
            .with(eq(Signature::from_str(signature_str)?))
            .times(1)
            .returning(|_| Box::pin(async { Ok(SolanaTransactionStatus::Failed) }));

        let tx_id = tx.id.clone();
        let tx_id_clone = tx_id.clone();

        // Expect get_by_id call when status changes
        tx_repo
            .expect_get_by_id()
            .with(eq(tx_id.clone()))
            .times(1)
            .returning(move |_| {
                Ok(create_tx_with_signature(
                    TransactionStatus::Submitted,
                    Some(signature_str),
                ))
            });

        tx_repo
            .expect_partial_update()
            .withf(move |tx_id_param, update_req| {
                tx_id_param == &tx_id_clone && update_req.status == Some(TransactionStatus::Failed)
            })
            .times(1)
            .returning(move |_, _| {
                Ok(create_tx_with_signature(
                    TransactionStatus::Failed,
                    Some(signature_str),
                ))
            });

        let handler = SolanaRelayerTransaction::new(
            create_mock_solana_relayer("test-relayer".to_string(), false),
            relayer_repo,
            Arc::new(provider),
            Arc::new(tx_repo),
            Arc::new(job_producer),
            Arc::new(MockSolanaSignTrait::new()),
        )?;

        let result = handler.handle_transaction_status_impl(tx.clone()).await;

        assert!(result.is_ok());
        let updated_tx = result.unwrap();
        assert_eq!(updated_tx.id, tx.id);
        assert_eq!(updated_tx.status, TransactionStatus::Failed);
        Ok(())
    }

    #[tokio::test]
    async fn test_default_valid_until_expired() -> Result<()> {
        let provider = MockSolanaProviderTrait::new();
        let relayer_repo = Arc::new(MockRelayerRepository::new());
        let mut tx_repo = MockTransactionRepository::new();
        let job_producer = MockJobProducerTrait::new();

        // Create PENDING transaction with created_at older than SOLANA_DEFAULT_TX_VALID_TIMESPAN
        let old_created_at = (Utc::now()
            - Duration::milliseconds(SOLANA_DEFAULT_TX_VALID_TIMESPAN + 60000))
        .to_rfc3339();
        let mut tx = create_tx_with_signature(TransactionStatus::Pending, None);
        tx.created_at = old_created_at;
        tx.valid_until = None; // No user-provided valid_until

        let tx_id = tx.id.clone();

        // Should mark as expired
        tx_repo
            .expect_partial_update()
            .withf(move |tx_id_param, update_req| {
                tx_id_param == &tx_id && update_req.status == Some(TransactionStatus::Expired)
            })
            .times(1)
            .returning(move |_, _| {
                let mut expired_tx = create_tx_with_signature(TransactionStatus::Expired, None);
                expired_tx.status = TransactionStatus::Expired;
                Ok(expired_tx)
            });

        let handler = SolanaRelayerTransaction::new(
            create_mock_solana_relayer("test-relayer".to_string(), false),
            relayer_repo,
            Arc::new(provider),
            Arc::new(tx_repo),
            Arc::new(job_producer),
            Arc::new(MockSolanaSignTrait::new()),
        )?;

        let result = handler.handle_transaction_status_impl(tx).await;

        assert!(result.is_ok());
        let updated_tx = result.unwrap();
        assert_eq!(updated_tx.status, TransactionStatus::Expired);
        Ok(())
    }

    #[tokio::test]
    async fn test_default_valid_until_not_expired() -> Result<()> {
        let mut provider = MockSolanaProviderTrait::new();
        let relayer_repo = Arc::new(MockRelayerRepository::new());
        let mut tx_repo = MockTransactionRepository::new();
        let job_producer = MockJobProducerTrait::new();

        // Create transaction with created_at within SOLANA_DEFAULT_TX_VALID_TIMESPAN
        let recent_created_at = (Utc::now()
            - Duration::milliseconds(SOLANA_DEFAULT_TX_VALID_TIMESPAN - 60000))
        .to_rfc3339();
        let signature_str =
            "4XFPmbPT4TRchFWNmQD2N8BhjxJQKqYdXWQG7kJJtxCBZ8Y9WtNDoPAwQaHFYnVynCjMVyF9TCMrpPFkEpG7LpZr";
        let mut tx = create_tx_with_signature(TransactionStatus::Submitted, Some(signature_str));
        tx.created_at = recent_created_at.clone();
        tx.valid_until = None; // No user-provided valid_until

        let tx_id = tx.id.clone();
        let tx_id_clone = tx_id.clone();
        let recent_created_at_clone = recent_created_at.clone();

        // Mock provider to return processed status
        provider
            .expect_get_transaction_status()
            .with(eq(Signature::from_str(signature_str)?))
            .times(1)
            .returning(|_| Box::pin(async { Ok(SolanaTransactionStatus::Processed) }));

        // Expect get_by_id call when status changes
        tx_repo
            .expect_get_by_id()
            .with(eq(tx_id.clone()))
            .times(1)
            .returning(move |_| {
                let mut tx =
                    create_tx_with_signature(TransactionStatus::Submitted, Some(signature_str));
                tx.created_at = recent_created_at_clone.clone();
                tx.valid_until = None;
                Ok(tx)
            });

        // Expect status update from Submitted to Mined (Processed maps to Mined)
        tx_repo
            .expect_partial_update()
            .withf(move |tx_id_param, update_req| {
                tx_id_param == &tx_id_clone && update_req.status == Some(TransactionStatus::Mined)
            })
            .times(1)
            .returning(move |_, _| {
                Ok(create_tx_with_signature(
                    TransactionStatus::Mined,
                    Some(signature_str),
                ))
            });

        let handler = SolanaRelayerTransaction::new(
            create_mock_solana_relayer("test-relayer".to_string(), false),
            relayer_repo,
            Arc::new(provider),
            Arc::new(tx_repo),
            Arc::new(job_producer),
            Arc::new(MockSolanaSignTrait::new()),
        )?;

        let result = handler.handle_transaction_status_impl(tx.clone()).await;

        assert!(result.is_ok());
        let updated_tx = result.unwrap();
        // Should not be expired since within default timespan, status changes to Mined
        assert_eq!(updated_tx.status, TransactionStatus::Mined);
        Ok(())
    }

    #[tokio::test]
    async fn test_too_many_resubmission_attempts() -> Result<()> {
        let mut provider = MockSolanaProviderTrait::new();
        let relayer_repo = Arc::new(MockRelayerRepository::new());
        let mut tx_repo = MockTransactionRepository::new();
        let job_producer = MockJobProducerTrait::new();

        // Create transaction with too many signatures (attempts exceeded)
        let signature_str =
            "4XFPmbPT4TRchFWNmQD2N8BhjxJQKqYdXWQG7kJJtxCBZ8Y9WtNDoPAwQaHFYnVynCjMVyF9TCMrpPFkEpG7LpZr";
        let mut tx = create_tx_with_signature(TransactionStatus::Submitted, Some(signature_str));
        tx.hashes = vec!["sig".to_string(); MAXIMUM_SOLANA_TX_ATTEMPTS + 1];
        tx.sent_at = Some(Utc::now().to_rfc3339()); // Ensure sent_at is set

        let tx_id = tx.id.clone();

        // Mock provider call - return error to skip status update, go straight to resubmit check
        provider
            .expect_get_transaction_status()
            .with(eq(Signature::from_str(signature_str)?))
            .times(1)
            .returning(|_| {
                Box::pin(async {
                    Err(crate::services::provider::SolanaProviderError::RpcError(
                        "test error".to_string(),
                    ))
                })
            });

        // Should mark as failed due to too many attempts (happens after status check)
        tx_repo
            .expect_partial_update()
            .withf(move |tx_id_param, update_req| {
                tx_id_param == &tx_id && update_req.status == Some(TransactionStatus::Failed)
            })
            .times(1)
            .returning(move |_, _| {
                let mut failed_tx = create_tx_with_signature(TransactionStatus::Failed, None);
                failed_tx.status = TransactionStatus::Failed;
                Ok(failed_tx)
            });

        let handler = SolanaRelayerTransaction::new(
            create_mock_solana_relayer("test-relayer".to_string(), false),
            relayer_repo,
            Arc::new(provider),
            Arc::new(tx_repo),
            Arc::new(job_producer),
            Arc::new(MockSolanaSignTrait::new()),
        )?;

        let result = handler.handle_transaction_status_impl(tx).await;

        assert!(result.is_ok());
        let updated_tx = result.unwrap();
        assert_eq!(updated_tx.status, TransactionStatus::Failed);
        Ok(())
    }

    #[tokio::test]
    async fn test_handle_pending_status_schedules_recovery_job() -> Result<()> {
        let provider = MockSolanaProviderTrait::new();
        let relayer_repo = Arc::new(MockRelayerRepository::new());
        let tx_repo = Arc::new(MockTransactionRepository::new());
        let mut job_producer = MockJobProducerTrait::new();

        // Create transaction that's been pending long enough to trigger recovery
        let mut tx = create_tx_with_signature(TransactionStatus::Pending, None);
        tx.created_at = (Utc::now()
            - Duration::seconds(SOLANA_PENDING_RECOVERY_TRIGGER_SECONDS + 10))
        .to_rfc3339();

        let tx_id = tx.id.clone();

        // Expect transaction request job to be produced
        job_producer
            .expect_produce_transaction_request_job()
            .withf(move |job, _delay| job.transaction_id == tx_id)
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let handler = SolanaRelayerTransaction::new(
            create_mock_solana_relayer("test-relayer".to_string(), false),
            relayer_repo,
            Arc::new(provider),
            tx_repo,
            Arc::new(job_producer),
            Arc::new(MockSolanaSignTrait::new()),
        )?;

        let result = handler.handle_pending_status(tx.clone()).await;

        assert!(result.is_ok());
        let returned_tx = result.unwrap();
        assert_eq!(returned_tx.status, TransactionStatus::Pending); // Status unchanged
        Ok(())
    }

    #[tokio::test]
    async fn test_handle_pending_status_too_young() -> Result<()> {
        let provider = MockSolanaProviderTrait::new();
        let relayer_repo = Arc::new(MockRelayerRepository::new());
        let tx_repo = Arc::new(MockTransactionRepository::new());
        let job_producer = Arc::new(MockJobProducerTrait::new());

        // Create transaction that's too young for recovery
        let mut tx = create_tx_with_signature(TransactionStatus::Pending, None);
        tx.created_at = (Utc::now()
            - Duration::seconds(SOLANA_PENDING_RECOVERY_TRIGGER_SECONDS - 10))
        .to_rfc3339();

        let handler = SolanaRelayerTransaction::new(
            create_mock_solana_relayer("test-relayer".to_string(), false),
            relayer_repo,
            Arc::new(provider),
            tx_repo,
            job_producer,
            Arc::new(MockSolanaSignTrait::new()),
        )?;

        let result = handler.handle_pending_status(tx.clone()).await;

        assert!(result.is_ok());
        let returned_tx = result.unwrap();
        assert_eq!(returned_tx.status, TransactionStatus::Pending); // Status unchanged, no job scheduled
        Ok(())
    }

    #[tokio::test]
    async fn test_handle_pending_status_timeout() -> Result<()> {
        let provider = MockSolanaProviderTrait::new();
        let relayer_repo = Arc::new(MockRelayerRepository::new());
        let mut tx_repo = MockTransactionRepository::new();
        let job_producer = Arc::new(MockJobProducerTrait::new());

        // Create transaction that's exceeded pending timeout
        let mut tx = create_tx_with_signature(TransactionStatus::Pending, None);
        tx.created_at =
            (Utc::now() - Duration::minutes(SOLANA_PENDING_TIMEOUT_MINUTES + 1)).to_rfc3339();

        let tx_id = tx.id.clone();

        // Should mark as failed due to timeout
        tx_repo
            .expect_partial_update()
            .withf(move |tx_id_param, update_req| {
                tx_id_param == &tx_id && update_req.status == Some(TransactionStatus::Failed)
            })
            .times(1)
            .returning(move |_, _| {
                let mut failed_tx = create_tx_with_signature(TransactionStatus::Failed, None);
                failed_tx.status = TransactionStatus::Failed;
                Ok(failed_tx)
            });

        let handler = SolanaRelayerTransaction::new(
            create_mock_solana_relayer("test-relayer".to_string(), false),
            relayer_repo,
            Arc::new(provider),
            Arc::new(tx_repo),
            job_producer,
            Arc::new(MockSolanaSignTrait::new()),
        )?;

        let result = handler.handle_pending_status(tx).await;

        assert!(result.is_ok());
        let updated_tx = result.unwrap();
        assert_eq!(updated_tx.status, TransactionStatus::Failed);
        Ok(())
    }

    #[tokio::test]
    async fn test_handle_resubmit_blockhash_expired_resubmitable() -> Result<()> {
        let mut provider = MockSolanaProviderTrait::new();
        let relayer_repo = Arc::new(MockRelayerRepository::new());
        let tx_repo = Arc::new(MockTransactionRepository::new());
        let mut job_producer = MockJobProducerTrait::new();

        // Create a simple transaction for testing
        let payer = Pubkey::new_unique();
        let instruction =
            solana_system_interface::instruction::transfer(&payer, &Pubkey::new_unique(), 1000);
        let mut transaction = SolanaTransaction::new_with_payer(&[instruction], Some(&payer));
        transaction.message.recent_blockhash = Hash::from_str("11111111111111111111111111111112")?;
        let transaction_bytes = bincode::serialize(&transaction)?;
        let transaction_b64 = base64_encode(&transaction_bytes);

        // Create transaction with expired blockhash that's resubmitable
        let signature_str = "4XFPmbPT4TRchFWNmQD2N8BhjxJQKqYdXWQG7kJJtxCBZ8Y9WtNDoPAwQaHFYnVynCjMVyF9TCMrpPFkEpG7LpZr";
        let mut tx = create_tx_with_signature(TransactionStatus::Submitted, Some(signature_str));
        tx.sent_at = Some(
            (Utc::now() - Duration::seconds(SOLANA_MIN_AGE_FOR_RESUBMIT_CHECK_SECONDS + 10))
                .to_rfc3339(),
        );
        tx.network_data = NetworkTransactionData::Solana(SolanaTransactionData {
            transaction: Some(transaction_b64),
            instructions: None,
            signature: Some(signature_str.to_string()),
        });

        let tx_id = tx.id.clone();

        // Mock provider calls
        provider
            .expect_is_blockhash_valid()
            .with(
                eq(Hash::from_str("11111111111111111111111111111112")?),
                eq(CommitmentConfig::confirmed()),
            )
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(false) })); // Blockhash expired

        // Expect resubmit job to be produced
        job_producer
            .expect_produce_submit_transaction_job()
            .withf(move |job, _delay| {
                matches!(job.command, TransactionCommand::Resubmit) && job.transaction_id == tx_id
            })
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let handler = SolanaRelayerTransaction::new(
            create_mock_solana_relayer("test-relayer".to_string(), false),
            relayer_repo,
            Arc::new(provider),
            tx_repo,
            Arc::new(job_producer),
            Arc::new(MockSolanaSignTrait::new()),
        )?;

        let result = handler.handle_resubmit_or_expiration(tx.clone()).await;

        assert!(result.is_ok());
        let returned_tx = result.unwrap();
        assert_eq!(returned_tx.status, TransactionStatus::Submitted); // Status unchanged
        Ok(())
    }

    #[tokio::test]
    async fn test_handle_resubmit_blockhash_expired_not_resubmitable() -> Result<()> {
        let mut provider = MockSolanaProviderTrait::new();
        let relayer_repo = Arc::new(MockRelayerRepository::new());
        let mut tx_repo = MockTransactionRepository::new();
        let job_producer = Arc::new(MockJobProducerTrait::new());

        // Create multi-signature transaction (not resubmitable)
        let payer = Pubkey::new_unique();
        let recipient = Pubkey::new_unique();
        let additional_signer = Pubkey::new_unique();
        let instruction = system_instruction::transfer(&payer, &recipient, 1000);

        // Create message with multiple signers
        let mut message = Message::new(&[instruction], Some(&payer));
        message.account_keys.push(additional_signer);
        message.header.num_required_signatures = 2; // Multi-sig
        message.recent_blockhash = Hash::from_str("11111111111111111111111111111112")?;

        let transaction = SolanaTransaction::new_unsigned(message);
        let transaction_bytes = bincode::serialize(&transaction)?;
        let transaction_b64 = base64_encode(&transaction_bytes);

        let signature_str = "4XFPmbPT4TRchFWNmQD2N8BhjxJQKqYdXWQG7kJJtxCBZ8Y9WtNDoPAwQaHFYnVynCjMVyF9TCMrpPFkEpG7LpZr";
        let mut tx = create_tx_with_signature(TransactionStatus::Submitted, Some(signature_str));
        tx.sent_at = Some(
            (Utc::now() - Duration::seconds(SOLANA_MIN_AGE_FOR_RESUBMIT_CHECK_SECONDS + 10))
                .to_rfc3339(),
        );
        tx.network_data = NetworkTransactionData::Solana(SolanaTransactionData {
            transaction: Some(transaction_b64),
            instructions: None,
            signature: Some(signature_str.to_string()),
        });

        let tx_id = tx.id.clone();

        // Mock provider calls
        provider
            .expect_is_blockhash_valid()
            .with(
                eq(Hash::from_str("11111111111111111111111111111112")?),
                eq(CommitmentConfig::confirmed()),
            )
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(false) })); // Blockhash expired

        // Should mark as expired
        tx_repo
            .expect_partial_update()
            .withf(move |tx_id_param, update_req| {
                tx_id_param == &tx_id && update_req.status == Some(TransactionStatus::Expired)
            })
            .times(1)
            .returning(move |_, _| {
                let mut expired_tx = create_tx_with_signature(TransactionStatus::Expired, None);
                expired_tx.status = TransactionStatus::Expired;
                Ok(expired_tx)
            });

        let handler = SolanaRelayerTransaction::new(
            create_mock_solana_relayer("test-relayer".to_string(), false),
            relayer_repo,
            Arc::new(provider),
            Arc::new(tx_repo),
            job_producer,
            Arc::new(MockSolanaSignTrait::new()),
        )?;

        let result = handler.handle_resubmit_or_expiration(tx).await;

        assert!(result.is_ok());
        let updated_tx = result.unwrap();
        assert_eq!(updated_tx.status, TransactionStatus::Expired);
        Ok(())
    }

    #[tokio::test]
    async fn test_check_any_signature_on_chain_found() -> Result<()> {
        let mut provider = MockSolanaProviderTrait::new();
        let relayer_repo = Arc::new(MockRelayerRepository::new());
        let tx_repo = Arc::new(MockTransactionRepository::new());
        let job_producer = Arc::new(MockJobProducerTrait::new());

        let signature1 = "4XFPmbPT4TRchFWNmQD2N8BhjxJQKqYdXWQG7kJJtxCBZ8Y9WtNDoPAwQaHFYnVynCjMVyF9TCMrpPFkEpG7LpZr";
        let signature2 = "3XFPmbPT4TRchFWNmQD2N8BhjxJQKqYdXWQG7kJJtxCBZ8Y9WtNDoPAwQaHFYnVynCjMVyF9TCMrpPFkEpG7LpZr";

        let mut tx = create_tx_with_signature(TransactionStatus::Submitted, Some(signature1));
        tx.hashes = vec![signature1.to_string(), signature2.to_string()];

        // Mock provider to return error for first signature, success for second
        provider
            .expect_get_transaction_status()
            .with(eq(Signature::from_str(signature1)?))
            .times(1)
            .returning(|_| {
                Box::pin(async { Err(SolanaProviderError::RpcError("not found".to_string())) })
            });

        provider
            .expect_get_transaction_status()
            .with(eq(Signature::from_str(signature2)?))
            .times(1)
            .returning(|_| Box::pin(async { Ok(SolanaTransactionStatus::Processed) }));

        let handler = SolanaRelayerTransaction::new(
            create_mock_solana_relayer("test-relayer".to_string(), false),
            relayer_repo,
            Arc::new(provider),
            tx_repo,
            job_producer,
            Arc::new(MockSolanaSignTrait::new()),
        )?;

        let result = handler.check_any_signature_on_chain(&tx).await;

        assert!(result.is_ok());
        let found = result.unwrap();
        assert!(found.is_some());
        let (found_sig, status) = found.unwrap();
        assert_eq!(found_sig, signature2);
        assert_eq!(status, SolanaTransactionStatus::Processed);
        Ok(())
    }

    #[tokio::test]
    async fn test_check_any_signature_on_chain_not_found() -> Result<()> {
        let mut provider = MockSolanaProviderTrait::new();
        let relayer_repo = Arc::new(MockRelayerRepository::new());
        let tx_repo = Arc::new(MockTransactionRepository::new());
        let job_producer = Arc::new(MockJobProducerTrait::new());

        let signature1 = "4XFPmbPT4TRchFWNmQD2N8BhjxJQKqYdXWQG7kJJtxCBZ8Y9WtNDoPAwQaHFYnVynCjMVyF9TCMrpPFkEpG7LpZr";
        let signature2 = "3XFPmbPT4TRchFWNmQD2N8BhjxJQKqYdXWQG7kJJtxCBZ8Y9WtNDoPAwQaHFYnVynCjMVyF9TCMrpPFkEpG7LpZr";

        let mut tx = create_tx_with_signature(TransactionStatus::Submitted, Some(signature1));
        tx.hashes = vec![signature1.to_string(), signature2.to_string()];

        // Mock provider to return error for both signatures
        provider
            .expect_get_transaction_status()
            .with(eq(Signature::from_str(signature1)?))
            .times(1)
            .returning(|_| {
                Box::pin(async { Err(SolanaProviderError::RpcError("not found".to_string())) })
            });

        provider
            .expect_get_transaction_status()
            .with(eq(Signature::from_str(signature2)?))
            .times(1)
            .returning(|_| {
                Box::pin(async { Err(SolanaProviderError::RpcError("not found".to_string())) })
            });

        let handler = SolanaRelayerTransaction::new(
            create_mock_solana_relayer("test-relayer".to_string(), false),
            relayer_repo,
            Arc::new(provider),
            tx_repo,
            job_producer,
            Arc::new(MockSolanaSignTrait::new()),
        )?;

        let result = handler.check_any_signature_on_chain(&tx).await;

        assert!(result.is_ok());
        let found = result.unwrap();
        assert!(found.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn test_is_blockhash_valid_true() -> Result<()> {
        let mut provider = MockSolanaProviderTrait::new();
        let relayer_repo = Arc::new(MockRelayerRepository::new());
        let tx_repo = Arc::new(MockTransactionRepository::new());
        let job_producer = Arc::new(MockJobProducerTrait::new());

        let blockhash = Hash::from_str("11111111111111111111111111111112")?;

        provider
            .expect_is_blockhash_valid()
            .with(eq(blockhash), eq(CommitmentConfig::confirmed()))
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(true) }));

        let handler = SolanaRelayerTransaction::new(
            create_mock_solana_relayer("test-relayer".to_string(), false),
            relayer_repo,
            Arc::new(provider),
            tx_repo,
            job_producer,
            Arc::new(MockSolanaSignTrait::new()),
        )?;

        let mut transaction =
            SolanaTransaction::new_unsigned(Message::new(&[], Some(&Pubkey::new_unique())));
        transaction.message.recent_blockhash = blockhash;

        let result = handler.is_blockhash_valid(&transaction).await;

        assert!(result.is_ok());
        assert!(result.unwrap());
        Ok(())
    }

    #[tokio::test]
    async fn test_is_blockhash_valid_false() -> Result<()> {
        let mut provider = MockSolanaProviderTrait::new();
        let relayer_repo = Arc::new(MockRelayerRepository::new());
        let tx_repo = Arc::new(MockTransactionRepository::new());
        let job_producer = Arc::new(MockJobProducerTrait::new());

        let blockhash = Hash::from_str("11111111111111111111111111111112")?;

        provider
            .expect_is_blockhash_valid()
            .with(eq(blockhash), eq(CommitmentConfig::confirmed()))
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(false) }));

        let handler = SolanaRelayerTransaction::new(
            create_mock_solana_relayer("test-relayer".to_string(), false),
            relayer_repo,
            Arc::new(provider),
            tx_repo,
            job_producer,
            Arc::new(MockSolanaSignTrait::new()),
        )?;

        let mut transaction =
            SolanaTransaction::new_unsigned(Message::new(&[], Some(&Pubkey::new_unique())));
        transaction.message.recent_blockhash = blockhash;

        let result = handler.is_blockhash_valid(&transaction).await;

        assert!(result.is_ok());
        assert!(!result.unwrap());
        Ok(())
    }

    #[tokio::test]
    async fn test_is_blockhash_valid_error() -> Result<()> {
        let mut provider = MockSolanaProviderTrait::new();
        let relayer_repo = Arc::new(MockRelayerRepository::new());
        let tx_repo = Arc::new(MockTransactionRepository::new());
        let job_producer = Arc::new(MockJobProducerTrait::new());

        let blockhash = Hash::from_str("11111111111111111111111111111112")?;

        provider
            .expect_is_blockhash_valid()
            .with(eq(blockhash), eq(CommitmentConfig::confirmed()))
            .times(1)
            .returning(|_, _| {
                Box::pin(async { Err(SolanaProviderError::RpcError("test error".to_string())) })
            });

        let handler = SolanaRelayerTransaction::new(
            create_mock_solana_relayer("test-relayer".to_string(), false),
            relayer_repo,
            Arc::new(provider),
            tx_repo,
            job_producer,
            Arc::new(MockSolanaSignTrait::new()),
        )?;

        let mut transaction =
            SolanaTransaction::new_unsigned(Message::new(&[], Some(&Pubkey::new_unique())));
        transaction.message.recent_blockhash = blockhash;

        let result = handler.is_blockhash_valid(&transaction).await;

        assert!(result.is_err());
        let error = result.unwrap_err();
        match error {
            TransactionError::UnderlyingSolanaProvider(_) => {} // Expected
            _ => panic!("Expected UnderlyingSolanaProvider error"),
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_get_time_since_sent_or_created_at_with_sent_at() {
        let provider = MockSolanaProviderTrait::new();
        let relayer_repo = Arc::new(MockRelayerRepository::new());
        let tx_repo = Arc::new(MockTransactionRepository::new());
        let job_producer = Arc::new(MockJobProducerTrait::new());

        let handler = SolanaRelayerTransaction::new(
            create_mock_solana_relayer("test-relayer".to_string(), false),
            relayer_repo,
            Arc::new(provider),
            tx_repo,
            job_producer,
            Arc::new(MockSolanaSignTrait::new()),
        )
        .unwrap();

        let mut tx = create_tx_with_signature(TransactionStatus::Pending, None);
        let past_time = Utc::now() - Duration::minutes(5);
        tx.sent_at = Some(past_time.to_rfc3339());

        let result = handler.get_time_since_sent_or_created_at(&tx);

        assert!(result.is_some());
        let duration = result.unwrap();
        assert!(duration.num_minutes() >= 5);
    }

    #[tokio::test]
    async fn test_get_time_since_sent_or_created_at_with_created_at() {
        let provider = MockSolanaProviderTrait::new();
        let relayer_repo = Arc::new(MockRelayerRepository::new());
        let tx_repo = Arc::new(MockTransactionRepository::new());
        let job_producer = Arc::new(MockJobProducerTrait::new());

        let handler = SolanaRelayerTransaction::new(
            create_mock_solana_relayer("test-relayer".to_string(), false),
            relayer_repo,
            Arc::new(provider),
            tx_repo,
            job_producer,
            Arc::new(MockSolanaSignTrait::new()),
        )
        .unwrap();

        let mut tx = create_tx_with_signature(TransactionStatus::Pending, None);
        let past_time = Utc::now() - Duration::minutes(10);
        tx.created_at = past_time.to_rfc3339();
        tx.sent_at = None; // No sent_at

        let result = handler.get_time_since_sent_or_created_at(&tx);

        assert!(result.is_some());
        let duration = result.unwrap();
        assert!(duration.num_minutes() >= 10);
    }

    #[tokio::test]
    async fn test_has_exceeded_timeout_pending() {
        let provider = MockSolanaProviderTrait::new();
        let relayer_repo = Arc::new(MockRelayerRepository::new());
        let tx_repo = Arc::new(MockTransactionRepository::new());
        let job_producer = Arc::new(MockJobProducerTrait::new());

        let handler = SolanaRelayerTransaction::new(
            create_mock_solana_relayer("test-relayer".to_string(), false),
            relayer_repo,
            Arc::new(provider),
            tx_repo,
            job_producer,
            Arc::new(MockSolanaSignTrait::new()),
        )
        .unwrap();

        let mut tx = create_tx_with_signature(TransactionStatus::Pending, None);
        tx.created_at =
            (Utc::now() - Duration::minutes(SOLANA_PENDING_TIMEOUT_MINUTES + 1)).to_rfc3339();

        let result = handler.has_exceeded_timeout(&tx);

        assert!(result.is_ok());
        assert!(result.unwrap());
    }

    #[tokio::test]
    async fn test_has_exceeded_timeout_sent() {
        let provider = MockSolanaProviderTrait::new();
        let relayer_repo = Arc::new(MockRelayerRepository::new());
        let tx_repo = Arc::new(MockTransactionRepository::new());
        let job_producer = Arc::new(MockJobProducerTrait::new());

        let handler = SolanaRelayerTransaction::new(
            create_mock_solana_relayer("test-relayer".to_string(), false),
            relayer_repo,
            Arc::new(provider),
            tx_repo,
            job_producer,
            Arc::new(MockSolanaSignTrait::new()),
        )
        .unwrap();

        let mut tx = create_tx_with_signature(TransactionStatus::Sent, None);
        tx.sent_at =
            Some((Utc::now() - Duration::minutes(SOLANA_SENT_TIMEOUT_MINUTES + 1)).to_rfc3339());

        let result = handler.has_exceeded_timeout(&tx);

        assert!(result.is_ok());
        assert!(result.unwrap());
    }

    #[tokio::test]
    async fn test_is_valid_until_expired_user_provided() {
        let provider = MockSolanaProviderTrait::new();
        let relayer_repo = Arc::new(MockRelayerRepository::new());
        let tx_repo = Arc::new(MockTransactionRepository::new());
        let job_producer = Arc::new(MockJobProducerTrait::new());

        let handler = SolanaRelayerTransaction::new(
            create_mock_solana_relayer("test-relayer".to_string(), false),
            relayer_repo,
            Arc::new(provider),
            tx_repo,
            job_producer,
            Arc::new(MockSolanaSignTrait::new()),
        )
        .unwrap();

        let mut tx = create_tx_with_signature(TransactionStatus::Pending, None);
        let past_time = Utc::now() - Duration::minutes(1);
        tx.valid_until = Some(past_time.to_rfc3339());

        assert!(handler.is_valid_until_expired(&tx));
    }

    #[tokio::test]
    async fn test_is_valid_until_expired_default() {
        let provider = MockSolanaProviderTrait::new();
        let relayer_repo = Arc::new(MockRelayerRepository::new());
        let tx_repo = Arc::new(MockTransactionRepository::new());
        let job_producer = Arc::new(MockJobProducerTrait::new());

        let handler = SolanaRelayerTransaction::new(
            create_mock_solana_relayer("test-relayer".to_string(), false),
            relayer_repo,
            Arc::new(provider),
            tx_repo,
            job_producer,
            Arc::new(MockSolanaSignTrait::new()),
        )
        .unwrap();

        let mut tx = create_tx_with_signature(TransactionStatus::Pending, None);
        let past_time =
            Utc::now() - Duration::milliseconds(SOLANA_DEFAULT_TX_VALID_TIMESPAN + 1000);
        tx.created_at = past_time.to_rfc3339();
        tx.valid_until = None; // Use default

        assert!(handler.is_valid_until_expired(&tx));
    }

    #[tokio::test]
    async fn test_mark_as_expired() -> Result<()> {
        let provider = MockSolanaProviderTrait::new();
        let relayer_repo = Arc::new(MockRelayerRepository::new());
        let mut tx_repo = MockTransactionRepository::new();
        let job_producer = Arc::new(MockJobProducerTrait::new());

        let tx = create_tx_with_signature(TransactionStatus::Pending, None);
        let tx_id = tx.id.clone();
        let reason = "Test expiration";

        tx_repo
            .expect_partial_update()
            .withf(move |tx_id_param, update_req| {
                tx_id_param == &tx_id
                    && update_req.status == Some(TransactionStatus::Expired)
                    && update_req.status_reason == Some(reason.to_string())
            })
            .times(1)
            .returning(move |_, _| {
                let mut expired_tx = create_tx_with_signature(TransactionStatus::Expired, None);
                expired_tx.status = TransactionStatus::Expired;
                Ok(expired_tx)
            });

        let handler = SolanaRelayerTransaction::new(
            create_mock_solana_relayer("test-relayer".to_string(), false),
            relayer_repo,
            Arc::new(provider),
            Arc::new(tx_repo),
            job_producer,
            Arc::new(MockSolanaSignTrait::new()),
        )?;

        let result = handler.mark_as_expired(tx, reason.to_string()).await;

        assert!(result.is_ok());
        let updated_tx = result.unwrap();
        assert_eq!(updated_tx.status, TransactionStatus::Expired);
        Ok(())
    }

    #[tokio::test]
    async fn test_mark_as_failed() -> Result<()> {
        let provider = MockSolanaProviderTrait::new();
        let relayer_repo = Arc::new(MockRelayerRepository::new());
        let mut tx_repo = MockTransactionRepository::new();
        let job_producer = Arc::new(MockJobProducerTrait::new());

        let tx = create_tx_with_signature(TransactionStatus::Pending, None);
        let tx_id = tx.id.clone();
        let reason = "Test failure";

        tx_repo
            .expect_partial_update()
            .withf(move |tx_id_param, update_req| {
                tx_id_param == &tx_id
                    && update_req.status == Some(TransactionStatus::Failed)
                    && update_req.status_reason == Some(reason.to_string())
            })
            .times(1)
            .returning(move |_, _| {
                let mut failed_tx = create_tx_with_signature(TransactionStatus::Failed, None);
                failed_tx.status = TransactionStatus::Failed;
                Ok(failed_tx)
            });

        let handler = SolanaRelayerTransaction::new(
            create_mock_solana_relayer("test-relayer".to_string(), false),
            relayer_repo,
            Arc::new(provider),
            Arc::new(tx_repo),
            job_producer,
            Arc::new(MockSolanaSignTrait::new()),
        )?;

        let result = handler.mark_as_failed(tx, reason.to_string()).await;

        assert!(result.is_ok());
        let updated_tx = result.unwrap();
        assert_eq!(updated_tx.status, TransactionStatus::Failed);
        Ok(())
    }

    #[tokio::test]
    async fn test_update_transaction_status_and_send_notification() -> Result<()> {
        let provider = MockSolanaProviderTrait::new();
        let relayer_repo = Arc::new(MockRelayerRepository::new());
        let mut tx_repo = MockTransactionRepository::new();
        let mut job_producer = MockJobProducerTrait::new();

        // Create relayer with notification configured
        let mut relayer = create_mock_solana_relayer("test-relayer".to_string(), false);
        relayer.notification_id = Some("test-notification".to_string());

        let tx = create_tx_with_signature(TransactionStatus::Submitted, None);
        let tx_id = tx.id.clone();
        let new_status = TransactionStatus::Confirmed;

        tx_repo
            .expect_partial_update()
            .withf(move |tx_id_param, update_req| {
                tx_id_param == &tx_id && update_req.status == Some(TransactionStatus::Confirmed)
            })
            .times(1)
            .returning(move |_, _| {
                let mut confirmed_tx = create_tx_with_signature(TransactionStatus::Confirmed, None);
                confirmed_tx.status = TransactionStatus::Confirmed;
                Ok(confirmed_tx)
            });

        job_producer
            .expect_produce_send_notification_job()
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let handler = SolanaRelayerTransaction::new(
            relayer,
            relayer_repo,
            Arc::new(provider),
            Arc::new(tx_repo),
            Arc::new(job_producer),
            Arc::new(MockSolanaSignTrait::new()),
        )?;

        let result = handler
            .update_transaction_status_and_send_notification(tx, new_status, None)
            .await;

        assert!(result.is_ok());
        let updated_tx = result.unwrap();
        assert_eq!(updated_tx.status, TransactionStatus::Confirmed);
        Ok(())
    }
}
