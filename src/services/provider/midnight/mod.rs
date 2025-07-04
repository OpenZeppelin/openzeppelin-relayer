//! Midnight Provider implementation for interacting with Midnight blockchain networks.
//!
//! This module provides functionality to interact with Midnight blockchain through RPC calls.
//! It implements common operations like getting balances, sending transactions, and querying
//! blockchain state.

pub mod remote_prover;

use async_trait::async_trait;
use eyre::Result;
use hex;
use midnight_node_ledger_helpers::{serialize, DefaultDB, NetworkId, Proof, Transaction};
use midnight_node_res::subxt_metadata::api as mn_meta;
use subxt::{OnlineClient, PolkadotConfig};

use super::rpc_selector::RpcSelector;
use super::{retry_rpc_call, RetryConfig};
use crate::models::{RpcConfig, U256};

#[cfg(test)]
use mockall::automock;

use super::ProviderError;

/// Provider implementation for Midnight blockchain networks.
///
/// Wraps a Substrate/Subxt client to interact with Midnight blockchain.
#[derive(Clone)]
pub struct MidnightProvider {
    /// RPC selector for managing and selecting providers
    selector: RpcSelector,
    /// Timeout in seconds for new HTTP clients
    timeout_seconds: u64,
    /// Configuration for retry behavior
    retry_config: RetryConfig,
    /// Network ID for transaction serialization
    network_id: NetworkId,
}

/// Trait defining the interface for EVM blockchain interactions.
///
/// This trait provides methods for common blockchain operations like querying balances,
/// sending transactions, and getting network state.
#[async_trait]
#[cfg_attr(test, automock)]
#[allow(dead_code)]
pub trait MidnightProviderTrait: Send + Sync {
    /// Gets the balance of an address in the native currency.
    ///
    /// # Arguments
    /// * `address` - The address to query the balance for
    async fn get_balance(&self, address: &str) -> Result<U256, ProviderError>;

    /// Gets the current block number of the chain.
    async fn get_block_number(&self) -> Result<u64, ProviderError>;

    /// Sends a transaction to the network.
    ///
    /// # Arguments
    /// * `tx` - The transaction request to send
    async fn send_transaction(
        &self,
        tx: Transaction<Proof, DefaultDB>,
    ) -> Result<String, ProviderError>;

    /// Performs a health check by attempting to get the latest block number.
    async fn health_check(&self) -> Result<bool, ProviderError>;
}

impl MidnightProvider {
    /// Creates a new Midnight provider instance.
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
            .map_err(|e| ProviderError::NetworkConfiguration(format!("Invalid URL: {}", e)))?;

        // Create the RPC selector
        let selector = RpcSelector::new(configs).map_err(|e| {
            ProviderError::NetworkConfiguration(format!("Failed to create RPC selector: {}", e))
        })?;

        let retry_config = RetryConfig::from_env();

        // TODO: Get network_id from configuration or determine from chain
        let network_id = NetworkId::TestNet;

