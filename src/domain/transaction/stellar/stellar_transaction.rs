/// This module defines the `StellarRelayerTransaction` struct and its associated
/// functionality for handling Stellar transactions.
/// It includes methods for preparing, submitting, handling status, and
/// managing notifications for transactions. The module leverages various
/// services and repositories to perform these operations asynchronously.
use crate::{
    domain::transaction::{stellar::fetch_next_sequence_from_chain, Transaction},
    jobs::{JobProducer, JobProducerTrait, TransactionRequest},
    models::{
        produce_transaction_update_notification_payload, NetworkTransactionRequest,
        RelayerRepoModel, TransactionError, TransactionRepoModel, TransactionStatus,
        TransactionUpdateRequest,
    },
    repositories::{
        RelayerRepositoryStorage, Repository, TransactionCounterRepositoryStorage,
        TransactionCounterTrait, TransactionRepository, TransactionRepositoryStorage,
    },
    services::{Signer, StellarProvider, StellarProviderTrait, StellarSigner},
};
use async_trait::async_trait;
use eyre::Result;
use log::info;
use std::sync::Arc;

use super::lane_gate;

#[allow(dead_code)]
pub struct StellarRelayerTransaction<R, T, J, S, P, C>
where
    R: Repository<RelayerRepoModel, String>,
    T: TransactionRepository,
    J: JobProducerTrait,
    S: Signer,
    P: StellarProviderTrait,
    C: TransactionCounterTrait,
{
    relayer: RelayerRepoModel,
    relayer_repository: Arc<R>,
    transaction_repository: Arc<T>,
    job_producer: Arc<J>,
    signer: Arc<S>,
    provider: P,
    transaction_counter_service: Arc<C>,
}

