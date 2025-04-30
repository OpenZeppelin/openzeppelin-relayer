use serde::Serialize;
use thiserror::Error;

pub mod evm;
pub use evm::*;

mod solana;
pub use solana::*;

mod stellar;
pub use stellar::*;

use crate::config::{RpcConfig, ServerConfig};
use crate::models::{EvmNetwork, SolanaNetwork};

#[derive(Error, Debug, Serialize)]
pub enum ProviderError {
    #[error("RPC client error: {0}")]
    SolanaRpcError(#[from] SolanaProviderError),
    #[error("Invalid address: {0}")]
    InvalidAddress(String),
    #[error("Network configuration error: {0}")]
    NetworkConfiguration(String),
}

pub fn get_evm_network_provider(
    network: EvmNetwork,
    custom_rpc_urls: Option<Vec<RpcConfig>>,
) -> Result<EvmProvider, ProviderError> {
    let rpc_timeout_ms = ServerConfig::from_env().rpc_timeout_ms;

    let rpc_urls = match custom_rpc_urls {
        Some(configs) if !configs.is_empty() => configs,
        _ => {
            // Get public RPC URLs from network and convert to RpcConfig
            let urls = network.public_rpc_urls();
            match urls {
                Some(url_list) if !url_list.is_empty() => url_list
                    .iter()
                    .map(|url| RpcConfig::new(url.to_string()))
                    .collect(),
                _ => {
                    return Err(ProviderError::NetworkConfiguration(
                        "No RPC URLs configured for this network".to_string(),
                    ));
                }
            }
        }
    };
    let evm_provider: EvmProvider = EvmProvider::new(rpc_urls, rpc_timeout_ms)
        .map_err(|e| ProviderError::NetworkConfiguration(e.to_string()))?;

    Ok(evm_provider)
}

pub fn get_solana_network_provider(
    network: &str,
    custom_rpc_urls: Option<Vec<RpcConfig>>,
) -> Result<SolanaProvider, ProviderError> {
    let network = match SolanaNetwork::from_network_str(network) {
        Ok(network) => network,
        Err(e) => return Err(ProviderError::NetworkConfiguration(e.to_string())),
    };

    // Use custom RPC configs if provided, otherwise create configs from network URLs
    let rpc_urls = match custom_rpc_urls {
        Some(configs) if !configs.is_empty() => configs,
        _ => {
            // Get URLs from network and convert to RpcConfig
            let urls = network.public_rpc_urls();
            if urls.is_empty() {
                return Err(ProviderError::NetworkConfiguration(
                    "No RPC URLs configured for this network".to_string(),
                ));
            }
            urls.iter()
                .map(|url| RpcConfig::new(url.to_string()))
                .collect()
        }
    };

    let timeout = ServerConfig::from_env().rpc_timeout_ms;

    SolanaProvider::new(rpc_urls, timeout)
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
        let result = get_evm_network_provider(network, None);

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
                weight: Some(1),
            },
            RpcConfig {
                url: "https://custom-rpc2.example.com".to_string(),
                weight: Some(1),
            },
        ];
        let result = get_evm_network_provider(network, Some(custom_urls));

        cleanup_test_env();
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_evm_network_provider_with_empty_custom_urls() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        setup_test_env();

        let network = EvmNetwork::from_str("sepolia").unwrap();
        let custom_urls: Vec<RpcConfig> = vec![];
        let result = get_evm_network_provider(network, Some(custom_urls));

        cleanup_test_env();
        assert!(result.is_ok()); // Should fall back to public URLs
    }

    #[test]
    fn test_get_solana_network_provider_valid_network_mainnet_beta() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        setup_test_env();

        let result = get_solana_network_provider("mainnet-beta", None);

        cleanup_test_env();
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_solana_network_provider_valid_network_testnet() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        setup_test_env();

        let result = get_solana_network_provider("testnet", None);

        cleanup_test_env();
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_solana_network_provider_with_custom_urls() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        setup_test_env();

        let custom_urls = vec![
            RpcConfig {
                url: "https://custom-rpc1.example.com".to_string(),
                weight: Some(1),
            },
            RpcConfig {
                url: "https://custom-rpc2.example.com".to_string(),
                weight: Some(1),
            },
        ];
        let result = get_solana_network_provider("testnet", Some(custom_urls));

        cleanup_test_env();
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_solana_network_provider_with_empty_custom_urls() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        setup_test_env();

        let custom_urls: Vec<RpcConfig> = vec![];
        let result = get_solana_network_provider("testnet", Some(custom_urls));

        cleanup_test_env();
        assert!(result.is_ok()); // Should fall back to public URLs
    }

    #[test]
    fn test_get_solana_network_provider_invalid_network() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        setup_test_env();

        let result = get_solana_network_provider("invalid-network", None);

        cleanup_test_env();
        assert!(result.is_err());
    }
}
