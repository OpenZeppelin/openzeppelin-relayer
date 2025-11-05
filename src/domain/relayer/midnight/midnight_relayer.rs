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
        BalanceResponse, SignDataRequest, SignDataResponse, SignTransactionExternalResponse,
        SignTransactionRequest, SignTypedDataRequest,
        stellar::{i64_from_u64, next_sequence_u64},
    },
    jobs::{JobProducerTrait, TransactionRequest},
    models::{
        DeletePendingTransactionsResponse, DisabledReason, HealthCheckFailure, JsonRpcRequest,
        JsonRpcResponse, MidnightNetwork, NetworkRpcRequest, NetworkRpcResult,
        NetworkTransactionRequest, NetworkType, RelayerRepoModel, RelayerStatus, RepositoryError,
        TransactionRepoModel, TransactionStatus, produce_relayer_disabled_payload,
    },
    repositories::{
        NetworkRepository, RelayerRepository, RelayerStateRepositoryStorage, Repository,
        TransactionRepository,
    },
    services::{
        TransactionCounterService, TransactionCounterServiceTrait,
        provider::{MidnightProvider, MidnightProviderTrait},
        signer::{MidnightSigner, MidnightSignerTrait},
        sync::midnight::handler::{QuickSyncStrategy, SyncManager, SyncManagerTrait},
    },
};
use async_trait::async_trait;
use eyre::Result;
use log::{info, warn};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::debug;

use crate::domain::relayer::{Relayer, RelayerError};

/// Dependencies container for `MidnightRelayer` construction.
pub struct MidnightRelayerDependencies<R, N, T, J, C, S, SR>
where
    R: Repository<RelayerRepoModel, String> + RelayerRepository + Send + Sync,
    N: NetworkRepository + Send + Sync,
    T: Repository<TransactionRepoModel, String> + Send + Sync,
    J: JobProducerTrait + Send + Sync,
    C: TransactionCounterServiceTrait + Send + Sync,
    S: SyncManagerTrait + Send + Sync,
    SR: MidnightSignerTrait + Send + Sync,
{
    pub relayer_repository: Arc<R>,
    pub network_repository: Arc<N>,
    pub transaction_repository: Arc<T>,
    pub signer: Arc<SR>,
    pub transaction_counter_service: Arc<C>,
    pub sync_service: Arc<Mutex<S>>,
    pub job_producer: Arc<J>,
}

impl<R, N, T, J, C, S, SR> MidnightRelayerDependencies<R, N, T, J, C, S, SR>
where
    R: Repository<RelayerRepoModel, String> + RelayerRepository + Send + Sync,
    N: NetworkRepository + Send + Sync,
    T: Repository<TransactionRepoModel, String> + Send + Sync,
    J: JobProducerTrait + Send + Sync,
    C: TransactionCounterServiceTrait + Send + Sync,
    S: SyncManagerTrait + Send + Sync,
    SR: MidnightSignerTrait + Send + Sync,
{
    /// Creates a new dependencies container for `MidnightRelayer`.
    ///
    /// # Arguments
    ///
    /// * `relayer_repository` - Repository for managing relayer model persistence
    /// * `network_repository` - Repository for accessing network configuration data (RPC URLs, chain settings)
    /// * `transaction_repository` - Repository for storing and retrieving transaction models
    /// * `signer` - Transaction signer for signing transactions
    /// * `transaction_counter_service` - Service for managing sequence numbers to ensure proper transaction ordering
    /// * `sync_service` - Service for syncing wallet state with the blockchain
    /// * `job_producer` - Service for creating background jobs for transaction processing and notifications
    ///
    /// # Returns
    ///
    /// Returns a new `MidnightRelayerDependencies` instance containing all provided dependencies.
    pub fn new(
        relayer_repository: Arc<R>,
        network_repository: Arc<N>,
        transaction_repository: Arc<T>,
        signer: Arc<SR>,
        transaction_counter_service: Arc<C>,
        sync_service: Arc<Mutex<S>>,
        job_producer: Arc<J>,
    ) -> Self {
        Self {
            relayer_repository,
            network_repository,
            transaction_repository,
            signer,
            transaction_counter_service,
            sync_service,
            job_producer,
        }
    }
}

