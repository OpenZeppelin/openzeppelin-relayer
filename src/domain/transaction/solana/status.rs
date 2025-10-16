//! Solana transaction status handling implementation
//!
//! This module provides transaction status checking for Solana transactions,
//! including status updates, repository management, and webhook notifications.

use chrono::{DateTime, Duration, Utc};
use solana_sdk::{signature::Signature, transaction::Transaction as SolanaTransaction};
use std::str::FromStr;
use tracing::{debug, info, warn};

/// Timeout before attempting to resubmit a Solana transaction with fresh blockhash
/// Solana blockhashes are valid for ~60-90 seconds, so we wait at least 90 seconds
const SOLANA_RESUBMIT_TIMEOUT_SECONDS: i64 = 90;

use super::{utils::decode_solana_transaction, SolanaRelayerTransaction};
use crate::domain::transaction::common::is_final_state;
use crate::services::SolanaSignTrait;
use crate::{
    jobs::{JobProducerTrait, TransactionSend},
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
    /// Similar to EVM approach:
    /// 1. Check transaction status (query chain or return current for Pending/Sent)
    /// 2. Update transaction in DB if status changed
    /// 3. Handle based on the (potentially new) status
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
        let status = self.check_transaction_status(&tx).await?;

        debug!(
            tx_id = %tx.id,
            previous_status = ?tx.status,
            new_status = ?status,
            "transaction status check completed"
        );

        // Step 2: Update transaction in DB if status changed
        let tx = if status != tx.status {
            debug!(
                tx_id = %tx.id,
                old_status = ?tx.status,
                new_status = ?status,
                "status changed, updating transaction"
            );
            self.update_transaction_status_and_notify(tx, status.clone())
                .await?
        } else {
            tx
        };

        // Step 3: Handle based on (potentially new) status
        match status {
            TransactionStatus::Pending => self.handle_pending_status(tx).await,
            TransactionStatus::Sent => self.handle_sent_status(tx).await,
            TransactionStatus::Submitted => self.handle_submitted_status(tx).await,
            TransactionStatus::Mined => self.handle_mined_status(tx).await,
            // Final states
            TransactionStatus::Confirmed
            | TransactionStatus::Failed
            | TransactionStatus::Canceled
            | TransactionStatus::Expired => {
                debug!(tx_id = %tx.id, status = ?status, "transaction in final state");
                Ok(tx)
            }
        }
    }

    /// Check transaction status from chain (or return current for Pending/Sent)
    ///
    /// Similar to EVM's check_transaction_status, this method:
    /// - Returns current status for Pending/Sent (no on-chain query needed)
    /// - Queries chain for Submitted/Mined and returns appropriate status
    async fn check_transaction_status(
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
                    SolanaTransactionStatus::Processed => {
                        // Keep as Submitted or upgrade to Mined
                        if tx.status == TransactionStatus::Submitted {
                            TransactionStatus::Mined
                        } else {
                            tx.status.clone()
                        }
                    }
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

    /// Update transaction status in DB and send notification
    ///
    /// Used after check_transaction_status detects a status change
    async fn update_transaction_status_and_notify(
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
            tracing::error!(
                tx_id = %updated_tx.id,
                status = ?new_status,
                "sending transaction update notification failed: {:?}",
                e
            );
        }

        Ok(updated_tx)
    }

    /// Handle Pending status - transaction not yet signed/submitted
    ///
    /// This is a transient state, should not be in status check loop
    async fn handle_pending_status(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        debug!(tx_id = %tx.id, "transaction in Pending status");
        // Nothing to do - transaction is waiting to be prepared
        Ok(tx)
    }

    /// Handle Sent status - transaction is signed but not yet submitted to network
    ///
    /// This is a transient state between prepare and submit
    async fn handle_sent_status(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        debug!(tx_id = %tx.id, "transaction in Sent status, waiting for submission");
        // Nothing to do - submit job will handle this
        // TODO: Add timeout logic to detect stuck Sent transactions
        Ok(tx)
    }

    /// Handle Submitted status - transaction submitted but not yet mined
    ///
    /// Check if blockhash has expired and transaction needs resubmission
    async fn handle_submitted_status(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        debug!(tx_id = %tx.id, "handling submitted transaction");

        // Check_transaction_status already queried the chain
        // If transaction is still Submitted here, it means it wasn't found on-chain
        // Check if we should resubmit with fresh blockhash

        let transaction = match decode_solana_transaction(&tx) {
            Ok(tx) => tx,
            Err(e) => {
                // If we can't decode the transaction, we can't check for resubmit
                // Just log and return - will retry status check next cycle
                warn!(
                    tx_id = %tx.id,
                    error = %e,
                    "failed to decode transaction, cannot check for resubmit"
                );
                return Ok(tx);
            }
        };

        if self.should_resubmit_with_fresh_blockhash(&tx, &transaction)? {
            info!(
                tx_id = %tx.id,
                "transaction not found on-chain and timeout reached, enqueuing resubmit job"
            );

            // Enqueue resubmit job (like EVM does)
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
        } else {
            // Not ready to resubmit yet, just return and check again later
            debug!(
                tx_id = %tx.id,
                "transaction waiting for confirmation or resubmit timeout"
            );
        }

        Ok(tx)
    }

    /// Handle Mined status - transaction is mined but not yet confirmed/finalized
    ///
    /// Continue checking - check_transaction_status will upgrade to Confirmed when ready
    async fn handle_mined_status(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        debug!(tx_id = %tx.id, "transaction mined, waiting for finalization");
        // check_transaction_status already checked and will update if finalized
        // Just return and let the next check cycle handle it
        Ok(tx)
    }

    /// Check if transaction should be resubmitted due to expired blockhash
    ///
    /// Returns true if:
    /// - Transaction has been submitted for longer than SOLANA_RESUBMIT_TIMEOUT_SECONDS
    /// - Transaction has only one required signature (relayer can update blockhash)
    fn should_resubmit_with_fresh_blockhash(
        &self,
        tx: &TransactionRepoModel,
        transaction: &SolanaTransaction,
    ) -> Result<bool, TransactionError> {
        // Check if transaction is single-signer (can update blockhash)
        if transaction.message.header.num_required_signatures > 1 {
            debug!(
                tx_id = %tx.id,
                num_signatures = transaction.message.header.num_required_signatures,
                "multi-signer transaction, cannot update blockhash for resubmit"
            );
            return Ok(false);
        }

        // Check if enough time has passed since submission
        let sent_at = tx.sent_at.as_ref().ok_or_else(|| {
            TransactionError::ValidationError(
                "Transaction sent_at timestamp is missing".to_string(),
            )
        })?;

        let sent_time = DateTime::parse_from_rfc3339(sent_at)
            .map_err(|e| {
                TransactionError::ValidationError(format!("Invalid sent_at timestamp: {}", e))
            })?
            .with_timezone(&Utc);

        let time_since_sent = Utc::now().signed_duration_since(sent_time);
        let timeout = Duration::seconds(SOLANA_RESUBMIT_TIMEOUT_SECONDS);

        if time_since_sent < timeout {
            debug!(
                tx_id = %tx.id,
                time_since_sent_secs = time_since_sent.num_seconds(),
                timeout_secs = SOLANA_RESUBMIT_TIMEOUT_SECONDS,
                "not enough time passed since submission"
            );
            return Ok(false);
        }

        Ok(true)
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
