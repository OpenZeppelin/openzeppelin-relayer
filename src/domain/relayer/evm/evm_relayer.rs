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
        BalanceResponse, SignDataRequest, SignDataResponse, SignTypedDataRequest,
    },
    jobs::{JobProducerTrait, TransactionRequest},
    models::{
        produce_relayer_disabled_payload, DeletePendingTransactionsResponse, EvmNetwork,
        JsonRpcRequest, JsonRpcResponse, NetworkRepoModel, NetworkRpcRequest, NetworkRpcResult,
        NetworkTransactionRequest, NetworkType, RelayerRepoModel, RelayerStatus, RepositoryError,
        RpcErrorCodes, TransactionRepoModel, TransactionStatus,
    },
    repositories::{NetworkRepository, RelayerRepository, Repository, TransactionRepository},
    services::{
        DataSignerTrait, EvmProvider, EvmProviderTrait, EvmSigner, TransactionCounterService,
        TransactionCounterServiceTrait,
    },
};
use async_trait::async_trait;
use eyre::Result;
use log::{debug, info, warn};

use super::{
    create_error_response, create_success_response, map_provider_error, EvmTransactionValidator,
};

#[allow(dead_code)]
pub struct EvmRelayer<P, RR, NR, TR, J, S, TCS>
where
    P: EvmProviderTrait + Send + Sync,
    RR: Repository<RelayerRepoModel, String> + RelayerRepository + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    TR: Repository<TransactionRepoModel, String> + TransactionRepository + Send + Sync + 'static,
    J: JobProducerTrait + Send + Sync + 'static,
    S: DataSignerTrait + Send + Sync + 'static,
{
    relayer: RelayerRepoModel,
    signer: S,
    network: EvmNetwork,
    provider: P,
    relayer_repository: Arc<RR>,
    network_repository: Arc<NR>,
    transaction_repository: Arc<TR>,
    job_producer: Arc<J>,
    transaction_counter_service: Arc<TCS>,
}

#[allow(clippy::too_many_arguments)]
impl<P, RR, NR, TR, J, S, TCS> EvmRelayer<P, RR, NR, TR, J, S, TCS>
where
    P: EvmProviderTrait + Send + Sync,
    RR: Repository<RelayerRepoModel, String> + RelayerRepository + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    TR: Repository<TransactionRepoModel, String> + TransactionRepository + Send + Sync + 'static,
    J: JobProducerTrait + Send + Sync + 'static,
    S: DataSignerTrait + Send + Sync + 'static,
    TCS: TransactionCounterServiceTrait + Send + Sync + 'static,
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
        relayer_repository: Arc<RR>,
        network_repository: Arc<NR>,
        transaction_repository: Arc<TR>,
        transaction_counter_service: Arc<TCS>,
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

        let transaction_counter_nonce = self
            .transaction_counter_service
            .get()
            .await
            .unwrap_or(Some(0))
            .unwrap_or(0);

        let nonce = std::cmp::max(on_chain_nonce, transaction_counter_nonce);

        debug!(
            "Relayer: {} - On-chain nonce: {}, Transaction counter nonce: {}",
            self.relayer.id, on_chain_nonce, transaction_counter_nonce
        );

        info!("Setting nonce: {} for relayer: {}", nonce, self.relayer.id);

        self.transaction_counter_service.set(nonce).await?;

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

    /// Initiates transaction cancellation via the job queue system.
    ///
    /// # Arguments
    ///
    /// * `transaction` - The transaction model to cancel.
    ///
    /// # Returns
    ///
    /// A `Result` indicating success or a `RelayerError` if the job creation fails.
    async fn cancel_transaction_via_job(
        &self,
        transaction: TransactionRepoModel,
    ) -> Result<(), RelayerError> {
        use crate::jobs::TransactionSend;

        let cancel_job = TransactionSend::cancel(
            transaction.id.clone(),
            transaction.relayer_id.clone(),
            "Cancelled via delete_pending_transactions".to_string(),
        );

        self.job_producer
            .produce_submit_transaction_job(cancel_job, None)
            .await
            .map_err(RelayerError::from)?;

        Ok(())
    }
}

// Define a concrete type alias for common usage
pub type DefaultEvmRelayer<J, T, RR, NR, TCR> =
    EvmRelayer<EvmProvider, RR, NR, T, J, EvmSigner, TransactionCounterService<TCR>>;