#[allow(dead_code)]
impl<R, T, J, S, P, C> StellarRelayerTransaction<R, T, J, S, P, C>
where
    R: Repository<RelayerRepoModel, String>,
    T: TransactionRepository,
    J: JobProducerTrait,
    S: Signer,
    P: StellarProviderTrait,
    C: TransactionCounterTrait,
{
    /// Creates a new `StellarRelayerTransaction`.
    ///
    /// # Arguments
    ///
    /// * `relayer` - The relayer model.
    /// * `relayer_repository` - Storage for relayer repository.
    /// * `transaction_repository` - Storage for transaction repository.
    /// * `job_producer` - Producer for job queue.
    /// * `signer` - The Stellar signer.
    /// * `provider` - The Stellar provider.
    /// * `transaction_counter_service` - Service for managing transaction counters.
    ///
    /// # Returns
    ///
    /// A result containing the new `StellarRelayerTransaction` or a `TransactionError`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        relayer: RelayerRepoModel,
        relayer_repository: Arc<R>,
        transaction_repository: Arc<T>,
        job_producer: Arc<J>,
        signer: Arc<S>,
        provider: P,
        transaction_counter_service: Arc<C>,
    ) -> Result<Self, TransactionError> {
        Ok(Self {
            relayer,
            relayer_repository,
            transaction_repository,
            job_producer,
            signer,
            provider,
            transaction_counter_service,
        })
    }

    pub fn provider(&self) -> &P {
        &self.provider
    }

    pub fn relayer(&self) -> &RelayerRepoModel {
        &self.relayer
    }

    pub fn job_producer(&self) -> &J {
        &self.job_producer
    }

    pub fn transaction_repository(&self) -> &T {
        &self.transaction_repository
    }

    pub fn signer(&self) -> &S {
        &self.signer
    }

    pub fn transaction_counter_service(&self) -> &C {
        &self.transaction_counter_service
    }

    /// Send a transaction-request job for the given transaction.
    pub async fn send_transaction_request_job(
        &self,
        tx: &TransactionRepoModel,
        delay_seconds: Option<i64>,
    ) -> Result<(), TransactionError> {
        let job = TransactionRequest::new(tx.id.clone(), tx.relayer_id.clone());
        self.job_producer()
            .produce_transaction_request_job(job, delay_seconds)
            .await?;
        Ok(())
    }

    /// Sends a transaction update notification if a notification ID is configured.
    pub(super) async fn send_transaction_update_notification(
        &self,
        tx: &TransactionRepoModel,
    ) -> Result<(), TransactionError> {
        if let Some(notification_id) = &self.relayer().notification_id {
            self.job_producer()
                .produce_send_notification_job(
                    produce_transaction_update_notification_payload(notification_id, tx),
                    None,
                )
                .await
                .map_err(|e| {
                    TransactionError::UnexpectedError(format!("Failed to send notification: {}", e))
                })?;
        }
        Ok(())
    }

    /// Helper function to update transaction status, save it, and send a notification.
    pub async fn finalize_transaction_state(
        &self,
        tx_id: String,
        update_req: TransactionUpdateRequest,
    ) -> Result<TransactionRepoModel, TransactionError> {
        let updated_tx = self
            .transaction_repository()
            .partial_update(tx_id, update_req)
            .await?;

        self.send_transaction_update_notification(&updated_tx)
            .await?;
        Ok(updated_tx)
    }

    pub async fn enqueue_next_pending_transaction(
        &self,
        finished_tx_id: &str,
    ) -> Result<(), TransactionError> {
        if let Some(next) = self
            .find_oldest_pending_for_relayer(&self.relayer().id)
            .await?
        {
            // Atomic hand-over while still owning the lane
            info!("Handing over lane from {} to {}", finished_tx_id, next.id);
            lane_gate::pass_to(&self.relayer().id, finished_tx_id, &next.id);
            self.send_transaction_request_job(&next, None).await?;
        } else {
            info!("Releasing relayer lane after {}", finished_tx_id);
            lane_gate::free(&self.relayer().id, finished_tx_id);
        }
        Ok(())
    }

    /// Finds the oldest pending transaction for a relayer.
    async fn find_oldest_pending_for_relayer(
        &self,
        relayer_id: &str,
    ) -> Result<Option<TransactionRepoModel>, TransactionError> {
        let pending_txs = self
            .transaction_repository()
            .find_by_status(relayer_id, &[TransactionStatus::Pending])
            .await
            .map_err(TransactionError::from)?;

        Ok(pending_txs.into_iter().next())
    }

    /// Syncs the sequence number from the blockchain for the relayer's address.
    /// This fetches the on-chain sequence number and updates the local counter to the next usable value.
    pub async fn sync_sequence_from_chain(
        &self,
        relayer_address: &str,
    ) -> Result<(), TransactionError> {
        info!(
            "Syncing sequence number from chain for address: {}",
            relayer_address
        );

        // Use the shared helper to fetch the next sequence
        let next_usable_seq = fetch_next_sequence_from_chain(self.provider(), relayer_address)
            .await
            .map_err(TransactionError::UnexpectedError)?;

        // Update the local counter to the next usable sequence
        self.transaction_counter_service()
            .set(&self.relayer().id, relayer_address, next_usable_seq)
            .await
            .map_err(|e| {
                TransactionError::UnexpectedError(format!(
                    "Failed to update sequence counter: {}",
                    e
                ))
            })?;

        info!("Updated local sequence counter to {}", next_usable_seq);
        Ok(())
    }

    /// Resets a transaction to its pre-prepare state for reprocessing through the pipeline.
    /// This is used when a transaction fails with a bad sequence error and needs to be retried.
    pub async fn reset_transaction_for_retry(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        info!("Resetting transaction {} for retry through pipeline", tx.id);

        // Use the model's built-in reset method
        let update_req = tx.create_reset_update_request()?;

        // Update the transaction
        let reset_tx = self
            .transaction_repository()
            .partial_update(tx.id.clone(), update_req)
            .await?;

        info!(
            "Transaction {} reset successfully to pre-prepare state",
            reset_tx.id
        );
        Ok(reset_tx)
    }
}

