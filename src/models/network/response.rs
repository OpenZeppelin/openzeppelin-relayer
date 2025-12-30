//! API response models for network endpoints.
//!
//! This module provides response structures for network operations, converting
//! internal repository models to API-friendly formats.

use crate::models::{
    network::{NetworkConfigData, NetworkRepoModel},
    NetworkType, RpcConfig,
};
use serde::Serialize;
use utoipa::ToSchema;

/// Network response model for API endpoints.
///
/// This flattens the internal NetworkRepoModel structure for API responses,
/// making network-type-specific fields available at the top level.
#[derive(Debug, Serialize, Clone, PartialEq, ToSchema)]
pub struct NetworkResponse {
    /// Unique identifier composed of network_type and name, e.g., "evm:mainnet"
    pub id: String,
    /// Name of the network (e.g., "mainnet", "sepolia")
    pub name: String,
    /// Type of the network (EVM, Solana, Stellar)
    pub network_type: NetworkType,
    /// List of RPC endpoint configurations
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub rpc_urls: Option<Vec<RpcConfig>>,
    /// List of Explorer endpoint URLs
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub explorer_urls: Option<Vec<String>>,
    /// Estimated average time between blocks in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub average_blocktime_ms: Option<u64>,
    /// Flag indicating if the network is a testnet
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub is_testnet: Option<bool>,
    /// List of arbitrary tags for categorizing or filtering networks
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub tags: Option<Vec<String>>,
    /// EVM-specific: Chain ID
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub chain_id: Option<u64>,
    /// EVM-specific: Required confirmations
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub required_confirmations: Option<u64>,
    /// EVM-specific: Network features (e.g., "eip1559")
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub features: Option<Vec<String>>,
    /// EVM-specific: Native token symbol
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub symbol: Option<String>,
    /// Stellar-specific: Network passphrase
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub passphrase: Option<String>,
    /// Stellar-specific: Horizon URL
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub horizon_url: Option<String>,
}

impl From<NetworkRepoModel> for NetworkResponse {
    fn from(model: NetworkRepoModel) -> Self {
        let id = model.id.clone();
        let name = model.name.clone();
        let network_type = model.network_type;
        let common = model.common();
        let mut response = NetworkResponse {
            id,
            name,
            network_type,
            rpc_urls: common.rpc_urls.clone(),
            explorer_urls: common.explorer_urls.clone(),
            average_blocktime_ms: common.average_blocktime_ms,
            is_testnet: common.is_testnet,
            tags: common.tags.clone(),
            chain_id: None,
            required_confirmations: None,
            features: None,
            symbol: None,
            passphrase: None,
            horizon_url: None,
        };

        // Add network-type-specific fields
        match model.config {
            NetworkConfigData::Evm(evm_config) => {
                response.chain_id = evm_config.chain_id;
                response.required_confirmations = evm_config.required_confirmations;
                response.features = evm_config.features.clone();
                response.symbol = evm_config.symbol.clone();
            }
            NetworkConfigData::Solana(_) => {
                // Solana doesn't have additional fields beyond common
            }
            NetworkConfigData::Stellar(stellar_config) => {
                response.passphrase = stellar_config.passphrase.clone();
                response.horizon_url = stellar_config.horizon_url.clone();
            }
        }

        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::RpcConfig;

    fn create_test_evm_network() -> NetworkRepoModel {
        use crate::config::EvmNetworkConfig;
        use crate::config::NetworkConfigCommon;
        NetworkRepoModel::new_evm(EvmNetworkConfig {
            common: NetworkConfigCommon {
                network: "mainnet".to_string(),
                from: None,
                rpc_urls: Some(vec![RpcConfig::new("https://rpc.example.com".to_string())]),
                explorer_urls: Some(vec!["https://explorer.example.com".to_string()]),
                average_blocktime_ms: Some(12000),
                is_testnet: Some(false),
                tags: Some(vec!["mainnet".to_string()]),
            },
            chain_id: Some(1),
            required_confirmations: Some(12),
            features: Some(vec!["eip1559".to_string()]),
            symbol: Some("ETH".to_string()),
            gas_price_cache: None,
        })
    }

    #[test]
    fn test_from_network_repo_model_evm() {
        let model = create_test_evm_network();
        let response = NetworkResponse::from(model);

        assert_eq!(response.id, "evm:mainnet");
        assert_eq!(response.name, "mainnet");
        assert_eq!(response.network_type, NetworkType::Evm);
        assert_eq!(response.chain_id, Some(1));
        assert_eq!(response.required_confirmations, Some(12));
        assert_eq!(response.symbol, Some("ETH".to_string()));
        assert_eq!(response.features, Some(vec!["eip1559".to_string()]));
    }

    #[test]
    fn test_from_network_repo_model_common_fields() {
        let model = create_test_evm_network();
        let response = NetworkResponse::from(model);

        assert!(response.rpc_urls.is_some());
        assert!(response.explorer_urls.is_some());
        assert_eq!(response.average_blocktime_ms, Some(12000));
        assert_eq!(response.is_testnet, Some(false));
        assert!(response.tags.is_some());
    }
}