#[async_trait]
impl<P, RR, NR, TR, J, S, TCS> Relayer for EvmRelayer<P, RR, NR, TR, J, S, TCS>
where
    P: EvmProviderTrait + Send + Sync,
    RR: Repository<RelayerRepoModel, String> + RelayerRepository + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    TR: Repository<TransactionRepoModel, String> + TransactionRepository + Send + Sync + 'static,
    J: JobProducerTrait + Send + Sync + 'static,
    S: DataSignerTrait + Send + Sync + 'static,
    TCS: TransactionCounterServiceTrait + Send + Sync + 'static,
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
    /// A `Result` containing a `DeletePendingTransactionsResponse` with details
    /// about which transactions were cancelled and which failed, or a `RelayerError`.
    async fn delete_pending_transactions(
        &self,
    ) -> Result<DeletePendingTransactionsResponse, RelayerError> {
        let pending_statuses = [
            TransactionStatus::Pending,
            TransactionStatus::Sent,
            TransactionStatus::Submitted,
        ];

        // Get all pending transactions
        let pending_transactions = self
            .transaction_repository
            .find_by_status(&self.relayer.id, &pending_statuses[..])
            .await
            .map_err(RelayerError::from)?;

        let transaction_count = pending_transactions.len();

        if transaction_count == 0 {
            info!(
                "No pending transactions found for relayer: {}",
                self.relayer.id
            );
            return Ok(DeletePendingTransactionsResponse {
                queued_for_cancellation_transaction_ids: vec![],
                failed_to_queue_transaction_ids: vec![],
                total_processed: 0,
            });
        }

        info!(
            "Processing {} pending transactions for relayer: {}",
            transaction_count, self.relayer.id
        );

        let mut cancelled_transaction_ids = Vec::new();
        let mut failed_transaction_ids = Vec::new();

        // Process all pending transactions using the proper cancellation logic via job queue
        for transaction in pending_transactions {
            match self.cancel_transaction_via_job(transaction.clone()).await {
                Ok(_) => {
                    cancelled_transaction_ids.push(transaction.id.clone());
                    info!(
                        "Initiated cancellation for transaction {} with status {:?} for relayer {}",
                        transaction.id, transaction.status, self.relayer.id
                    );
                }
                Err(e) => {
                    failed_transaction_ids.push(transaction.id.clone());
                    warn!(
                        "Failed to cancel transaction {} for relayer {}: {}",
                        transaction.id, self.relayer.id, e
                    );
                }
            }
        }

        let total_processed = cancelled_transaction_ids.len() + failed_transaction_ids.len();

        info!("Completed processing pending transactions for relayer {}: {} queued for cancellation, {} failed to queue",
              self.relayer.id, cancelled_transaction_ids.len(), failed_transaction_ids.len());

        Ok(DeletePendingTransactionsResponse {
            queued_for_cancellation_transaction_ids: cancelled_transaction_ids,
            failed_to_queue_transaction_ids: failed_transaction_ids,
            total_processed: total_processed as u32,
        })
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
    /// * `request` - The JSON-RPC request to handle.
    ///
    /// # Returns
    ///
    /// A `Result` containing the `JsonRpcResponse` or a `RelayerError`.
    async fn rpc(
        &self,
        request: JsonRpcRequest<NetworkRpcRequest>,
    ) -> Result<JsonRpcResponse<NetworkRpcResult>, RelayerError> {
        let evm_request = match request.params {
            NetworkRpcRequest::Evm(evm_req) => evm_req,
            _ => {
                return Ok(create_error_response(
                    request.id,
                    RpcErrorCodes::INVALID_PARAMS,
                    "Invalid params",
                    "Expected EVM network request",
                ))
            }
        };

        // Parse method and params from the EVM request
        let (method, params_json) = match evm_request {
            crate::models::EvmRpcRequest::GenericRpcRequest { method, params } => {
                (method, serde_json::Value::String(params))
            }
            crate::models::EvmRpcRequest::RawRpcRequest { method, params } => (method, params),
        };

        // Forward the RPC call to the provider
        match self.provider.raw_request_dyn(&method, params_json).await {
            Ok(result_value) => Ok(create_success_response(request.id, result_value)),
            Err(provider_error) => {
                let (error_code, error_message) = map_provider_error(&provider_error);
                Ok(create_error_response(
                    request.id,
                    error_code,
                    error_message,
                    &provider_error.to_string(),
                ))
            }
        }
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
            EvmRpcRequest, EvmRpcResult, JsonRpcId, NetworkRepoModel, NetworkType,
            RelayerEvmPolicy, RelayerNetworkPolicy, RepositoryError, SignerError,
            TransactionStatus, U256,
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
                min_balance: Some(100000000000000000u128), // 0.1 ETH
                whitelist_receivers: Some(vec!["0xRecipient".to_string()]),
                gas_price_cap: Some(100000000000), // 100 Gwei
                eip1559_pricing: Some(true),
                private_transactions: Some(false),
                gas_limit_estimation: Some(true),
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
            gas_limit: Some(21000),
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

        counter
            .expect_get()
            .returning(|| Box::pin(ready(Ok(Some(42u64)))));

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
    async fn test_sync_nonce_lower_on_chain_nonce() {
        let (mut provider, relayer_repo, network_repo, tx_repo, job_producer, signer, mut counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        provider
            .expect_get_transaction_count()
            .returning(|_| Box::pin(ready(Ok(40u64))));

        counter
            .expect_set()
            .with(eq(42u64))
            .returning(|_nonce| Box::pin(ready(Ok(()))));

        counter
            .expect_get()
            .returning(|| Box::pin(ready(Ok(Some(42u64)))));

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
    async fn test_sync_nonce_lower_transaction_counter_nonce() {
        let (mut provider, relayer_repo, network_repo, tx_repo, job_producer, signer, mut counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        provider
            .expect_get_transaction_count()
            .returning(|_| Box::pin(ready(Ok(42u64))));

        counter
            .expect_set()
            .with(eq(42u64))
            .returning(|_nonce| Box::pin(ready(Ok(()))));

        counter
            .expect_get()
            .returning(|| Box::pin(ready(Ok(Some(40u64)))));

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

    #[tokio::test]
    async fn test_cancel_transaction_via_job_success() {
        let (provider, relayer_repo, network_repo, tx_repo, mut job_producer, signer, counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        let test_transaction = TransactionRepoModel {
            id: "test-tx-id".to_string(),
            relayer_id: relayer_model.id.clone(),
            status: TransactionStatus::Pending,
            ..TransactionRepoModel::default()
        };

        job_producer
            .expect_produce_submit_transaction_job()
            .withf(|job, delay| {
                matches!(job.command, crate::jobs::TransactionCommand::Cancel { ref reason }
                    if job.transaction_id == "test-tx-id"
                    && job.relayer_id == "test-relayer-id"
                    && reason == "Cancelled via delete_pending_transactions")
                    && delay.is_none()
            })
            .returning(|_, _| Box::pin(ready(Ok(()))))
            .once();

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

        let result = relayer.cancel_transaction_via_job(test_transaction).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_cancel_transaction_via_job_failure() {
        let (provider, relayer_repo, network_repo, tx_repo, mut job_producer, signer, counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        let test_transaction = TransactionRepoModel {
            id: "test-tx-id".to_string(),
            relayer_id: relayer_model.id.clone(),
            status: TransactionStatus::Pending,
            ..TransactionRepoModel::default()
        };

        job_producer
            .expect_produce_submit_transaction_job()
            .returning(|_, _| {
                Box::pin(ready(Err(crate::jobs::JobProducerError::QueueError(
                    "Queue is full".to_string(),
                ))))
            })
            .once();

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

        let result = relayer.cancel_transaction_via_job(test_transaction).await;
        assert!(result.is_err());
        match result.err().unwrap() {
            RelayerError::QueueError(_) => (),
            _ => panic!("Expected QueueError"),
        }
    }

    #[tokio::test]
    async fn test_delete_pending_transactions_no_pending() {
        let (provider, relayer_repo, network_repo, mut tx_repo, job_producer, signer, counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        tx_repo
            .expect_find_by_status()
            .withf(|relayer_id, statuses| {
                relayer_id == "test-relayer-id"
                    && statuses
                        == [
                            TransactionStatus::Pending,
                            TransactionStatus::Sent,
                            TransactionStatus::Submitted,
                        ]
            })
            .returning(|_, _| Ok(vec![]))
            .once();

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

        let result = relayer.delete_pending_transactions().await.unwrap();
        assert_eq!(result.queued_for_cancellation_transaction_ids.len(), 0);
        assert_eq!(result.failed_to_queue_transaction_ids.len(), 0);
        assert_eq!(result.total_processed, 0);
    }

    #[tokio::test]
    async fn test_delete_pending_transactions_all_successful() {
        let (provider, relayer_repo, network_repo, mut tx_repo, mut job_producer, signer, counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        let pending_transactions = vec![
            TransactionRepoModel {
                id: "tx1".to_string(),
                relayer_id: relayer_model.id.clone(),
                status: TransactionStatus::Pending,
                ..TransactionRepoModel::default()
            },
            TransactionRepoModel {
                id: "tx2".to_string(),
                relayer_id: relayer_model.id.clone(),
                status: TransactionStatus::Sent,
                ..TransactionRepoModel::default()
            },
            TransactionRepoModel {
                id: "tx3".to_string(),
                relayer_id: relayer_model.id.clone(),
                status: TransactionStatus::Submitted,
                ..TransactionRepoModel::default()
            },
        ];

        tx_repo
            .expect_find_by_status()
            .withf(|relayer_id, statuses| {
                relayer_id == "test-relayer-id"
                    && statuses
                        == [
                            TransactionStatus::Pending,
                            TransactionStatus::Sent,
                            TransactionStatus::Submitted,
                        ]
            })
            .returning(move |_, _| Ok(pending_transactions.clone()))
            .once();

        job_producer
            .expect_produce_submit_transaction_job()
            .returning(|_, _| Box::pin(ready(Ok(()))))
            .times(3);

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

        let result = relayer.delete_pending_transactions().await.unwrap();
        assert_eq!(result.queued_for_cancellation_transaction_ids.len(), 3);
        assert_eq!(result.failed_to_queue_transaction_ids.len(), 0);
        assert_eq!(result.total_processed, 3);

        let expected_ids = vec!["tx1", "tx2", "tx3"];
        for id in expected_ids {
            assert!(result
                .queued_for_cancellation_transaction_ids
                .contains(&id.to_string()));
        }
    }

    #[tokio::test]
    async fn test_delete_pending_transactions_partial_failures() {
        let (provider, relayer_repo, network_repo, mut tx_repo, mut job_producer, signer, counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        let pending_transactions = vec![
            TransactionRepoModel {
                id: "tx1".to_string(),
                relayer_id: relayer_model.id.clone(),
                status: TransactionStatus::Pending,
                ..TransactionRepoModel::default()
            },
            TransactionRepoModel {
                id: "tx2".to_string(),
                relayer_id: relayer_model.id.clone(),
                status: TransactionStatus::Sent,
                ..TransactionRepoModel::default()
            },
            TransactionRepoModel {
                id: "tx3".to_string(),
                relayer_id: relayer_model.id.clone(),
                status: TransactionStatus::Submitted,
                ..TransactionRepoModel::default()
            },
        ];

        tx_repo
            .expect_find_by_status()
            .withf(|relayer_id, statuses| {
                relayer_id == "test-relayer-id"
                    && statuses
                        == [
                            TransactionStatus::Pending,
                            TransactionStatus::Sent,
                            TransactionStatus::Submitted,
                        ]
            })
            .returning(move |_, _| Ok(pending_transactions.clone()))
            .once();

        // First job succeeds, second fails, third succeeds
        job_producer
            .expect_produce_submit_transaction_job()
            .returning(|_, _| Box::pin(ready(Ok(()))))
            .times(1);
        job_producer
            .expect_produce_submit_transaction_job()
            .returning(|_, _| {
                Box::pin(ready(Err(crate::jobs::JobProducerError::QueueError(
                    "Queue is full".to_string(),
                ))))
            })
            .times(1);
        job_producer
            .expect_produce_submit_transaction_job()
            .returning(|_, _| Box::pin(ready(Ok(()))))
            .times(1);

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

        let result = relayer.delete_pending_transactions().await.unwrap();
        assert_eq!(result.queued_for_cancellation_transaction_ids.len(), 2);
        assert_eq!(result.failed_to_queue_transaction_ids.len(), 1);
        assert_eq!(result.total_processed, 3);
    }

    #[tokio::test]
    async fn test_delete_pending_transactions_repository_error() {
        let (provider, relayer_repo, network_repo, mut tx_repo, job_producer, signer, counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        tx_repo
            .expect_find_by_status()
            .withf(|relayer_id, statuses| {
                relayer_id == "test-relayer-id"
                    && statuses
                        == [
                            TransactionStatus::Pending,
                            TransactionStatus::Sent,
                            TransactionStatus::Submitted,
                        ]
            })
            .returning(|_, _| {
                Err(RepositoryError::Unknown(
                    "Database connection failed".to_string(),
                ))
            })
            .once();

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

        let result = relayer.delete_pending_transactions().await;
        assert!(result.is_err());
        match result.err().unwrap() {
            RelayerError::NetworkConfiguration(msg) => {
                assert!(msg.contains("Database connection failed"))
            }
            _ => panic!("Expected NetworkConfiguration error for repository failure"),
        }
    }

    #[tokio::test]
    async fn test_delete_pending_transactions_all_failures() {
        let (provider, relayer_repo, network_repo, mut tx_repo, mut job_producer, signer, counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        let pending_transactions = vec![
            TransactionRepoModel {
                id: "tx1".to_string(),
                relayer_id: relayer_model.id.clone(),
                status: TransactionStatus::Pending,
                ..TransactionRepoModel::default()
            },
            TransactionRepoModel {
                id: "tx2".to_string(),
                relayer_id: relayer_model.id.clone(),
                status: TransactionStatus::Sent,
                ..TransactionRepoModel::default()
            },
        ];

        tx_repo
            .expect_find_by_status()
            .withf(|relayer_id, statuses| {
                relayer_id == "test-relayer-id"
                    && statuses
                        == [
                            TransactionStatus::Pending,
                            TransactionStatus::Sent,
                            TransactionStatus::Submitted,
                        ]
            })
            .returning(move |_, _| Ok(pending_transactions.clone()))
            .once();

        job_producer
            .expect_produce_submit_transaction_job()
            .returning(|_, _| {
                Box::pin(ready(Err(crate::jobs::JobProducerError::QueueError(
                    "Queue is full".to_string(),
                ))))
            })
            .times(2);

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

        let result = relayer.delete_pending_transactions().await.unwrap();
        assert_eq!(result.queued_for_cancellation_transaction_ids.len(), 0);
        assert_eq!(result.failed_to_queue_transaction_ids.len(), 2);
        assert_eq!(result.total_processed, 2);

        let expected_failed_ids = vec!["tx1", "tx2"];
        for id in expected_failed_ids {
            assert!(result
                .failed_to_queue_transaction_ids
                .contains(&id.to_string()));
        }
    }

    #[tokio::test]
    async fn test_rpc_eth_get_balance() {
        let (mut provider, relayer_repo, network_repo, tx_repo, job_producer, signer, counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        provider
            .expect_raw_request_dyn()
            .withf(|method, params| {
                method == "eth_getBalance"
                    && params.as_str()
                        == Some(r#"["0x742d35Cc6634C0532925a3b844Bc454e4438f44e", "latest"]"#)
            })
            .returning(|_, _| Box::pin(async { Ok(serde_json::json!("0xde0b6b3a7640000")) }));

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

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            params: NetworkRpcRequest::Evm(EvmRpcRequest::GenericRpcRequest {
                method: "eth_getBalance".to_string(),
                params: r#"["0x742d35Cc6634C0532925a3b844Bc454e4438f44e", "latest"]"#.to_string(),
            }),
            id: Some(JsonRpcId::Number(1)),
        };

        let response = relayer.rpc(request).await.unwrap();
        assert!(response.error.is_none());
        assert!(response.result.is_some());

        if let Some(NetworkRpcResult::Evm(EvmRpcResult::RawRpcResult(result))) = response.result {
            assert_eq!(result, serde_json::json!("0xde0b6b3a7640000")); // 1 ETH in hex
        }
    }

    #[tokio::test]
    async fn test_rpc_eth_block_number() {
        let (mut provider, relayer_repo, network_repo, tx_repo, job_producer, signer, counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        provider
            .expect_raw_request_dyn()
            .withf(|method, params| method == "eth_blockNumber" && params.as_str() == Some("[]"))
            .returning(|_, _| Box::pin(async { Ok(serde_json::json!("0x3039")) }));

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

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            params: NetworkRpcRequest::Evm(EvmRpcRequest::GenericRpcRequest {
                method: "eth_blockNumber".to_string(),
                params: "[]".to_string(),
            }),
            id: Some(JsonRpcId::Number(1)),
        };

        let response = relayer.rpc(request).await.unwrap();
        assert!(response.error.is_none());
        assert!(response.result.is_some());

        if let Some(NetworkRpcResult::Evm(EvmRpcResult::RawRpcResult(result))) = response.result {
            assert_eq!(result, serde_json::json!("0x3039")); // 12345 in hex
        }
    }

    #[tokio::test]
    async fn test_rpc_unsupported_method() {
        let (mut provider, relayer_repo, network_repo, tx_repo, job_producer, signer, counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        provider
            .expect_raw_request_dyn()
            .withf(|method, _| method == "eth_unsupportedMethod")
            .returning(|_, _| {
                Box::pin(async {
                    Err(ProviderError::Other(
                        "Unsupported method: eth_unsupportedMethod".to_string(),
                    ))
                })
            });

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

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            params: NetworkRpcRequest::Evm(EvmRpcRequest::GenericRpcRequest {
                method: "eth_unsupportedMethod".to_string(),
                params: "[]".to_string(),
            }),
            id: Some(JsonRpcId::Number(1)),
        };

        let response = relayer.rpc(request).await.unwrap();
        assert!(response.result.is_none());
        assert!(response.error.is_some());

        let error = response.error.unwrap();
        assert_eq!(error.code, -32603); // RpcErrorCodes::INTERNAL_ERROR
    }

    #[tokio::test]
    async fn test_rpc_invalid_params() {
        let (mut provider, relayer_repo, network_repo, tx_repo, job_producer, signer, counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        provider
            .expect_raw_request_dyn()
            .withf(|method, params| method == "eth_getBalance" && params.as_str() == Some("[]"))
            .returning(|_, _| {
                Box::pin(async {
                    Err(ProviderError::Other(
                        "Missing address parameter".to_string(),
                    ))
                })
            });

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

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            params: NetworkRpcRequest::Evm(EvmRpcRequest::GenericRpcRequest {
                method: "eth_getBalance".to_string(),
                params: "[]".to_string(), // Missing address parameter
            }),
            id: Some(JsonRpcId::Number(1)),
        };

        let response = relayer.rpc(request).await.unwrap();
        assert!(response.result.is_none());
        assert!(response.error.is_some());

        let error = response.error.unwrap();
        assert_eq!(error.code, -32603); // RpcErrorCodes::INTERNAL_ERROR
    }

    #[tokio::test]
    async fn test_rpc_non_evm_request() {
        let (provider, relayer_repo, network_repo, tx_repo, job_producer, signer, counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

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

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            params: NetworkRpcRequest::Solana(crate::models::SolanaRpcRequest::GetSupportedTokens(
                crate::models::GetSupportedTokensRequestParams {},
            )),
            id: Some(JsonRpcId::Number(1)),
        };

        let response = relayer.rpc(request).await.unwrap();
        assert!(response.result.is_none());
        assert!(response.error.is_some());

        let error = response.error.unwrap();
        assert_eq!(error.code, -32602); // RpcErrorCodes::INVALID_PARAMS
    }

    #[tokio::test]
    async fn test_rpc_raw_request_with_array_params() {
        let (mut provider, relayer_repo, network_repo, tx_repo, job_producer, signer, counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        provider
            .expect_raw_request_dyn()
            .withf(|method, params| {
                method == "eth_getTransactionByHash"
                    && params.as_array().is_some_and(|arr| {
                        arr.len() == 1 && arr[0].as_str() == Some("0x1234567890abcdef")
                    })
            })
            .returning(|_, _| {
                Box::pin(async {
                    Ok(serde_json::json!({
                        "hash": "0x1234567890abcdef",
                        "blockNumber": "0x1",
                        "gasUsed": "0x5208"
                    }))
                })
            });

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

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            params: NetworkRpcRequest::Evm(EvmRpcRequest::RawRpcRequest {
                method: "eth_getTransactionByHash".to_string(),
                params: serde_json::json!(["0x1234567890abcdef"]),
            }),
            id: Some(JsonRpcId::Number(42)),
        };

        let response = relayer.rpc(request).await.unwrap();
        assert!(response.error.is_none());
        assert!(response.result.is_some());
        assert_eq!(response.id, Some(JsonRpcId::Number(42)));

        if let Some(NetworkRpcResult::Evm(EvmRpcResult::RawRpcResult(result))) = response.result {
            assert!(result.get("hash").is_some());
            assert!(result.get("blockNumber").is_some());
        }
    }

    #[tokio::test]
    async fn test_rpc_raw_request_with_object_params() {
        let (mut provider, relayer_repo, network_repo, tx_repo, job_producer, signer, counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        provider
            .expect_raw_request_dyn()
            .withf(|method, params| {
                method == "eth_call"
                    && params
                        .as_object()
                        .is_some_and(|obj| obj.contains_key("to") && obj.contains_key("data"))
            })
            .returning(|_, _| {
                Box::pin(async {
                    Ok(serde_json::json!(
                        "0x0000000000000000000000000000000000000000000000000000000000000001"
                    ))
                })
            });

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

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            params: NetworkRpcRequest::Evm(EvmRpcRequest::RawRpcRequest {
                method: "eth_call".to_string(),
                params: serde_json::json!({
                    "to": "0x742d35Cc6634C0532925a3b844Bc454e4438f44e",
                    "data": "0x70a08231000000000000000000000000742d35cc6634c0532925a3b844bc454e4438f44e"
                }),
            }),
            id: Some(JsonRpcId::Number(123)),
        };

        let response = relayer.rpc(request).await.unwrap();
        assert!(response.error.is_none());
        assert!(response.result.is_some());
        assert_eq!(response.id, Some(JsonRpcId::Number(123)));
    }

    #[tokio::test]
    async fn test_rpc_generic_request_with_empty_params() {
        let (mut provider, relayer_repo, network_repo, tx_repo, job_producer, signer, counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        provider
            .expect_raw_request_dyn()
            .withf(|method, params| method == "net_version" && params.as_str() == Some("[]"))
            .returning(|_, _| Box::pin(async { Ok(serde_json::json!("1")) }));

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

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            params: NetworkRpcRequest::Evm(EvmRpcRequest::GenericRpcRequest {
                method: "net_version".to_string(),
                params: "[]".to_string(),
            }),
            id: Some(JsonRpcId::Number(999)),
        };

        let response = relayer.rpc(request).await.unwrap();
        assert!(response.error.is_none());
        assert!(response.result.is_some());
        assert_eq!(response.id, Some(JsonRpcId::Number(999)));
    }

    #[tokio::test]
    async fn test_rpc_provider_invalid_address_error() {
        let (mut provider, relayer_repo, network_repo, tx_repo, job_producer, signer, counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        provider.expect_raw_request_dyn().returning(|_, _| {
            Box::pin(async {
                Err(ProviderError::InvalidAddress(
                    "Invalid address format".to_string(),
                ))
            })
        });

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

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            params: NetworkRpcRequest::Evm(EvmRpcRequest::GenericRpcRequest {
                method: "eth_getBalance".to_string(),
                params: r#"["invalid_address", "latest"]"#.to_string(),
            }),
            id: Some(JsonRpcId::Number(1)),
        };

        let response = relayer.rpc(request).await.unwrap();
        assert!(response.result.is_none());
        assert!(response.error.is_some());

        let error = response.error.unwrap();
        assert_eq!(error.code, -32602); // RpcErrorCodes::INVALID_PARAMS
    }

    #[tokio::test]
    async fn test_rpc_provider_network_configuration_error() {
        let (mut provider, relayer_repo, network_repo, tx_repo, job_producer, signer, counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        provider.expect_raw_request_dyn().returning(|_, _| {
            Box::pin(async {
                Err(ProviderError::NetworkConfiguration(
                    "Network not reachable".to_string(),
                ))
            })
        });

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

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            params: NetworkRpcRequest::Evm(EvmRpcRequest::GenericRpcRequest {
                method: "eth_chainId".to_string(),
                params: "[]".to_string(),
            }),
            id: Some(JsonRpcId::Number(2)),
        };

        let response = relayer.rpc(request).await.unwrap();
        assert!(response.result.is_none());
        assert!(response.error.is_some());

        let error = response.error.unwrap();
        assert_eq!(error.code, -33004); // OpenZeppelinErrorCodes::NETWORK_CONFIGURATION
    }

    #[tokio::test]
    async fn test_rpc_provider_timeout_error() {
        let (mut provider, relayer_repo, network_repo, tx_repo, job_producer, signer, counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        provider
            .expect_raw_request_dyn()
            .returning(|_, _| Box::pin(async { Err(ProviderError::Timeout) }));

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

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            params: NetworkRpcRequest::Evm(EvmRpcRequest::RawRpcRequest {
                method: "eth_blockNumber".to_string(),
                params: serde_json::json!([]),
            }),
            id: Some(JsonRpcId::Number(3)),
        };

        let response = relayer.rpc(request).await.unwrap();
        assert!(response.result.is_none());
        assert!(response.error.is_some());

        let error = response.error.unwrap();
        assert_eq!(error.code, -33000); // OpenZeppelinErrorCodes::TIMEOUT
    }

    #[tokio::test]
    async fn test_rpc_provider_rate_limited_error() {
        let (mut provider, relayer_repo, network_repo, tx_repo, job_producer, signer, counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        provider
            .expect_raw_request_dyn()
            .returning(|_, _| Box::pin(async { Err(ProviderError::RateLimited) }));

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

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            params: NetworkRpcRequest::Evm(EvmRpcRequest::GenericRpcRequest {
                method: "eth_getBalance".to_string(),
                params: r#"["0x742d35Cc6634C0532925a3b844Bc454e4438f44e", "latest"]"#.to_string(),
            }),
            id: Some(JsonRpcId::Number(4)),
        };

        let response = relayer.rpc(request).await.unwrap();
        assert!(response.result.is_none());
        assert!(response.error.is_some());

        let error = response.error.unwrap();
        assert_eq!(error.code, -33001); // OpenZeppelinErrorCodes::RATE_LIMITED
    }

    #[tokio::test]
    async fn test_rpc_provider_bad_gateway_error() {
        let (mut provider, relayer_repo, network_repo, tx_repo, job_producer, signer, counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        provider
            .expect_raw_request_dyn()
            .returning(|_, _| Box::pin(async { Err(ProviderError::BadGateway) }));

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

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            params: NetworkRpcRequest::Evm(EvmRpcRequest::RawRpcRequest {
                method: "eth_gasPrice".to_string(),
                params: serde_json::json!([]),
            }),
            id: Some(JsonRpcId::Number(5)),
        };

        let response = relayer.rpc(request).await.unwrap();
        assert!(response.result.is_none());
        assert!(response.error.is_some());

        let error = response.error.unwrap();
        assert_eq!(error.code, -33002); // OpenZeppelinErrorCodes::BAD_GATEWAY
    }

    #[tokio::test]
    async fn test_rpc_provider_request_error() {
        let (mut provider, relayer_repo, network_repo, tx_repo, job_producer, signer, counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        provider.expect_raw_request_dyn().returning(|_, _| {
            Box::pin(async {
                Err(ProviderError::RequestError {
                    error: "Bad request".to_string(),
                    status_code: 400,
                })
            })
        });

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

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            params: NetworkRpcRequest::Evm(EvmRpcRequest::GenericRpcRequest {
                method: "invalid_method".to_string(),
                params: "{}".to_string(),
            }),
            id: Some(JsonRpcId::Number(6)),
        };

        let response = relayer.rpc(request).await.unwrap();
        assert!(response.result.is_none());
        assert!(response.error.is_some());

        let error = response.error.unwrap();
        assert_eq!(error.code, -33003); // OpenZeppelinErrorCodes::REQUEST_ERROR
    }

    #[tokio::test]
    async fn test_rpc_provider_other_error() {
        let (mut provider, relayer_repo, network_repo, tx_repo, job_producer, signer, counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        provider.expect_raw_request_dyn().returning(|_, _| {
            Box::pin(async {
                Err(ProviderError::Other(
                    "Unexpected error occurred".to_string(),
                ))
            })
        });

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

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            params: NetworkRpcRequest::Evm(EvmRpcRequest::RawRpcRequest {
                method: "eth_getBalance".to_string(),
                params: serde_json::json!(["0x742d35Cc6634C0532925a3b844Bc454e4438f44e", "latest"]),
            }),
            id: Some(JsonRpcId::Number(7)),
        };

        let response = relayer.rpc(request).await.unwrap();
        assert!(response.result.is_none());
        assert!(response.error.is_some());

        let error = response.error.unwrap();
        assert_eq!(error.code, -32603); // RpcErrorCodes::INTERNAL_ERROR
    }

    #[tokio::test]
    async fn test_rpc_response_preserves_request_id() {
        let (mut provider, relayer_repo, network_repo, tx_repo, job_producer, signer, counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        provider
            .expect_raw_request_dyn()
            .returning(|_, _| Box::pin(async { Ok(serde_json::json!("0x1")) }));

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

        let request_id = u64::MAX;
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            params: NetworkRpcRequest::Evm(EvmRpcRequest::GenericRpcRequest {
                method: "eth_chainId".to_string(),
                params: "[]".to_string(),
            }),
            id: Some(JsonRpcId::Number(request_id as i64)),
        };

        let response = relayer.rpc(request).await.unwrap();
        assert_eq!(response.id, Some(JsonRpcId::Number(request_id as i64)));
        assert_eq!(response.jsonrpc, "2.0");
    }

    #[tokio::test]
    async fn test_rpc_handles_complex_json_response() {
        let (mut provider, relayer_repo, network_repo, tx_repo, job_producer, signer, counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        let complex_response = serde_json::json!({
            "number": "0x1b4",
            "hash": "0xdc0818cf78f21a8e70579cb46a43643f78291264dda342ae31049421c82d21ae",
            "parentHash": "0xe99e022112df268ce40b8b654759b4f39c3cc1b8c86b2f4c7da48ba6d8a6ae8b",
            "transactions": [
                {
                    "hash": "0x5c504ed432cb51138bcf09aa5e8a410dd4a1e204ef84bfed1be16dfba1b22060",
                    "from": "0xa7d9ddbe1f17865597fbd27ec712455208b6b76d",
                    "to": "0xf02c1c8e6114b1dbe8937a39260b5b0a374432bb",
                    "value": "0xf3dbb76162000"
                }
            ],
            "gasUsed": "0x5208"
        });

        provider.expect_raw_request_dyn().returning(move |_, _| {
            let response = complex_response.clone();
            Box::pin(async move { Ok(response) })
        });

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

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            params: NetworkRpcRequest::Evm(EvmRpcRequest::RawRpcRequest {
                method: "eth_getBlockByNumber".to_string(),
                params: serde_json::json!(["0x1b4", true]),
            }),
            id: Some(JsonRpcId::Number(8)),
        };

        let response = relayer.rpc(request).await.unwrap();
        assert!(response.error.is_none());
        assert!(response.result.is_some());

        if let Some(NetworkRpcResult::Evm(EvmRpcResult::RawRpcResult(result))) = response.result {
            assert!(result.get("transactions").is_some());
            assert!(result.get("hash").is_some());
            assert!(result.get("gasUsed").is_some());
        }
    }
}