#[allow(dead_code)]
pub struct MidnightRelayer<P, R, N, T, J, C, S, SR>
where
    P: MidnightProviderTrait + Send + Sync,
    R: Repository<RelayerRepoModel, String> + RelayerRepository + Send + Sync,
    N: NetworkRepository + Send + Sync,
    T: Repository<TransactionRepoModel, String> + TransactionRepository + Send + Sync,
    J: JobProducerTrait + Send + Sync,
    C: TransactionCounterServiceTrait + Send + Sync,
    S: SyncManagerTrait + Send + Sync,
    SR: MidnightSignerTrait + Send + Sync,
{
    relayer: RelayerRepoModel,
    network: MidnightNetwork,
    provider: P,
    relayer_repository: Arc<R>,
    network_repository: Arc<N>,
    transaction_repository: Arc<T>,
    signer: Arc<SR>,
    transaction_counter_service: Arc<C>,
    sync_service: Arc<Mutex<S>>,
    job_producer: Arc<J>,
}

pub type DefaultMidnightRelayer<J, TR, NR, RR, TCR, RSR = RelayerStateRepositoryStorage> =
    MidnightRelayer<
        MidnightProvider,
        RR,
        NR,
        TR,
        J,
        TransactionCounterService<TCR>,
        SyncManager<QuickSyncStrategy, RSR>,
        MidnightSigner,
    >;

