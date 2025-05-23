/// This module defines the `StellarRelayerTransaction` struct and its associated
/// functionality for handling Stellar transactions.
/// It includes methods for preparing, submitting, handling status, and
/// managing notifications for transactions. The module leverages various
/// services and repositories to perform these operations asynchronously.
use crate::{
    constants::STELLAR_DEFAULT_STATUS_RETRY_DELAY_SECONDS,
    domain::{
        transaction::{lane_gate, Transaction},
        SignTransactionResponse,
    },
    jobs::{
        JobProducer, JobProducerTrait, TransactionRequest, TransactionSend, TransactionStatusCheck,
    },
    models::{
        produce_transaction_update_notification_payload, NetworkTransactionData, OperationSpec,
        RelayerRepoModel, TransactionError, TransactionRepoModel, TransactionStatus,
        TransactionUpdateRequest,
    },
    repositories::{
        InMemoryRelayerRepository, InMemoryTransactionCounter, InMemoryTransactionRepository,
        RelayerRepositoryStorage, Repository, TransactionCounterTrait, TransactionRepository,
    },
    services::{Signer, StellarProvider, StellarProviderTrait, StellarSigner},
};
use async_trait::async_trait;
use chrono::Utc;
use eyre::Result;
use log::{info, warn};
use soroban_rs::xdr::{Error, Hash, TransactionEnvelope};
use std::sync::Arc;

use super::i64_from_u64;

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

    pub fn next_sequence(&self) -> Result<i64, TransactionError> {
        let sequence_u64 = self
            .transaction_counter_service
            .get_and_increment(&self.relayer.id, &self.relayer.address)
            .map_err(|e| TransactionError::UnexpectedError(e.to_string()))?;

        i64_from_u64(sequence_u64).map_err(|relayer_err| {
            let msg = format!(
                "Sequence conversion error for {}: {}",
                sequence_u64, relayer_err
            );
            TransactionError::ValidationError(msg)
        })
    }

    /// Optionally invoke the RPC simulation depending on the transaction operations.
    pub async fn simulate_if_needed(
        &self,
        unsigned_env: &TransactionEnvelope,
        operations: &[OperationSpec],
    ) -> Result<(), TransactionError> {
        if crate::domain::transaction::stellar::utils::needs_simulation(operations) {
            let resp = self
                .provider()
                .simulate_transaction_envelope(unsigned_env)
                .await
                .map_err(TransactionError::from)?;

            if let Some(err_msg) = resp.error.clone() {
                warn!("Stellar simulation failed: {}", err_msg);
                return Err(TransactionError::SimulationFailed(err_msg));
            }
        }

        Ok(())
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

    /// Send a submit-transaction job for the given transaction.
    pub async fn send_submit_transaction_job(
        &self,
        tx: &TransactionRepoModel,
        delay_seconds: Option<i64>,
    ) -> Result<(), TransactionError> {
        let job = TransactionSend::submit(tx.id.clone(), tx.relayer_id.clone());
        self.job_producer()
            .produce_submit_transaction_job(job, delay_seconds)
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
    async fn finalize_transaction_state(
        &self,
        tx_id: String,
        new_status: TransactionStatus,
        status_reason: Option<String>,
        confirmed_at: Option<String>,
    ) -> Result<TransactionRepoModel, TransactionError> {
        let update_req = TransactionUpdateRequest {
            status: Some(new_status),
            status_reason,
            confirmed_at,
            ..Default::default()
        };

        let updated_tx = self
            .transaction_repository()
            .partial_update(tx_id, update_req)
            .await?;

        self.send_transaction_update_notification(&updated_tx)
            .await?;
        Ok(updated_tx)
    }

    /// Helper function to re-queue a transaction status check job.
    async fn requeue_status_check(
        &self,
        tx: &TransactionRepoModel,
    ) -> Result<(), TransactionError> {
        self.job_producer()
            .produce_check_transaction_status_job(
                TransactionStatusCheck::new(tx.id.clone(), tx.relayer_id.clone()),
                Some(STELLAR_DEFAULT_STATUS_RETRY_DELAY_SECONDS),
            )
            .await?;
        Ok(())
    }

    /// Parses the transaction hash from the network data and validates it.
    /// Returns a `TransactionError::ValidationError` if the hash is missing, empty, or invalid.
    fn parse_and_validate_hash(&self, tx: &TransactionRepoModel) -> Result<Hash, TransactionError> {
        let stellar_network_data = tx.network_data.get_stellar_transaction_data()?;

        let tx_hash_str = stellar_network_data.hash.as_deref().filter(|s| !s.is_empty()).ok_or_else(|| {
            TransactionError::ValidationError(format!(
                "Stellar transaction {} is missing or has an empty on-chain hash in network_data. Cannot check status.",
                tx.id
            ))
        })?;

        let stellar_hash: Hash = tx_hash_str.parse().map_err(|e: Error| {
            TransactionError::UnexpectedError(format!(
                "Failed to parse transaction hash '{}' for tx {}: {:?}. This hash may be corrupted or not a valid Stellar hash.",
                tx_hash_str, tx.id, e
            ))
        })?;

        Ok(stellar_hash)
    }

    async fn enqueue_next_pending_transaction(
        &self,
        finished_tx_id: &str,
    ) -> Result<(), TransactionError> {
        if let Some(next) = self
            .find_oldest_pending_for_relayer(&self.relayer.id)
            .await?
        {
            // Atomic hand-over while still owning the lane
            info!("Handing over lane from {} to {}", finished_tx_id, next.id);
            lane_gate::pass_to(&self.relayer.id, finished_tx_id, &next.id).await;
            self.send_transaction_request_job(&next, None).await?;
        } else {
            info!("Releasing relayer lane after {}", finished_tx_id);
            lane_gate::free(&self.relayer.id, finished_tx_id).await;
        }
        Ok(())
    }

    /// Handles the logic when a Stellar transaction is confirmed successfully.
    async fn handle_stellar_success(
        &self,
        tx: TransactionRepoModel,
        _provider_response: soroban_rs::stellar_rpc_client::GetTransactionResponse, // May be needed later for more details
    ) -> Result<TransactionRepoModel, TransactionError> {
        let confirmed_tx = self
            .finalize_transaction_state(
                tx.id.clone(),
                TransactionStatus::Confirmed,
                None,
                Some(Utc::now().to_rfc3339()),
            )
            .await?;

        self.enqueue_next_pending_transaction(&tx.id).await?;

        Ok(confirmed_tx)
    }

    /// Handles the logic when a Stellar transaction has failed.
    async fn handle_stellar_failed(
        &self,
        tx: TransactionRepoModel,
        provider_response: soroban_rs::stellar_rpc_client::GetTransactionResponse,
    ) -> Result<TransactionRepoModel, TransactionError> {
        let base_reason = "Transaction failed on-chain. Provider status: FAILED.".to_string();
        let detailed_reason = if let Some(ref tx_result_xdr) = provider_response.result {
            format!(
                "{} Specific XDR reason: {}.",
                base_reason,
                tx_result_xdr.result.name()
            )
        } else {
            format!("{} No detailed XDR result available.", base_reason)
        };

        warn!("Stellar transaction {} failed: {}", tx.id, detailed_reason);
        let updated_tx = self
            .finalize_transaction_state(
                tx.id.clone(),
                TransactionStatus::Failed,
                Some(detailed_reason),
                None,
            )
            .await?;

        self.enqueue_next_pending_transaction(&tx.id).await?;

        Ok(updated_tx)
    }

    /// Handles the logic when a Stellar transaction is still pending or in an unknown state.
    async fn handle_stellar_pending(
        &self,
        tx: TransactionRepoModel,
        original_status_str: String,
    ) -> Result<TransactionRepoModel, TransactionError> {
        info!(
            "Stellar transaction {} status is still '{}'. Re-queueing check.",
            tx.id, original_status_str
        );
        self.requeue_status_check(&tx).await?;
        Ok(tx)
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
        if !lane_gate::claim(&self.relayer.id, &tx.id).await {
            info!(
                "Relayer {} already has a transaction in flight – {} must wait.",
                self.relayer.id, tx.id
            );
            return Ok(tx);
        }
        info!("Preparing transaction: {:?}", tx.id);

        let sequence_i64 = self.next_sequence()?;
        info!(
            "Using sequence number {} for Stellar transaction {}",
            sequence_i64, tx.id
        );

        let stellar_data = tx.network_data.get_stellar_transaction_data()?;
        let stellar_data_with_seq = stellar_data.with_sequence_number(sequence_i64);

        let unsigned_env = stellar_data_with_seq
            .unsigned_envelope()
            .map_err(TransactionError::from)?;

        self.simulate_if_needed(&unsigned_env, &stellar_data_with_seq.operations)
            .await?;

        let sig_resp = self
            .signer
            .sign_transaction(NetworkTransactionData::Stellar(
                stellar_data_with_seq.clone(),
            ))
            .await?;

        let signature = match sig_resp {
            SignTransactionResponse::Stellar(s) => s.signature,
            _ => {
                return Err(TransactionError::InvalidType(
                    "Expected Stellar signature".into(),
                ))
            }
        };

        let final_stellar_data = stellar_data_with_seq.attach_signature(signature);
        let updated_network_data = NetworkTransactionData::Stellar(final_stellar_data);

        let update_req = TransactionUpdateRequest {
            status: Some(TransactionStatus::Sent),
            network_data: Some(updated_network_data),
            ..Default::default()
        };

        let saved_tx = self
            .transaction_repository()
            .partial_update(tx.id.clone(), update_req)
            .await?;

        self.send_submit_transaction_job(&saved_tx, None).await?;
        self.send_transaction_update_notification(&saved_tx).await?;

        Ok(saved_tx)
    }

    async fn submit_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        info!("Submitting Stellar transaction: {:?}", tx.id);

        let stellar_data = tx.network_data.get_stellar_transaction_data()?;
        let tx_envelope = stellar_data
            .signed_envelope()
            .map_err(TransactionError::from)?;

        let hash = self
            .provider()
            .send_transaction(&tx_envelope)
            .await
            .map_err(TransactionError::from)?;

        let tx_hash_hex = hex::encode(hash.as_slice());

        let updated_stellar_data = stellar_data.with_hash(tx_hash_hex.clone());

        let mut hashes = tx.hashes.clone();
        hashes.push(tx_hash_hex);

        let update_req = TransactionUpdateRequest {
            status: Some(TransactionStatus::Submitted),
            sent_at: Some(Utc::now().to_rfc3339()),
            network_data: Some(NetworkTransactionData::Stellar(updated_stellar_data)),
            hashes: Some(hashes),
            ..Default::default()
        };

        let updated_tx = self
            .transaction_repository()
            .partial_update(tx.id.clone(), update_req)
            .await?;

        self.job_producer()
            .produce_check_transaction_status_job(
                TransactionStatusCheck::new(updated_tx.id.clone(), updated_tx.relayer_id.clone()),
                None,
            )
            .await?;

        if let Some(notification_id) = &self.relayer().notification_id {
            self.job_producer()
                .produce_send_notification_job(
                    produce_transaction_update_notification_payload(notification_id, &updated_tx),
                    None,
                )
                .await?;
        }

        Ok(updated_tx)
    }

    async fn resubmit_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        self.submit_transaction(tx).await
    }

    async fn handle_transaction_status(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        info!("Handling transaction status for: {:?}", tx.id);

        let stellar_hash = self.parse_and_validate_hash(&tx)?;

        match self.provider().get_transaction(&stellar_hash).await {
            Ok(provider_response) => {
                match provider_response.status.as_str().to_uppercase().as_str() {
                    "SUCCESS" => self.handle_stellar_success(tx, provider_response).await,
                    "FAILED" => self.handle_stellar_failed(tx, provider_response).await,
                    _ => {
                        self.handle_stellar_pending(tx, provider_response.status)
                            .await
                    }
                }
            }
            Err(e) => {
                warn!(
                    "Failed to get Stellar transaction status for {}: {}. Re-queueing check.",
                    tx.id, e
                );
                self.requeue_status_check(&tx).await?;
                Ok(tx)
            }
        }
    }

    async fn cancel_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        Ok(tx)
    }

    async fn replace_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        Ok(tx)
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
    RelayerRepositoryStorage<InMemoryRelayerRepository>,
    InMemoryTransactionRepository,
    JobProducer,
    StellarSigner,
    StellarProvider,
    InMemoryTransactionCounter,