#[async_trait]
impl<R, T, J, S, P, C> Transaction for StellarRelayerTransaction<R, T, J, S, P, C>
where
    R: Repository<RelayerRepoModel, String> + Send + Sync,
    T: TransactionRepository + Send + Sync,
    J: JobProducerTrait + Send + Sync,
    S: Signer + Send + Sync,
    P: StellarProviderTrait + Send + Sync,
    C: TransactionCounterTrait + Send + Sync,
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
        Ok(tx)
    }

    async fn handle_transaction_status(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        self.handle_transaction_status_impl(tx).await
    }

    async fn cancel_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        Ok(tx)
    }

    async fn replace_transaction(
        &self,
        _old_tx: TransactionRepoModel,
        _new_tx_request: NetworkTransactionRequest,
    ) -> Result<TransactionRepoModel, TransactionError> {
        Ok(_old_tx)
    }

    async fn sign_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        Ok(tx)
    }

    async fn validate_transaction(
        &self,
        _tx: TransactionRepoModel,
    ) -> Result<bool, TransactionError> {
        Ok(true)
    }
}

pub type DefaultStellarTransaction = StellarRelayerTransaction<
    RelayerRepositoryStorage,
    TransactionRepositoryStorage,
    JobProducer,
    StellarSigner,
    StellarProvider,
    TransactionCounterRepositoryStorage,