        Ok(Self {
            selector,
            timeout_seconds,
            retry_config,
            network_id,
        })
    }

    // Error codes that indicate we can't use a provider
    fn should_mark_provider_failed(error: &ProviderError) -> bool {
        match error {
            ProviderError::RequestError { status_code, .. } => {
                match *status_code {
                    // 5xx Server Errors - RPC node is having issues
                    500..=599 => true,

                    // 4xx Client Errors that indicate we can't use this provider
                    401 => true, // Unauthorized - auth required but not provided
                    403 => true, // Forbidden - node is blocking requests or auth issues
                    404 => true, // Not Found - endpoint doesn't exist or misconfigured
                    410 => true, // Gone - endpoint permanently removed

                    _ => false,
                }
            }
            _ => false,
        }
    }

    // Errors that are retriable
    fn is_retriable_error(error: &ProviderError) -> bool {
        match error {
            // Only retry these specific error types
            ProviderError::Timeout | ProviderError::RateLimited | ProviderError::BadGateway => true,

            // Any other errors are not automatically retriable
            _ => {
                // Optionally inspect error message for network-related issues
                let err_msg = format!("{}", error);
                err_msg.to_lowercase().contains("timeout")
                    || err_msg.to_lowercase().contains("connection")
                    || err_msg.to_lowercase().contains("reset")
            }
        }
    }

    /// Initialize a provider for a given URL
    async fn initialize_provider(
        &self,
        url: &str,
    ) -> Result<OnlineClient<PolkadotConfig>, ProviderError> {
        OnlineClient::<PolkadotConfig>::from_url(url)
            .await
            .map_err(|e| {
                ProviderError::NetworkConfiguration(format!("Failed to connect to {}: {}", url, e))
            })
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
        F: Fn(OnlineClient<PolkadotConfig>) -> Fut,
        Fut: std::future::Future<Output = Result<T, ProviderError>>,
    {
        // Classify which errors should be retried

        log::debug!(
            "Starting RPC operation '{}' with timeout: {}s",
            operation_name,
            self.timeout_seconds
        );

        retry_rpc_call(
            &self.selector,
            operation_name,
            Self::is_retriable_error,
            Self::should_mark_provider_failed,
            |url| {
                // Block on the async initialization since retry_rpc_call expects sync
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current()
                        .block_on(async { self.initialize_provider(url).await })
                })
            },
            operation,
            Some(self.retry_config.clone()),
        )
        .await
    }
}

impl AsRef<MidnightProvider> for MidnightProvider {
    fn as_ref(&self) -> &MidnightProvider {
        self
    }
}

#[async_trait]
impl MidnightProviderTrait for MidnightProvider {
    async fn get_balance(&self, _address: &str) -> Result<U256, ProviderError> {
        self.retry_rpc_call("get_balance", move |_api| async move {
            // TODO: Implement balance query from wallet state by iterating coins after indexer sync
            log::warn!("get_balance not yet implemented for Midnight provider");
            Ok(U256::from(0))
        })
        .await
    }

    async fn get_block_number(&self) -> Result<u64, ProviderError> {
        self.retry_rpc_call("get_block_number", |api| async move {
            let block =
                api.blocks().at_latest().await.map_err(|e| {
                    ProviderError::Other(format!("Failed to get latest block: {}", e))
                })?;

            Ok(block.number().into())
        })
        .await
    }

    async fn send_transaction(
        &self,
        tx: Transaction<Proof, DefaultDB>,
    ) -> Result<String, ProviderError> {
        let network_id = self.network_id;
        self.retry_rpc_call("send_transaction", move |api| {
            let tx_clone = tx.clone();
            async move {
                // Serialize the transaction
                let serialized = serialize(&tx_clone, network_id).map_err(|e| {
                    ProviderError::Other(format!("Failed to serialize transaction: {:?}", e))
                })?;

                let mn_tx = mn_meta::tx()
                    .midnight()
                    .send_mn_transaction(hex::encode(serialized).into_bytes());

                let unsigned_extrinsic = api.tx().create_unsigned(&mn_tx).map_err(|e| {
                    ProviderError::Other(format!("Failed to create extrinsic: {}", e))
                })?;

                let tx_hash_string =
                    format!("0x{}", hex::encode(unsigned_extrinsic.hash().as_bytes()));

                let validation_result = unsigned_extrinsic.validate().await.map_err(|e| {
                    ProviderError::Other(format!("Failed to validate transaction: {}", e))
                })?;

                // Check if validation result indicates success
                match validation_result {
                    subxt::tx::ValidationResult::Valid(_) => {
                        // Transaction is valid, proceed with submission
                    }
                    subxt::tx::ValidationResult::Invalid(e) => {
                        return Err(ProviderError::Other(format!(
                            "Transaction validation failed: {:?}",
                            e
                        )));
                    }
                    subxt::tx::ValidationResult::Unknown(e) => {
                        return Err(ProviderError::Other(format!(
                            "Transaction validation unknown: {:?}",
                            e
                        )));
                    }
                }

                let _tx_progress = unsigned_extrinsic.submit_and_watch().await.map_err(|e| {
                    ProviderError::Other(format!("Failed to submit transaction: {}", e))
                })?;

                // TODO: Track and apply successful transaction to wallet state
                Ok(tx_hash_string)
            }
        })
        .await
    }

