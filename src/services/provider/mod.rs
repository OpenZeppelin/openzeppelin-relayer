use crate::config::ServerConfig;
use crate::models::{EvmNetwork, RpcConfig, SolanaNetwork};
use serde::Serialize;
use thiserror::Error;

pub mod evm;
pub use evm::*;

mod solana;
pub use solana::*;

mod stellar;
pub use stellar::*;

#[derive(Error, Debug, Serialize)]
pub enum ProviderError {
    #[error("RPC client error: {0}")]
    SolanaRpcError(#[from] SolanaProviderError),
    #[error("Invalid address: {0}")]
    InvalidAddress(String),
    #[error("Network configuration error: {0}")]
    NetworkConfiguration(String),
}

pub trait NetworkConfiguration: Sized {
    type Provider;

    fn public_rpc_urls(&self) -> Vec<String>;

    fn new_provider(
        rpc_urls: Vec<RpcConfig>,
        timeout_seconds: u64,
    ) -> Result<Self::Provider, ProviderError>;
}

impl NetworkConfiguration for EvmNetwork {
    type Provider = EvmProvider;

    fn public_rpc_urls(&self) -> Vec<String> {
        (*self)
            .public_rpc_urls()
            .map(|urls| urls.iter().map(|&url| url.to_string()).collect())
            .unwrap_or_default()
    }

    fn new_provider(
        rpc_urls: Vec<RpcConfig>,
        timeout_seconds: u64,
    ) -> Result<Self::Provider, ProviderError> {
        EvmProvider::new(rpc_urls, timeout_seconds)
    }
}

impl NetworkConfiguration for SolanaNetwork {
    type Provider = SolanaProvider;

    fn public_rpc_urls(&self) -> Vec<String> {
        (*self)
            .public_rpc_urls()
            .iter()
            .map(|&url| url.to_string())
            .collect()
    }

    fn new_provider(
        rpc_urls: Vec<RpcConfig>,
        timeout_seconds: u64,
    ) -> Result<Self::Provider, ProviderError> {
        SolanaProvider::new(rpc_urls, timeout_seconds)
    }
}

/// Creates a network-specific provider instance based on the provided configuration.
///
/// # Type Parameters
///
/// * `N`: The type of the network, which must implement the `NetworkConfiguration` trait.
///   This determines the specific provider type (`N::Provider`) and how to obtain
///   public RPC URLs.
///
/// # Arguments
///
/// * `network`: A reference to the network configuration object (`&N`).
/// * `custom_rpc_urls`: An `Option<Vec<RpcConfig>>`. If `Some` and not empty, these URLs
///   are used to configure the provider. If `None` or `Some` but empty, the function
///   falls back to using the public RPC URLs defined by the `network`'s
///   `NetworkConfiguration` implementation.
///
/// # Returns
///
/// * `Ok(N::Provider)`: An instance of the network-specific provider on success.
/// * `Err(ProviderError)`: An error if configuration fails, such as when no custom URLs
///   are provided and the network has no public RPC URLs defined
///   (`ProviderError::NetworkConfiguration`).
pub fn get_network_provider<N: NetworkConfiguration>(
    network: &N,
    custom_rpc_urls: Option<Vec<RpcConfig>>,
) -> Result<N::Provider, ProviderError> {
    let rpc_timeout_ms = ServerConfig::from_env().rpc_timeout_ms;
    let timeout_seconds = rpc_timeout_ms / 1000; // Convert ms to s

    let rpc_urls = match custom_rpc_urls {
        Some(configs) if !configs.is_empty() => configs,
        _ => {
            let urls = network.public_rpc_urls();
            if urls.is_empty() {
                return Err(ProviderError::NetworkConfiguration(
                    "No public RPC URLs available for this network".to_string(),
                ));
            }
            urls.into_iter().map(RpcConfig::new).collect()
        }
    };

    N::new_provider(rpc_urls, timeout_seconds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lazy_static::lazy_static;
    use std::env;
    use std::str::FromStr;
    use std::sync::Mutex;

    // Use a mutex to ensure tests don't run in parallel when modifying env vars
    lazy_static! {
        static ref ENV_MUTEX: Mutex<()> = Mutex::new(());
    }

    fn setup_test_env() {
        env::set_var("API_KEY", "7EF1CB7C-5003-4696-B384-C72AF8C3E15D"); // noboost
        env::set_var("REDIS_URL", "redis://localhost:6379");
        env::set_var("RPC_TIMEOUT_MS", "5000");
    }

    fn cleanup_test_env() {
        env::remove_var("API_KEY");
        env::remove_var("REDIS_URL");
        env::remove_var("RPC_TIMEOUT_MS");
    }

    #[test]
    fn test_get_evm_network_provider_valid_network() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        setup_test_env();

        let network = EvmNetwork::from_str("sepolia").unwrap();
        let result = get_network_provider(&network, None);

        cleanup_test_env();
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_evm_network_provider_with_custom_urls() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        setup_test_env();

        let network = EvmNetwork::from_str("sepolia").unwrap();
        let custom_urls = vec![
            RpcConfig {
                url: "https://custom-rpc1.example.com".to_string(),
                weight: 1,
            },
            RpcConfig {
                url: "https://custom-rpc2.example.com".to_string(),
                weight: 1,
            },
        ];
        let result = get_network_provider(&network, Some(custom_urls));

        cleanup_test_env();
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_evm_network_provider_with_empty_custom_urls() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        setup_test_env();

        let network = EvmNetwork::from_str("sepolia").unwrap();
        let custom_urls: Vec<RpcConfig> = vec![];
        let result = get_network_provider(&network, Some(custom_urls));

        cleanup_test_env();
        assert!(result.is_ok()); // Should fall back to public URLs
    }

    #[test]
    fn test_get_solana_network_provider_valid_network_mainnet_beta() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        setup_test_env();

        let network = SolanaNetwork::from_network_str("mainnet-beta").unwrap();
        let result = get_network_provider(&network, None);

        cleanup_test_env();
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_solana_network_provider_valid_network_testnet() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        setup_test_env();

        let network = SolanaNetwork::from_network_str("testnet").unwrap();
        let result = get_network_provider(&network, None);

        cleanup_test_env();
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_solana_network_provider_with_custom_urls() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        setup_test_env();

        let network = SolanaNetwork::from_network_str("testnet").unwrap();
        let custom_urls = vec![
            RpcConfig {
                url: "https://custom-rpc1.example.com".to_string(),
                weight: 1,
            },
            RpcConfig {
                url: "https://custom-rpc2.example.com".to_string(),
                weight: 1,
            },
        ];
        let result = get_network_provider(&network, Some(custom_urls));

        cleanup_test_env();
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_solana_network_provider_with_empty_custom_urls() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        setup_test_env();

        let network = SolanaNetwork::from_network_str("testnet").unwrap();
        let custom_urls: Vec<RpcConfig> = vec![];
        let result = get_network_provider(&network, Some(custom_urls));

        cleanup_test_env();
        assert!(result.is_ok()); // Should fall back to public URLs
    }

    #[test]
    fn test_get_solana_network_provider_invalid_network() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        setup_test_env();

        let network_str = "invalid-network";
        let network_result = SolanaNetwork::from_network_str(network_str);

        cleanup_test_env();
        assert!(network_result.is_err());
    }
}
