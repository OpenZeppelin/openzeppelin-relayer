//! EVM Provider implementation for interacting with EVM-compatible blockchain networks.
//!
//! This module provides functionality to interact with EVM-based blockchains through RPC calls.
//! It implements common operations like getting balances, sending transactions, and querying
//! blockchain state.

use std::time::Duration;

use alloy::{
    network::AnyNetwork,
    primitives::{Bytes, TxKind, Uint},
    providers::{
        fillers::{BlobGasFiller, ChainIdFiller, FillProvider, GasFiller, JoinFill, NonceFiller},
        Identity, Provider, ProviderBuilder, RootProvider,
    },
    rpc::{
        client::ClientBuilder,
        types::{BlockNumberOrTag, FeeHistory, TransactionInput, TransactionRequest},
    },
    transports::http::Http,
};

type EvmProviderType = FillProvider<
    JoinFill<
        Identity,
        JoinFill<GasFiller, JoinFill<BlobGasFiller, JoinFill<NonceFiller, ChainIdFiller>>>,
    >,
    RootProvider<AnyNetwork>,
    AnyNetwork,
>;
use async_trait::async_trait;
use eyre::Result;
use reqwest::ClientBuilder as ReqwestClientBuilder;
use serde_json;

use super::rpc_selector::RpcSelector;
use super::{retry_rpc_call, RetryConfig};
use crate::{
    models::{
        BlockResponse, EvmTransactionData, RpcConfig, TransactionError, TransactionReceipt, U256,
    },
    services::provider::{is_retriable_error, should_mark_provider_failed},
};

#[cfg(test)]
use mockall::automock;

use super::ProviderError;

/// Provider implementation for EVM-compatible blockchain networks.
///
/// Wraps an HTTP RPC provider to interact with EVM chains like Ethereum, Polygon, etc.
#[derive(Clone)]
pub struct EvmProvider {
    /// RPC selector for managing and selecting providers
    selector: RpcSelector,
    /// Timeout in seconds for new HTTP clients
    timeout_seconds: u64,
    /// Configuration for retry behavior
    retry_config: RetryConfig,
}

/// Trait defining the interface for EVM blockchain interactions.
///
/// This trait provides methods for common blockchain operations like querying balances,
/// sending transactions, and getting network state.
#[async_trait]
#[cfg_attr(test, automock)]
#[allow(dead_code)]
pub trait EvmProviderTrait: Send + Sync {
    /// Gets the balance of an address in the native currency.
    ///
    /// # Arguments
    /// * `address` - The address to query the balance for
    async fn get_balance(&self, address: &str) -> Result<U256, ProviderError>;

    /// Gets the current block number of the chain.
    async fn get_block_number(&self) -> Result<u64, ProviderError>;

    /// Estimates the gas required for a transaction.
    ///
    /// # Arguments
    /// * `tx` - The transaction data to estimate gas for
    async fn estimate_gas(&self, tx: &EvmTransactionData) -> Result<u64, ProviderError>;

    /// Gets the current gas price from the network.
    async fn get_gas_price(&self) -> Result<u128, ProviderError>;

    /// Sends a transaction to the network.
    ///
    /// # Arguments
    /// * `tx` - The transaction request to send
    async fn send_transaction(&self, tx: TransactionRequest) -> Result<String, ProviderError>;

    /// Sends a raw signed transaction to the network.
    ///
    /// # Arguments
    /// * `tx` - The raw transaction bytes to send
    async fn send_raw_transaction(&self, tx: &[u8]) -> Result<String, ProviderError>;

    /// Performs a health check by attempting to get the latest block number.
    async fn health_check(&self) -> Result<bool, ProviderError>;

    /// Gets the transaction count (nonce) for an address.
    ///
    /// # Arguments
    /// * `address` - The address to query the transaction count for
    async fn get_transaction_count(&self, address: &str) -> Result<u64, ProviderError>;

