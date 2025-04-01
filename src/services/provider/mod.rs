use serde::Serialize;
use thiserror::Error;

pub mod evm;
pub use evm::*;

mod solana;
pub use solana::*;

use crate::models::SolanaNetwork;

#[derive(Error, Debug, Serialize)]
pub enum ProviderError {
    #[error("RPC client error: {0}")]
    SolanaRpcError(#[from] SolanaProviderError),
    #[error("Invalid address: {0}")]
    InvalidAddress(String),
    #[error("Network configuration error: {0}")]
    NetworkConfiguration(String),
}

pub fn get_solana_network_provider(
    network: &str,
    custom_rpc_urls: Option<Vec<String>>,
) -> Result<SolanaProvider, ProviderError> {
    let network = match SolanaNetwork::from_network_str(network) {
        Ok(network) => network,
        Err(e) => return Err(ProviderError::NetworkConfiguration(e.to_string())),
    };

    let rpc_url = custom_rpc_urls
        .and_then(|urls| urls.first().cloned())
        .or_else(|| network.public_rpc_urls().first().copied().map(String::from))
        .ok_or(ProviderError::NetworkConfiguration(
            "No RPC URLs configured".to_string(),
        ))?;

    SolanaProvider::new(&rpc_url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_solana_network_provider_valid_network_mainnet_beta() {
        let result = get_solana_network_provider("mainnet-beta", None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_solana_network_provider_valid_network_testnet() {
        let result = get_solana_network_provider("testnet", None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_solana_network_provider_with_custom_urls() {
        let custom_urls = vec![
            "https://custom-rpc1.example.com".to_string(),
            "https://custom-rpc2.example.com".to_string(),
        ];
        let result = get_solana_network_provider("testnet", Some(custom_urls));
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_solana_network_provider_with_empty_custom_urls() {
        let custom_urls: Vec<String> = vec![];
        let result = get_solana_network_provider("testnet", Some(custom_urls));
        assert!(result.is_ok()); // Should fall back to public URLs
    }

    #[test]
    fn test_get_solana_network_provider_invalid_network() {
        let result = get_solana_network_provider("invalid-network", None);
        assert!(result.is_err());
    }
}
