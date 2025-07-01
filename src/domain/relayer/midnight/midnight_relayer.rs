/// This module defines the `MidnightRelayer` struct and its associated functionality for
/// interacting with Midnight networks. The `MidnightRelayer` is responsible for managing
/// transactions, synchronizing sequence numbers, and ensuring the relayer's state is
/// consistent with the Midnight blockchain.
///
/// # Components
///
/// - `MidnightRelayer`: The main struct that encapsulates the relayer's state and operations for Midnight.
/// - `RelayerRepoModel`: Represents the relayer's data model.
/// - `MidnightProvider`: Provides blockchain interaction capabilities, such as fetching account details.
/// - `TransactionCounterService`: Manages the sequence number for transactions to ensure correct ordering.
/// - `JobProducer`: Produces jobs for processing transactions and sending notifications.
///
/// # Error Handling
///
/// The module uses the `RelayerError` enum to handle various errors that can occur during
/// operations, such as provider errors, sequence synchronization failures, and transaction failures.
///
/// # Usage
///
/// To use the `MidnightRelayer`, create an instance using the `new` method, providing the necessary
/// components. Then, call the appropriate methods to process transactions and manage the relayer's state.
use crate::{
    constants::MIDNIGHT_SMALLEST_UNIT_NAME,
    domain::{
        next_sequence_u64, BalanceResponse, JsonRpcRequest, JsonRpcResponse, SignDataRequest,
        SignDataResponse, SignTypedDataRequest,
    },
    jobs::{JobProducer, JobProducerTrait, TransactionRequest},
    models::{
        produce_relayer_disabled_payload, MidnightNetwork, MidnightRpcResult, NetworkRpcRequest,
        NetworkRpcResult, NetworkTransactionRequest, NetworkType, RelayerRepoModel, RelayerStatus,
        RepositoryError, TransactionRepoModel, TransactionStatus,
    },
    repositories::{
        InMemoryNetworkRepository, InMemoryRelayerRepository, InMemoryTransactionCounter,
        InMemoryTransactionRepository, NetworkRepository, RelayerRepository,
        RelayerRepositoryStorage, Repository, TransactionRepository,
    },
    services::{
        MidnightProvider, MidnightProviderTrait, TransactionCounterService,
        TransactionCounterServiceTrait,
    },
};
use async_trait::async_trait;
use eyre::Result;
use log::{info, warn};
use std::sync::Arc;

use crate::domain::relayer::{Relayer, RelayerError};

/// Dependencies container for `MidnightRelayer` construction.
pub struct MidnightRelayerDependencies<R, N, T, J, C>
where
    R: Repository<RelayerRepoModel, String> + RelayerRepository + Send + Sync,
    N: NetworkRepository + Send + Sync,
    T: Repository<TransactionRepoModel, String> + Send + Sync,
    J: JobProducerTrait + Send + Sync,
    C: TransactionCounterServiceTrait + Send + Sync,
{
    pub relayer_repository: Arc<R>,
    pub network_repository: Arc<N>,
    pub transaction_repository: Arc<T>,
    pub transaction_counter_service: Arc<C>,
    pub job_producer: Arc<J>,
}

impl<R, N, T, J, C> MidnightRelayerDependencies<R, N, T, J, C>
where
    R: Repository<RelayerRepoModel, String> + RelayerRepository + Send + Sync,
    N: NetworkRepository + Send + Sync,
    T: Repository<TransactionRepoModel, String> + Send + Sync,
    J: JobProducerTrait + Send + Sync,
    C: TransactionCounterServiceTrait + Send + Sync,
{
    /// Creates a new dependencies container for `MidnightRelayer`.
    ///
    /// # Arguments
    ///
    /// * `relayer_repository` - Repository for managing relayer model persistence
    /// * `network_repository` - Repository for accessing network configuration data (RPC URLs, chain settings)
    /// * `transaction_repository` - Repository for storing and retrieving transaction models
    /// * `transaction_counter_service` - Service for managing sequence numbers to ensure proper transaction ordering
    /// * `job_producer` - Service for creating background jobs for transaction processing and notifications
    ///
    /// # Returns
    ///
    /// Returns a new `MidnightRelayerDependencies` instance containing all provided dependencies.
    pub fn new(
        relayer_repository: Arc<R>,
        network_repository: Arc<N>,
        transaction_repository: Arc<T>,
        transaction_counter_service: Arc<C>,
        job_producer: Arc<J>,
    ) -> Self {
        Self {
            relayer_repository,
            network_repository,
            transaction_repository,
            transaction_counter_service,
            job_producer,
        }
    }
}