    async fn health_check(&self) -> Result<bool, ProviderError> {
        match self.get_block_number().await {
            Ok(_) => Ok(true),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lazy_static::lazy_static;
    use std::{sync::Mutex, time::Duration};

    lazy_static! {
        static ref MIDNIGHT_TEST_ENV_MUTEX: Mutex<()> = Mutex::new(());
    }

    struct MidnightTestEnvGuard {
        _mutex_guard: std::sync::MutexGuard<'static, ()>,
    }

    impl MidnightTestEnvGuard {
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

    impl Drop for MidnightTestEnvGuard {
        fn drop(&mut self) {
            std::env::remove_var("API_KEY");
            std::env::remove_var("REDIS_URL");
        }
    }

    // Helper function to set up the test environment
    fn setup_test_env() -> MidnightTestEnvGuard {
        let guard = MIDNIGHT_TEST_ENV_MUTEX
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        MidnightTestEnvGuard::new(guard)
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
    fn test_new_provider() {
        let _env_guard = setup_test_env();

        let provider = MidnightProvider::new(
            vec![RpcConfig::new("http://localhost:8545".to_string())],
            30,
        );
        assert!(provider.is_ok());

        // Test with invalid URL
        let provider = MidnightProvider::new(vec![RpcConfig::new("invalid-url".to_string())], 30);
        assert!(provider.is_err());
    }

    #[test]
    fn test_new_provider_with_timeout() {
        let _env_guard = setup_test_env();

        // Test with valid URL and timeout
        let provider = MidnightProvider::new(
            vec![RpcConfig::new("http://localhost:8545".to_string())],
            30,
        );
        assert!(provider.is_ok());

        // Test with invalid URL
        let provider = MidnightProvider::new(vec![RpcConfig::new("invalid-url".to_string())], 30);
        assert!(provider.is_err());

        // Test with zero timeout
        let provider =
            MidnightProvider::new(vec![RpcConfig::new("http://localhost:8545".to_string())], 0);
        assert!(provider.is_ok());

        // Test with large timeout
        let provider = MidnightProvider::new(
            vec![RpcConfig::new("http://localhost:8545".to_string())],
            3600,
        );
        assert!(provider.is_ok());
    }

    #[test]
    fn test_should_mark_provider_failed_server_errors() {
        // 5xx errors should mark provider as failed
        for status_code in 500..=599 {
            let error = ProviderError::RequestError {
                error: format!("Server error {}", status_code),
                status_code,
            };
            assert!(
                MidnightProvider::should_mark_provider_failed(&error),
                "Status code {} should mark provider as failed",
                status_code
            );
        }
    }

    #[test]
    fn test_should_mark_provider_failed_auth_errors() {
        // Authentication/authorization errors should mark provider as failed
        let auth_errors = [401, 403];
        for &status_code in &auth_errors {
            let error = ProviderError::RequestError {
                error: format!("Auth error {}", status_code),
                status_code,
            };
            assert!(
                MidnightProvider::should_mark_provider_failed(&error),
                "Status code {} should mark provider as failed",
                status_code
            );
        }
    }

    #[test]
    fn test_should_mark_provider_failed_not_found_errors() {
        // 404 and 410 should mark provider as failed (endpoint issues)
        let not_found_errors = [404, 410];
        for &status_code in &not_found_errors {
            let error = ProviderError::RequestError {
                error: format!("Not found error {}", status_code),
                status_code,
            };
            assert!(
                MidnightProvider::should_mark_provider_failed(&error),
                "Status code {} should mark provider as failed",
                status_code
            );
        }
    }

    #[test]
    fn test_should_mark_provider_failed_client_errors_not_failed() {
        // These 4xx errors should NOT mark provider as failed (client-side issues)
        let client_errors = [400, 405, 413, 414, 415, 422, 429];
        for &status_code in &client_errors {
            let error = ProviderError::RequestError {
                error: format!("Client error {}", status_code),
                status_code,
            };
            assert!(
                !MidnightProvider::should_mark_provider_failed(&error),
                "Status code {} should NOT mark provider as failed",
                status_code
            );
        }
    }

    #[test]
    fn test_should_mark_provider_failed_other_error_types() {
        // Test non-RequestError types - these should NOT mark provider as failed
        let errors = [
            ProviderError::Timeout,
            ProviderError::RateLimited,
            ProviderError::BadGateway,
            ProviderError::InvalidAddress("test".to_string()),
            ProviderError::NetworkConfiguration("test".to_string()),
            ProviderError::Other("test".to_string()),
        ];

        for error in errors {
            assert!(
                !MidnightProvider::should_mark_provider_failed(&error),
                "Error type {:?} should NOT mark provider as failed",
                error
            );
        }
    }

    #[test]
    fn test_should_mark_provider_failed_edge_cases() {
        // Test some edge case status codes
        let edge_cases = [
            (200, false), // Success - shouldn't happen in error context but test anyway
            (300, false), // Redirection
            (418, false), // I'm a teapot - should not mark as failed
            (451, false), // Unavailable for legal reasons - client issue
            (499, false), // Client closed request - client issue
        ];

        for (status_code, should_fail) in edge_cases {
            let error = ProviderError::RequestError {
                error: format!("Edge case error {}", status_code),
                status_code,
            };
            assert_eq!(
                MidnightProvider::should_mark_provider_failed(&error),
                should_fail,
                "Status code {} should {} mark provider as failed",
                status_code,
                if should_fail { "" } else { "NOT" }
            );
        }
    }

    #[test]
    fn test_is_retriable_error_retriable_types() {
        // These error types should be retriable
        let retriable_errors = [
            ProviderError::Timeout,
            ProviderError::RateLimited,
            ProviderError::BadGateway,
        ];

        for error in retriable_errors {
            assert!(
                MidnightProvider::is_retriable_error(&error),
                "Error type {:?} should be retriable",
                error
            );
        }
    }

    #[test]
    fn test_is_retriable_error_non_retriable_types() {
        // These error types should NOT be retriable
        let non_retriable_errors = [
            ProviderError::InvalidAddress("test".to_string()),
            ProviderError::NetworkConfiguration("test".to_string()),
            ProviderError::RequestError {
                error: "Some error".to_string(),
                status_code: 400,
            },
        ];

        for error in non_retriable_errors {
            assert!(
                !MidnightProvider::is_retriable_error(&error),
                "Error type {:?} should NOT be retriable",
                error
            );
        }
    }

    #[test]
    fn test_is_retriable_error_message_based_detection() {
        // Test errors that should be retriable based on message content
        let retriable_messages = [
            "Connection timeout occurred",
            "Network connection reset",
            "Connection refused",
            "TIMEOUT error happened",
            "Connection was reset by peer",
        ];

        for message in retriable_messages {
            let error = ProviderError::Other(message.to_string());
            assert!(
                MidnightProvider::is_retriable_error(&error),
                "Error with message '{}' should be retriable",
                message
            );
        }
    }

    #[test]
    fn test_is_retriable_error_message_based_non_retriable() {
        // Test errors that should NOT be retriable based on message content
        let non_retriable_messages = [
            "Invalid address format",
            "Bad request parameters",
            "Authentication failed",
            "Method not found",
            "Some other error",
        ];

        for message in non_retriable_messages {
            let error = ProviderError::Other(message.to_string());
            assert!(
                !MidnightProvider::is_retriable_error(&error),
                "Error with message '{}' should NOT be retriable",
                message
            );
        }
    }

    #[test]
    fn test_is_retriable_error_case_insensitive() {
        // Test that message-based detection is case insensitive
        let case_variations = [
            "TIMEOUT",
            "Timeout",
            "timeout",
            "CONNECTION",
            "Connection",
            "connection",
            "RESET",
            "Reset",
            "reset",
        ];

        for message in case_variations {
            let error = ProviderError::Other(message.to_string());
            assert!(
                MidnightProvider::is_retriable_error(&error),
                "Error with message '{}' should be retriable (case insensitive)",
                message
            );
        }
    }
}
