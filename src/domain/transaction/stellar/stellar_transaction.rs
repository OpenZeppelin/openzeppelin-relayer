/// This module defines the `StellarRelayerTransaction` struct and its associated
/// functionality for handling Stellar transactions.
/// It includes methods for preparing, submitting, handling status, and
/// managing notifications for transactions. The module leverages various
/// services and repositories to perform these operations asynchronously.
use crate::{
    constants::DEFAULT_STELLAR_CONCURRENT_TRANSACTIONS,
    domain::transaction::{stellar::fetch_next_sequence_from_chain, Transaction},
    jobs::{JobProducer, JobProducerTrait, StatusCheckContext, TransactionRequest},
    models::{
        produce_transaction_update_notification_payload, NetworkTransactionRequest,
        PaginationQuery, RelayerNetworkPolicy, RelayerRepoModel, TransactionError,
        TransactionRepoModel, TransactionStatus, TransactionUpdateRequest,
    },
    repositories::{
        RelayerRepositoryStorage, Repository, TransactionCounterRepositoryStorage,
        TransactionCounterTrait, TransactionRepository, TransactionRepositoryStorage,
    },
    services::{
        provider::{StellarProvider, StellarProviderTrait},
        signer::{Signer, StellarSigner},
        stellar_dex::{OrderBookService, StellarDexServiceTrait},
    },
    utils::calculate_scheduled_timestamp,
};
use async_trait::async_trait;
use std::sync::Arc;
use tracing::{error, info, warn};

use super::lane_gate;

#[allow(dead_code)]
pub struct StellarRelayerTransaction<R, T, J, S, P, C, D>
where
    R: Repository<RelayerRepoModel, String>,
    T: TransactionRepository,
    J: JobProducerTrait,
    S: Signer,
    P: StellarProviderTrait,
    C: TransactionCounterTrait,
    D: StellarDexServiceTrait + Send + Sync + 'static,
{
    relayer: RelayerRepoModel,
    relayer_repository: Arc<R>,
    transaction_repository: Arc<T>,
    job_producer: Arc<J>,
    signer: Arc<S>,
    provider: P,
    transaction_counter_service: Arc<C>,
    dex_service: Arc<D>,
}