#[allow(dead_code)]
pub struct MidnightRelayer<P, R, N, T, J, C>
where
    P: MidnightProviderTrait + Send + Sync,
    R: Repository<RelayerRepoModel, String> + RelayerRepository + Send + Sync,
    N: NetworkRepository + Send + Sync,
    T: Repository<TransactionRepoModel, String> + TransactionRepository + Send + Sync,
    J: JobProducerTrait + Send + Sync,
    C: TransactionCounterServiceTrait + Send + Sync,
{
    relayer: RelayerRepoModel,
    network: MidnightNetwork,
    provider: P,
    relayer_repository: Arc<R>,
    network_repository: Arc<N>,
    transaction_repository: Arc<T>,
    transaction_counter_service: Arc<C>,
    job_producer: Arc<J>,
}

pub type DefaultMidnightRelayer = MidnightRelayer<
    MidnightProvider,
    RelayerRepositoryStorage<InMemoryRelayerRepository>,
    InMemoryNetworkRepository,
    InMemoryTransactionRepository,
    JobProducer,
    TransactionCounterService<InMemoryTransactionCounter>,
>;

impl<P, R, N, T, J, C> MidnightRelayer<P, R, N, T, J, C>
where
    P: MidnightProviderTrait + Send + Sync,
    R: Repository<RelayerRepoModel, String> + RelayerRepository + Send + Sync,
    N: NetworkRepository + Send + Sync,
    T: Repository<TransactionRepoModel, String> + TransactionRepository + Send + Sync,
    J: JobProducerTrait + Send + Sync,
    C: TransactionCounterServiceTrait + Send + Sync,
{
    /// Creates a new `MidnightRelayer` instance.
    ///
    /// This constructor initializes a new Midnight relayer with the provided configuration,
    /// provider, and dependencies. It validates the network configuration and sets up
    /// all necessary components for transaction processing.
    ///
    /// # Arguments
    ///
    /// * `relayer` - The relayer model containing configuration like ID, address, network name, and policies
    /// * `provider` - The Midnight provider implementation for blockchain interactions
    /// * `dependencies` - Container with all required repositories and services (see [`MidnightRelayerDependencies`])
    ///
    /// # Returns
    ///
    /// * `Ok(MidnightRelayer)` - Successfully initialized relayer ready for operation
    /// * `Err(RelayerError)` - If initialization fails due to configuration or validation errors
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        relayer: RelayerRepoModel,
        provider: P,
        dependencies: MidnightRelayerDependencies<R, N, T, J, C>,
    ) -> Result<Self, RelayerError> {
        let network_repo = dependencies
            .network_repository
            .get_by_name(NetworkType::Midnight, &relayer.network)
            .await
            .ok()
            .flatten()
            .ok_or_else(|| {
                RelayerError::NetworkConfiguration(format!("Network {} not found", relayer.network))
            })?;

        let network = MidnightNetwork::try_from(network_repo)?;

        Ok(Self {
            relayer,
            network,
            provider,
            relayer_repository: dependencies.relayer_repository,
            network_repository: dependencies.network_repository,
            transaction_repository: dependencies.transaction_repository,
            transaction_counter_service: dependencies.transaction_counter_service,
            job_producer: dependencies.job_producer,
        })
    }

    async fn sync_nonce(&self) -> Result<(), RelayerError> {
        info!(
            "Fetching nonce for relayer: {} ({})",
            self.relayer.id, self.relayer.address
        );

        let nonce = self
            .provider
            .get_nonce(&self.relayer.address)
            .await
            .map_err(|e| RelayerError::ProviderError(format!("Failed to fetch account: {}", e)))?;

        let next = next_sequence_u64(nonce as i64)?;

        info!(
            "Setting next nonce {} for relayer {}",
            next, self.relayer.id
        );
        self.transaction_counter_service
            .set(next)
            .await
            .map_err(RelayerError::from)?;
        Ok(())
    }

    async fn disable_relayer(&self, reasons: &[String]) -> Result<(), RelayerError> {
        let reason = reasons.join(", ");
        warn!("Disabling relayer {} due to: {}", self.relayer.id, reason);

        let updated = self
            .relayer_repository
            .disable_relayer(self.relayer.id.clone())
            .await?;

        if let Some(nid) = &self.relayer.notification_id {
            self.job_producer
                .produce_send_notification_job(
                    produce_relayer_disabled_payload(nid, &updated, &reason),
                    None,
                )
                .await?;
        }
        Ok(())
    }
}