    /// Gets the fee history for a range of blocks.
    ///
    /// # Arguments
    /// * `block_count` - Number of blocks to get fee history for
    /// * `newest_block` - The newest block to start from
    /// * `reward_percentiles` - Percentiles to sample reward data from
    async fn get_fee_history(
        &self,
        block_count: u64,
        newest_block: BlockNumberOrTag,
        reward_percentiles: Vec<f64>,
    ) -> Result<FeeHistory, ProviderError>;

    /// Gets the latest block from the network.
    async fn get_block_by_number(&self) -> Result<BlockResponse, ProviderError>;

    /// Gets a transaction receipt by its hash.
    ///
    /// # Arguments
    /// * `tx_hash` - The transaction hash to query
    async fn get_transaction_receipt(
        &self,
        tx_hash: &str,
    ) -> Result<Option<TransactionReceipt>, ProviderError>;

    /// Calls a contract function.
    ///
    /// # Arguments
    /// * `tx` - The transaction request to call the contract function
    async fn call_contract(&self, tx: &TransactionRequest) -> Result<Bytes, ProviderError>;

    /// Sends a raw JSON-RPC request.
    ///
    /// # Arguments
    /// * `method` - The JSON-RPC method name
    /// * `params` - The parameters as a JSON value
    async fn raw_request_dyn(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError>;
}

impl EvmProvider {
    /// Creates a new EVM provider instance.
    ///
    /// # Arguments
    /// * `configs` - A vector of RPC configurations (URL and weight)
    /// * `timeout_seconds` - The timeout duration in seconds (defaults to 30 if None)
    ///
    /// # Returns
    /// * `Result<Self>` - A new provider instance or an error
    pub fn new(configs: Vec<RpcConfig>, timeout_seconds: u64) -> Result<Self, ProviderError> {
        if configs.is_empty() {
            return Err(ProviderError::NetworkConfiguration(
                "At least one RPC configuration must be provided".to_string(),
            ));
        }

        RpcConfig::validate_list(&configs)
            .map_err(|e| ProviderError::NetworkConfiguration(format!("Invalid URL: {e}")))?;

        // Create the RPC selector
        let selector = RpcSelector::new(configs).map_err(|e| {
            ProviderError::NetworkConfiguration(format!("Failed to create RPC selector: {e}"))
        })?;

        let retry_config = RetryConfig::from_env();

        Ok(Self {
            selector,
            timeout_seconds,
            retry_config,
        })
    }

    /// Initialize a provider for a given URL
    fn initialize_provider(&self, url: &str) -> Result<EvmProviderType, ProviderError> {
        let rpc_url = url
            .parse()
            .map_err(|e| ProviderError::NetworkConfiguration(format!("Invalid URL format: {e}")))?;

        // Using use_rustls_tls() forces the use of rustls instead of native-tls to support TLS 1.3
        let client = ReqwestClientBuilder::new()
            .timeout(Duration::from_secs(self.timeout_seconds))
            .use_rustls_tls()
            .build()
            .map_err(|e| ProviderError::Other(format!("Failed to build HTTP client: {e}")))?;

        let mut transport = Http::new(rpc_url);
        transport.set_client(client);

        let is_local = transport.guess_local();
        let client = ClientBuilder::default().transport(transport, is_local);

        let provider = ProviderBuilder::new()
            .network::<AnyNetwork>()
            .connect_client(client);

        Ok(provider)
    }