#[allow(dead_code)]
impl<R, T, J, S, P, C, D> StellarRelayerTransaction<R, T, J, S, P, C, D>
where
    R: Repository<RelayerRepoModel, String>,
    T: TransactionRepository,
    J: JobProducerTrait,
    S: Signer,
    P: StellarProviderTrait,
    C: TransactionCounterTrait,
    D: StellarDexServiceTrait + Send + Sync + 'static,
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
    /// * `dex_service` - The DEX service implementation for swap operations and validations.
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
        dex_service: Arc<D>,
    ) -> Result<Self, TransactionError> {
        Ok(Self {
            relayer,
            relayer_repository,
            transaction_repository,
            job_producer,
            signer,
            provider,
            transaction_counter_service,
            dex_service,
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

    pub fn dex_service(&self) -> &D {
        &self.dex_service
    }

    pub fn concurrent_transactions_enabled(&self) -> bool {
        if let RelayerNetworkPolicy::Stellar(policy) = &self.relayer().policies {
            policy
                .concurrent_transactions
                .unwrap_or(DEFAULT_STELLAR_CONCURRENT_TRANSACTIONS)
        } else {
            DEFAULT_STELLAR_CONCURRENT_TRANSACTIONS
        }
    }

    /// Send a transaction-request job for the given transaction.
    pub async fn send_transaction_request_job(
        &self,
        tx: &TransactionRepoModel,
        delay_seconds: Option<i64>,
    ) -> Result<(), TransactionError> {
        let job = TransactionRequest::new(tx.id.clone(), tx.relayer_id.clone());
        let scheduled_on = delay_seconds.map(calculate_scheduled_timestamp);
        self.job_producer()
            .produce_transaction_request_job(job, scheduled_on)
            .await?;
        Ok(())
    }

    /// Sends a transaction update notification if a notification ID is configured.
    ///
    /// This is a best-effort operation that logs errors but does not propagate them,
    /// as notification failures should not affect the transaction lifecycle.
    pub(super) async fn send_transaction_update_notification(&self, tx: &TransactionRepoModel) {
        if let Some(notification_id) = &self.relayer().notification_id {
            if let Err(e) = self
                .job_producer()
                .produce_send_notification_job(
                    produce_transaction_update_notification_payload(notification_id, tx),
                    None,
                )
                .await
            {
                error!(error = %e, "failed to produce notification job");
            }
        }
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

        self.send_transaction_update_notification(&updated_tx).await;
        Ok(updated_tx)
    }

    pub async fn enqueue_next_pending_transaction(
        &self,
        finished_tx_id: &str,
    ) -> Result<(), TransactionError> {
        if !self.concurrent_transactions_enabled() {
            if let Some(next) = self
                .find_oldest_pending_for_relayer(&self.relayer().id)
                .await?
            {
                // Atomic hand-over while still owning the lane
                info!(to_tx_id = %next.id, finished_tx_id = %finished_tx_id, "handing over lane");
                lane_gate::pass_to(&self.relayer().id, finished_tx_id, &next.id);
                self.send_transaction_request_job(&next, None).await?;
            } else {
                info!(finished_tx_id = %finished_tx_id, "releasing relayer lane");
                lane_gate::free(&self.relayer().id, finished_tx_id);
            }
        }
        Ok(())
    }

    /// Finds the oldest pending transaction for a relayer.
    ///
    /// Uses optimized paginated query with `oldest_first: true` and `per_page: 1`
    /// to fetch only the single oldest pending transaction from Redis in O(log N).
    async fn find_oldest_pending_for_relayer(
        &self,
        relayer_id: &str,
    ) -> Result<Option<TransactionRepoModel>, TransactionError> {
        let result = self
            .transaction_repository()
            .find_by_status_paginated(
                relayer_id,
                &[TransactionStatus::Pending],
                PaginationQuery {
                    page: 1,
                    per_page: 1,
                },
                true, // oldest_first=true - query returns oldest transaction first
            )
            .await
            .map_err(TransactionError::from)?;

        // oldest_first=true so .next() yields the oldest pending transaction (FIFO order)
        Ok(result.items.into_iter().next())
    }

    /// Syncs the sequence number from the blockchain for the relayer's address.
    /// This fetches the on-chain sequence number and updates the local counter to the next usable value.
    pub async fn sync_sequence_from_chain(
        &self,
        relayer_address: &str,
    ) -> Result<(), TransactionError> {
        info!(address = %relayer_address, "syncing sequence number from chain");

        // Use the shared helper to fetch the next sequence
        let next_usable_seq = fetch_next_sequence_from_chain(self.provider(), relayer_address)
            .await
            .map_err(|e| {
                warn!(
                    address = %relayer_address,
                    error = %e,
                    "failed to fetch sequence from chain in sync_sequence_from_chain"
                );
                TransactionError::UnexpectedError(format!("Failed to sync sequence from chain: {e}"))
            })?;

        // Update the local counter to the next usable sequence
        self.transaction_counter_service()
            .set(&self.relayer().id, relayer_address, next_usable_seq)
            .await
            .map_err(|e| {
                TransactionError::UnexpectedError(format!("Failed to update sequence counter: {e}"))
            })?;

        info!(sequence = %next_usable_seq, "updated local sequence counter");
        Ok(())
    }

    /// Resets a transaction to its pre-prepare state for reprocessing through the pipeline.
    /// This is used when a transaction fails with a bad sequence error and needs to be retried.
    pub async fn reset_transaction_for_retry(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        info!("resetting transaction for retry through pipeline");

        // Use the model's built-in reset method
        let update_req = tx.create_reset_update_request()?;

        // Update the transaction
        let reset_tx = self
            .transaction_repository()
            .partial_update(tx.id.clone(), update_req)
            .await?;

        info!("transaction reset successfully to pre-prepare state");
        Ok(reset_tx)
    }
}

#[async_trait]
impl<R, T, J, S, P, C, D> Transaction for StellarRelayerTransaction<R, T, J, S, P, C, D>
where
    R: Repository<RelayerRepoModel, String> + Send + Sync,
    T: TransactionRepository + Send + Sync,
    J: JobProducerTrait + Send + Sync,
    S: Signer + Send + Sync,
    P: StellarProviderTrait + Send + Sync,
    C: TransactionCounterTrait + Send + Sync,
    D: StellarDexServiceTrait + Send + Sync + 'static,
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
        context: Option<StatusCheckContext>,
    ) -> Result<TransactionRepoModel, TransactionError> {
        self.handle_transaction_status_impl(tx, context).await
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
    OrderBookService<StellarProvider, StellarSigner>,
>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repositories::transaction::RedisTransactionRepository;
    use crate::utils::RedisConnections;
    use crate::{
        models::{NetworkTransactionData, RepositoryError},
        services::provider::ProviderError,
    };
    use deadpool_redis::{Config, Runtime};
    use std::sync::Arc;
    use uuid::Uuid;

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
            Arc::new(mocks.dex_service),
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
                job.transaction_id == "tx-1"
                    && job.relayer_id == "relayer-1"
                    && delay.is_some()
                    && delay.unwrap() > chrono::Utc::now().timestamp()
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
            .expect_find_by_status_paginated()
            .withf(|_relayer_id, statuses, query, oldest_first| {
                statuses == [TransactionStatus::Pending]
                    && query.page == 1
                    && query.per_page == 1
                    && *oldest_first == true
            })
            .times(1)
            .returning(move |_, _, _, _| {
                let mut tx = create_test_transaction("relayer-1");
                tx.id = "pending-tx-1".to_string();
                Ok(crate::repositories::PaginatedResult {
                    items: vec![tx],
                    total: 1,
                    page: 1,
                    per_page: 1,
                })
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
            .expect_find_by_status_paginated()
            .withf(|_relayer_id, statuses, query, oldest_first| {
                statuses == [TransactionStatus::Pending]
                    && query.page == 1
                    && query.per_page == 1
                    && *oldest_first == true
            })
            .times(1)
            .returning(|_, _, _, _| {
                Ok(crate::repositories::PaginatedResult {
                    items: vec![],
                    total: 0,
                    page: 1,
                    per_page: 1,
                })
            });

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
        mocks.provider.expect_get_account().times(1).returning(|_| {
            Box::pin(async { Err(ProviderError::Other("Account not found".to_string())) })
        });

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

    #[test]
    fn test_concurrent_transactions_enabled() {
        // Test with concurrent transactions explicitly enabled
        let mut relayer = create_test_relayer();
        if let RelayerNetworkPolicy::Stellar(ref mut policy) = relayer.policies {
            policy.concurrent_transactions = Some(true);
        }
        let mocks = default_test_mocks();
        let handler = make_stellar_tx_handler(relayer, mocks);
        assert!(handler.concurrent_transactions_enabled());

        // Test with concurrent transactions explicitly disabled
        let mut relayer = create_test_relayer();
        if let RelayerNetworkPolicy::Stellar(ref mut policy) = relayer.policies {
            policy.concurrent_transactions = Some(false);
        }
        let mocks = default_test_mocks();
        let handler = make_stellar_tx_handler(relayer, mocks);
        assert!(!handler.concurrent_transactions_enabled());

        // Test with default (None) - should use DEFAULT_STELLAR_CONCURRENT_TRANSACTIONS
        let relayer = create_test_relayer();
        let mocks = default_test_mocks();
        let handler = make_stellar_tx_handler(relayer, mocks);
        assert_eq!(
            handler.concurrent_transactions_enabled(),
            DEFAULT_STELLAR_CONCURRENT_TRANSACTIONS
        );
    }

    #[tokio::test]
    async fn test_enqueue_next_pending_transaction_with_concurrency_enabled() {
        // With concurrent transactions enabled, lane management should be skipped
        let mut relayer = create_test_relayer();
        if let RelayerNetworkPolicy::Stellar(ref mut policy) = relayer.policies {
            policy.concurrent_transactions = Some(true);
        }
        let mut mocks = default_test_mocks();

        // Should NOT look for pending transactions when concurrency is enabled
        mocks.tx_repo.expect_find_by_status_paginated().times(0); // Expect zero calls

        // Should NOT produce any job when concurrency is enabled
        mocks
            .job_producer
            .expect_produce_transaction_request_job()
            .times(0); // Expect zero calls

        let handler = make_stellar_tx_handler(relayer, mocks);

        let result = handler
            .enqueue_next_pending_transaction("finished-tx")
            .await;
        assert!(result.is_ok());
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

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_find_oldest_pending_for_relayer_with_redis() {
        // Setup Redis repository
        let redis_url = std::env::var("REDIS_TEST_URL")
            .unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
        let pool = Arc::new(
            Config::from_url(&redis_url)
                .builder()
                .expect("Failed to create Redis pool builder")
                .max_size(16)
                .runtime(Runtime::Tokio1)
                .build()
                .expect("Failed to build Redis pool"),
        );
        let connections = Arc::new(RedisConnections::new_single_pool(pool));

        let random_id = Uuid::new_v4().to_string();
        let key_prefix = format!("test_stellar:{}", random_id);
        let tx_repo = Arc::new(
            RedisTransactionRepository::new(connections, key_prefix)
                .expect("Failed to create RedisTransactionRepository"),
        );

        let relayer_id = format!("relayer-{}", Uuid::new_v4());

        // Create three pending transactions with different created_at timestamps
        // tx1: oldest (created first)
        let mut tx1 = create_test_transaction(&relayer_id);
        tx1.id = format!("tx-1-{}", Uuid::new_v4());
        tx1.status = TransactionStatus::Pending;
        tx1.created_at = "2025-01-27T10:00:00.000000+00:00".to_string();

        // tx2: middle
        let mut tx2 = create_test_transaction(&relayer_id);
        tx2.id = format!("tx-2-{}", Uuid::new_v4());
        tx2.status = TransactionStatus::Pending;
        tx2.created_at = "2025-01-27T11:00:00.000000+00:00".to_string();

        // tx3: newest (created last)
        let mut tx3 = create_test_transaction(&relayer_id);
        tx3.id = format!("tx-3-{}", Uuid::new_v4());
        tx3.status = TransactionStatus::Pending;
        tx3.created_at = "2025-01-27T12:00:00.000000+00:00".to_string();

        // Create transactions in Redis
        tx_repo.create(tx1.clone()).await.unwrap();
        tx_repo.create(tx2.clone()).await.unwrap();
        tx_repo.create(tx3.clone()).await.unwrap();

        // Create a minimal StellarRelayerTransaction instance to test the method
        // We'll use mocks for other dependencies since we only need the transaction repository
        let relayer = create_test_relayer();
        let mut relayer_model = relayer.clone();
        relayer_model.id = relayer_id.clone();

        let mocks = default_test_mocks();
        let handler = StellarRelayerTransaction::new(
            relayer_model,
            Arc::new(mocks.relayer_repo),
            tx_repo.clone(),
            Arc::new(mocks.job_producer),
            Arc::new(mocks.signer),
            mocks.provider,
            Arc::new(mocks.counter),
            Arc::new(mocks.dex_service),
        )
        .unwrap();

        // Call find_oldest_pending_for_relayer
        let result = handler
            .find_oldest_pending_for_relayer(&relayer_id)
            .await
            .unwrap();

        // Verify the result
        assert!(result.is_some(), "Should find a pending transaction");
        let found_tx = result.unwrap();

        assert_eq!(
            found_tx.id, tx1.id,
            "Should get oldest transaction (tx1) - oldest_first=true so .next() yields oldest"
        );
        assert_eq!(
            found_tx.created_at, tx1.created_at,
            "Should match oldest transaction's created_at"
        );

        // Cleanup: delete test transactions
        let _ = tx_repo.delete_by_id(tx1.id.clone()).await;
        let _ = tx_repo.delete_by_id(tx2.id.clone()).await;
        let _ = tx_repo.delete_by_id(tx3.id.clone()).await;
    }

    #[tokio::test]
    async fn test_find_oldest_pending_for_relayer_with_in_memory() {
        use crate::repositories::transaction::InMemoryTransactionRepository;
        use uuid::Uuid;

        // Setup in-memory repository
        let tx_repo = Arc::new(InMemoryTransactionRepository::new());

        let relayer_id = format!("relayer-{}", Uuid::new_v4());

        // Create three pending transactions with different created_at timestamps
        // tx1: oldest (created first)
        let mut tx1 = create_test_transaction(&relayer_id);
        tx1.id = format!("tx-1-{}", Uuid::new_v4());
        tx1.status = TransactionStatus::Pending;
        tx1.created_at = "2025-01-27T10:00:00.000000+00:00".to_string();

        // tx2: middle
        let mut tx2 = create_test_transaction(&relayer_id);
        tx2.id = format!("tx-2-{}", Uuid::new_v4());
        tx2.status = TransactionStatus::Pending;
        tx2.created_at = "2025-01-27T11:00:00.000000+00:00".to_string();

        // tx3: newest (created last)
        let mut tx3 = create_test_transaction(&relayer_id);
        tx3.id = format!("tx-3-{}", Uuid::new_v4());
        tx3.status = TransactionStatus::Pending;
        tx3.created_at = "2025-01-27T12:00:00.000000+00:00".to_string();

        // Create transactions in memory store
        tx_repo.create(tx1.clone()).await.unwrap();
        tx_repo.create(tx2.clone()).await.unwrap();
        tx_repo.create(tx3.clone()).await.unwrap();

        // Create a minimal StellarRelayerTransaction instance to test the method
        // We'll use mocks for other dependencies since we only need the transaction repository
        let relayer = create_test_relayer();
        let mut relayer_model = relayer.clone();
        relayer_model.id = relayer_id.clone();

        let mocks = default_test_mocks();
        let handler = StellarRelayerTransaction::new(
            relayer_model,
            Arc::new(mocks.relayer_repo),
            tx_repo.clone(),
            Arc::new(mocks.job_producer),
            Arc::new(mocks.signer),
            mocks.provider,
            Arc::new(mocks.counter),
            Arc::new(mocks.dex_service),
        )
        .unwrap();

        // Call find_oldest_pending_for_relayer
        let result = handler
            .find_oldest_pending_for_relayer(&relayer_id)
            .await
            .unwrap();

        // Verify the result
        assert!(result.is_some(), "Should find a pending transaction");
        let found_tx = result.unwrap();

        // oldest_first=true so .next() yields the oldest pending transaction (FIFO order)
        assert_eq!(
            found_tx.id, tx1.id,
            "Should get oldest transaction (tx1) - oldest_first=true so .next() yields oldest"
        );
        assert_eq!(
            found_tx.created_at, tx1.created_at,
            "Should match oldest transaction's created_at"
        );
    }
}
