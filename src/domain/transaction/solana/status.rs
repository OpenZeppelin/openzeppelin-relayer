//! Solana transaction status handling implementation
//!
//! This module provides transaction status checking for Solana transactions,
//! including status updates, repository management, and webhook notifications.

use chrono::Utc;
use log::{debug, error, info, warn};
use solana_sdk::signature::Signature;
use std::str::FromStr;

use super::SolanaRelayerTransaction;
use crate::{
    jobs::{JobProducerTrait, TransactionStatusCheck},
    models::{
        produce_transaction_update_notification_payload, RelayerRepoModel, SolanaTransactionStatus,
        TransactionError, TransactionRepoModel, TransactionStatus, TransactionUpdateRequest,
    },
    repositories::{transaction::TransactionRepository, RelayerRepository, Repository},
    services::provider::SolanaProviderTrait,
};

/// Default delay for retrying status checks after failures (in seconds)
const SOLANA_DEFAULT_STATUS_RETRY_DELAY_SECONDS: i64 = 10;

impl<P, RR, TR, J> SolanaRelayerTransaction<P, RR, TR, J>
where
    P: SolanaProviderTrait,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    J: JobProducerTrait + Send + Sync + 'static,
{
    /// Main status handling method with error handling and retries
    pub async fn handle_transaction_status_impl(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        info!("Handling Solana transaction status for: {:?}", tx.id);

        // Skip if already in final state
        if matches!(
            tx.status,
            TransactionStatus::Confirmed | TransactionStatus::Failed | TransactionStatus::Expired
        ) {
            info!(
                "Transaction {} already in final state: {:?}",
                tx.id, tx.status
            );
            return Ok(tx);
        }

        // Call core status checking logic with error handling
        match self.check_and_update_status(tx.clone()).await {
            Ok(updated_tx) => Ok(updated_tx),
            Err(error) => {
                // Only retry for provider errors, not validation errors
                match error {
                    TransactionError::ValidationError(_) => {
                        // Don't retry validation errors (like missing signature)
                        Err(error)
                    }
                    _ => {
                        // Handle status check failure - requeue for retry
                        self.handle_status_check_failure(tx, error).await
                    }
                }
            }
        }
    }

    /// Handles status check failures with retry logic.
    /// This method ensures failed status checks are retried appropriately.
    async fn handle_status_check_failure(
        &self,
        tx: TransactionRepoModel,
        error: TransactionError,
    ) -> Result<TransactionRepoModel, TransactionError> {
        warn!(
            "Failed to get Solana transaction status for {}: {}. Re-queueing check.",
            tx.id, error
        );

        if let Err(requeue_error) = self
            .schedule_status_check(&tx, Some(2 * SOLANA_DEFAULT_STATUS_RETRY_DELAY_SECONDS))
            .await
        {
            warn!(
                "Failed to requeue status check for transaction {}: {}",
                tx.id, requeue_error
            );
        }

        info!(
            "Transaction {} status check failure handled. Will retry later. Error: {}",
            tx.id, error
        );

        // Return the original error even though we scheduled a retry
        Err(error)
    }

    /// Core status checking logic
    async fn check_and_update_status(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        // Extract signature from Solana transaction data
        let solana_data = tx.network_data.get_solana_transaction_data()?;
        let signature_str = solana_data.signature.as_ref().ok_or_else(|| {
            TransactionError::ValidationError("Transaction signature is missing".to_string())
        })?;

        let signature = Signature::from_str(signature_str).map_err(|e| {
            TransactionError::ValidationError(format!("Invalid signature format: {}", e))
        })?;

        // Get transaction status from provider
        let solana_status = self
            .provider()
            .get_transaction_status(&signature)
            .await
            .map_err(|e| {
                TransactionError::UnexpectedError(format!(
                    "Failed to get Solana transaction status for tx {} (signature {}): {}",
                    tx.id, signature_str, e
                ))
            })?;

        println!("solana_status: {:?}", solana_status);

        // Map Solana status to repository status and handle accordingly
        match solana_status {
            SolanaTransactionStatus::Processed => self.handle_processed_status(tx).await,
            SolanaTransactionStatus::Confirmed => self.handle_confirmed_status(tx).await,
            SolanaTransactionStatus::Finalized => self.handle_finalized_status(tx).await,
            SolanaTransactionStatus::Failed => self.handle_failed_status(tx).await,
        }
    }

    /// Helper method that updates transaction status only if it's different from the current status
    async fn update_transaction_status_if_needed(
        &self,
        tx: TransactionRepoModel,
        new_status: TransactionStatus,
    ) -> Result<TransactionRepoModel, TransactionError> {
        if tx.status != new_status {
            let update_request = TransactionUpdateRequest {
                status: Some(new_status.clone()),
                confirmed_at: if matches!(new_status, TransactionStatus::Confirmed) {
                    Some(Utc::now().to_rfc3339())
                } else {
                    None
                },
                ..Default::default()
            };
            return self
                .finalize_transaction_state(tx.id.clone(), update_request)
                .await;
        }
        Ok(tx)
    }

    /// Helper method to schedule a transaction status check job
    async fn schedule_status_check(
        &self,
        tx: &TransactionRepoModel,
        delay_seconds: Option<i64>,
    ) -> Result<(), TransactionError> {
        let delay = delay_seconds.map(|seconds| Utc::now().timestamp() + seconds);
        self.job_producer()
            .produce_check_transaction_status_job(
                TransactionStatusCheck::new(tx.id.clone(), tx.relayer_id.clone()),
                delay,
            )
            .await
            .map_err(|e| {
                TransactionError::UnexpectedError(format!("Failed to schedule status check: {}", e))
            })
    }

    /// Handle processed status (transaction processed by leader but not yet confirmed)
    async fn handle_processed_status(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        info!(
            "Transaction {} is processed but waiting for supermajority confirmation",
            tx.id
        );

        // Schedule another status check since transaction is not in final state
        self.schedule_status_check(&tx, Some(SOLANA_DEFAULT_STATUS_RETRY_DELAY_SECONDS))
            .await?;

        // Keep current status - will check again later for confirmation/finalization
        Ok(tx)
    }

    /// Handle confirmed status (transaction confirmed by supermajority)
    /// We are mapping this to mined status because we don't have a separate finalized status
    /// and we want to keep the status consistent with the other networks
    async fn handle_confirmed_status(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        debug!("Transaction {} is confirmed by supermajority", tx.id);

        // Update status to mined only if not already mined
        let updated_tx = self
            .update_transaction_status_if_needed(tx, TransactionStatus::Mined)
            .await?;

        // Schedule another status check since transaction could progress to finalized
        self.schedule_status_check(&updated_tx, Some(SOLANA_DEFAULT_STATUS_RETRY_DELAY_SECONDS))
            .await?;

        Ok(updated_tx)
    }

    /// Handle finalized status (transaction is finalized and irreversible)
    /// We are mapping this to confirmed status because we don't have a separate finalized status
    /// and we want to keep the status consistent with the other networks
    async fn handle_finalized_status(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        info!("Transaction {} is finalized and irreversible", tx.id);

        // Update status to confirmed only if not already confirmed (final success state)
        self.update_transaction_status_if_needed(tx, TransactionStatus::Confirmed)
            .await
    }

    /// Handle failed status (transaction failed on-chain)
    async fn handle_failed_status(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        warn!("Transaction {} failed on-chain", tx.id);

        // Update status to failed only if not already failed (final failure state)
        self.update_transaction_status_if_needed(tx, TransactionStatus::Failed)
            .await
    }

    /// Helper function to update transaction status, save it, and send notification
    async fn finalize_transaction_state(
        &self,
        tx_id: String,
        update_req: TransactionUpdateRequest,
    ) -> Result<TransactionRepoModel, TransactionError> {
        // Update transaction in repository
        let updated_tx = self
            .transaction_repository()
            .partial_update(tx_id, update_req)
            .await
            .map_err(|e| TransactionError::UnexpectedError(e.to_string()))?;

        // Send webhook notification if relayer has notification configured
        self.send_transaction_update_notification(&updated_tx)
            .await?;

        Ok(updated_tx)
    }

    /// Send webhook notification for transaction updates
    async fn send_transaction_update_notification(
        &self,
        tx: &TransactionRepoModel,
    ) -> Result<(), TransactionError> {
        if let Some(notification_id) = &self.relayer().notification_id {
            info!("Sending webhook notification for transaction: {}", tx.id);

            let notification_payload =
                produce_transaction_update_notification_payload(notification_id, tx);

            if let Err(e) = self
                .job_producer()
                .produce_send_notification_job(notification_payload, None)
                .await
            {
                error!("Failed to produce notification job: {}", e);
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        jobs::MockJobProducerTrait,
        models::{NetworkTransactionData, SolanaTransactionData},
        repositories::{MockRelayerRepository, MockTransactionRepository},
        services::{MockSolanaProviderTrait, SolanaProviderError},
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
                transaction: "test".to_string(),
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

        let handler =
            SolanaRelayerTransaction::new(relayer, relayer_repo, provider, tx_repo, job_producer)
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
        let tx_repo = Arc::new(MockTransactionRepository::new());
        let mut job_producer = MockJobProducerTrait::new();

        let signature_str =
            "4XFPmbPT4TRchFWNmQD2N8BhjxJQKqYdXWQG7kJJtxCBZ8Y9WtNDoPAwQaHFYnVynCjMVyF9TCMrpPFkEpG7LpZr";
        let tx = create_tx_with_signature(TransactionStatus::Pending, Some(signature_str));

        provider
            .expect_get_transaction_status()
            .with(eq(Signature::from_str(signature_str)?))
            .times(1)
            .returning(|_| Box::pin(async { Ok(SolanaTransactionStatus::Processed) }));

        job_producer
            .expect_produce_check_transaction_status_job()
            .withf(|check, delay| check.transaction_id == "test" && delay.is_some())
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let handler = SolanaRelayerTransaction::new(
            create_mock_solana_relayer("test-relayer".to_string(), false),
            relayer_repo,
            Arc::new(provider),
            tx_repo,
            Arc::new(job_producer),
        )?;

        let result = handler.handle_transaction_status_impl(tx.clone()).await;

        assert!(result.is_ok());
        let updated_tx = result.unwrap();
        assert_eq!(updated_tx.id, tx.id);
        assert_eq!(updated_tx.status, TransactionStatus::Pending); // Status should not change
        Ok(())
    }

    #[tokio::test]
    async fn test_handle_status_confirmed() -> Result<()> {
        let mut provider = MockSolanaProviderTrait::new();
        let relayer_repo = Arc::new(MockRelayerRepository::new());
        let mut tx_repo = MockTransactionRepository::new();
        let mut job_producer = MockJobProducerTrait::new();

        let signature_str =
            "4XFPmbPT4TRchFWNmQD2N8BhjxJQKqYdXWQG7kJJtxCBZ8Y9WtNDoPAwQaHFYnVynCjMVyF9TCMrpPFkEpG7LpZr";
        let tx = create_tx_with_signature(TransactionStatus::Submitted, Some(signature_str));

        provider
            .expect_get_transaction_status()
            .with(eq(Signature::from_str(signature_str)?))
            .times(1)
            .returning(|_| Box::pin(async { Ok(SolanaTransactionStatus::Confirmed) }));

        job_producer
            .expect_produce_check_transaction_status_job()
            .withf(|check, delay| check.transaction_id == "test" && delay.is_some())
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(()) }));

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
        let mut job_producer = MockJobProducerTrait::new();

        let signature_str = "4XFPmbPT4TRchFWNmQD2N8BhjxJQKqYdXWQG7kJJtxCBZ8Y9WtNDoPAwQaHFYnVynCjMVyF9TCMrpPFkEpG7LpZr";
        let tx = create_tx_with_signature(TransactionStatus::Pending, Some(signature_str));
        let error_message = "Provider is down";

        provider
            .expect_get_transaction_status()
            .with(eq(Signature::from_str(signature_str)?))
            .times(1)
            .returning(move |_| {
                Box::pin(async { Err(SolanaProviderError::RpcError(error_message.to_string())) })
            });

        job_producer
            .expect_produce_check_transaction_status_job()
            .withf(|check, delay| check.transaction_id == "test" && delay.is_some())
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let handler = SolanaRelayerTransaction::new(
            create_mock_solana_relayer("test-relayer".to_string(), false),
            relayer_repo,
            Arc::new(provider),
            tx_repo,
            Arc::new(job_producer),
        )?;

        let result = handler.handle_transaction_status_impl(tx.clone()).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, TransactionError::UnexpectedError(_)));
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
        )?;

        let result = handler.handle_transaction_status_impl(tx.clone()).await;

        assert!(result.is_ok());
        let updated_tx = result.unwrap();
        assert_eq!(updated_tx.id, tx.id);
        assert_eq!(updated_tx.status, TransactionStatus::Failed);
        Ok(())
    }
}
