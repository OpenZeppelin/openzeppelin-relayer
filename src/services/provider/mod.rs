use crate::config::ServerConfig;
use crate::models::{EvmNetwork, RpcConfig, SolanaNetwork, StellarNetwork};
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

impl NetworkConfiguration for StellarNetwork {
    type Provider = StellarProvider;

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
        StellarProvider::new(rpc_urls, timeout_seconds)
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

    // Tests for Stellar Network Provider
    #[test]
    fn test_get_stellar_network_provider_valid_network_fallback_public() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        setup_test_env();

        let network = StellarNetwork::from_str("testnet").unwrap();
        let result = get_network_provider(&network, None); // No custom URLs

        cleanup_test_env();
        assert!(result.is_ok()); // Should fall back to public URLs for testnet
                                 // StellarProvider::new will use the first public URL: https://soroban-testnet.stellar.org
    }

    #[test]
    fn test_get_stellar_network_provider_with_custom_urls() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        setup_test_env();

        let network = StellarNetwork::from_str("testnet").unwrap();
        let custom_urls = vec![
            RpcConfig::new("https://custom-stellar-rpc1.example.com".to_string()),
            RpcConfig::with_weight("http://custom-stellar-rpc2.example.com".to_string(), 50)
                .unwrap(),
        ];
        let result = get_network_provider(&network, Some(custom_urls));

        cleanup_test_env();
        assert!(result.is_ok());
        // StellarProvider::new will pick custom-stellar-rpc1 (default weight 100) over custom-stellar-rpc2 (weight 50)
    }

    #[test]
    fn test_get_stellar_network_provider_with_empty_custom_urls_fallback() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        setup_test_env();

        let network = StellarNetwork::from_str("mainnet").unwrap();
        let custom_urls: Vec<RpcConfig> = vec![]; // Empty custom URLs
        let result = get_network_provider(&network, Some(custom_urls));

        cleanup_test_env();
        assert!(result.is_ok()); // Should fall back to public URLs for mainnet
                                 // StellarProvider::new will use the first public URL: https://horizon.stellar.org
    }

    #[test]
    fn test_get_stellar_network_provider_custom_urls_with_zero_weight() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        setup_test_env();

        let network = StellarNetwork::from_str("testnet").unwrap();
        let custom_urls = vec![
            RpcConfig::with_weight("http://zero-weight-rpc.example.com".to_string(), 0).unwrap(),
            RpcConfig::new("http://active-rpc.example.com".to_string()), // Default weight 100
        ];
        let result = get_network_provider(&network, Some(custom_urls));
        cleanup_test_env();
        assert!(result.is_ok()); // active-rpc should be chosen
    }

    #[test]
    fn test_get_stellar_network_provider_all_custom_urls_zero_weight_fallback() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        setup_test_env();

        let network = StellarNetwork::from_str("testnet").unwrap();
        let custom_urls = vec![
            RpcConfig::with_weight("http://zero1.example.com".to_string(), 0).unwrap(),
            RpcConfig::with_weight("http://zero2.example.com".to_string(), 0).unwrap(),
        ];
        // Since StellarProvider::new filters out zero-weight URLs, and if the list becomes empty,
        // get_network_provider does NOT re-trigger fallback to public. Instead, StellarProvider::new itself will error.
        // The current get_network_provider logic passes the custom_urls to N::new_provider if Some and not empty.
        // If custom_urls becomes effectively empty *inside* N::new_provider (like StellarProvider::new after filtering weights),
        // then N::new_provider is responsible for erroring or handling.
        let result = get_network_provider(&network, Some(custom_urls));
        cleanup_test_env();
        assert!(result.is_err());
        match result.unwrap_err() {
            ProviderError::NetworkConfiguration(msg) => {
                assert!(msg.contains("No active RPC configurations provided"));
            }
            _ => panic!("Unexpected error type"),
        }
    }

    #[test]
    fn test_get_stellar_network_provider_invalid_custom_url_scheme() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        setup_test_env();
        let network = StellarNetwork::from_str("testnet").unwrap();
        let custom_urls = vec![RpcConfig::new("ftp://custom-ftp.example.com".to_string())];
        let result = get_network_provider(&network, Some(custom_urls));
        cleanup_test_env();
        assert!(result.is_err());
        match result.unwrap_err() {
            ProviderError::NetworkConfiguration(msg) => {
                // This error comes from RpcConfig::validate_list inside StellarProvider::new
                assert!(msg.contains("Invalid URL scheme"));
            }
            _ => panic!("Unexpected error type"),
        }
    }
}
