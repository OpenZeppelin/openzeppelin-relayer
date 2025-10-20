//! Solana transaction status handling implementation
//!
//! This module provides transaction status checking for Solana transactions,
//! including status updates, repository management, and webhook notifications.

use crate::constants::{
    SOLANA_MIN_AGE_FOR_RESUBMIT_CHECK_SECONDS, SOLANA_PENDING_TIMEOUT_MINUTES,
    SOLANA_SENT_TIMEOUT_MINUTES, SOLANA_SUBMITTED_TIMEOUT_MINUTES,
};
use chrono::{DateTime, Duration, Utc};
use solana_commitment_config::CommitmentConfig;
use solana_sdk::{signature::Signature, transaction::Transaction as SolanaTransaction};
use std::str::FromStr;
use tracing::{debug, error, info, warn};

use super::{utils::decode_solana_transaction, SolanaRelayerTransaction};
use crate::domain::transaction::{common::is_final_state, solana::utils::is_resubmitable};
use crate::services::{SolanaProviderError, SolanaSignTrait};
use crate::{
    jobs::{JobProducerTrait, TransactionRequest, TransactionSend},
    models::{
        RelayerRepoModel, SolanaTransactionStatus, TransactionError, TransactionRepoModel,
        TransactionStatus, TransactionUpdateRequest,
    },
    repositories::{transaction::TransactionRepository, RelayerRepository, Repository},
    services::provider::SolanaProviderTrait,
};