    /// Helper method to retry RPC calls with exponential backoff
    ///
    /// Uses the generic retry_rpc_call utility to handle retries and provider failover
    async fn retry_rpc_call<T, F, Fut>(
        &self,
        operation_name: &str,
        operation: F,
    ) -> Result<T, ProviderError>
    where
        F: Fn(EvmProviderType) -> Fut,
        Fut: std::future::Future<Output = Result<T, ProviderError>>,
    {
        // Classify which errors should be retried

        tracing::debug!(
            "Starting RPC operation '{}' with timeout: {}s",
            operation_name,
            self.timeout_seconds
        );

        retry_rpc_call(
            &self.selector,
            operation_name,
            is_retriable_error,
            should_mark_provider_failed,
            |url| match self.initialize_provider(url) {
                Ok(provider) => Ok(provider),
                Err(e) => Err(e),
            },
            operation,
            Some(self.retry_config.clone()),
        )
        .await
    }
}

impl AsRef<EvmProvider> for EvmProvider {
    fn as_ref(&self) -> &EvmProvider {
        self
    }
}

#[async_trait]
impl EvmProviderTrait for EvmProvider {
    async fn get_balance(&self, address: &str) -> Result<U256, ProviderError> {
        let parsed_address = address
            .parse::<alloy::primitives::Address>()
            .map_err(|e| ProviderError::InvalidAddress(e.to_string()))?;

        self.retry_rpc_call("get_balance", move |provider| async move {
            provider
                .get_balance(parsed_address)
                .await
                .map_err(ProviderError::from)
        })
        .await
    }

    async fn get_block_number(&self) -> Result<u64, ProviderError> {
        self.retry_rpc_call("get_block_number", |provider| async move {
            provider
                .get_block_number()
                .await
                .map_err(ProviderError::from)
        })
        .await
    }

    async fn estimate_gas(&self, tx: &EvmTransactionData) -> Result<u64, ProviderError> {
        let transaction_request = TransactionRequest::try_from(tx)
            .map_err(|e| ProviderError::Other(format!("Failed to convert transaction: {e}")))?;

        self.retry_rpc_call("estimate_gas", move |provider| {
            let tx_req = transaction_request.clone();
            async move {
                provider
                    .estimate_gas(tx_req.into())
                    .await
                    .map_err(ProviderError::from)
            }
        })
        .await
    }

    async fn get_gas_price(&self) -> Result<u128, ProviderError> {
        self.retry_rpc_call("get_gas_price", |provider| async move {
            provider.get_gas_price().await.map_err(ProviderError::from)
        })
        .await
    }

    async fn send_transaction(&self, tx: TransactionRequest) -> Result<String, ProviderError> {
        let pending_tx = self
            .retry_rpc_call("send_transaction", move |provider| {
                let tx_req = tx.clone();
                async move {
                    provider
                        .send_transaction(tx_req.into())
                        .await
                        .map_err(ProviderError::from)
                }
            })
            .await?;

        let tx_hash = pending_tx.tx_hash().to_string();
        Ok(tx_hash)
    }

    async fn send_raw_transaction(&self, tx: &[u8]) -> Result<String, ProviderError> {
        let pending_tx = self
            .retry_rpc_call("send_raw_transaction", move |provider| {
                let tx_data = tx.to_vec();
                async move {
                    provider
                        .send_raw_transaction(&tx_data)
                        .await
                        .map_err(ProviderError::from)
                }
            })
            .await?;

        let tx_hash = pending_tx.tx_hash().to_string();
        Ok(tx_hash)
    }

    async fn health_check(&self) -> Result<bool, ProviderError> {
        match self.get_block_number().await {
            Ok(_) => Ok(true),
            Err(e) => Err(e),
        }
    }

    async fn get_transaction_count(&self, address: &str) -> Result<u64, ProviderError> {
        let parsed_address = address
            .parse::<alloy::primitives::Address>()
            .map_err(|e| ProviderError::InvalidAddress(e.to_string()))?;

        self.retry_rpc_call("get_transaction_count", move |provider| async move {
            provider
                .get_transaction_count(parsed_address)
                .await
                .map_err(ProviderError::from)
        })
        .await
    }