>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{NetworkTransactionData, RepositoryError};
    use std::sync::Arc;

    use crate::domain::transaction::stellar::test_helpers::*;

    #[test]
    fn new_returns_ok() {
        let relayer = create_test_relayer();
        let mocks = default_test_mocks();
        let result = StellarRelayerTransaction::new(
            relayer,
            Arc::new(mocks.relayer_repo),
            Arc::new(mocks.tx_repo),
            Arc::new(mocks.job_producer),
            Arc::new(mocks.signer),
            mocks.provider,
            Arc::new(mocks.counter),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn accessor_methods_return_correct_references() {
        let relayer = create_test_relayer();
        let mocks = default_test_mocks();
        let handler = make_stellar_tx_handler(relayer.clone(), mocks);

        // Test all accessor methods
        assert_eq!(handler.relayer().id, "relayer-1");
        assert_eq!(handler.relayer().address, TEST_PK);

        // These should not panic and return valid references
        let _ = handler.provider();
        let _ = handler.job_producer();
        let _ = handler.transaction_repository();
        let _ = handler.signer();
        let _ = handler.transaction_counter_service();
    }

    #[tokio::test]
    async fn send_transaction_request_job_success() {
        let relayer = create_test_relayer();
        let mut mocks = default_test_mocks();

        mocks
            .job_producer
            .expect_produce_transaction_request_job()
            .withf(|job, delay| {
                job.transaction_id == "tx-1" && job.relayer_id == "relayer-1" && delay.is_none()
            })
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let handler = make_stellar_tx_handler(relayer.clone(), mocks);
        let tx = create_test_transaction(&relayer.id);

        let result = handler.send_transaction_request_job(&tx, None).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn send_transaction_request_job_with_delay() {
        let relayer = create_test_relayer();
        let mut mocks = default_test_mocks();

        mocks
            .job_producer
            .expect_produce_transaction_request_job()
            .withf(|job, delay| {
                job.transaction_id == "tx-1" && job.relayer_id == "relayer-1" && delay == &Some(60)
            })
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let handler = make_stellar_tx_handler(relayer.clone(), mocks);
        let tx = create_test_transaction(&relayer.id);

        let result = handler.send_transaction_request_job(&tx, Some(60)).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn finalize_transaction_state_success() {
        let relayer = create_test_relayer();
        let mut mocks = default_test_mocks();

        // Mock repository update
        mocks
            .tx_repo
            .expect_partial_update()
            .withf(|tx_id, update| {
                tx_id == "tx-1"
                    && update.status == Some(TransactionStatus::Confirmed)
                    && update.status_reason == Some("Transaction confirmed".to_string())
            })
            .times(1)
            .returning(|tx_id, update| {
                let mut tx = create_test_transaction("relayer-1");
                tx.id = tx_id;
                tx.status = update.status.unwrap();
                tx.status_reason = update.status_reason;
                tx.confirmed_at = update.confirmed_at;
                Ok::<_, RepositoryError>(tx)
            });

        // Mock notification
        mocks
            .job_producer
            .expect_produce_send_notification_job()
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let handler = make_stellar_tx_handler(relayer, mocks);

        let update_request = TransactionUpdateRequest {
            status: Some(TransactionStatus::Confirmed),
            status_reason: Some("Transaction confirmed".to_string()),
            confirmed_at: Some("2023-01-01T00:00:00Z".to_string()),
            ..Default::default()
        };

        let result = handler
            .finalize_transaction_state("tx-1".to_string(), update_request)
            .await;

        assert!(result.is_ok());
        let updated_tx = result.unwrap();
        assert_eq!(updated_tx.status, TransactionStatus::Confirmed);
        assert_eq!(
            updated_tx.status_reason,
            Some("Transaction confirmed".to_string())
        );
    }

    #[tokio::test]
    async fn enqueue_next_pending_transaction_with_pending_tx() {
        let relayer = create_test_relayer();
        let mut mocks = default_test_mocks();

        // Mock finding a pending transaction
        let mut pending_tx = create_test_transaction(&relayer.id);
        pending_tx.id = "pending-tx-1".to_string();

        mocks
            .tx_repo
            .expect_find_by_status()
            .withf(|relayer_id, statuses| {
                relayer_id == "relayer-1" && statuses == [TransactionStatus::Pending]
            })
            .times(1)
            .returning(move |_, _| {
                let mut tx = create_test_transaction("relayer-1");
                tx.id = "pending-tx-1".to_string();
                Ok(vec![tx])
            });

        // Mock job production for the next transaction
        mocks
            .job_producer
            .expect_produce_transaction_request_job()
            .withf(|job, delay| job.transaction_id == "pending-tx-1" && delay.is_none())
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let handler = make_stellar_tx_handler(relayer, mocks);

        let result = handler
            .enqueue_next_pending_transaction("finished-tx")
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn enqueue_next_pending_transaction_no_pending_tx() {
        let relayer = create_test_relayer();
        let mut mocks = default_test_mocks();

        // Mock finding no pending transactions
        mocks
            .tx_repo
            .expect_find_by_status()
            .times(1)
            .returning(|_, _| Ok(vec![]));

        let handler = make_stellar_tx_handler(relayer, mocks);

        let result = handler
            .enqueue_next_pending_transaction("finished-tx")
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_sync_sequence_from_chain() {
        let relayer = create_test_relayer();
        let mut mocks = default_test_mocks();

        // Mock provider to return account with sequence 100
        mocks
            .provider
            .expect_get_account()
            .withf(|addr| addr == TEST_PK)
            .times(1)
            .returning(|_| {
                Box::pin(async {
                    use soroban_rs::xdr::{
                        AccountEntry, AccountEntryExt, AccountId, PublicKey, SequenceNumber,
                        String32, Thresholds, Uint256,
                    };
                    use stellar_strkey::ed25519;

                    // Create a dummy public key for account ID
                    let pk = ed25519::PublicKey::from_string(TEST_PK).unwrap();
                    let account_id = AccountId(PublicKey::PublicKeyTypeEd25519(Uint256(pk.0)));

                    Ok(AccountEntry {
                        account_id,
                        balance: 1000000,
                        seq_num: SequenceNumber(100),
                        num_sub_entries: 0,
                        inflation_dest: None,
                        flags: 0,
                        home_domain: String32::default(),
                        thresholds: Thresholds([1, 1, 1, 1]),
                        signers: Default::default(),
                        ext: AccountEntryExt::V0,
                    })
                })
            });

        // Mock counter set to verify it's called with next usable sequence (101)
        mocks
            .counter
            .expect_set()
            .withf(|relayer_id, addr, seq| {
                relayer_id == "relayer-1" && addr == TEST_PK && *seq == 101
            })
            .times(1)
            .returning(|_, _, _| Box::pin(async { Ok(()) }));

        let handler = make_stellar_tx_handler(relayer.clone(), mocks);

        let result = handler.sync_sequence_from_chain(&relayer.address).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_sync_sequence_from_chain_provider_error() {
        let relayer = create_test_relayer();
        let mut mocks = default_test_mocks();

        // Mock provider to fail
        mocks
            .provider
            .expect_get_account()
            .times(1)
            .returning(|_| Box::pin(async { Err(eyre::eyre!("Account not found")) }));

        let handler = make_stellar_tx_handler(relayer.clone(), mocks);

        let result = handler.sync_sequence_from_chain(&relayer.address).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            TransactionError::UnexpectedError(msg) => {
                assert!(msg.contains("Failed to fetch account from chain"));
            }
            _ => panic!("Expected UnexpectedError"),
        }
    }

    #[tokio::test]
    async fn test_sync_sequence_from_chain_counter_error() {
        let relayer = create_test_relayer();
        let mut mocks = default_test_mocks();

        // Mock provider success
        mocks.provider.expect_get_account().times(1).returning(|_| {
            Box::pin(async {
                use soroban_rs::xdr::{
                    AccountEntry, AccountEntryExt, AccountId, PublicKey, SequenceNumber, String32,
                    Thresholds, Uint256,
                };
                use stellar_strkey::ed25519;

                // Create a dummy public key for account ID
                let pk = ed25519::PublicKey::from_string(TEST_PK).unwrap();
                let account_id = AccountId(PublicKey::PublicKeyTypeEd25519(Uint256(pk.0)));

                Ok(AccountEntry {
                    account_id,
                    balance: 1000000,
                    seq_num: SequenceNumber(100),
                    num_sub_entries: 0,
                    inflation_dest: None,
                    flags: 0,
                    home_domain: String32::default(),
                    thresholds: Thresholds([1, 1, 1, 1]),
                    signers: Default::default(),
                    ext: AccountEntryExt::V0,
                })
            })
        });

        // Mock counter set to fail
        mocks.counter.expect_set().times(1).returning(|_, _, _| {
            Box::pin(async {
                Err(RepositoryError::Unknown(
                    "Counter update failed".to_string(),
                ))
            })
        });

        let handler = make_stellar_tx_handler(relayer.clone(), mocks);

        let result = handler.sync_sequence_from_chain(&relayer.address).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            TransactionError::UnexpectedError(msg) => {
                assert!(msg.contains("Failed to update sequence counter"));
            }
            _ => panic!("Expected UnexpectedError"),
        }
    }

    #[tokio::test]
    async fn test_reset_transaction_for_retry() {
        let relayer = create_test_relayer();
        let mut mocks = default_test_mocks();

        // Create a transaction with stellar data that has been prepared
        let mut tx = create_test_transaction(&relayer.id);
        if let NetworkTransactionData::Stellar(ref mut data) = tx.network_data {
            data.sequence_number = Some(42);
            data.signatures.push(dummy_signature());
            data.hash = Some("test-hash".to_string());
            data.signed_envelope_xdr = Some("test-xdr".to_string());
        }

        // Mock partial_update to reset transaction
        mocks
            .tx_repo
            .expect_partial_update()
            .withf(|tx_id, upd| {
                tx_id == "tx-1"
                    && upd.status == Some(TransactionStatus::Pending)
                    && upd.sent_at.is_none()
                    && upd.confirmed_at.is_none()
            })
            .times(1)
            .returning(|id, upd| {
                let mut tx = create_test_transaction("relayer-1");
                tx.id = id;
                tx.status = upd.status.unwrap();
                if let Some(network_data) = upd.network_data {
                    tx.network_data = network_data;
                }
                Ok::<_, RepositoryError>(tx)
            });

        let handler = make_stellar_tx_handler(relayer.clone(), mocks);

        let result = handler.reset_transaction_for_retry(tx).await;
        assert!(result.is_ok());

        let reset_tx = result.unwrap();
        assert_eq!(reset_tx.status, TransactionStatus::Pending);

        // Verify stellar data was reset
        if let NetworkTransactionData::Stellar(data) = &reset_tx.network_data {
            assert!(data.sequence_number.is_none());
            assert!(data.signatures.is_empty());
            assert!(data.hash.is_none());
            assert!(data.signed_envelope_xdr.is_none());
        } else {
            panic!("Expected Stellar transaction data");
        }
    }
}
