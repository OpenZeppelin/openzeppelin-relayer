/// This module defines the `StellarRelayerTransaction` struct and its associated
/// functionality for handling Stellar transactions.
/// It includes methods for preparing, submitting, handling status, and
/// managing notifications for transactions. The module leverages various
/// services and repositories to perform these operations asynchronously.
use crate::{
    constants::STELLAR_DEFAULT_STATUS_RETRY_DELAY_SECONDS,
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

    /// Enqueue a submit-transaction job for the given transaction.
    pub async fn enqueue_submit(
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

        if let Some(oldest_pending) = self
            .transaction_repository()
            .find_oldest_pending_for_relayer(&confirmed_tx.relayer_id)
            .await?
        {
            info!(
                "Enqueuing next pending transaction {} for relayer {}",
                oldest_pending.id, confirmed_tx.relayer_id
            );
            self.enqueue_submit(&oldest_pending, None).await?;
        }
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
        self.finalize_transaction_state(
            tx.id.clone(),
            TransactionStatus::Failed,
            Some(detailed_reason),
            None,
        )
        .await
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

    /// Checks if a new transaction can be submitted for a given relayer.
    /// Returns true if no other transaction is currently in a 'Submitted' state for this relayer,
    /// otherwise false.
    async fn can_submit_transaction(
        &self,
        relayer_id: &str,
        current_tx_id: &str,
    ) -> Result<bool, TransactionError> {
        let existing_submitted_tx = self
            .transaction_repository()
            .find_submitted_for_relayer(relayer_id, None)
            .await?;

        if existing_submitted_tx.is_none() {
            info!(
                "Relayer {}: No existing submitted tx. {} can proceed.",
                relayer_id, current_tx_id
            );
            Ok(true)
        } else {
            let submitted_tx_id = existing_submitted_tx.unwrap().id;
            info!(
                "Relayer {}: Existing submitted tx {}. {} must wait.",
                relayer_id, submitted_tx_id, current_tx_id
            );
            Ok(false)
        }
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

        if self
            .can_submit_transaction(&saved_tx.relayer_id, &saved_tx.id)
            .await?
        {
            self.enqueue_submit(&saved_tx, None).await?;
        }

        self.send_transaction_update_notification(&saved_tx).await?;
        Ok(saved_tx)
    }

    async fn submit_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        info!("Submitting Stellar transaction: {:?}", tx.id);

        if let Some(existing_tx) = self
            .transaction_repository()
            .find_submitted_for_relayer(&tx.relayer_id, Some(tx.id.clone()))
            .await?
        {
            warn!(
                "Relayer {} already has a submitted transaction {}. Re-queueing {} for later.",
                tx.relayer_id, existing_tx.id, tx.id
            );
            self.enqueue_submit(&tx, Some(5)).await?;
            return Ok(tx);
        }

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
        async fn prepare_transaction_happy_path_no_existing_submitted() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            mocks
                .counter
                .expect_get_and_increment()
                .returning(|_, _| Ok(1));
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
                .expect_update_network_data()
                .returning(|id, data| {
                    let mut updated_tx = create_test_transaction("relayer-1");
                    updated_tx.id = id;
                    updated_tx.network_data = data;
                    updated_tx.relayer_id = "relayer-1".to_string();
                    Ok::<_, RepositoryError>(updated_tx)
                });

            // Expect find_submitted_for_relayer to be called and return None
            mocks
                .tx_repo
                .expect_find_submitted_for_relayer()
                .with(eq(relayer.id.clone()), eq(None))
                .times(1)
                .returning(|_, _| Ok(None));

            // Expect submit job to be produced
            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            // Expect notification job to be produced
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);
            let tx = create_test_transaction(&relayer.id);
            let result = handler.prepare_transaction(tx).await;
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn prepare_transaction_happy_path_existing_submitted() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            mocks
                .counter
                .expect_get_and_increment()
                .returning(|_, _| Ok(1));
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
                .expect_update_network_data()
                .returning(|id, data| {
                    let mut updated_tx = create_test_transaction("relayer-1");
                    updated_tx.id = id;
                    updated_tx.network_data = data;
                    updated_tx.relayer_id = "relayer-1".to_string();
                    Ok::<_, RepositoryError>(updated_tx)
                });

            // Expect find_submitted_for_relayer to be called and return an existing transaction
            let mut existing_submitted = create_test_transaction(&relayer.id);
            existing_submitted.id = "existing-tx-123".to_string();
            existing_submitted.status = TransactionStatus::Submitted;
            mocks
                .tx_repo
                .expect_find_submitted_for_relayer()
                .with(eq(relayer.id.clone()), eq(None))
                .times(1)
                .returning(move |_, _| Ok(Some(existing_submitted.clone())));

            // Expect submit job NOT to be produced
            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .never();

            // Expect notification job to be produced
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .times(1)
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
            let relayer_id_clone = relayer.id.clone(); // Define and clone relayer.id for subsequent use
            let mut mocks = default_test_mocks();

            // Expect find_submitted_for_relayer to be called and return None first
            mocks
                .tx_repo
                .expect_find_submitted_for_relayer()
                .with(eq(relayer_id_clone.clone()), eq(Some("tx-1".to_string())))
                .times(1)
                .returning(|_, _| Ok(None));

            // Provider returns dummy hash
            mocks
                .provider
                .expect_send_transaction()
                .returning(|_| Box::pin(async { Ok(Hash([1u8; 32])) }));

            // Transaction repo partial_update returns updated tx
            let relayer_id_for_update_closure = relayer_id_clone.clone(); // Clone for the move closure
            mocks
                .tx_repo
                .expect_partial_update()
                .returning(move |tx_id, update_req| {
                    let mut current_tx_state =
                        create_test_transaction(&relayer_id_for_update_closure);
                    current_tx_state.id = tx_id.clone();
                    if let Some(status) = update_req.status {
                        current_tx_state.status = status;
                    }
                    if let Some(sent_at) = update_req.sent_at {
                        current_tx_state.sent_at = Some(sent_at);
                    }
                    if let Some(network_data) = update_req.network_data {
                        current_tx_state.network_data = network_data;
                    }
                    if let Some(hashes) = update_req.hashes {
                        current_tx_state.hashes = hashes;
                    }
                    Ok::<_, RepositoryError>(current_tx_state)
                });

            // Job producer expectations
            mocks
                .job_producer
                .expect_produce_check_transaction_status_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            // Expect notification job to be produced since relayer has notification_id
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);

            // Create a signed transaction so signed_envelope() succeeds
            // Since relayer was moved, use relayer_id_clone here.
            let mut tx = create_test_transaction(&relayer_id_clone);
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

            // Expect find_submitted_for_relayer to be called and return None first
            mocks
                .tx_repo
                .expect_find_submitted_for_relayer()
                .with(eq(relayer.id.clone()), eq(Some("tx-1".to_string())))
                .times(1)
                .returning(|_, _| Ok(None));

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
        async fn submit_transaction_sequential_gate() {
            let relayer = create_test_relayer();
            let mut mocks = default_test_mocks();

            // 1. Create an existing submitted transaction model
            let mut existing_tx = create_test_transaction(&relayer.id);
            existing_tx.id = "existing-submitted-tx".to_string();
            existing_tx.status = TransactionStatus::Submitted;

            // 2. Mock find_submitted_for_relayer to return the existing_tx
            mocks
                .tx_repo
                .expect_find_submitted_for_relayer()
                .returning(move |_, exclude_id| {
                    assert_eq!(exclude_id, Some("new-pending-tx".to_string())); // Ensure we exclude the tx being submitted
                    Ok(Some(existing_tx.clone()))
                });

            // 3. Mock job_producer to expect a re-enqueue call for the new transaction
            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .withf(|job, delay| job.transaction_id == "new-pending-tx" && delay == &Some(5i64))
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            // 4. Provider send_transaction should NOT be called
            mocks.provider.expect_send_transaction().never();

            let handler = make_stellar_tx_handler(relayer.clone(), mocks);

            // 5. Create a new transaction to submit (should be Pending initially)
            let mut tx_to_submit = create_test_transaction(&relayer.id);
            tx_to_submit.id = "new-pending-tx".to_string();
            // Ensure it's signed, as this would happen before submit_transaction is called in a real flow after prepare.
            if let NetworkTransactionData::Stellar(ref mut data) = tx_to_submit.network_data {
                data.signatures.push(super::dummy_signature());
            }
            let original_status = tx_to_submit.status.clone();

            // 6. Call submit_transaction
            let res = handler.submit_transaction(tx_to_submit.clone()).await;

            // 7. Assertions
            assert!(res.is_ok());
            let returned_tx = res.unwrap();
            // The transaction should be returned unchanged (still Pending) because it was re-queued
            assert_eq!(returned_tx.id, "new-pending-tx");
            assert_eq!(returned_tx.status, original_status);
            assert_eq!(returned_tx.status, TransactionStatus::Pending);
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
            let res = handler.enqueue_submit(&tx, None).await;
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
            let res = handler.enqueue_submit(&tx, None).await;
            assert!(res.is_err());
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

            // 3. Mock find_oldest_pending_for_relayer
            let mut oldest_pending_tx = create_test_transaction(&relayer.id);
            oldest_pending_tx.id = "tx-oldest-pending".to_string();
            oldest_pending_tx.status = TransactionStatus::Pending;
            let captured_oldest_pending_tx = oldest_pending_tx.clone();
            mocks
                .tx_repo
                .expect_find_oldest_pending_for_relayer()
                .with(eq(relayer.id.clone()))
                .times(1)
                .returning(move |_| Ok(Some(captured_oldest_pending_tx.clone())));

            // 4. Mock enqueue_submit for the next pending transaction
            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
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
                    job.transaction_id == "tx-pending-check"
                        && delay == &Some(STELLAR_DEFAULT_STATUS_RETRY_DELAY_SECONDS)
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

            // 2. Mock partial_update for failure
            let captured_tx_id = tx_to_handle.id.clone();
            let tx_for_closure = tx_to_handle.clone();
            mocks
                .tx_repo
                .expect_partial_update()
                .withf(move |id, update| {
                    id == &captured_tx_id
                        && update.status == Some(TransactionStatus::Failed)
                        && update.status_reason.is_some()
                })
                .times(1)
                .returning(move |id, update| {
                    let mut updated_tx = tx_for_closure.clone();
                    updated_tx.id = id;
                    updated_tx.status = update.status.unwrap();
                    updated_tx.status_reason = update.status_reason;
                    Ok(updated_tx)
                });

            // Send notification for failed tx
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            // Should NOT try to submit next pending
            mocks
                .tx_repo
                .expect_find_oldest_pending_for_relayer()
                .never();
            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
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
                    job.transaction_id == "tx-provider-error"
                        && delay == &Some(STELLAR_DEFAULT_STATUS_RETRY_DELAY_SECONDS)
                })
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            // Notifications should NOT be sent
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .never();
            // Should NOT try to submit next pending
            mocks
                .tx_repo
                .expect_find_oldest_pending_for_relayer()
                .never();
            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
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
}