    async fn get_fee_history(
        &self,
        block_count: u64,
        newest_block: BlockNumberOrTag,
        reward_percentiles: Vec<f64>,
    ) -> Result<FeeHistory, ProviderError> {
        self.retry_rpc_call("get_fee_history", move |provider| {
            let reward_percentiles_clone = reward_percentiles.clone();
            async move {
                provider
                    .get_fee_history(block_count, newest_block, &reward_percentiles_clone)
                    .await
                    .map_err(ProviderError::from)
            }
        })
        .await
    }

    async fn get_block_by_number(&self) -> Result<BlockResponse, ProviderError> {
        let block_result = self
            .retry_rpc_call("get_block_by_number", |provider| async move {
                provider
                    .get_block_by_number(BlockNumberOrTag::Latest)
                    .await
                    .map_err(ProviderError::from)
            })
            .await?;

        match block_result {
            Some(block) => Ok(block),
            None => Err(ProviderError::Other("Block not found".to_string())),
        }
    }

    async fn get_transaction_receipt(
        &self,
        tx_hash: &str,
    ) -> Result<Option<TransactionReceipt>, ProviderError> {
        let parsed_tx_hash = tx_hash
            .parse::<alloy::primitives::TxHash>()
            .map_err(|e| ProviderError::Other(format!("Invalid transaction hash: {e}")))?;

        self.retry_rpc_call("get_transaction_receipt", move |provider| async move {
            provider
                .get_transaction_receipt(parsed_tx_hash)
                .await
                .map_err(ProviderError::from)
        })
        .await
    }

    async fn call_contract(&self, tx: &TransactionRequest) -> Result<Bytes, ProviderError> {
        self.retry_rpc_call("call_contract", move |provider| {
            let tx_req = tx.clone();
            async move {
                provider
                    .call(tx_req.into())
                    .await
                    .map_err(ProviderError::from)
            }
        })
        .await
    }

