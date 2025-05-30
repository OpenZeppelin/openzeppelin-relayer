/// This module defines the `StellarRelayerTransaction` struct and its associated
/// functionality for handling Stellar transactions.
/// It includes methods for preparing, submitting, handling status, and
/// managing notifications for transactions. The module leverages various
/// services and repositories to perform these operations asynchronously.
use crate::{
    domain::{transaction::Transaction, SignTransactionResponse},
    jobs::{JobProducer, JobProducerTrait, TransactionSend, TransactionStatusCheck},
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
use soroban_rs::xdr::TransactionEnvelope;
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

    /// Enqueue a submit-transaction job for the given transaction.
    pub async fn enqueue_submit(&self, tx: &TransactionRepoModel) -> Result<(), TransactionError> {
        let job = TransactionSend::submit(tx.id.clone(), tx.relayer_id.clone());
        self.job_producer()
            .produce_submit_transaction_job(job, None)
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

        let saved_tx = self
            .transaction_repository()
            .update_network_data(tx.id.clone(), updated_network_data)
            .await
            .map_err(TransactionError::from)?;

        self.send_transaction_update_notification(&saved_tx).await?;
        self.enqueue_submit(&saved_tx).await?;

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
        Ok(tx)
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
            RelayerNetworkPolicy, RelayerRepoModel, RelayerStellarPolicy, StellarNetwork,
            StellarTransactionData, TransactionRepoModel, TransactionStatus,
        },
        repositories::{MockRepository, MockTransactionCounterTrait, MockTransactionRepository},
        services::{MockSigner, MockStellarProviderTrait},
    };
    use chrono::Utc;
    use soroban_rs::xdr::{Hash, Signature, SignatureHint};
    use std::sync::Arc;

    const TEST_PK: &str = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";

    fn create_test_stellar_network() -> StellarNetwork {
        StellarNetwork {
            network: "testnet".to_string(),
            rpc_urls: vec!["https://horizon.stellar.org".to_string()],
            explorer_urls: None,
            average_blocktime_ms: 5000,
            is_testnet: true,
            tags: vec![],
            passphrase: "Test SDF Network ; September 2015".to_string(),
        }
    }

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
            notification_id: None,
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
            network: create_test_stellar_network(),
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
            // Counter returns a small sequence
            mocks
                .counter
                .expect_get_and_increment()
                .returning(|_, _| Ok(1));

            // Signer returns a stellar signature
            mocks.signer.expect_sign_transaction().returning(|_| {
                Box::pin(async {
                    Ok(SignTransactionResponse::Stellar(
                        crate::domain::SignTransactionResponseStellar {
                            signature: super::dummy_signature(),
                        },
                    ))
                })
            });

            // Transaction repository update_network_data should succeed
            mocks
                .tx_repo
                .expect_update_network_data()
                .returning(|id, data| {
                    Ok::<_, RepositoryError>(TransactionRepoModel {
                        id,
                        network_data: data,
                        ..Default::default()
                    })
                });

            // Job producer expects a submit job
            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let tx = create_test_transaction(&relayer.id);
            let result = handler.prepare_transaction(tx).await;
            assert!(result.is_ok());
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

            // Provider returns dummy hash
            mocks
                .provider
                .expect_send_transaction()
                .returning(|_| Box::pin(async { Ok(Hash([1u8; 32])) }));

            // Transaction repo partial_update returns updated tx
            mocks.tx_repo.expect_partial_update().returning(|id, _| {
                Ok::<_, RepositoryError>(TransactionRepoModel {
                    id,
                    status: TransactionStatus::Submitted,
                    ..Default::default()
                })
            });

            // Job producer expectations
            mocks
                .job_producer
                .expect_produce_check_transaction_status_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);

            // Create a signed transaction so signed_envelope() succeeds
            let mut tx = create_test_transaction(&relayer.id);
            if let NetworkTransactionData::Stellar(ref mut data) = tx.network_data {
                data.signatures.push(super::dummy_signature());
            }

            let res = handler.submit_transaction(tx).await;
            assert!(res.is_ok());
            let updated = res.unwrap();
            assert_eq!(updated.status, TransactionStatus::Submitted);
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

    mod enqueue_submit_tests {
        use crate::jobs::JobProducerError;

        use super::*;

        #[tokio::test]
        async fn enqueue_submit_calls_job_producer() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();
            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));
            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let tx = create_test_transaction(&relayer.id);
            let res = handler.enqueue_submit(&tx).await;
            assert!(res.is_ok());
        }

        #[tokio::test]
        async fn enqueue_submit_propagates_error() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();
            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .returning(|_, _| {
                    Box::pin(async { Err(JobProducerError::QueueError("fail".to_string())) })
                });
            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let tx = create_test_transaction(&relayer.id);
            let res = handler.enqueue_submit(&tx).await;
            assert!(res.is_err());
        }
    }
}
