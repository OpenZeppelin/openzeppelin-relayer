/// This module defines the `EvmRelayer` struct and its associated functionality for
/// interacting with Ethereum Virtual Machine (EVM) networks. The `EvmRelayer` is responsible
/// for managing transactions, signing data, and ensuring the relayer's state is synchronized
/// with the blockchain.
///
/// # Components
///
/// - `EvmRelayer`: The main struct that encapsulates the relayer's state and operations.
/// - `RelayerRepoModel`: Represents the relayer's data model.
/// - `EvmSigner`: Handles signing of data and transactions.
/// - `EvmProvider`: Provides blockchain interaction capabilities, such as fetching balances
///   and transaction counts.
/// - `TransactionCounterService`: Manages the nonce for transactions to ensure they are
///   processed in the correct order.
/// - `JobProducer`: Produces jobs for processing transactions and sending notifications.
///
/// # Error Handling
///
/// The module uses the `RelayerError` enum to handle various errors that can occur during
/// operations, such as provider errors, insufficient balance, and transaction failures.
///
/// # Usage
///
/// To use the `EvmRelayer`, create an instance using the `new` method, providing the necessary
/// components. Then, call the appropriate methods to process transactions, sign data, and
/// manage the relayer's state.
use std::sync::Arc;

use crate::{
    constants::EVM_SMALLEST_UNIT_NAME,
    domain::{
        relayer::{Relayer, RelayerError},
        BalanceResponse, JsonRpcRequest, JsonRpcResponse, SignDataRequest, SignDataResponse,
        SignTypedDataRequest,
    },
    jobs::{JobProducer, JobProducerTrait, TransactionRequest},
    models::{
        produce_relayer_disabled_payload, EvmNetwork, EvmRpcResult, NetworkRpcRequest,
        NetworkRpcResult, NetworkTransactionRequest, NetworkType, RelayerRepoModel, RelayerStatus,
        RepositoryError, TransactionRepoModel, TransactionStatus,
    },
    repositories::{
        InMemoryNetworkRepository, InMemoryRelayerRepository, InMemoryTransactionCounter,
        InMemoryTransactionRepository, NetworkRepository, RelayerRepository,
        RelayerRepositoryStorage, Repository, TransactionRepository,
    },
    services::{
        DataSignerTrait, EvmProvider, EvmProviderTrait, EvmSigner, TransactionCounterService,
        TransactionCounterServiceTrait,
    },
};
use async_trait::async_trait;
use eyre::Result;
use log::{info, warn};

use super::EvmTransactionValidator;

#[allow(dead_code)]
pub struct EvmRelayer<P, R, N, T, J, S, C>
where
    P: EvmProviderTrait + Send + Sync,
    R: Repository<RelayerRepoModel, String> + RelayerRepository + Send + Sync,
    T: Repository<TransactionRepoModel, String> + TransactionRepository + Send + Sync,
    N: NetworkRepository + Send + Sync,
    J: JobProducerTrait + Send + Sync,
    S: DataSignerTrait + Send + Sync,
    C: TransactionCounterServiceTrait + Send + Sync,
{
    relayer: RelayerRepoModel,
    signer: S,
    network: EvmNetwork,
    provider: P,
    relayer_repository: Arc<R>,
    network_repository: Arc<N>,
    transaction_repository: Arc<T>,
    transaction_counter_service: Arc<C>,
    job_producer: Arc<J>,
}