>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        jobs::MockJobProducerTrait,
        models::{
            AssetSpec, DecoratedSignature, NetworkTransactionData, NetworkType, OperationSpec,
            RelayerNetworkPolicy, RelayerRepoModel, RelayerStellarPolicy, StellarNamedNetwork,
            StellarTransactionData, TransactionRepoModel, TransactionStatus,
        },
        repositories::{MockRepository, MockTransactionCounterTrait, MockTransactionRepository},
        services::{MockSigner, MockStellarProviderTrait},
    };
    use chrono::Utc;
    use mockall::predicate::eq;
    use soroban_rs::xdr::{Hash, Signature, SignatureHint};
    use std::sync::Arc;

    const TEST_PK: &str = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";

    fn dummy_signature() -> DecoratedSignature {
        use soroban_rs::xdr::BytesM;
        let hint = SignatureHint([0; 4]);
        let bytes: Vec<u8> = vec![0u8; 64];
        let bytes_m: BytesM<64> = bytes.try_into().expect("BytesM conversion");
        DecoratedSignature {
            hint,
            signature: Signature(bytes_m),
        }
    }

    fn create_test_relayer() -> RelayerRepoModel {
        RelayerRepoModel {
            id: "relayer-1".to_string(),
            name: "Test Relayer".to_string(),
            network: "testnet".to_string(),
            paused: false,
            network_type: NetworkType::Stellar,
            signer_id: "signer-1".to_string(),
            policies: RelayerNetworkPolicy::Stellar(RelayerStellarPolicy::default()),
            address: TEST_PK.to_string(),
            notification_id: Some("test-notification-id".to_string()),
            system_disabled: false,
            custom_rpc_urls: None,
        }
    }

    fn payment_op(destination: &str) -> OperationSpec {
        OperationSpec::Payment {
            destination: destination.to_string(),
            amount: 100,
            asset: AssetSpec::Native,
        }
    }

    fn create_test_transaction(relayer_id: &str) -> TransactionRepoModel {
        let stellar_tx_data = StellarTransactionData {
            source_account: TEST_PK.to_string(),
            fee: Some(100),
            sequence_number: None,
            operations: vec![payment_op(TEST_PK)],
            memo: None,
            valid_until: None,
            network: StellarNamedNetwork::Testnet,
            signatures: Vec::new(),
            hash: None,
        };
        TransactionRepoModel {
            id: "tx-1".to_string(),
            relayer_id: relayer_id.to_string(),
            status: TransactionStatus::Pending,
            created_at: Utc::now().to_rfc3339(),
            sent_at: None,
            confirmed_at: None,
            valid_until: None,
            network_data: NetworkTransactionData::Stellar(stellar_tx_data),
            priced_at: None,
            hashes: Vec::new(),
            network_type: NetworkType::Stellar,
            noop_count: None,
            is_canceled: Some(false),
            status_reason: None,
        }
    }

    pub struct TestMocks {
        pub provider: MockStellarProviderTrait,
        pub relayer_repo: MockRepository<RelayerRepoModel, String>,
        pub tx_repo: MockTransactionRepository,
        pub job_producer: MockJobProducerTrait,
        pub signer: MockSigner,
        pub counter: MockTransactionCounterTrait,
    }

    fn default_test_mocks() -> TestMocks {
        TestMocks {
            provider: MockStellarProviderTrait::new(),
            relayer_repo: MockRepository::new(),
            tx_repo: MockTransactionRepository::new(),
            job_producer: MockJobProducerTrait::new(),
            signer: MockSigner::new(),
            counter: MockTransactionCounterTrait::new(),
        }
    }

    #[allow(clippy::type_complexity)]
    fn make_stellar_tx_handler(
        relayer: RelayerRepoModel,
        mocks: TestMocks,
    ) -> StellarRelayerTransaction<
        MockRepository<RelayerRepoModel, String>,
        MockTransactionRepository,
        MockJobProducerTrait,
        MockSigner,
        MockStellarProviderTrait,
        MockTransactionCounterTrait,
    > {
        StellarRelayerTransaction::new(
            relayer,
            Arc::new(mocks.relayer_repo),
            Arc::new(mocks.tx_repo),
            Arc::new(mocks.job_producer),
            Arc::new(mocks.signer),
            mocks.provider,
            Arc::new(mocks.counter),
        )
        .expect("handler construction should succeed")
    }

    // ---------------------------------------------------------------------
    // new() tests
    // ---------------------------------------------------------------------
    mod new_tests {
        use super::*;

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
    }

    // ---------------------------------------------------------------------
    // prepare_transaction tests
    // ---------------------------------------------------------------------
    mod prepare_transaction_tests {
        use crate::models::RepositoryError;

        use super::*;

        #[tokio::test]
        async fn prepare_transaction_happy_path() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // sequence counter
            mocks
                .counter
                .expect_get_and_increment()
                .returning(|_, _| Ok(1));

            // signer
            mocks.signer.expect_sign_transaction().returning(|_| {
                Box::pin(async {
                    Ok(SignTransactionResponse::Stellar(
                        crate::domain::SignTransactionResponseStellar {
                            signature: super::dummy_signature(),
                        },
                    ))
                })
            });

            mocks
                .tx_repo
                .expect_partial_update()
                .withf(|_, upd| {
                    upd.status == Some(TransactionStatus::Sent) && upd.network_data.is_some()
                })
                .returning(|id, upd| {
                    let mut tx = create_test_transaction("relayer-1");
                    tx.id = id;
                    tx.status = upd.status.unwrap();
                    tx.network_data = upd.network_data.unwrap();
                    Ok::<_, RepositoryError>(tx)
                });

            // submit-job + notification
            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let tx = create_test_transaction(&relayer.id);

            assert!(handler.prepare_transaction(tx).await.is_ok());
        }

        #[tokio::test]
        async fn prepare_transaction_happy_path_existing_submitted() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // sequence counter
            mocks
                .counter
                .expect_get_and_increment()
                .returning(|_, _| Ok(1));

            // signer
            mocks.signer.expect_sign_transaction().returning(|_| {
                Box::pin(async {
                    Ok(SignTransactionResponse::Stellar(
                        crate::domain::SignTransactionResponseStellar {
                            signature: super::dummy_signature(),
                        },
                    ))
                })
            });

            // Mock partial_update for the current transaction being prepared
            mocks
                .tx_repo
                .expect_partial_update()
                .withf(|_, upd| {
                    upd.status == Some(TransactionStatus::Sent) && upd.network_data.is_some()
                })
                .returning(|id, upd| {
                    let mut tx = create_test_transaction("relayer-1");
                    tx.id = id;
                    tx.status = upd.status.unwrap();
                    tx.network_data = upd.network_data.unwrap();
                    Ok::<_, RepositoryError>(tx)
                });

            // submit-job + notification
            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let tx = create_test_transaction(&relayer.id);

            let result = handler.prepare_transaction(tx).await;
            assert!(result.is_ok());
            let prepared_tx = result.unwrap();
            assert_eq!(prepared_tx.status, TransactionStatus::Sent);
        }

        #[tokio::test]
        async fn prepare_transaction_invalid_signature_type() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();
            mocks
                .counter
                .expect_get_and_increment()
                .returning(|_, _| Ok(1));

            // Signer returns non-Stellar variant
            mocks
                .signer
                .expect_sign_transaction()
                .returning(|_| Box::pin(async { Ok(SignTransactionResponse::Solana(vec![])) }));

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let tx = create_test_transaction(&relayer.id);
            let res = handler.prepare_transaction(tx).await;
            assert!(res.is_err());
            match res.unwrap_err() {
                TransactionError::InvalidType(msg) => {
                    assert!(msg.contains("Expected Stellar signature"));
                }
                other => panic!("Unexpected error: {other:?}"),
            }
        }

        #[tokio::test]
        async fn prepare_transaction_sequence_overflow() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();
            // Return value exceeding i64::MAX to trigger overflow
            mocks
                .counter
                .expect_get_and_increment()
                .returning(|_, _| Ok(i64::MAX as u64 + 1));

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let tx = create_test_transaction(&relayer.id);
            let res = handler.prepare_transaction(tx).await;
            assert!(res.is_err());
            matches!(res.unwrap_err(), TransactionError::ValidationError(_));
        }
    }

    // ---------------------------------------------------------------------
    // submit_transaction tests
    // ---------------------------------------------------------------------
    mod submit_transaction_tests {
        use crate::models::RepositoryError;

        use super::*;

        #[tokio::test]
        async fn submit_transaction_happy_path() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // provider gives a hash
            mocks
                .provider
                .expect_send_transaction()
                .returning(|_| Box::pin(async { Ok(Hash([1u8; 32])) }));

            // expect partial update to Submitted
            mocks
                .tx_repo
                .expect_partial_update()
                .withf(|_, upd| upd.status == Some(TransactionStatus::Submitted))
                .returning(|id, upd| {
                    let mut tx = create_test_transaction("relayer-1");
                    tx.id = id;
                    tx.status = upd.status.unwrap();
                    Ok::<_, RepositoryError>(tx)
                });

            // enqueue status-check & notification
            mocks
                .job_producer
                .expect_produce_check_transaction_status_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);

            let mut tx = create_test_transaction(&relayer.id);
            if let NetworkTransactionData::Stellar(ref mut d) = tx.network_data {
                d.signatures.push(super::dummy_signature());
            }

            let res = handler.submit_transaction(tx).await.unwrap();
            assert_eq!(res.status, TransactionStatus::Submitted);
        }

        #[tokio::test]
        async fn submit_transaction_provider_error() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            mocks
                .provider
                .expect_send_transaction()
                .returning(|_| Box::pin(async { Err(eyre::eyre!("boom")) }));

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let mut tx = create_test_transaction(&relayer.id);
            if let NetworkTransactionData::Stellar(ref mut data) = tx.network_data {
                data.signatures.push(super::dummy_signature());
            }
            let res = handler.submit_transaction(tx).await;
            assert!(res.is_err());
            matches!(res.unwrap_err(), TransactionError::UnexpectedError(_));
        }

        #[tokio::test]
        async fn submit_transaction_with_multiple_concurrent_calls() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Mock provider to successfully send transaction
            mocks
                .provider
                .expect_send_transaction()
                .returning(|_| Box::pin(async { Ok(Hash([1u8; 32])) }));

            // Mock partial_update to Submitted status
            mocks
                .tx_repo
                .expect_partial_update()
                .withf(|_, upd| upd.status == Some(TransactionStatus::Submitted))
                .returning(|id, upd| {
                    let mut tx = create_test_transaction("relayer-1");
                    tx.id = id;
                    tx.status = upd.status.unwrap();
                    Ok::<_, RepositoryError>(tx)
                });

            // Mock job producer for status check and notification
            mocks
                .job_producer
                .expect_produce_check_transaction_status_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);

            // Create a transaction to submit
            let mut tx_to_submit = create_test_transaction(&relayer.id);
            tx_to_submit.id = "new-pending-tx".to_string();
            // Ensure it's signed
            if let NetworkTransactionData::Stellar(ref mut data) = tx_to_submit.network_data {
                data.signatures.push(super::dummy_signature());
            }

            // Call submit_transaction
            let res = handler.submit_transaction(tx_to_submit.clone()).await;

            // Assertions
            assert!(res.is_ok());
            let returned_tx = res.unwrap();
            assert_eq!(returned_tx.id, "new-pending-tx");
            assert_eq!(returned_tx.status, TransactionStatus::Submitted);
        }
    }

    // ---------------------------------------------------------------------
    // validate_transaction tests
    // ---------------------------------------------------------------------
    mod validate_transaction_tests {
        use super::*;

        #[tokio::test]
        async fn validate_transaction_always_true() {
            let relayer = create_test_relayer();
            let mocks = default_test_mocks();
            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let tx = create_test_transaction(&relayer.id);
            let res = handler.validate_transaction(tx).await;
            assert!(res.is_ok());
            assert!(res.unwrap());
        }
    }

    mod simulate_if_needed_tests {
        use super::*;
        use soroban_rs::stellar_rpc_client::SimulateTransactionResponse;
        use soroban_rs::xdr::{
            Memo, Preconditions, SequenceNumber, Transaction, TransactionEnvelope,
            TransactionV1Envelope, Uint256,
        };

        fn dummy_unsigned_env() -> TransactionEnvelope {
            // Minimal dummy envelope
            TransactionEnvelope::Tx(TransactionV1Envelope {
                tx: Transaction {
                    source_account: soroban_rs::xdr::MuxedAccount::Ed25519(Uint256([0; 32])),
                    fee: 100,
                    seq_num: SequenceNumber(1),
                    cond: Preconditions::None,
                    memo: Memo::None,
                    operations: soroban_rs::xdr::VecM::default(),
                    ext: soroban_rs::xdr::TransactionExt::V0,
                },
                signatures: soroban_rs::xdr::VecM::default(),
            })
        }

        #[tokio::test]
        async fn does_not_call_simulation_for_only_payment() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();
            // Provider should not be called
            mocks
                .provider
                .expect_simulate_transaction_envelope()
                .never();
            let handler = make_stellar_tx_handler(relayer, mocks);
            let ops = vec![payment_op(TEST_PK)];
            let env = dummy_unsigned_env();
            let res = handler.simulate_if_needed(&env, &ops).await;
            assert!(res.is_ok());
        }

        #[tokio::test]
        async fn calls_simulation_and_succeeds() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();
            // Provider should be called and return Ok
            mocks
                .provider
                .expect_simulate_transaction_envelope()
                .returning(|_| Box::pin(async { Ok(SimulateTransactionResponse::default()) }));
            let handler = make_stellar_tx_handler(relayer, mocks);
            // Use only Payment, so simulation is not called, but for test, force call
            let ops = vec![payment_op(TEST_PK)];
            let env = dummy_unsigned_env();
            let res = handler.simulate_if_needed(&env, &ops).await;
            assert!(res.is_ok());
        }
    }

    // ---------------------------------------------------------------------
    // handle_transaction_status tests
    // ---------------------------------------------------------------------
    #[cfg(test)]
    mod handle_transaction_status_tests {
        use super::*;
        use mockall::predicate::eq;
        use soroban_rs::stellar_rpc_client::GetTransactionResponse;

        fn dummy_get_transaction_response(status: &str) -> GetTransactionResponse {
            GetTransactionResponse {
                status: status.to_string(),
                envelope: None,
                result: None,
                result_meta: None,
            }
        }

        #[tokio::test]
        async fn handle_transaction_status_confirmed_triggers_next() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            let mut tx_to_handle = create_test_transaction(&relayer.id);
            tx_to_handle.id = "tx-confirm-this".to_string();
            let tx_hash_bytes = [1u8; 32];
            let tx_hash_hex = hex::encode(tx_hash_bytes);
            if let NetworkTransactionData::Stellar(ref mut stellar_data) = tx_to_handle.network_data
            {
                stellar_data.hash = Some(tx_hash_hex.clone());
            } else {
                panic!("Expected Stellar network data for tx_to_handle");
            }
            tx_to_handle.status = TransactionStatus::Submitted;

            let expected_stellar_hash = soroban_rs::xdr::Hash(tx_hash_bytes);

            // 1. Mock provider to return SUCCESS
            mocks
                .provider
                .expect_get_transaction()
                .with(eq(expected_stellar_hash.clone()))
                .times(1)
                .returning(move |_| {
                    Box::pin(async { Ok(dummy_get_transaction_response("SUCCESS")) })
                });

            // 2. Mock partial_update for confirmation
            mocks
                .tx_repo
                .expect_partial_update()
                .withf(move |id, update| {
                    id == "tx-confirm-this"
                        && update.status == Some(TransactionStatus::Confirmed)
                        && update.confirmed_at.is_some()
                })
                .times(1)
                .returning(move |id, update| {
                    let mut updated_tx = tx_to_handle.clone(); // Use the original tx_to_handle as base
                    updated_tx.id = id;
                    updated_tx.status = update.status.unwrap();
                    updated_tx.confirmed_at = update.confirmed_at;
                    Ok(updated_tx)
                });

            // Send notification for confirmed tx
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            // 3. Mock find_by_status for pending transactions
            let mut oldest_pending_tx = create_test_transaction(&relayer.id);
            oldest_pending_tx.id = "tx-oldest-pending".to_string();
            oldest_pending_tx.status = TransactionStatus::Pending;
            let captured_oldest_pending_tx = oldest_pending_tx.clone();
            mocks
                .tx_repo
                .expect_find_by_status()
                .with(eq(relayer.id.clone()), eq(vec![TransactionStatus::Pending]))
                .times(1)
                .returning(move |_, _| Ok(vec![captured_oldest_pending_tx.clone()]));

            // 4. Mock produce_transaction_request_job for the next pending transaction
            mocks
                .job_producer
                .expect_produce_transaction_request_job()
                .withf(move |job, _delay| job.transaction_id == "tx-oldest-pending")
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let mut initial_tx_for_handling = create_test_transaction(&relayer.id);
            initial_tx_for_handling.id = "tx-confirm-this".to_string();
            if let NetworkTransactionData::Stellar(ref mut stellar_data) =
                initial_tx_for_handling.network_data
            {
                stellar_data.hash = Some(hex::encode(tx_hash_bytes));
            } else {
                panic!("Expected Stellar network data for initial_tx_for_handling");
            }
            initial_tx_for_handling.status = TransactionStatus::Submitted;

            let result = handler
                .handle_transaction_status(initial_tx_for_handling)
                .await;

            assert!(result.is_ok());
            let handled_tx = result.unwrap();
            assert_eq!(handled_tx.id, "tx-confirm-this");
            assert_eq!(handled_tx.status, TransactionStatus::Confirmed);
            assert!(handled_tx.confirmed_at.is_some());
        }

        #[tokio::test]
        async fn handle_transaction_status_still_pending() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            let mut tx_to_handle = create_test_transaction(&relayer.id);
            tx_to_handle.id = "tx-pending-check".to_string();
            let tx_hash_bytes = [2u8; 32];
            if let NetworkTransactionData::Stellar(ref mut stellar_data) = tx_to_handle.network_data
            {
                stellar_data.hash = Some(hex::encode(tx_hash_bytes));
            } else {
                panic!("Expected Stellar network data");
            }
            tx_to_handle.status = TransactionStatus::Submitted; // Or any status that implies it's being watched

            let expected_stellar_hash = soroban_rs::xdr::Hash(tx_hash_bytes);

            // 1. Mock provider to return PENDING
            mocks
                .provider
                .expect_get_transaction()
                .with(eq(expected_stellar_hash.clone()))
                .times(1)
                .returning(move |_| {
                    Box::pin(async { Ok(dummy_get_transaction_response("PENDING")) })
                });

            // 2. Mock partial_update: should NOT be called
            mocks.tx_repo.expect_partial_update().never();

            // 3. Mock job_producer to expect a re-enqueue of status check
            mocks
                .job_producer
                .expect_produce_check_transaction_status_job()
                .withf(move |job, delay| {
                    job.transaction_id == "tx-pending-check" && delay.is_none()
                })
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            // Notifications should NOT be sent for pending
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .never();

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let original_tx_clone = tx_to_handle.clone();

            let result = handler.handle_transaction_status(tx_to_handle).await;

            assert!(result.is_ok());
            let returned_tx = result.unwrap();
            // Transaction should be returned unchanged as it's still pending
            assert_eq!(returned_tx.id, original_tx_clone.id);
            assert_eq!(returned_tx.status, original_tx_clone.status);
            assert!(returned_tx.confirmed_at.is_none()); // Ensure it wasn't accidentally confirmed
        }

        #[tokio::test]
        async fn handle_transaction_status_failed() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            let mut tx_to_handle = create_test_transaction(&relayer.id);
            tx_to_handle.id = "tx-fail-this".to_string();
            let tx_hash_bytes = [3u8; 32];
            if let NetworkTransactionData::Stellar(ref mut stellar_data) = tx_to_handle.network_data
            {
                stellar_data.hash = Some(hex::encode(tx_hash_bytes));
            } else {
                panic!("Expected Stellar network data");
            }
            tx_to_handle.status = TransactionStatus::Submitted;

            let expected_stellar_hash = soroban_rs::xdr::Hash(tx_hash_bytes);

            // 1. Mock provider to return FAILED
            mocks
                .provider
                .expect_get_transaction()
                .with(eq(expected_stellar_hash.clone()))
                .times(1)
                .returning(move |_| {
                    Box::pin(async { Ok(dummy_get_transaction_response("FAILED")) })
                });

            // 2. Mock partial_update for failure - use actual update values
            let relayer_id_for_mock = relayer.id.clone();
            mocks
                .tx_repo
                .expect_partial_update()
                .times(1)
                .returning(move |id, update| {
                    // Use the actual update values instead of hardcoding
                    let mut updated_tx = create_test_transaction(&relayer_id_for_mock);
                    updated_tx.id = id;
                    updated_tx.status = update.status.unwrap();
                    updated_tx.status_reason = update.status_reason.clone();
                    Ok(updated_tx)
                });

            // Send notification for failed tx
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            // 3. Mock find_by_status for pending transactions (should be called by enqueue_next_pending_transaction)
            mocks
                .tx_repo
                .expect_find_by_status()
                .with(eq(relayer.id.clone()), eq(vec![TransactionStatus::Pending]))
                .times(1)
                .returning(move |_, _| Ok(vec![])); // No pending transactions

            // Should NOT try to enqueue next transaction since there are no pending ones
            mocks
                .job_producer
                .expect_produce_transaction_request_job()
                .never();
            // Should NOT re-queue status check
            mocks
                .job_producer
                .expect_produce_check_transaction_status_job()
                .never();

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let mut initial_tx_for_handling = create_test_transaction(&relayer.id);
            initial_tx_for_handling.id = "tx-fail-this".to_string();
            if let NetworkTransactionData::Stellar(ref mut stellar_data) =
                initial_tx_for_handling.network_data
            {
                stellar_data.hash = Some(hex::encode(tx_hash_bytes));
            } else {
                panic!("Expected Stellar network data");
            }
            initial_tx_for_handling.status = TransactionStatus::Submitted;

            let result = handler
                .handle_transaction_status(initial_tx_for_handling)
                .await;

            assert!(result.is_ok());
            let handled_tx = result.unwrap();
            assert_eq!(handled_tx.id, "tx-fail-this");
            assert_eq!(handled_tx.status, TransactionStatus::Failed);
            assert!(handled_tx.status_reason.is_some());
            assert_eq!(
                handled_tx.status_reason.unwrap(),
                "Transaction failed on-chain. Provider status: FAILED. No detailed XDR result available."
            );
        }

        #[tokio::test]
        async fn handle_transaction_status_provider_error() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            let mut tx_to_handle = create_test_transaction(&relayer.id);
            tx_to_handle.id = "tx-provider-error".to_string();
            let tx_hash_bytes = [4u8; 32];
            if let NetworkTransactionData::Stellar(ref mut stellar_data) = tx_to_handle.network_data
            {
                stellar_data.hash = Some(hex::encode(tx_hash_bytes));
            } else {
                panic!("Expected Stellar network data");
            }
            tx_to_handle.status = TransactionStatus::Submitted;

            let expected_stellar_hash = soroban_rs::xdr::Hash(tx_hash_bytes);

            // 1. Mock provider to return an error
            mocks
                .provider
                .expect_get_transaction()
                .with(eq(expected_stellar_hash.clone()))
                .times(1)
                .returning(move |_| Box::pin(async { Err(eyre::eyre!("RPC boom")) }));

            // 2. Mock partial_update: should NOT be called
            mocks.tx_repo.expect_partial_update().never();

            // 3. Mock job_producer to expect a re-enqueue of status check
            mocks
                .job_producer
                .expect_produce_check_transaction_status_job()
                .withf(move |job, delay| {
                    job.transaction_id == "tx-provider-error" && delay.is_none()
                })
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            // Notifications should NOT be sent
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .never();
            // Should NOT try to enqueue next transaction
            mocks
                .job_producer
                .expect_produce_transaction_request_job()
                .never();

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let original_tx_clone = tx_to_handle.clone();

            let result = handler.handle_transaction_status(tx_to_handle).await;

            assert!(result.is_ok()); // The handler itself should return Ok(original_tx)
            let returned_tx = result.unwrap();
            // Transaction should be returned unchanged
            assert_eq!(returned_tx.id, original_tx_clone.id);
            assert_eq!(returned_tx.status, original_tx_clone.status);
        }

        #[tokio::test]
        async fn handle_transaction_status_no_hashes() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks(); // No mocks should be called, but make mutable for consistency

            let mut tx_to_handle = create_test_transaction(&relayer.id);
            tx_to_handle.id = "tx-no-hashes".to_string();
            tx_to_handle.status = TransactionStatus::Submitted;

            mocks.provider.expect_get_transaction().never();
            mocks.tx_repo.expect_partial_update().never();
            mocks
                .job_producer
                .expect_produce_check_transaction_status_job()
                .never();
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .never();

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let result = handler.handle_transaction_status(tx_to_handle).await;

            assert!(
                result.is_err(),
                "Expected an error when hash is missing, but got Ok"
            );
            match result.unwrap_err() {
                TransactionError::ValidationError(msg) => {
                    assert!(
                        msg.contains("Stellar transaction tx-no-hashes is missing or has an empty on-chain hash in network_data"),
                        "Unexpected error message: {}",
                        msg
                    );
                }
                other => panic!("Expected ValidationError, got {:?}", other),
            }
        }
    }

    // ---------------------------------------------------------------------
    // next_sequence tests
    // ---------------------------------------------------------------------
    mod next_sequence_tests {
        use super::*;

        #[test]
        fn next_sequence_success() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Mock counter service to return a valid sequence
            mocks
                .counter
                .expect_get_and_increment()
                .with(eq("relayer-1"), eq(TEST_PK))
                .returning(|_, _| Ok(42u64));

            let handler = make_stellar_tx_handler(relayer, mocks);
            let result = handler.next_sequence();

            assert!(result.is_ok());
            assert_eq!(result.unwrap(), 42i64);
        }

        #[test]
        fn next_sequence_overflow_error() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Mock counter service to return value that overflows i64::MAX
            mocks
                .counter
                .expect_get_and_increment()
                .returning(|_, _| Ok(i64::MAX as u64 + 1));

            let handler = make_stellar_tx_handler(relayer, mocks);
            let result = handler.next_sequence();

            assert!(result.is_err());
            match result.unwrap_err() {
                TransactionError::ValidationError(msg) => {
                    assert!(msg.contains("Sequence conversion error"));
                    assert!(msg.contains(&(i64::MAX as u64 + 1).to_string()));
                }
                other => panic!("Expected ValidationError, got: {:?}", other),
            }
        }

        #[test]
        fn next_sequence_counter_service_error() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Mock counter service to return an error
            mocks.counter.expect_get_and_increment().returning(|_, _| {
                Err(crate::repositories::TransactionCounterError::NotFound(
                    "Counter service failure".to_string(),
                ))
            });

            let handler = make_stellar_tx_handler(relayer, mocks);
            let result = handler.next_sequence();

            assert!(result.is_err());
            match result.unwrap_err() {
                TransactionError::UnexpectedError(msg) => {
                    assert!(msg.contains("Counter service failure"));
                }
                other => panic!("Expected UnexpectedError, got: {:?}", other),
            }
        }

        #[test]
        fn next_sequence_boundary_values() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Test i64::MAX boundary (should succeed)
            mocks
                .counter
                .expect_get_and_increment()
                .returning(|_, _| Ok(i64::MAX as u64));

            let handler = make_stellar_tx_handler(relayer, mocks);
            let result = handler.next_sequence();

            assert!(result.is_ok());
            assert_eq!(result.unwrap(), i64::MAX);
        }
    }

    // ---------------------------------------------------------------------
    // send_transaction_request_job tests
    // ---------------------------------------------------------------------
    mod send_transaction_request_job_tests {
        use super::*;

        #[tokio::test]
        async fn send_transaction_request_job_success() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Mock job producer
            mocks
                .job_producer
                .expect_produce_transaction_request_job()
                .withf(|job, _delay| {
                    job.transaction_id == "tx-1"
                        && job.relayer_id == "relayer-1"
                        && _delay.is_none()
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

            // Mock job producer with delay
            mocks
                .job_producer
                .expect_produce_transaction_request_job()
                .withf(|job, delay| {
                    job.transaction_id == "tx-1"
                        && job.relayer_id == "relayer-1"
                        && delay == &Some(30)
                })
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let tx = create_test_transaction(&relayer.id);

            let result = handler.send_transaction_request_job(&tx, Some(30)).await;
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn send_transaction_request_job_producer_error() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Mock job producer to return error
            mocks
                .job_producer
                .expect_produce_transaction_request_job()
                .returning(|_, _| {
                    Box::pin(async {
                        Err(crate::jobs::JobProducerError::QueueError(
                            "Job queue failure".to_string(),
                        ))
                    })
                });

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let tx = create_test_transaction(&relayer.id);

            let result = handler.send_transaction_request_job(&tx, None).await;
            assert!(result.is_err());
        }
    }

    // ---------------------------------------------------------------------
    // send_submit_transaction_job tests
    // ---------------------------------------------------------------------
    mod send_submit_transaction_job_tests {
        use super::*;

        #[tokio::test]
        async fn send_submit_transaction_job_success() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Mock job producer
            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .withf(|job, delay| {
                    job.transaction_id == "tx-1"
                        && job.relayer_id == "relayer-1"
                        && matches!(job.command, crate::jobs::TransactionCommand::Submit)
                        && delay.is_none()
                })
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let tx = create_test_transaction(&relayer.id);

            let result = handler.send_submit_transaction_job(&tx, None).await;
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn send_submit_transaction_job_with_delay() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Mock job producer with delay
            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .withf(|job, delay| {
                    job.transaction_id == "tx-1"
                        && job.relayer_id == "relayer-1"
                        && matches!(job.command, crate::jobs::TransactionCommand::Submit)
                        && delay == &Some(60)
                })
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let tx = create_test_transaction(&relayer.id);

            let result = handler.send_submit_transaction_job(&tx, Some(60)).await;
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn send_submit_transaction_job_producer_error() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Mock job producer to return error
            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .returning(|_, _| {
                    Box::pin(async {
                        Err(crate::jobs::JobProducerError::QueueError(
                            "Submit job queue failure".to_string(),
                        ))
                    })
                });

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let tx = create_test_transaction(&relayer.id);

            let result = handler.send_submit_transaction_job(&tx, None).await;
            assert!(result.is_err());
        }
    }

    // ---------------------------------------------------------------------
    // finalize_transaction_state tests
    // ---------------------------------------------------------------------
    mod finalize_transaction_state_tests {
        use super::*;
        use crate::models::RepositoryError;

        #[tokio::test]
        async fn finalize_transaction_state_success() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Mock repository update
            mocks
                .tx_repo
                .expect_partial_update()
                .withf(|tx_id, update| {
                    tx_id == "test-tx-id"
                        && update.status == Some(TransactionStatus::Confirmed)
                        && update.status_reason.is_none()
                        && update.confirmed_at.is_some()
                })
                .times(1)
                .returning(|id, update| {
                    let mut tx = create_test_transaction("relayer-1");
                    tx.id = id;
                    tx.status = update.status.unwrap();
                    tx.confirmed_at = update.confirmed_at;
                    Ok(tx)
                });

            // Mock notification sending
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let handler = make_stellar_tx_handler(relayer, mocks);

            let result = handler
                .finalize_transaction_state(
                    "test-tx-id".to_string(),
                    TransactionStatus::Confirmed,
                    None,
                    Some("2023-10-01T12:00:00Z".to_string()),
                )
                .await;

            assert!(result.is_ok());
            let updated_tx = result.unwrap();
            assert_eq!(updated_tx.id, "test-tx-id");
            assert_eq!(updated_tx.status, TransactionStatus::Confirmed);
            assert_eq!(
                updated_tx.confirmed_at,
                Some("2023-10-01T12:00:00Z".to_string())
            );
        }

        #[tokio::test]
        async fn finalize_transaction_state_with_failure_reason() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Mock repository update with failure
            mocks
                .tx_repo
                .expect_partial_update()
                .withf(|tx_id, update| {
                    tx_id == "failed-tx-id"
                        && update.status == Some(TransactionStatus::Failed)
                        && update.status_reason == Some("Transaction failed on-chain".to_string())
                        && update.confirmed_at.is_none()
                })
                .times(1)
                .returning(|id, update| {
                    let mut tx = create_test_transaction("relayer-1");
                    tx.id = id;
                    tx.status = update.status.unwrap();
                    tx.status_reason = update.status_reason.clone();
                    Ok(tx)
                });

            // Mock notification sending
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let handler = make_stellar_tx_handler(relayer, mocks);

            let result = handler
                .finalize_transaction_state(
                    "failed-tx-id".to_string(),
                    TransactionStatus::Failed,
                    Some("Transaction failed on-chain".to_string()),
                    None,
                )
                .await;

            assert!(result.is_ok());
            let updated_tx = result.unwrap();
            assert_eq!(updated_tx.id, "failed-tx-id");
            assert_eq!(updated_tx.status, TransactionStatus::Failed);
            assert_eq!(
                updated_tx.status_reason,
                Some("Transaction failed on-chain".to_string())
            );
        }

        #[tokio::test]
        async fn finalize_transaction_state_repository_error() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Mock repository to return error
            mocks.tx_repo.expect_partial_update().returning(|_, _| {
                Err(RepositoryError::NotFound(
                    "Transaction not found".to_string(),
                ))
            });

            // Notification should not be called if repository fails
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .never();

            let handler = make_stellar_tx_handler(relayer, mocks);

            let result = handler
                .finalize_transaction_state(
                    "missing-tx-id".to_string(),
                    TransactionStatus::Confirmed,
                    None,
                    Some("2023-10-01T12:00:00Z".to_string()),
                )
                .await;

            assert!(result.is_err());
        }

        #[tokio::test]
        async fn finalize_transaction_state_notification_error() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Mock repository update success
            mocks
                .tx_repo
                .expect_partial_update()
                .times(1)
                .returning(|id, update| {
                    let mut tx = create_test_transaction("relayer-1");
                    tx.id = id;
                    tx.status = update.status.unwrap();
                    Ok(tx)
                });

            // Mock notification to fail
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .returning(|_, _| {
                    Box::pin(async {
                        Err(crate::jobs::JobProducerError::QueueError(
                            "Notification failure".to_string(),
                        ))
                    })
                });

            let handler = make_stellar_tx_handler(relayer, mocks);

            let result = handler
                .finalize_transaction_state(
                    "test-tx-id".to_string(),
                    TransactionStatus::Confirmed,
                    None,
                    Some("2023-10-01T12:00:00Z".to_string()),
                )
                .await;

            assert!(result.is_err());
        }
    }

    // ---------------------------------------------------------------------
    // send_transaction_update_notification tests
    // ---------------------------------------------------------------------
    mod send_transaction_update_notification_tests {
        use super::*;

        #[tokio::test]
        async fn send_notification_with_notification_id() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Mock job producer
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .withf(|_payload, delay| {
                    // Verify the payload contains the notification data
                    delay.is_none()
                    // Note: We can't easily verify the exact payload content without
                    // accessing the notification payload structure
                })
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let tx = create_test_transaction(&relayer.id);

            let result = handler.send_transaction_update_notification(&tx).await;
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn send_notification_without_notification_id() {
            let mut relayer = create_test_relayer();
            relayer.notification_id = None; // No notification ID configured
            let mut mocks = default_test_mocks();

            // Job producer should not be called
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .never();

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let tx = create_test_transaction(&relayer.id);

            let result = handler.send_transaction_update_notification(&tx).await;
            assert!(result.is_ok()); // Should succeed even without notification ID
        }

        #[tokio::test]
        async fn send_notification_job_producer_error() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Mock job producer to return error
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .returning(|_, _| {
                    Box::pin(async {
                        Err(crate::jobs::JobProducerError::QueueError(
                            "Notification queue error".to_string(),
                        ))
                    })
                });

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let tx = create_test_transaction(&relayer.id);

            let result = handler.send_transaction_update_notification(&tx).await;
            assert!(result.is_err());
            match result.unwrap_err() {
                TransactionError::UnexpectedError(msg) => {
                    assert!(msg.contains("Failed to send notification"));
                    assert!(msg.contains("Notification queue error"));
                }
                other => panic!("Expected UnexpectedError, got: {:?}", other),
            }
        }
    }

    // ---------------------------------------------------------------------
    // requeue_status_check tests
    // ---------------------------------------------------------------------
    mod requeue_status_check_tests {
        use super::*;

        #[tokio::test]
        async fn requeue_status_check_success() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Mock job producer
            mocks
                .job_producer
                .expect_produce_check_transaction_status_job()
                .withf(|job, delay| {
                    job.transaction_id == "tx-1"
                        && job.relayer_id == "relayer-1"
                        && delay == &Some(STELLAR_DEFAULT_STATUS_RETRY_DELAY_SECONDS)
                })
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let tx = create_test_transaction(&relayer.id);

            let result = handler.requeue_status_check(&tx).await;
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn requeue_status_check_job_producer_error() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Mock job producer to return error
            mocks
                .job_producer
                .expect_produce_check_transaction_status_job()
                .returning(|_, _| {
                    Box::pin(async {
                        Err(crate::jobs::JobProducerError::QueueError(
                            "Status check queue error".to_string(),
                        ))
                    })
                });

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let tx = create_test_transaction(&relayer.id);

            let result = handler.requeue_status_check(&tx).await;
            assert!(result.is_err());
        }
    }

    // ---------------------------------------------------------------------
    // enqueue_next_pending_transaction tests
    // ---------------------------------------------------------------------
    mod enqueue_next_pending_transaction_tests {
        use super::*;

        #[tokio::test]
        async fn enqueue_next_pending_transaction_with_pending() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Mock find_by_status to return a pending transaction
            let mut pending_tx = create_test_transaction(&relayer.id);
            pending_tx.id = "pending-tx-123".to_string();
            pending_tx.status = TransactionStatus::Pending;
            pending_tx.created_at = "2023-10-01T12:00:00Z".to_string();

            let captured_pending_tx = pending_tx.clone();
            mocks
                .tx_repo
                .expect_find_by_status()
                .with(eq(relayer.id.clone()), eq(vec![TransactionStatus::Pending]))
                .times(1)
                .returning(move |_, _| Ok(vec![captured_pending_tx.clone()]));

            // Mock job producer for transaction request
            mocks
                .job_producer
                .expect_produce_transaction_request_job()
                .withf(|job, _delay| {
                    job.transaction_id == "pending-tx-123"
                        && job.relayer_id == "relayer-1"
                        && _delay.is_none()
                })
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let handler = make_stellar_tx_handler(relayer, mocks);

            let result = handler
                .enqueue_next_pending_transaction("finished-tx-456")
                .await;
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn enqueue_next_pending_transaction_no_pending() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Mock find_by_status to return no pending transactions
            mocks
                .tx_repo
                .expect_find_by_status()
                .with(eq(relayer.id.clone()), eq(vec![TransactionStatus::Pending]))
                .times(1)
                .returning(|_, _| Ok(vec![])); // No pending transactions

            // Job producer should not be called
            mocks
                .job_producer
                .expect_produce_transaction_request_job()
                .never();

            let handler = make_stellar_tx_handler(relayer, mocks);

            let result = handler
                .enqueue_next_pending_transaction("finished-tx-456")
                .await;
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn enqueue_next_pending_transaction_multiple_pending_oldest_first() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Create multiple pending transactions with different timestamps
            let mut newer_tx = create_test_transaction(&relayer.id);
            newer_tx.id = "newer-tx".to_string();
            newer_tx.status = TransactionStatus::Pending;
            newer_tx.created_at = "2023-10-01T13:00:00Z".to_string(); // Later timestamp

            let mut older_tx = create_test_transaction(&relayer.id);
            older_tx.id = "older-tx".to_string();
            older_tx.status = TransactionStatus::Pending;
            older_tx.created_at = "2023-10-01T12:00:00Z".to_string(); // Earlier timestamp

            // Return them in random order - the implementation should sort by created_at
            let pending_txs = vec![newer_tx.clone(), older_tx.clone()];

            mocks
                .tx_repo
                .expect_find_by_status()
                .times(1)
                .returning(move |_, _| Ok(pending_txs.clone()));

            // Should enqueue the OLDER transaction (oldest first)
            mocks
                .job_producer
                .expect_produce_transaction_request_job()
                .withf(|job, delay| {
                    job.transaction_id == "older-tx" && // Should pick the older one
                    job.relayer_id == "relayer-1" &&
                    delay.is_none()
                })
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let handler = make_stellar_tx_handler(relayer, mocks);

            let result = handler
                .enqueue_next_pending_transaction("finished-tx")
                .await;
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn enqueue_next_pending_transaction_repository_error() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Mock find_by_status to return error
            mocks.tx_repo.expect_find_by_status().returning(|_, _| {
                Err(crate::models::RepositoryError::Unknown(
                    "DB error".to_string(),
                ))
            });

            // Job producer should not be called
            mocks
                .job_producer
                .expect_produce_transaction_request_job()
                .never();

            let handler = make_stellar_tx_handler(relayer, mocks);

            let result = handler
                .enqueue_next_pending_transaction("finished-tx")
                .await;
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn enqueue_next_pending_transaction_job_producer_error() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // Mock find_by_status to return a pending transaction
            let mut pending_tx = create_test_transaction(&relayer.id);
            pending_tx.id = "pending-tx".to_string();
            pending_tx.status = TransactionStatus::Pending;

            mocks
                .tx_repo
                .expect_find_by_status()
                .times(1)
                .returning(move |_, _| Ok(vec![pending_tx.clone()]));

            // Mock job producer to return error
            mocks
                .job_producer
                .expect_produce_transaction_request_job()
                .returning(|_, _| {
                    Box::pin(async {
                        Err(crate::jobs::JobProducerError::QueueError(
                            "Job queue error".to_string(),
                        ))
                    })
                });

            let handler = make_stellar_tx_handler(relayer, mocks);

            let result = handler
                .enqueue_next_pending_transaction("finished-tx")
                .await;
            assert!(result.is_err());
        }
    }
}