impl<P, R, N, T, J, C, S, SR> MidnightRelayer<P, R, N, T, J, C, S, SR>
where
    P: MidnightProviderTrait + Send + Sync,
    R: Repository<RelayerRepoModel, String> + RelayerRepository + Send + Sync,
    N: NetworkRepository + Send + Sync,
    T: Repository<TransactionRepoModel, String> + TransactionRepository + Send + Sync,
    J: JobProducerTrait + Send + Sync,
    C: TransactionCounterServiceTrait + Send + Sync,
    S: SyncManagerTrait + Send + Sync,
    SR: MidnightSignerTrait + Send + Sync,
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
        dependencies: MidnightRelayerDependencies<R, N, T, J, C, S, SR>,
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
            signer: dependencies.signer,
            transaction_counter_service: dependencies.transaction_counter_service,
            sync_service: dependencies.sync_service,
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

        let next = next_sequence_u64(i64_from_u64(nonce)?)?;

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
            .disable_relayer(
                self.relayer.id.clone(),
                DisabledReason::NonceSyncFailed(reason.clone()),
            )
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
impl<P, R, N, T, J, C, S, SR> Relayer for MidnightRelayer<P, R, N, T, J, C, S, SR>
where
    P: MidnightProviderTrait + Send + Sync,
    R: Repository<RelayerRepoModel, String> + RelayerRepository + Send + Sync,
    N: NetworkRepository + Send + Sync,
    T: Repository<TransactionRepoModel, String> + TransactionRepository + Send + Sync,
    J: JobProducerTrait + Send + Sync,
    C: TransactionCounterServiceTrait + Send + Sync,
    S: SyncManagerTrait + Send + Sync,
    SR: MidnightSignerTrait + Send + Sync,
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
        // Get the ledger context from the sync service
        let sync_service = self.sync_service.lock().await;
        let context = sync_service.get_context();
        drop(sync_service); // Drop the lock early

        let wallet_seed = self.signer.wallet_seed();

        let balance = self
            .provider
            .get_balance(wallet_seed, &context)
            .await
            .map_err(|e| RelayerError::ProviderError(format!("Failed to fetch balance: {}", e)))?;

        let balance_u128 = balance
            .to_string()
            .parse::<u128>()
            .map_err(|e| RelayerError::ProviderError(format!("Failed to parse balance: {}", e)))?;

        Ok(BalanceResponse {
            balance: balance_u128,
            unit: MIDNIGHT_SMALLEST_UNIT_NAME.to_string(),
        })
    }

    /// Initializes the relayer by performing necessary checks and synchronizations.
    ///
    /// # Returns
    ///
    /// A `Result` indicating success or a `RelayerError` if any initialization step fails.
    async fn check_health(&self) -> Result<(), Vec<HealthCheckFailure>> {
        debug!(
            "running health checks for Midnight relayer {}",
            self.relayer.id
        );

        let nonce_sync_result = self.sync_nonce().await;

        // Collect all failures
        let failures: Vec<HealthCheckFailure> = vec![
            nonce_sync_result
                .err()
                .map(|e| HealthCheckFailure::NonceSyncFailed(e.to_string())),
        ]
        .into_iter()
        .flatten()
        .collect();

        if failures.is_empty() {
            info!("all health checks passed");
            Ok(())
        } else {
            warn!("health checks failed: {:?}", failures);
            Err(failures)
        }
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
        let pending_transactions_count = u64::try_from(pending_transactions.len())
            .map_err(|_| RelayerError::ProviderError("Transaction count overflow".into()))?;

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

    async fn delete_pending_transactions(
        &self,
    ) -> Result<DeletePendingTransactionsResponse, RelayerError> {
        Err(RelayerError::NotSupported(
            "Deleting transactions is not supported for Midnight".to_string(),
        ))
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
        Err(RelayerError::NotSupported(
            "RPC is not supported for Midnight".to_string(),
        ))
    }

    async fn validate_min_balance(&self) -> Result<(), RelayerError> {
        Ok(())
    }

    async fn initialize_relayer(&self) -> Result<(), RelayerError> {
        info!("Initializing Midnight relayer: {}", self.relayer.id);

        // First sync the wallet from genesis on initial setup
        info!(
            "Performing initial wallet sync from genesis for relayer: {}",
            self.relayer.id
        );

        // Synchronises the LedgerContext (including ledger and wallet state) with the network
        // This is required to get the latest merkle tree state before submitting a transaction to ensure the transaction is valid locally AND on-chain
        // We use the QuickSyncStrategy by default which is a lightweight sync that uses the indexer to get the latest wallet-relevant states
        // The main difference between QuickSyncStrategy and FullSyncStrategy is that QuickSyncStrategy only syncs the wallet state and the ledger state, while FullSyncStrategy syncs the entire ledger state
        // This is useful because it's much faster and doesn't require downloading the entire ledger state from genesis
        // However, it requires trusting the indexer with your wallet viewing key (which is read-only key used to identify transactions belonging to your wallet).
        let mut sync_service = self.sync_service.lock().await;
        sync_service
            .sync(0)
            .await
            .map_err(|e| RelayerError::ProviderError(format!("Initial sync failed: {}", e)))?;
        drop(sync_service); // Explicitly drop the lock

        info!("Initial wallet sync completed successfully");

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

    async fn sign_transaction(
        &self,
        _request: &SignTransactionRequest,
    ) -> Result<SignTransactionExternalResponse, RelayerError> {
        Err(RelayerError::NotSupported(
            "Transaction signing not supported for Midnight".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{MidnightNetworkConfig, NetworkConfigCommon, network::IndexerUrls},
        constants::MIDNIGHT_SMALLEST_UNIT_NAME,
        jobs::MockJobProducerTrait,
        models::{
            LocalSignerConfigStorage, NetworkConfigData, NetworkRepoModel, NetworkType,
            RelayerMidnightPolicy, RelayerNetworkPolicy, RelayerRepoModel, SignerConfigStorage,
            SignerRepoModel,
        },
        repositories::{
            InMemoryNetworkRepository, InMemorySignerRepository, MockRelayerRepository,
            MockTransactionRepository,
        },
        services::{
            MockTransactionCounterServiceTrait,
            provider::{MockMidnightProviderTrait, ProviderError},
            signer::MidnightSignerFactory,
        },
    };
    use midnight_node_ledger_helpers::{DefaultDB, LedgerContext, NetworkId, WalletSeed};
    use mockall::{mock, predicate::*};
    use secrets::SecretVec;
    use std::future::ready;
    use std::sync::Arc;

    // Mock for SyncManagerTrait - using a simple pointer cast for testing
    mock! {
        pub SyncManager {}

        #[async_trait]
        impl SyncManagerTrait for SyncManager {
            async fn sync(&mut self, start_height: u64) -> Result<(), crate::services::midnight::SyncError>;
            fn get_context(&self) -> Arc<LedgerContext<DefaultDB>>;
        }
    }

    /// Test context structure to manage test dependencies
    struct TestCtx {
        relayer_model: RelayerRepoModel,
        network_repository: Arc<InMemoryNetworkRepository>,
        signer_repository: Arc<InMemorySignerRepository>,
        signer_model: SignerRepoModel,
    }

    impl Default for TestCtx {
        fn default() -> Self {
            // Set required environment variable for Midnight tests
            unsafe {
                std::env::set_var(
                    "MIDNIGHT_LEDGER_TEST_STATIC_DIR",
                    "/tmp/midnight-test-static",
                );
            }

            let network_repository = Arc::new(InMemoryNetworkRepository::new());
            let signer_repository = Arc::new(InMemorySignerRepository::new());

            // Create a 32-byte test key
            let signer_model = SignerRepoModel {
                id: "signer-id".to_string(),
                config: SignerConfigStorage::Local(LocalSignerConfigStorage {
                    raw_key: SecretVec::new(32, |buffer| {
                        buffer[0] = 1; // Make it non-zero
                    }),
                }),
            };

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
                disabled_reason: None,
            };

            TestCtx {
                relayer_model,
                network_repository,
                signer_repository,
                signer_model,
            }
        }
    }

    impl TestCtx {
        async fn setup_network(&self) {
            // Store the signer in repository
            self.signer_repository
                .create(self.signer_model.clone())
                .await
                .unwrap();

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
                    prover_url: "https://prover.midnight.network".to_string(),
                    commitment_tree_ttl: Some(60),
                    indexer_urls: IndexerUrls {
                        http: "https://indexer.midnight.network".to_string(),
                        ws: "wss://indexer.midnight.network".to_string(),
                    },
                }),
            };

            self.network_repository.create(test_network).await.unwrap();
        }

        fn create_mock_sync_manager() -> MockSyncManager {
            let mut sync_manager = MockSyncManager::new();
            // Note: This requires MIDNIGHT_LEDGER_TEST_STATIC_DIR=/path/to/midnightntwrk/midnight-node/static/contracts
            // to be set in the environment when running tests
            let wallet_seed = WalletSeed::from([1u8; 32]);
            let context = Arc::new(LedgerContext::new_from_wallet_seeds(
                NetworkId::TestNet,
                &[wallet_seed],
            ));
            sync_manager.expect_get_context().return_const(context);
            sync_manager
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
        let sync_manager = TestCtx::create_mock_sync_manager();
        let signer = MidnightSignerFactory::create_midnight_signer(
            &ctx.signer_model.into(),
            NetworkId::TestNet,
        )
        .unwrap();

        let relayer = MidnightRelayer::new(
            relayer_model.clone(),
            provider,
            MidnightRelayerDependencies::new(
                Arc::new(relayer_repo),
                ctx.network_repository.clone(),
                Arc::new(tx_repo),
                Arc::new(signer),
                Arc::new(counter),
                Arc::new(Mutex::new(sync_manager)),
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
        let sync_manager = TestCtx::create_mock_sync_manager();
        let signer = MidnightSignerFactory::create_midnight_signer(
            &ctx.signer_model.into(),
            NetworkId::TestNet,
        )
        .unwrap();
        let relayer = MidnightRelayer::new(
            relayer_model.clone(),
            provider,
            MidnightRelayerDependencies::new(
                Arc::new(relayer_repo),
                ctx.network_repository.clone(),
                Arc::new(tx_repo),
                Arc::new(signer),
                Arc::new(counter),
                Arc::new(Mutex::new(sync_manager)),
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
        let expected_reason = DisabledReason::NonceSyncFailed("reason1, reason2".to_string());
        relayer_repo
            .expect_disable_relayer()
            .with(eq(relayer_model.id.clone()), eq(expected_reason.clone()))
            .returning(move |_, _| Ok::<RelayerRepoModel, RepositoryError>(updated_model.clone()));
        let mut job_producer = MockJobProducerTrait::new();
        job_producer
            .expect_produce_send_notification_job()
            .returning(|_, _| Box::pin(async { Ok(()) }));
        let tx_repo = MockTransactionRepository::new();
        let counter = MockTransactionCounterServiceTrait::new();
        let sync_manager = TestCtx::create_mock_sync_manager();
        let signer = MidnightSignerFactory::create_midnight_signer(
            &ctx.signer_model.into(),
            NetworkId::TestNet,
        )
        .unwrap();
        let relayer = MidnightRelayer::new(
            relayer_model.clone(),
            provider,
            MidnightRelayerDependencies::new(
                Arc::new(relayer_repo),
                ctx.network_repository.clone(),
                Arc::new(tx_repo),
                Arc::new(signer),
                Arc::new(counter),
                Arc::new(Mutex::new(sync_manager)),
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
        let signer = MidnightSignerFactory::create_midnight_signer(
            &ctx.signer_model.into(),
            NetworkId::TestNet,
        )
        .unwrap();
        provider_mock
            .expect_get_nonce()
            .times(1)
            .returning(|_| Box::pin(ready(Ok(12345))));

        provider_mock
            .expect_get_balance()
            .times(1)
            .returning(|_, _| Box::pin(ready(Ok(crate::models::U256::from(10000000u128)))));

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

        let sync_manager = TestCtx::create_mock_sync_manager();

        let midnight_relayer = MidnightRelayer::new(
            relayer_model.clone(),
            provider_mock,
            MidnightRelayerDependencies::new(
                Arc::new(relayer_repo_mock),
                ctx.network_repository.clone(),
                Arc::new(tx_repo_mock),
                Arc::new(signer),
                Arc::new(counter_mock),
                Arc::new(Mutex::new(sync_manager)),
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
        let signer = MidnightSignerFactory::create_midnight_signer(
            &ctx.signer_model.into(),
            NetworkId::TestNet,
        )
        .unwrap();
        provider_mock
            .expect_get_nonce()
            .with(eq(relayer_model.address.clone()))
            .returning(|_| {
                Box::pin(async { Err(ProviderError::Other("Midnight provider down".to_string())) })
            });

        let sync_manager = TestCtx::create_mock_sync_manager();

        let midnight_relayer = MidnightRelayer::new(
            relayer_model.clone(),
            provider_mock,
            MidnightRelayerDependencies::new(
                Arc::new(relayer_repo_mock),
                ctx.network_repository.clone(),
                Arc::new(tx_repo_mock),
                Arc::new(signer),
                Arc::new(counter_mock),
                Arc::new(Mutex::new(sync_manager)),
                Arc::new(job_producer_mock),
            ),
        )
        .await
        .unwrap();

        let result = midnight_relayer.get_status().await;
        assert!(result.is_err());
        match result.err().unwrap() {
            RelayerError::ProviderError(msg) => {
                assert!(msg.contains("Failed to get account details"))
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
        let expected_balance = 100_000_000u128;

        provider.expect_get_balance().returning(move |_, _| {
            Box::pin(async move { Ok(crate::models::U256::from(expected_balance)) })
        });

        let relayer_repo = Arc::new(MockRelayerRepository::new());
        let tx_repo = Arc::new(MockTransactionRepository::new());
        let job_producer = Arc::new(MockJobProducerTrait::new());
        let counter = Arc::new(MockTransactionCounterServiceTrait::new());
        let sync_manager = TestCtx::create_mock_sync_manager();
        let signer = MidnightSignerFactory::create_midnight_signer(
            &ctx.signer_model.into(),
            NetworkId::TestNet,
        )
        .unwrap();
        let relayer = MidnightRelayer::new(
            relayer_model,
            provider,
            MidnightRelayerDependencies::new(
                relayer_repo,
                ctx.network_repository.clone(),
                tx_repo,
                Arc::new(signer),
                counter,
                Arc::new(Mutex::new(sync_manager)),
                job_producer,
            ),
        )
        .await
        .unwrap();

        let result = relayer.get_balance().await;
        assert!(result.is_ok());
        let balance_response = result.unwrap();
        assert_eq!(balance_response.balance, expected_balance);
        assert_eq!(balance_response.unit, MIDNIGHT_SMALLEST_UNIT_NAME);
    }

    #[tokio::test]
    async fn test_get_balance_provider_error() {
        let ctx = TestCtx::default();
        ctx.setup_network().await;
        let relayer_model = ctx.relayer_model.clone();
        let mut provider = MockMidnightProviderTrait::new();

        provider.expect_get_balance().returning(|_, _| {
            Box::pin(async { Err(ProviderError::Other("provider failed".to_string())) })
        });

        let relayer_repo = Arc::new(MockRelayerRepository::new());
        let tx_repo = Arc::new(MockTransactionRepository::new());
        let job_producer = Arc::new(MockJobProducerTrait::new());
        let counter = Arc::new(MockTransactionCounterServiceTrait::new());
        let sync_manager = TestCtx::create_mock_sync_manager();
        let signer = MidnightSignerFactory::create_midnight_signer(
            &ctx.signer_model.into(),
            NetworkId::TestNet,
        )
        .unwrap();
        let relayer = MidnightRelayer::new(
            relayer_model,
            provider,
            MidnightRelayerDependencies::new(
                relayer_repo,
                ctx.network_repository.clone(),
                tx_repo,
                Arc::new(signer),
                counter,
                Arc::new(Mutex::new(sync_manager)),
                job_producer,
            ),
        )
        .await
        .unwrap();

        let result = relayer.get_balance().await;
        assert!(result.is_err());
        match result.err().unwrap() {
            RelayerError::ProviderError(msg) => {
                assert!(msg.contains("Failed to fetch balance") && msg.contains("provider failed"));
            }
            _ => panic!("Unexpected error type"),
        }
    }
}