#[allow(clippy::too_many_arguments)]
impl<P, R, N, T, J, S, C> EvmRelayer<P, R, N, T, J, S, C>
where
    P: EvmProviderTrait + Send + Sync,
    R: Repository<RelayerRepoModel, String> + RelayerRepository + Send + Sync,
    T: Repository<TransactionRepoModel, String> + TransactionRepository + Send + Sync,
    N: NetworkRepository + Send + Sync,
    J: JobProducerTrait + Send + Sync,
    S: DataSignerTrait + Send + Sync,
    C: TransactionCounterServiceTrait + Send + Sync,
{
    /// Constructs a new `EvmRelayer` instance.
    ///
    /// # Arguments
    ///
    /// * `relayer` - The relayer's data model.
    /// * `signer` - The EVM signer for signing data and transactions.
    /// * `provider` - The EVM provider for blockchain interactions.
    /// * `network` - The EVM network configuration.
    /// * `relayer_repository` - The repository for relayer storage.
    /// * `transaction_repository` - The repository for transaction storage.
    /// * `transaction_counter_service` - The service for managing transaction nonces.
    /// * `job_producer` - The job producer for creating transaction jobs.
    ///
    /// # Returns
    ///
    /// A `Result` containing the new `EvmRelayer` instance or a `RelayerError`
    pub fn new(
        relayer: RelayerRepoModel,
        signer: S,
        provider: P,
        network: EvmNetwork,
        relayer_repository: Arc<R>,
        network_repository: Arc<N>,
        transaction_repository: Arc<T>,
        transaction_counter_service: Arc<C>,
        job_producer: Arc<J>,
    ) -> Result<Self, RelayerError> {
        Ok(Self {
            relayer,
            signer,
            network,
            provider,
            relayer_repository,
            network_repository,
            transaction_repository,
            transaction_counter_service,
            job_producer,
        })
    }

    /// Synchronizes the nonce with the blockchain.
    ///
    /// # Returns
    ///
    /// A `Result` indicating success or a `RelayerError` if the operation fails.
    async fn sync_nonce(&self) -> Result<(), RelayerError> {
        let on_chain_nonce = self
            .provider
            .get_transaction_count(&self.relayer.address)
            .await
            .map_err(|e| RelayerError::ProviderError(e.to_string()))?;

        info!(
            "Setting nonce: {} for relayer: {}",
            on_chain_nonce, self.relayer.id
        );

        self.transaction_counter_service.set(on_chain_nonce).await?;

        Ok(())
    }

    /// Validates the RPC connection to the blockchain provider.
    ///
    /// # Returns
    ///
    /// A `Result` indicating success or a `RelayerError` if the operation fails.
    async fn validate_rpc(&self) -> Result<(), RelayerError> {
        self.provider
            .health_check()
            .await
            .map_err(|e| RelayerError::ProviderError(e.to_string()))?;

        Ok(())
    }
}

// Define a concrete type alias for common usage
pub type DefaultEvmRelayer = EvmRelayer<
    EvmProvider,
    RelayerRepositoryStorage<InMemoryRelayerRepository>,
    InMemoryNetworkRepository,
    InMemoryTransactionRepository,
    JobProducer,
    EvmSigner,
    TransactionCounterService<InMemoryTransactionCounter>,
>;