#[async_trait]
impl<P, R, N, T, J, C> Relayer for MidnightRelayer<P, R, N, T, J, C>
where
    P: MidnightProviderTrait + Send + Sync,
    R: Repository<RelayerRepoModel, String> + RelayerRepository + Send + Sync,
    N: NetworkRepository + Send + Sync,
    T: Repository<TransactionRepoModel, String> + TransactionRepository + Send + Sync,
    J: JobProducerTrait + Send + Sync,
    C: TransactionCounterServiceTrait + Send + Sync,
{
    async fn process_transaction_request(
        &self,
        network_transaction: NetworkTransactionRequest,
    ) -> Result<TransactionRepoModel, RelayerError> {
        let network_model = self
            .network_repository
            .get_by_name(NetworkType::Midnight, &self.relayer.network)
            .await?
            .ok_or_else(|| {
                RelayerError::NetworkConfiguration(format!(
                    "Network {} not found",
                    self.relayer.network
                ))
            })?;
        let transaction =
            TransactionRepoModel::try_from((&network_transaction, &self.relayer, &network_model))?;

        self.transaction_repository
            .create(transaction.clone())
            .await
            .map_err(|e| RepositoryError::TransactionFailure(e.to_string()))?;

        self.job_producer
            .produce_transaction_request_job(
                TransactionRequest::new(transaction.id.clone(), transaction.relayer_id.clone()),
                None,
            )
            .await?;

        Ok(transaction)
    }

    async fn get_balance(&self) -> Result<BalanceResponse, RelayerError> {
        // let balance = self
        //     .provider
        //     .get_balance(&self.relayer.address)
        //     .await
        //     .map_err(|e| {
        //         RelayerError::ProviderError(format!("Failed to fetch account for balance: {}", e))
        //     })?;

        // TODO: find way to pass context and seed to relayer
        let balance = 0;

        let balance_u128 = balance
            .to_string()
            .parse::<u128>()
            .map_err(|e| RelayerError::ProviderError(format!("Failed to parse balance: {}", e)))?;

        Ok(BalanceResponse {
            balance: balance_u128,
            unit: MIDNIGHT_SMALLEST_UNIT_NAME.to_string(),
        })
    }

    async fn get_status(&self) -> Result<RelayerStatus, RelayerError> {
        let relayer_model = &self.relayer;

        let nonce = self
            .provider
            .get_nonce(&relayer_model.address)
            .await
            .map_err(|e| {
                RelayerError::ProviderError(format!("Failed to get account details: {}", e))
            })?;

        let nonce_str = nonce.to_string();

        let balance_response = self.get_balance().await?;

        let pending_statuses = [TransactionStatus::Pending, TransactionStatus::Submitted];
        let pending_transactions = self
            .transaction_repository
            .find_by_status(&relayer_model.id, &pending_statuses[..])
            .await
            .map_err(RelayerError::from)?;
        let pending_transactions_count = pending_transactions.len() as u64;

        let confirmed_statuses = [TransactionStatus::Confirmed];
        let confirmed_transactions = self
            .transaction_repository
            .find_by_status(&relayer_model.id, &confirmed_statuses[..])
            .await
            .map_err(RelayerError::from)?;

        let last_confirmed_transaction_timestamp = confirmed_transactions
            .iter()
            .filter_map(|tx| tx.confirmed_at.as_ref())
            .max()
            .cloned();

        Ok(RelayerStatus::Midnight {
            balance: balance_response.balance.to_string(),
            pending_transactions_count,
            last_confirmed_transaction_timestamp,
            system_disabled: relayer_model.system_disabled,
            paused: relayer_model.paused,
            nonce: nonce_str,
        })
    }

    async fn delete_pending_transactions(&self) -> Result<bool, RelayerError> {
        println!("Midnight delete_pending_transactions...");
        Ok(true)
    }

    async fn sign_data(&self, _request: SignDataRequest) -> Result<SignDataResponse, RelayerError> {
        Err(RelayerError::NotSupported(
            "Signing data not supported for Midnight".to_string(),
        ))
    }

    async fn sign_typed_data(
        &self,
        _request: SignTypedDataRequest,
    ) -> Result<SignDataResponse, RelayerError> {
        Err(RelayerError::NotSupported(
            "Signing typed data not supported for Midnight".to_string(),
        ))
    }

    async fn rpc(
        &self,
        _request: JsonRpcRequest<NetworkRpcRequest>,
    ) -> Result<JsonRpcResponse<NetworkRpcResult>, RelayerError> {
        println!("Midnight rpc...");
        Ok(JsonRpcResponse {
            id: Some(1),
            jsonrpc: "2.0".to_string(),
            result: Some(NetworkRpcResult::Midnight(
                MidnightRpcResult::GenericRpcResult("".to_string()),
            )),
            error: None,
        })
    }

    async fn validate_min_balance(&self) -> Result<(), RelayerError> {
        Ok(())
    }

    async fn initialize_relayer(&self) -> Result<(), RelayerError> {
        info!("Initializing Midnight relayer: {}", self.relayer.id);

        let seq_res = self.sync_nonce().await.err();

        let mut failures: Vec<String> = Vec::new();
        if let Some(e) = seq_res {
            failures.push(format!("Sequence sync failed: {}", e));
        }

        if !failures.is_empty() {
            self.disable_relayer(&failures).await?;
            return Ok(()); // same semantics as EVM
        }

        info!(
            "Midnight relayer initialized successfully: {}",
            self.relayer.id
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{network::IndexerUrls, MidnightNetworkConfig, NetworkConfigCommon},
        constants::MIDNIGHT_SMALLEST_UNIT_NAME,
        jobs::MockJobProducerTrait,
        models::{
            NetworkConfigData, NetworkRepoModel, NetworkType, RelayerMidnightPolicy,
            RelayerNetworkPolicy, RelayerRepoModel,
        },
        repositories::{
            InMemoryNetworkRepository, MockRelayerRepository, MockTransactionRepository,
        },
        services::{MockMidnightProviderTrait, MockTransactionCounterServiceTrait, ProviderError},
    };
    use mockall::predicate::*;
    use std::future::ready;
    use std::sync::Arc;

    /// Test context structure to manage test dependencies
    struct TestCtx {
        relayer_model: RelayerRepoModel,
        network_repository: Arc<InMemoryNetworkRepository>,
    }

    impl Default for TestCtx {
        fn default() -> Self {
            let network_repository = Arc::new(InMemoryNetworkRepository::new());

            let relayer_model = RelayerRepoModel {
                id: "test-relayer-id".to_string(),
                name: "Test Relayer".to_string(),
                network: "testnet".to_string(),
                paused: false,
                network_type: NetworkType::Midnight,
                signer_id: "signer-id".to_string(),
                policies: RelayerNetworkPolicy::Midnight(RelayerMidnightPolicy::default()),
                address: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string(),
                notification_id: Some("notification-id".to_string()),
                system_disabled: false,
                custom_rpc_urls: None,
            };

            TestCtx {
                relayer_model,
                network_repository,
            }
        }
    }

    impl TestCtx {
        async fn setup_network(&self) {
            let test_network = NetworkRepoModel {
                id: "midnight:testnet".to_string(),
                name: "testnet".to_string(),
                network_type: NetworkType::Midnight,
                config: NetworkConfigData::Midnight(MidnightNetworkConfig {
                    common: NetworkConfigCommon {
                        network: "testnet".to_string(),
                        from: None,
                        rpc_urls: Some(vec!["https://rpc.midnight.network".to_string()]),
                        explorer_urls: None,
                        average_blocktime_ms: Some(5000),
                        is_testnet: Some(true),
                        tags: None,
                    },
                    prover_url: Some("https://prover.midnight.network".to_string()),
                    commitment_tree_ttl: Some(60),
                    network_id: Some("testnet".to_string()),
                    indexer_urls: IndexerUrls {
                        http: "https://indexer.midnight.network".to_string(),
                        ws: "wss://indexer.midnight.network".to_string(),
                    },
                }),
            };

            self.network_repository.create(test_network).await.unwrap();
        }
    }

    #[tokio::test]
    async fn test_sync_nonce_success() {
        let ctx = TestCtx::default();
        ctx.setup_network().await;
        let relayer_model = ctx.relayer_model.clone();
        let mut provider = MockMidnightProviderTrait::new();
        provider
            .expect_get_nonce()
            .with(eq(relayer_model.address.clone()))
            .returning(|_| Box::pin(async { Ok(5) }));
        let mut counter = MockTransactionCounterServiceTrait::new();
        counter
            .expect_set()
            .with(eq(6u64))
            .returning(|_| Box::pin(async { Ok(()) }));
        let relayer_repo = MockRelayerRepository::new();
        let tx_repo = MockTransactionRepository::new();
        let job_producer = MockJobProducerTrait::new();

        let relayer = MidnightRelayer::new(
            relayer_model.clone(),
            provider,
            MidnightRelayerDependencies::new(
                Arc::new(relayer_repo),
                ctx.network_repository.clone(),
                Arc::new(tx_repo),
                Arc::new(counter),
                Arc::new(job_producer),
            ),
        )
        .await
        .unwrap();

        let result = relayer.sync_nonce().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_sync_nonce_provider_error() {
        let ctx = TestCtx::default();
        ctx.setup_network().await;
        let relayer_model = ctx.relayer_model.clone();
        let mut provider = MockMidnightProviderTrait::new();
        provider
            .expect_get_nonce()
            .with(eq(relayer_model.address.clone()))
            .returning(|_| Box::pin(async { Err(ProviderError::Other("fail".to_string())) }));
        let counter = MockTransactionCounterServiceTrait::new();
        let relayer_repo = MockRelayerRepository::new();
        let tx_repo = MockTransactionRepository::new();
        let job_producer = MockJobProducerTrait::new();

        let relayer = MidnightRelayer::new(
            relayer_model.clone(),
            provider,
            MidnightRelayerDependencies::new(
                Arc::new(relayer_repo),
                ctx.network_repository.clone(),
                Arc::new(tx_repo),
                Arc::new(counter),
                Arc::new(job_producer),
            ),
        )
        .await
        .unwrap();

        let result = relayer.sync_nonce().await;
        assert!(matches!(result, Err(RelayerError::ProviderError(_))));
    }

    #[tokio::test]
    async fn test_disable_relayer() {
        let ctx = TestCtx::default();
        ctx.setup_network().await;
        let relayer_model = ctx.relayer_model.clone();
        let provider = MockMidnightProviderTrait::new();
        let mut relayer_repo = MockRelayerRepository::new();
        let mut updated_model = relayer_model.clone();
        updated_model.system_disabled = true;
        relayer_repo
            .expect_disable_relayer()
            .with(eq(relayer_model.id.clone()))
            .returning(move |_| Ok::<RelayerRepoModel, RepositoryError>(updated_model.clone()));
        let mut job_producer = MockJobProducerTrait::new();
        job_producer
            .expect_produce_send_notification_job()
            .returning(|_, _| Box::pin(async { Ok(()) }));
        let tx_repo = MockTransactionRepository::new();
        let counter = MockTransactionCounterServiceTrait::new();

        let relayer = MidnightRelayer::new(
            relayer_model.clone(),
            provider,
            MidnightRelayerDependencies::new(
                Arc::new(relayer_repo),
                ctx.network_repository.clone(),
                Arc::new(tx_repo),
                Arc::new(counter),
                Arc::new(job_producer),
            ),
        )
        .await
        .unwrap();

        let reasons = vec!["reason1".to_string(), "reason2".to_string()];
        let result = relayer.disable_relayer(&reasons).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_get_status_success_midnight() {
        let ctx = TestCtx::default();
        ctx.setup_network().await;
        let relayer_model = ctx.relayer_model.clone();
        let mut provider_mock = MockMidnightProviderTrait::new();
        let mut tx_repo_mock = MockTransactionRepository::new();
        let relayer_repo_mock = MockRelayerRepository::new();
        let job_producer_mock = MockJobProducerTrait::new();
        let counter_mock = MockTransactionCounterServiceTrait::new();

        provider_mock
            .expect_get_nonce()
            .times(2)
            .returning(|_| Box::pin(ready(Ok(12345))));

        tx_repo_mock
            .expect_find_by_status()
            .withf(|relayer_id, statuses| {
                relayer_id == "test-relayer-id"
                    && statuses == [TransactionStatus::Pending, TransactionStatus::Submitted]
            })
            .returning(|_, _| Ok(vec![]) as Result<Vec<TransactionRepoModel>, RepositoryError>)
            .once();

        let confirmed_tx = TransactionRepoModel {
            id: "tx1_midnight".to_string(),
            relayer_id: relayer_model.id.clone(),
            status: TransactionStatus::Confirmed,
            confirmed_at: Some("2023-02-01T12:00:00Z".to_string()),
            ..TransactionRepoModel::default()
        };
        tx_repo_mock
            .expect_find_by_status()
            .withf(|relayer_id, statuses| {
                relayer_id == "test-relayer-id" && statuses == [TransactionStatus::Confirmed]
            })
            .returning(move |_, _| {
                Ok(vec![confirmed_tx.clone()]) as Result<Vec<TransactionRepoModel>, RepositoryError>
            })
            .once();

        let midnight_relayer = MidnightRelayer::new(
            relayer_model.clone(),
            provider_mock,
            MidnightRelayerDependencies::new(
                Arc::new(relayer_repo_mock),
                ctx.network_repository.clone(),
                Arc::new(tx_repo_mock),
                Arc::new(counter_mock),
                Arc::new(job_producer_mock),
            ),
        )
        .await
        .unwrap();

        let status = midnight_relayer.get_status().await.unwrap();

        match status {
            RelayerStatus::Midnight {
                balance,
                pending_transactions_count,
                last_confirmed_transaction_timestamp,
                system_disabled,
                paused,
                nonce,
            } => {
                assert_eq!(balance, "10000000");
                assert_eq!(pending_transactions_count, 0);
                assert_eq!(
                    last_confirmed_transaction_timestamp,
                    Some("2023-02-01T12:00:00Z".to_string())
                );
                assert_eq!(system_disabled, relayer_model.system_disabled);
                assert_eq!(paused, relayer_model.paused);
                assert_eq!(nonce, "12345");
            }
            _ => panic!("Expected Midnight RelayerStatus"),
        }
    }

    #[tokio::test]
    async fn test_get_status_midnight_provider_error() {
        let ctx = TestCtx::default();
        ctx.setup_network().await;
        let relayer_model = ctx.relayer_model.clone();
        let mut provider_mock = MockMidnightProviderTrait::new();
        let tx_repo_mock = MockTransactionRepository::new();
        let relayer_repo_mock = MockRelayerRepository::new();
        let job_producer_mock = MockJobProducerTrait::new();
        let counter_mock = MockTransactionCounterServiceTrait::new();

        provider_mock
            .expect_get_nonce()
            .with(eq(relayer_model.address.clone()))
            .returning(|_| {
                Box::pin(async { Err(ProviderError::Other("Midnight provider down".to_string())) })
            });

        let midnight_relayer = MidnightRelayer::new(
            relayer_model.clone(),
            provider_mock,
            MidnightRelayerDependencies::new(
                Arc::new(relayer_repo_mock),
                ctx.network_repository.clone(),
                Arc::new(tx_repo_mock),
                Arc::new(counter_mock),
                Arc::new(job_producer_mock),
            ),
        )
        .await
        .unwrap();

        let result = midnight_relayer.get_status().await;
        assert!(result.is_err());
        match result.err().unwrap() {
            RelayerError::ProviderError(msg) => {
                assert!(msg.contains("Failed to get nonce"))
            }
            _ => panic!("Expected ProviderError for get_account failure"),
        }
    }

    #[tokio::test]
    async fn test_get_balance_success() {
        let ctx = TestCtx::default();
        ctx.setup_network().await;
        let relayer_model = ctx.relayer_model.clone();
        let mut provider = MockMidnightProviderTrait::new();
        let expected_balance = 100_000_000i64; // 10 XLM in stroops

        provider
            .expect_get_nonce()
            .with(eq(relayer_model.address.clone()))
            .returning(|_| Box::pin(async { Ok(5) }));

        let relayer_repo = Arc::new(MockRelayerRepository::new());
        let tx_repo = Arc::new(MockTransactionRepository::new());
        let job_producer = Arc::new(MockJobProducerTrait::new());
        let counter = Arc::new(MockTransactionCounterServiceTrait::new());

        let relayer = MidnightRelayer::new(
            relayer_model,
            provider,
            MidnightRelayerDependencies::new(
                relayer_repo,
                ctx.network_repository.clone(),
                tx_repo,
                counter,
                job_producer,
            ),
        )
        .await
        .unwrap();

        let result = relayer.get_balance().await;
        assert!(result.is_ok());
        let balance_response = result.unwrap();
        assert_eq!(balance_response.balance, expected_balance as u128);
        assert_eq!(balance_response.unit, MIDNIGHT_SMALLEST_UNIT_NAME);
    }

    #[tokio::test]
    async fn test_get_balance_provider_error() {
        let ctx = TestCtx::default();
        ctx.setup_network().await;
        let relayer_model = ctx.relayer_model.clone();
        let mut provider = MockMidnightProviderTrait::new();

        provider
            .expect_get_nonce()
            .with(eq(relayer_model.address.clone()))
            .returning(|_| {
                Box::pin(async { Err(ProviderError::Other("provider failed".to_string())) })
            });

        let relayer_repo = Arc::new(MockRelayerRepository::new());
        let tx_repo = Arc::new(MockTransactionRepository::new());
        let job_producer = Arc::new(MockJobProducerTrait::new());
        let counter = Arc::new(MockTransactionCounterServiceTrait::new());

        let relayer = MidnightRelayer::new(
            relayer_model,
            provider,
            MidnightRelayerDependencies::new(
                relayer_repo,
                ctx.network_repository.clone(),
                tx_repo,
                counter,
                job_producer,
            ),
        )
        .await
        .unwrap();

        let result = relayer.get_balance().await;
        assert!(result.is_err());
        match result.err().unwrap() {
            RelayerError::ProviderError(msg) => {
                assert!(msg.contains("Failed to fetch account for balance: provider failed"));
            }
            _ => panic!("Unexpected error type"),
        }
    }
}