    async fn raw_request_dyn(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        self.retry_rpc_call("raw_request_dyn", move |provider| {
            let params_clone = params.clone();
            async move {
                // Convert params to RawValue and use Cow for method
                let params_raw = serde_json::value::to_raw_value(&params_clone).map_err(|e| {
                    ProviderError::Other(format!("Failed to serialize params: {e}"))
                })?;

                let result = provider
                    .raw_request_dyn(std::borrow::Cow::Owned(method.to_string()), &params_raw)
                    .await
                    .map_err(ProviderError::from)?;

                // Convert RawValue back to Value
                serde_json::from_str(result.get())
                    .map_err(|e| ProviderError::Other(format!("Failed to deserialize result: {e}")))
            }
        })
        .await
    }
}

impl TryFrom<&EvmTransactionData> for TransactionRequest {
    type Error = TransactionError;
    fn try_from(tx: &EvmTransactionData) -> Result<Self, Self::Error> {
        Ok(TransactionRequest {
            from: Some(tx.from.clone().parse().map_err(|_| {
                TransactionError::InvalidType("Invalid address format".to_string())
            })?),
            to: Some(TxKind::Call(
                tx.to
                    .clone()
                    .unwrap_or("".to_string())
                    .parse()
                    .map_err(|_| {
                        TransactionError::InvalidType("Invalid address format".to_string())
                    })?,
            )),
            gas_price: tx
                .gas_price
                .map(|gp| {
                    Uint::<256, 4>::from(gp)
                        .try_into()
                        .map_err(|_| TransactionError::InvalidType("Invalid gas price".to_string()))
                })
                .transpose()?,
            value: Some(Uint::<256, 4>::from(tx.value)),
            input: TransactionInput::from(tx.data_to_bytes()?),
            nonce: tx
                .nonce
                .map(|n| {
                    Uint::<256, 4>::from(n)
                        .try_into()
                        .map_err(|_| TransactionError::InvalidType("Invalid nonce".to_string()))
                })
                .transpose()?,
            chain_id: Some(tx.chain_id),
            max_fee_per_gas: tx
                .max_fee_per_gas
                .map(|mfpg| {
                    Uint::<256, 4>::from(mfpg).try_into().map_err(|_| {
                        TransactionError::InvalidType("Invalid max fee per gas".to_string())
                    })
                })
                .transpose()?,
            max_priority_fee_per_gas: tx
                .max_priority_fee_per_gas
                .map(|mpfpg| {
                    Uint::<256, 4>::from(mpfpg).try_into().map_err(|_| {
                        TransactionError::InvalidType(
                            "Invalid max priority fee per gas".to_string(),
                        )
                    })
                })
                .transpose()?,
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::Address;
    use futures::FutureExt;
    use lazy_static::lazy_static;
    use std::str::FromStr;
    use std::sync::Mutex;

    lazy_static! {
        static ref EVM_TEST_ENV_MUTEX: Mutex<()> = Mutex::new(());
    }

    struct EvmTestEnvGuard {
        _mutex_guard: std::sync::MutexGuard<'static, ()>,
    }

    impl EvmTestEnvGuard {
        fn new(mutex_guard: std::sync::MutexGuard<'static, ()>) -> Self {
            std::env::set_var(
                "API_KEY",
                "test_api_key_for_evm_provider_new_this_is_long_enough_32_chars",
            );
            std::env::set_var("REDIS_URL", "redis://test-dummy-url-for-evm-provider");

            Self {
                _mutex_guard: mutex_guard,
            }
        }
    }

    impl Drop for EvmTestEnvGuard {
        fn drop(&mut self) {
            std::env::remove_var("API_KEY");
            std::env::remove_var("REDIS_URL");
        }
    }

    // Helper function to set up the test environment
    fn setup_test_env() -> EvmTestEnvGuard {
        let guard = EVM_TEST_ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        EvmTestEnvGuard::new(guard)
    }

    #[tokio::test]
    async fn test_reqwest_error_conversion() {
        // Create a reqwest timeout error
        let client = reqwest::Client::new();
        let result = client
            .get("https://www.openzeppelin.com/")
            .timeout(Duration::from_millis(1))
            .send()
            .await;

        assert!(
            result.is_err(),
            "Expected the send operation to result in an error."
        );
        let err = result.unwrap_err();

        assert!(
            err.is_timeout(),
            "The reqwest error should be a timeout. Actual error: {:?}",
            err
        );

        let provider_error = ProviderError::from(err);
        assert!(
            matches!(provider_error, ProviderError::Timeout),
            "ProviderError should be Timeout. Actual: {:?}",
            provider_error
        );
    }

    #[test]
    fn test_address_parse_error_conversion() {
        // Create an address parse error
        let err = "invalid-address".parse::<Address>().unwrap_err();
        // Map the error manually using the same approach as in our From implementation
        let provider_error = ProviderError::InvalidAddress(err.to_string());
        assert!(matches!(provider_error, ProviderError::InvalidAddress(_)));
    }

    #[test]
    fn test_new_provider() {
        let _env_guard = setup_test_env();

        let provider = EvmProvider::new(
            vec![RpcConfig::new("http://localhost:8545".to_string())],
            30,
        );
        assert!(provider.is_ok());

        // Test with invalid URL
        let provider = EvmProvider::new(vec![RpcConfig::new("invalid-url".to_string())], 30);
        assert!(provider.is_err());
    }

    #[test]
    fn test_new_provider_with_timeout() {
        let _env_guard = setup_test_env();

        // Test with valid URL and timeout
        let provider = EvmProvider::new(
            vec![RpcConfig::new("http://localhost:8545".to_string())],
            30,
        );
        assert!(provider.is_ok());

        // Test with invalid URL
        let provider = EvmProvider::new(vec![RpcConfig::new("invalid-url".to_string())], 30);
        assert!(provider.is_err());

        // Test with zero timeout
        let provider =
            EvmProvider::new(vec![RpcConfig::new("http://localhost:8545".to_string())], 0);
        assert!(provider.is_ok());

        // Test with large timeout
        let provider = EvmProvider::new(
            vec![RpcConfig::new("http://localhost:8545".to_string())],
            3600,
        );
        assert!(provider.is_ok());
    }

    #[test]
    fn test_transaction_request_conversion() {
        let tx_data = EvmTransactionData {
            from: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            to: Some("0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string()),
            gas_price: Some(1000000000),
            value: Uint::<256, 4>::from(1000000000),
            data: Some("0x".to_string()),
            nonce: Some(1),
            chain_id: 1,
            gas_limit: Some(21000),
            hash: None,
            signature: None,
            speed: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            raw: None,
        };

        let result = TransactionRequest::try_from(&tx_data);
        assert!(result.is_ok());

        let tx_request = result.unwrap();
        assert_eq!(
            tx_request.from,
            Some(Address::from_str("0x742d35Cc6634C0532925a3b844Bc454e4438f44e").unwrap())
        );
        assert_eq!(tx_request.chain_id, Some(1));
    }

    #[tokio::test]
    async fn test_mock_provider_methods() {
        let mut mock = MockEvmProviderTrait::new();

        mock.expect_get_balance()
            .with(mockall::predicate::eq(
                "0x742d35Cc6634C0532925a3b844Bc454e4438f44e",
            ))
            .times(1)
            .returning(|_| async { Ok(U256::from(100)) }.boxed());

        mock.expect_get_block_number()
            .times(1)
            .returning(|| async { Ok(12345) }.boxed());

        mock.expect_get_gas_price()
            .times(1)
            .returning(|| async { Ok(20000000000) }.boxed());

        mock.expect_health_check()
            .times(1)
            .returning(|| async { Ok(true) }.boxed());

        mock.expect_get_transaction_count()
            .with(mockall::predicate::eq(
                "0x742d35Cc6634C0532925a3b844Bc454e4438f44e",
            ))
            .times(1)
            .returning(|_| async { Ok(42) }.boxed());

        mock.expect_get_fee_history()
            .with(
                mockall::predicate::eq(10u64),
                mockall::predicate::eq(BlockNumberOrTag::Latest),
                mockall::predicate::eq(vec![25.0, 50.0, 75.0]),
            )
            .times(1)
            .returning(|_, _, _| {
                async {
                    Ok(FeeHistory {
                        oldest_block: 100,
                        base_fee_per_gas: vec![1000],
                        gas_used_ratio: vec![0.5],
                        reward: Some(vec![vec![500]]),
                        base_fee_per_blob_gas: vec![1000],
                        blob_gas_used_ratio: vec![0.5],
                    })
                }
                .boxed()
            });

        // Test all methods
        let balance = mock
            .get_balance("0x742d35Cc6634C0532925a3b844Bc454e4438f44e")
            .await;
        assert!(balance.is_ok());
        assert_eq!(balance.unwrap(), U256::from(100));

        let block_number = mock.get_block_number().await;
        assert!(block_number.is_ok());
        assert_eq!(block_number.unwrap(), 12345);

        let gas_price = mock.get_gas_price().await;
        assert!(gas_price.is_ok());
        assert_eq!(gas_price.unwrap(), 20000000000);

        let health = mock.health_check().await;
        assert!(health.is_ok());
        assert!(health.unwrap());

        let count = mock
            .get_transaction_count("0x742d35Cc6634C0532925a3b844Bc454e4438f44e")
            .await;
        assert!(count.is_ok());
        assert_eq!(count.unwrap(), 42);

        let fee_history = mock
            .get_fee_history(10, BlockNumberOrTag::Latest, vec![25.0, 50.0, 75.0])
            .await;
        assert!(fee_history.is_ok());
        let fee_history = fee_history.unwrap();
        assert_eq!(fee_history.oldest_block, 100);
        assert_eq!(fee_history.gas_used_ratio, vec![0.5]);
    }

    #[tokio::test]
    async fn test_mock_transaction_operations() {
        let mut mock = MockEvmProviderTrait::new();

        // Setup mock for estimate_gas
        let tx_data = EvmTransactionData {
            from: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            to: Some("0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string()),
            gas_price: Some(1000000000),
            value: Uint::<256, 4>::from(1000000000),
            data: Some("0x".to_string()),
            nonce: Some(1),
            chain_id: 1,
            gas_limit: Some(21000),
            hash: None,
            signature: None,
            speed: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            raw: None,
        };

        mock.expect_estimate_gas()
            .with(mockall::predicate::always())
            .times(1)
            .returning(|_| async { Ok(21000) }.boxed());

        // Setup mock for send_raw_transaction
        mock.expect_send_raw_transaction()
            .with(mockall::predicate::always())
            .times(1)
            .returning(|_| async { Ok("0x123456789abcdef".to_string()) }.boxed());

        // Test the mocked methods
        let gas_estimate = mock.estimate_gas(&tx_data).await;
        assert!(gas_estimate.is_ok());
        assert_eq!(gas_estimate.unwrap(), 21000);

        let tx_hash = mock.send_raw_transaction(&[0u8; 32]).await;
        assert!(tx_hash.is_ok());
        assert_eq!(tx_hash.unwrap(), "0x123456789abcdef");
    }

    #[test]
    fn test_invalid_transaction_request_conversion() {
        let tx_data = EvmTransactionData {
            from: "invalid-address".to_string(),
            to: Some("0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string()),
            gas_price: Some(1000000000),
            value: Uint::<256, 4>::from(1000000000),
            data: Some("0x".to_string()),
            nonce: Some(1),
            chain_id: 1,
            gas_limit: Some(21000),
            hash: None,
            signature: None,
            speed: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            raw: None,
        };

        let result = TransactionRequest::try_from(&tx_data);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_mock_additional_methods() {
        let mut mock = MockEvmProviderTrait::new();

        // Setup mock for health_check
        mock.expect_health_check()
            .times(1)
            .returning(|| async { Ok(true) }.boxed());

        // Setup mock for get_transaction_count
        mock.expect_get_transaction_count()
            .with(mockall::predicate::eq(
                "0x742d35Cc6634C0532925a3b844Bc454e4438f44e",
            ))
            .times(1)
            .returning(|_| async { Ok(42) }.boxed());

        // Setup mock for get_fee_history
        mock.expect_get_fee_history()
            .with(
                mockall::predicate::eq(10u64),
                mockall::predicate::eq(BlockNumberOrTag::Latest),
                mockall::predicate::eq(vec![25.0, 50.0, 75.0]),
            )
            .times(1)
            .returning(|_, _, _| {
                async {
                    Ok(FeeHistory {
                        oldest_block: 100,
                        base_fee_per_gas: vec![1000],
                        gas_used_ratio: vec![0.5],
                        reward: Some(vec![vec![500]]),
                        base_fee_per_blob_gas: vec![1000],
                        blob_gas_used_ratio: vec![0.5],
                    })
                }
                .boxed()
            });

        // Test health check
        let health = mock.health_check().await;
        assert!(health.is_ok());
        assert!(health.unwrap());

        // Test get_transaction_count
        let count = mock
            .get_transaction_count("0x742d35Cc6634C0532925a3b844Bc454e4438f44e")
            .await;
        assert!(count.is_ok());
        assert_eq!(count.unwrap(), 42);

        // Test get_fee_history
        let fee_history = mock
            .get_fee_history(10, BlockNumberOrTag::Latest, vec![25.0, 50.0, 75.0])
            .await;
        assert!(fee_history.is_ok());
        let fee_history = fee_history.unwrap();
        assert_eq!(fee_history.oldest_block, 100);
        assert_eq!(fee_history.gas_used_ratio, vec![0.5]);
    }

    #[test]
    fn test_is_retriable_error_json_rpc_retriable_codes() {
        // Retriable JSON-RPC error codes per EIP-1474
        let retriable_codes = vec![
            (-32002, "Resource unavailable"),
            (-32005, "Limit exceeded"),
            (-32603, "Internal error"),
        ];

        for (code, message) in retriable_codes {
            let error = ProviderError::RpcErrorCode {
                code,
                message: message.to_string(),
            };
            assert!(
                is_retriable_error(&error),
                "Error code {} should be retriable",
                code
            );
        }
    }

    #[test]
    fn test_is_retriable_error_json_rpc_non_retriable_codes() {
        // Non-retriable JSON-RPC error codes per EIP-1474
        let non_retriable_codes = vec![
            (-32000, "insufficient funds"),
            (-32000, "execution reverted"),
            (-32000, "already known"),
            (-32000, "nonce too low"),
            (-32000, "invalid sender"),
            (-32001, "Resource not found"),
            (-32003, "Transaction rejected"),
            (-32004, "Method not supported"),
            (-32700, "Parse error"),
            (-32600, "Invalid request"),
            (-32601, "Method not found"),
            (-32602, "Invalid params"),
        ];

        for (code, message) in non_retriable_codes {
            let error = ProviderError::RpcErrorCode {
                code,
                message: message.to_string(),
            };
            assert!(
                !is_retriable_error(&error),
                "Error code {} with message '{}' should NOT be retriable",
                code,
                message
            );
        }
    }

    #[test]
    fn test_is_retriable_error_json_rpc_32000_specific_cases() {
        // Test specific -32000 error messages that users commonly encounter
        // -32000 is a catch-all for client errors and should NOT be retriable
        let test_cases = vec![
            (
                "tx already exists in cache",
                false,
                "Transaction already in mempool",
            ),
            ("already known", false, "Duplicate transaction submission"),
            (
                "insufficient funds for gas * price + value",
                false,
                "User needs more funds",
            ),
            ("execution reverted", false, "Smart contract rejected"),
            ("nonce too low", false, "Transaction already processed"),
            ("invalid sender", false, "Configuration issue"),
            ("gas required exceeds allowance", false, "Gas limit too low"),
            (
                "replacement transaction underpriced",
                false,
                "Need higher gas price",
            ),
        ];

        for (message, should_retry, description) in test_cases {
            let error = ProviderError::RpcErrorCode {
                code: -32000,
                message: message.to_string(),
            };
            assert_eq!(
                is_retriable_error(&error),
                should_retry,
                "{}: -32000 with '{}' should{} be retriable",
                description,
                message,
                if should_retry { "" } else { " NOT" }
            );
        }
    }

    #[tokio::test]
    async fn test_call_contract() {
        let mut mock = MockEvmProviderTrait::new();

        let tx = TransactionRequest {
            from: Some(Address::from_str("0x742d35Cc6634C0532925a3b844Bc454e4438f44e").unwrap()),
            to: Some(TxKind::Call(
                Address::from_str("0xCcCCccccCCCCcCCCCCCcCcCccCcCCCcCcccccccC").unwrap(),
            )),
            input: TransactionInput::from(
                hex::decode("a9059cbb000000000000000000000000742d35cc6634c0532925a3b844bc454e4438f44e0000000000000000000000000000000000000000000000000de0b6b3a7640000").unwrap()
            ),
            ..Default::default()
        };

        // Setup mock for call_contract
        mock.expect_call_contract()
            .with(mockall::predicate::always())
            .times(1)
            .returning(|_| {
                async {
                    Ok(Bytes::from(
                        hex::decode(
                            "0000000000000000000000000000000000000000000000000000000000000001",
                        )
                        .unwrap(),
                    ))
                }
                .boxed()
            });

        let result = mock.call_contract(&tx).await;
        assert!(result.is_ok());

        let data = result.unwrap();
        assert_eq!(
            hex::encode(data),
            "0000000000000000000000000000000000000000000000000000000000000001"
        );
    }
}