#[async_trait]
impl<P, R, N, T, J, S, C> Relayer for EvmRelayer<P, R, N, T, J, S, C>
where
    P: EvmProviderTrait + Send + Sync,
    R: Repository<RelayerRepoModel, String> + RelayerRepository + Send + Sync,
    N: NetworkRepository + Send + Sync,
    T: Repository<TransactionRepoModel, String> + TransactionRepository + Send + Sync,
    J: JobProducerTrait + Send + Sync,
    S: DataSignerTrait + Send + Sync,
    C: TransactionCounterServiceTrait + Send + Sync,
{
    /// Processes a transaction request and creates a job for it.
    ///
    /// # Arguments
    ///
    /// * `network_transaction` - The network transaction request to process.
    ///
    /// # Returns
    ///
    /// A `Result` containing the `TransactionRepoModel` or a `RelayerError`.
    async fn process_transaction_request(
        &self,
        network_transaction: NetworkTransactionRequest,
    ) -> Result<TransactionRepoModel, RelayerError> {
        let network_model = self
            .network_repository
            .get_by_name(NetworkType::Evm, &self.relayer.network)
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

    /// Retrieves the balance of the relayer's address.
    ///
    /// # Returns
    ///
    /// A `Result` containing the `BalanceResponse` or a `RelayerError`.
    async fn get_balance(&self) -> Result<BalanceResponse, RelayerError> {
        let balance: u128 = self
            .provider
            .get_balance(&self.relayer.address)
            .await
            .map_err(|e| RelayerError::ProviderError(e.to_string()))?
            .try_into()
            .map_err(|_| {
                RelayerError::ProviderError("Failed to convert balance to u128".to_string())
            })?;

        Ok(BalanceResponse {
            balance,
            unit: EVM_SMALLEST_UNIT_NAME.to_string(),
        })
    }

    /// Gets the status of the relayer.
    ///
    /// # Returns
    ///
    /// A `Result` containing a boolean indicating the status or a `RelayerError`.
    async fn get_status(&self) -> Result<RelayerStatus, RelayerError> {
        let relayer_model = &self.relayer;

        let nonce_u256 = self
            .provider
            .get_transaction_count(&relayer_model.address)
            .await
            .map_err(|e| RelayerError::ProviderError(format!("Failed to get nonce: {}", e)))?;
        let nonce_str = nonce_u256.to_string();

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

        Ok(RelayerStatus::Evm {
            balance: balance_response.balance.to_string(),
            pending_transactions_count,
            last_confirmed_transaction_timestamp,
            system_disabled: relayer_model.system_disabled,
            paused: relayer_model.paused,
            nonce: nonce_str,
        })
    }

    /// Deletes pending transactions.
    ///
    /// # Returns
    ///
    /// A `Result` containing a boolean indicating success or a `RelayerError`.
    async fn delete_pending_transactions(&self) -> Result<bool, RelayerError> {
        println!("EVM delete_pending_transactions...");
        Ok(true)
    }

    /// Signs data using the relayer's signer.
    ///
    /// # Arguments
    ///
    /// * `request` - The request containing the data to sign.
    ///
    /// # Returns
    ///
    /// A `Result` containing the `SignDataResponse` or a `RelayerError`.
    async fn sign_data(&self, request: SignDataRequest) -> Result<SignDataResponse, RelayerError> {
        let result = self.signer.sign_data(request).await?;

        Ok(result)
    }

    /// Signs typed data using the relayer's signer.
    ///
    /// # Arguments
    ///
    /// * `request` - The request containing the typed data to sign.
    ///
    /// # Returns
    ///
    /// A `Result` containing the `SignDataResponse` or a `RelayerError`.
    async fn sign_typed_data(
        &self,
        request: SignTypedDataRequest,
    ) -> Result<SignDataResponse, RelayerError> {
        let result = self.signer.sign_typed_data(request).await?;

        Ok(result)
    }

    /// Handles a JSON-RPC request.
    ///
    /// # Arguments
    ///
    /// * `_request` - The JSON-RPC request to handle.
    ///
    /// # Returns
    ///
    /// A `Result` containing the `JsonRpcResponse` or a `RelayerError`.
    async fn rpc(
        &self,
        _request: JsonRpcRequest<NetworkRpcRequest>,
    ) -> Result<JsonRpcResponse<NetworkRpcResult>, RelayerError> {
        println!("EVM rpc...");
        Ok(JsonRpcResponse {
            id: Some(1),
            jsonrpc: "2.0".to_string(),
            result: Some(NetworkRpcResult::Evm(EvmRpcResult::GenericRpcResult(
                "".to_string(),
            ))),
            error: None,
        })
    }

    /// Validates that the relayer's balance meets the minimum required balance.
    ///
    /// # Returns
    ///
    /// A `Result` indicating success or a `RelayerError` if the balance is insufficient.
    async fn validate_min_balance(&self) -> Result<(), RelayerError> {
        let policy = self.relayer.policies.get_evm_policy();
        EvmTransactionValidator::init_balance_validation(
            &self.relayer.address,
            &policy,
            &self.provider,
        )
        .await
        .map_err(|e| RelayerError::InsufficientBalanceError(e.to_string()))?;

        Ok(())
    }

    /// Initializes the relayer by performing necessary checks and synchronizations.
    ///
    /// # Returns
    ///
    /// A `Result` indicating success or a `RelayerError` if any initialization step fails.
    async fn initialize_relayer(&self) -> Result<(), RelayerError> {
        info!("Initializing relayer: {}", self.relayer.id);
        let nonce_sync_result = self.sync_nonce().await;
        let validate_rpc_result = self.validate_rpc().await;
        let validate_min_balance_result = self.validate_min_balance().await;

        // disable relayer if any check fails
        if nonce_sync_result.is_err()
            || validate_rpc_result.is_err()
            || validate_min_balance_result.is_err()
        {
            let reason = vec![
                nonce_sync_result
                    .err()
                    .map(|e| format!("Nonce sync failed: {}", e)),
                validate_rpc_result
                    .err()
                    .map(|e| format!("RPC validation failed: {}", e)),
                validate_min_balance_result
                    .err()
                    .map(|e| format!("Balance check failed: {}", e)),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<String>>()
            .join(", ");

            warn!("Disabling relayer: {} due to: {}", self.relayer.id, reason);
            let updated_relayer = self
                .relayer_repository
                .disable_relayer(self.relayer.id.clone())
                .await?;
            if let Some(notification_id) = &self.relayer.notification_id {
                self.job_producer
                    .produce_send_notification_job(
                        produce_relayer_disabled_payload(
                            notification_id,
                            &updated_relayer,
                            &reason,
                        ),
                        None,
                    )
                    .await?;
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
        models::{
            NetworkRepoModel, NetworkType, RelayerEvmPolicy, RelayerNetworkPolicy, RepositoryError,
            SignerError, TransactionStatus, U256,
        },
        repositories::{MockNetworkRepository, MockRelayerRepository, MockTransactionRepository},
        services::{MockEvmProviderTrait, MockTransactionCounterServiceTrait, ProviderError},
    };
    use mockall::predicate::*;
    use std::future::ready;

    mockall::mock! {
        pub DataSigner {}

        #[async_trait]
        impl DataSignerTrait for DataSigner {
            async fn sign_data(&self, request: SignDataRequest) -> Result<SignDataResponse, SignerError>;
            async fn sign_typed_data(&self, request: SignTypedDataRequest) -> Result<SignDataResponse, SignerError>;
        }
    }

    fn create_test_evm_network() -> EvmNetwork {
        EvmNetwork {
            network: "mainnet".to_string(),
            rpc_urls: vec!["https://mainnet.infura.io/v3/YOUR_INFURA_API_KEY".to_string()],
            explorer_urls: None,
            average_blocktime_ms: 12000,
            is_testnet: false,
            tags: vec!["mainnet".to_string()],
            chain_id: 1,
            required_confirmations: 1,
            features: vec!["eip1559".to_string()],
            symbol: "ETH".to_string(),
        }
    }

    fn create_test_network_repo_model() -> NetworkRepoModel {
        use crate::config::{EvmNetworkConfig, NetworkConfigCommon};

        let config = EvmNetworkConfig {
            common: NetworkConfigCommon {
                network: "mainnet".to_string(),
                from: None,
                rpc_urls: Some(vec![
                    "https://mainnet.infura.io/v3/YOUR_INFURA_API_KEY".to_string()
                ]),
                explorer_urls: None,
                average_blocktime_ms: Some(12000),
                is_testnet: Some(false),
                tags: Some(vec!["mainnet".to_string()]),
            },
            chain_id: Some(1),
            required_confirmations: Some(1),
            features: Some(vec!["eip1559".to_string()]),
            symbol: Some("ETH".to_string()),
        };

        NetworkRepoModel::new_evm(config)
    }

    fn create_test_relayer() -> RelayerRepoModel {
        RelayerRepoModel {
            id: "test-relayer-id".to_string(),
            name: "Test Relayer".to_string(),
            network: "mainnet".to_string(), // Changed from "1" to "mainnet"
            address: "0xSender".to_string(),
            paused: false,
            system_disabled: false,
            signer_id: "test-signer-id".to_string(),
            notification_id: Some("test-notification-id".to_string()),
            policies: RelayerNetworkPolicy::Evm(RelayerEvmPolicy {
                min_balance: 100000000000000000u128, // 0.1 ETH
                whitelist_receivers: Some(vec!["0xRecipient".to_string()]),
                gas_price_cap: Some(100000000000), // 100 Gwei
                eip1559_pricing: Some(false),
                private_transactions: false,
            }),
            network_type: NetworkType::Evm,
            custom_rpc_urls: None,
        }
    }

    fn setup_mocks() -> (
        MockEvmProviderTrait,
        MockRelayerRepository,
        MockNetworkRepository,
        MockTransactionRepository,
        MockJobProducerTrait,
        MockDataSigner,
        MockTransactionCounterServiceTrait,
    ) {
        (
            MockEvmProviderTrait::new(),
            MockRelayerRepository::new(),
            MockNetworkRepository::new(),
            MockTransactionRepository::new(),
            MockJobProducerTrait::new(),
            MockDataSigner::new(),
            MockTransactionCounterServiceTrait::new(),
        )
    }

    #[tokio::test]
    async fn test_get_balance() {
        let (mut provider, relayer_repo, network_repo, tx_repo, job_producer, signer, counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        provider
            .expect_get_balance()
            .with(eq("0xSender"))
            .returning(|_| Box::pin(ready(Ok(U256::from(1000000000000000000u64))))); // 1 ETH

        let relayer = EvmRelayer::new(
            relayer_model,
            signer,
            provider,
            create_test_evm_network(),
            Arc::new(relayer_repo),
            Arc::new(network_repo),
            Arc::new(tx_repo),
            Arc::new(counter),
            Arc::new(job_producer),
        )
        .unwrap();

        let balance = relayer.get_balance().await.unwrap();
        assert_eq!(balance.balance, 1000000000000000000u128);
        assert_eq!(balance.unit, EVM_SMALLEST_UNIT_NAME);
    }

    #[tokio::test]
    async fn test_process_transaction_request() {
        let (
            provider,
            relayer_repo,
            mut network_repo,
            mut tx_repo,
            mut job_producer,
            signer,
            counter,
        ) = setup_mocks();
        let relayer_model = create_test_relayer();

        let network_tx = NetworkTransactionRequest::Evm(crate::models::EvmTransactionRequest {
            to: Some("0xRecipient".to_string()),
            value: U256::from(1000000000000000000u64),
            data: Some("0xData".to_string()),
            gas_limit: 21000,
            gas_price: Some(20000000000),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            speed: None,
            valid_until: None,
        });

        network_repo
            .expect_get_by_name()
            .with(eq(NetworkType::Evm), eq("mainnet"))
            .returning(|_, _| Ok(Some(create_test_network_repo_model())));

        tx_repo.expect_create().returning(Ok);
        job_producer
            .expect_produce_transaction_request_job()
            .returning(|_, _| Box::pin(ready(Ok(()))));

        let relayer = EvmRelayer::new(
            relayer_model,
            signer,
            provider,
            create_test_evm_network(),
            Arc::new(relayer_repo),
            Arc::new(network_repo),
            Arc::new(tx_repo),
            Arc::new(counter),
            Arc::new(job_producer),
        )
        .unwrap();

        let result = relayer.process_transaction_request(network_tx).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validate_min_balance_sufficient() {
        let (mut provider, relayer_repo, network_repo, tx_repo, job_producer, signer, counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        provider
            .expect_get_balance()
            .returning(|_| Box::pin(ready(Ok(U256::from(200000000000000000u64))))); // 0.2 ETH > min_balance

        let relayer = EvmRelayer::new(
            relayer_model,
            signer,
            provider,
            create_test_evm_network(),
            Arc::new(relayer_repo),
            Arc::new(network_repo),
            Arc::new(tx_repo),
            Arc::new(counter),
            Arc::new(job_producer),
        )
        .unwrap();

        let result = relayer.validate_min_balance().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validate_min_balance_insufficient() {
        let (mut provider, relayer_repo, network_repo, tx_repo, job_producer, signer, counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        provider
            .expect_get_balance()
            .returning(|_| Box::pin(ready(Ok(U256::from(50000000000000000u64))))); // 0.05 ETH < min_balance

        let relayer = EvmRelayer::new(
            relayer_model,
            signer,
            provider,
            create_test_evm_network(),
            Arc::new(relayer_repo),
            Arc::new(network_repo),
            Arc::new(tx_repo),
            Arc::new(counter),
            Arc::new(job_producer),
        )
        .unwrap();

        let result = relayer.validate_min_balance().await;
        assert!(matches!(
            result,
            Err(RelayerError::InsufficientBalanceError(_))
        ));
    }

    #[tokio::test]
    async fn test_sync_nonce() {
        let (mut provider, relayer_repo, network_repo, tx_repo, job_producer, signer, mut counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        provider
            .expect_get_transaction_count()
            .returning(|_| Box::pin(ready(Ok(42u64))));

        counter
            .expect_set()
            .returning(|_nonce| Box::pin(ready(Ok(()))));

        let relayer = EvmRelayer::new(
            relayer_model,
            signer,
            provider,
            create_test_evm_network(),
            Arc::new(relayer_repo),
            Arc::new(network_repo),
            Arc::new(tx_repo),
            Arc::new(counter),
            Arc::new(job_producer),
        )
        .unwrap();

        let result = relayer.sync_nonce().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validate_rpc() {
        let (mut provider, relayer_repo, network_repo, tx_repo, job_producer, signer, counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        provider
            .expect_health_check()
            .returning(|| Box::pin(ready(Ok(true))));

        let relayer = EvmRelayer::new(
            relayer_model,
            signer,
            provider,
            create_test_evm_network(),
            Arc::new(relayer_repo),
            Arc::new(network_repo),
            Arc::new(tx_repo),
            Arc::new(counter),
            Arc::new(job_producer),
        )
        .unwrap();

        let result = relayer.validate_rpc().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_get_status_success() {
        let (mut provider, relayer_repo, network_repo, mut tx_repo, job_producer, signer, counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        provider
            .expect_get_transaction_count()
            .returning(|_| Box::pin(ready(Ok(10u64))))
            .once();
        provider
            .expect_get_balance()
            .returning(|_| Box::pin(ready(Ok(U256::from(1000000000000000000u64)))))
            .once();

        let pending_txs_clone = vec![];
        tx_repo
            .expect_find_by_status()
            .withf(|relayer_id, statuses| {
                relayer_id == "test-relayer-id"
                    && statuses == [TransactionStatus::Pending, TransactionStatus::Submitted]
            })
            .returning(move |_, _| {
                Ok(pending_txs_clone.clone()) as Result<Vec<TransactionRepoModel>, RepositoryError>
            })
            .once();

        let confirmed_txs_clone = vec![
            TransactionRepoModel {
                id: "tx1".to_string(),
                relayer_id: relayer_model.id.clone(),
                status: TransactionStatus::Confirmed,
                confirmed_at: Some("2023-01-01T12:00:00Z".to_string()),
                ..TransactionRepoModel::default()
            },
            TransactionRepoModel {
                id: "tx2".to_string(),
                relayer_id: relayer_model.id.clone(),
                status: TransactionStatus::Confirmed,
                confirmed_at: Some("2023-01-01T10:00:00Z".to_string()),
                ..TransactionRepoModel::default()
            },
        ];
        tx_repo
            .expect_find_by_status()
            .withf(|relayer_id, statuses| {
                relayer_id == "test-relayer-id" && statuses == [TransactionStatus::Confirmed]
            })
            .returning(move |_, _| {
                Ok(confirmed_txs_clone.clone())
                    as Result<Vec<TransactionRepoModel>, RepositoryError>
            })
            .once();

        let relayer = EvmRelayer::new(
            relayer_model.clone(),
            signer,
            provider,
            create_test_evm_network(),
            Arc::new(relayer_repo),
            Arc::new(network_repo),
            Arc::new(tx_repo),
            Arc::new(counter),
            Arc::new(job_producer),
        )
        .unwrap();

        let status = relayer.get_status().await.unwrap();

        match status {
            RelayerStatus::Evm {
                balance,
                pending_transactions_count,
                last_confirmed_transaction_timestamp,
                system_disabled,
                paused,
                nonce,
            } => {
                assert_eq!(balance, "1000000000000000000");
                assert_eq!(pending_transactions_count, 0);
                assert_eq!(
                    last_confirmed_transaction_timestamp,
                    Some("2023-01-01T12:00:00Z".to_string())
                );
                assert_eq!(system_disabled, relayer_model.system_disabled);
                assert_eq!(paused, relayer_model.paused);
                assert_eq!(nonce, "10");
            }
            _ => panic!("Expected EVM RelayerStatus"),
        }
    }

    #[tokio::test]
    async fn test_get_status_provider_nonce_error() {
        let (mut provider, relayer_repo, network_repo, tx_repo, job_producer, signer, counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        provider.expect_get_transaction_count().returning(|_| {
            Box::pin(ready(Err(ProviderError::Other(
                "Nonce fetch failed".to_string(),
            ))))
        });

        let relayer = EvmRelayer::new(
            relayer_model.clone(),
            signer,
            provider,
            create_test_evm_network(),
            Arc::new(relayer_repo),
            Arc::new(network_repo),
            Arc::new(tx_repo),
            Arc::new(counter),
            Arc::new(job_producer),
        )
        .unwrap();

        let result = relayer.get_status().await;
        assert!(result.is_err());
        match result.err().unwrap() {
            RelayerError::ProviderError(msg) => assert!(msg.contains("Failed to get nonce")),
            _ => panic!("Expected ProviderError for nonce failure"),
        }
    }

    #[tokio::test]
    async fn test_get_status_repository_pending_error() {
        let (mut provider, relayer_repo, network_repo, mut tx_repo, job_producer, signer, counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        provider
            .expect_get_transaction_count()
            .returning(|_| Box::pin(ready(Ok(10u64))));
        provider
            .expect_get_balance()
            .returning(|_| Box::pin(ready(Ok(U256::from(1000000000000000000u64)))));

        tx_repo
            .expect_find_by_status()
            .withf(|relayer_id, statuses| {
                relayer_id == "test-relayer-id"
                    && statuses == [TransactionStatus::Pending, TransactionStatus::Submitted]
            })
            .returning(|_, _| {
                Err(RepositoryError::Unknown("DB down".to_string()))
                    as Result<Vec<TransactionRepoModel>, RepositoryError>
            })
            .once();

        let relayer = EvmRelayer::new(
            relayer_model.clone(),
            signer,
            provider,
            create_test_evm_network(),
            Arc::new(relayer_repo),
            Arc::new(network_repo),
            Arc::new(tx_repo),
            Arc::new(counter),
            Arc::new(job_producer),
        )
        .unwrap();

        let result = relayer.get_status().await;
        assert!(result.is_err());
        match result.err().unwrap() {
            // Remember our From<RepositoryError> for RelayerError maps to NetworkConfiguration
            RelayerError::NetworkConfiguration(msg) => assert!(msg.contains("DB down")),
            _ => panic!("Expected NetworkConfiguration error for repo failure"),
        }
    }

    #[tokio::test]
    async fn test_get_status_no_confirmed_transactions() {
        let (mut provider, relayer_repo, network_repo, mut tx_repo, job_producer, signer, counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        provider
            .expect_get_transaction_count()
            .returning(|_| Box::pin(ready(Ok(10u64))));
        provider
            .expect_get_balance()
            .returning(|_| Box::pin(ready(Ok(U256::from(1000000000000000000u64)))));
        provider
            .expect_health_check()
            .returning(|| Box::pin(ready(Ok(true))));

        let pending_txs_empty_clone = vec![];
        tx_repo
            .expect_find_by_status()
            .withf(|relayer_id, statuses| {
                relayer_id == "test-relayer-id"
                    && statuses == [TransactionStatus::Pending, TransactionStatus::Submitted]
            })
            .returning(move |_, _| {
                Ok(pending_txs_empty_clone.clone())
                    as Result<Vec<TransactionRepoModel>, RepositoryError>
            })
            .once();

        let confirmed_txs_empty_clone = vec![];
        tx_repo
            .expect_find_by_status()
            .withf(|relayer_id, statuses| {
                relayer_id == "test-relayer-id" && statuses == [TransactionStatus::Confirmed]
            })
            .returning(move |_, _| {
                Ok(confirmed_txs_empty_clone.clone())
                    as Result<Vec<TransactionRepoModel>, RepositoryError>
            })
            .once();

        let relayer = EvmRelayer::new(
            relayer_model.clone(),
            signer,
            provider,
            create_test_evm_network(),
            Arc::new(relayer_repo),
            Arc::new(network_repo),
            Arc::new(tx_repo),
            Arc::new(counter),
            Arc::new(job_producer),
        )
        .unwrap();

        let status = relayer.get_status().await.unwrap();
        match status {
            RelayerStatus::Evm {
                balance,
                pending_transactions_count,
                last_confirmed_transaction_timestamp,
                system_disabled,
                paused,
                nonce,
            } => {
                assert_eq!(balance, "1000000000000000000");
                assert_eq!(pending_transactions_count, 0);
                assert_eq!(last_confirmed_transaction_timestamp, None);
                assert_eq!(system_disabled, relayer_model.system_disabled);
                assert_eq!(paused, relayer_model.paused);
                assert_eq!(nonce, "10");
            }
            _ => panic!("Expected EVM RelayerStatus"),
        }
    }
}