impl<P, RR, TR, J, S> SolanaRelayerTransaction<P, RR, TR, J, S>
where
    P: SolanaProviderTrait,
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
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        debug!(tx_id = %tx.id, status = ?tx.status, "handling solana transaction status");

        // Early return if transaction is already in a final state
        if is_final_state(&tx.status) {
            debug!(status = ?tx.status, "transaction already in final state");
            return Ok(tx);
        }

        // Step 1: Check transaction status (query chain or return current)
        let detected_status = self.check_onchain_transaction_status(&tx).await?;

        debug!(
            tx_id = %tx.id,
            current_status = ?tx.status,
            detected_status = ?detected_status,
            "transaction status check completed"
        );

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
            TransactionError::ValidationError(format!("Invalid signature format: {}", e))
        })?;

        // Query on-chain status
        match self.provider().get_transaction_status(&signature).await {
            Ok(solana_status) => {
                // Map Solana on-chain status to repository status
                let new_status = match solana_status {
                    SolanaTransactionStatus::Processed => TransactionStatus::Mined,
                    SolanaTransactionStatus::Confirmed => TransactionStatus::Mined,
                    SolanaTransactionStatus::Finalized => TransactionStatus::Confirmed,
                    SolanaTransactionStatus::Failed => TransactionStatus::Failed,
                };
                Ok(new_status)
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
    /// Used internally by update_transaction_status_if_needed
    async fn update_transaction_status_and_send_notification(
        &self,
        tx: TransactionRepoModel,
        new_status: TransactionStatus,
    ) -> Result<TransactionRepoModel, TransactionError> {
        let update_request = TransactionUpdateRequest {
            status: Some(new_status.clone()),
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
                .update_transaction_status_and_send_notification(tx, new_status)
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
        if self.has_exceeded_timeout(&tx)? {
            warn!(
                tx_id = %tx.id,
                timeout_minutes = SOLANA_PENDING_TIMEOUT_MINUTES,
                "pending transaction has exceeded timeout"
            );
            return self
                .mark_as_failed(
                    tx,
                    format!(
                        "Transaction stuck in Pending status for more than {} minutes",
                        SOLANA_PENDING_TIMEOUT_MINUTES
                    ),
                )
                .await;
        }

        // Step 3: Schedule transaction request job to prepare and submit the transaction
        info!(
            tx_id = %tx.id,
            "scheduling transaction request job for pending transaction"
        );

        let transaction_request = TransactionRequest::new(tx.id.clone(), tx.relayer_id.clone());

        self.job_producer()
            .produce_transaction_request_job(transaction_request, None)
            .await
            .map_err(|e| {
                TransactionError::UnexpectedError(format!(
                    "Failed to enqueue transaction request job: {}",
                    e
                ))
            })?;

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
    fn is_valid_until_expired(&self, tx: &TransactionRepoModel) -> bool {
        if let Some(valid_until_str) = &tx.valid_until {
            if let Ok(valid_until) = DateTime::parse_from_rfc3339(valid_until_str) {
                return Utc::now() > valid_until.with_timezone(&Utc);
            }
        }
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
            TransactionStatus::Submitted => Duration::minutes(SOLANA_SUBMITTED_TIMEOUT_MINUTES),
            _ => return Ok(false), // No timeout for other statuses
        };

        Ok(age >= timeout)
    }

    /// Handle resubmit or expiration logic based on blockhash validity
    ///
    /// This is the core logic that:
    /// 1. Checks if valid_until has expired
    /// 2. Checks if status-based timeout exceeded
    /// 3. Checks if enough time has passed (60s)
    /// 4. Checks if blockhash is expired
    /// 5. Schedules resubmit if possible, or marks as expired/failed
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

        // Step 2: Check if transaction has exceeded timeout for its status
        if self.has_exceeded_timeout(&tx)? {
            let timeout_minutes = match tx.status {
                TransactionStatus::Pending => SOLANA_PENDING_TIMEOUT_MINUTES,
                TransactionStatus::Sent => SOLANA_SENT_TIMEOUT_MINUTES,
                TransactionStatus::Submitted => SOLANA_SUBMITTED_TIMEOUT_MINUTES,
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
                        "Transaction stuck in {:?} status for more than {} minutes",
                        status, timeout_minutes
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

        // Step 4: Decode transaction to extract blockhash
        let transaction = decode_solana_transaction(&tx)?;

        // Step 5: Check if blockhash is expired
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

        // Step 6: Check if transaction can be resubmitted
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
                        "Failed to enqueue resubmit job: {}",
                        e
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
        jobs::MockJobProducerTrait,
        models::{NetworkTransactionData, SolanaTransactionData},
        repositories::{MockRelayerRepository, MockTransactionRepository},
        services::{MockSolanaProviderTrait, MockSolanaSignTrait, SolanaProviderError},
        utils::mocks::mockutils::{create_mock_solana_relayer, create_mock_solana_transaction},
    };
    use eyre::Result;
    use mockall::predicate::*;
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
        let provider = Arc::new(MockSolanaProviderTrait::new());
        let relayer_repo = Arc::new(MockRelayerRepository::new());
        let tx_repo = Arc::new(MockTransactionRepository::new());
        let job_producer = Arc::new(MockJobProducerTrait::new());
        let relayer = create_mock_solana_relayer("test-relayer".to_string(), false);

        let handler = SolanaRelayerTransaction::new(
            relayer,
            relayer_repo,
            provider,
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

        // Expect status update from Submitted to Mined (Processed maps to Mined)
        tx_repo
            .expect_partial_update()
            .withf(move |tx_id_param, update_req| {
                tx_id_param == &tx_id && update_req.status == Some(TransactionStatus::Mined)
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

        tx_repo
            .expect_partial_update()
            .withf(move |tx_id_param, update_req| {
                tx_id_param == &tx_id && update_req.status == Some(TransactionStatus::Mined)
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

        tx_repo
            .expect_partial_update()
            .withf(move |tx_id_param, update_req| {
                tx_id_param == &tx_id && update_req.status == Some(TransactionStatus::Confirmed)
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

        tx_repo
            .expect_partial_update()
            .withf(move |tx_id_param, update_req| {
                tx_id_param == &tx_id && update_req.status == Some(TransactionStatus::Failed)
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
}
