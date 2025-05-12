//! Configuration for RPC endpoints.
//!
//! This module provides configuration structures for RPC endpoints,
//! including URLs and weights for load balancing.

use crate::constants::DEFAULT_RPC_WEIGHT;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Returns the default RPC weight.
fn default_rpc_weight() -> u8 {
    DEFAULT_RPC_WEIGHT
}

/// Configuration for an RPC endpoint.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, ToSchema)]
pub struct RpcConfig {
    /// The RPC endpoint URL.
    pub url: String,
    /// The weight of this endpoint in the weighted round-robin selection.
    /// Defaults to DEFAULT_RPC_WEIGHT (255). Should be between 0 and 255.
    #[serde(default = "default_rpc_weight")]
    pub weight: u8,
}

impl RpcConfig {
    /// Creates a new RPC configuration with the given URL and default weight (DEFAULT_RPC_WEIGHT).
    pub fn new(url: String) -> Self {
        Self {
            url,
            weight: DEFAULT_RPC_WEIGHT,
        }
    }

    /// Creates a new RPC configuration with the given URL and weight.
    pub fn with_weight(url: String, weight: u8) -> Self {
        Self { url, weight }
    }

    /// Gets the weight of this RPC endpoint.
    pub fn get_weight(&self) -> u8 {
        self.weight
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::DEFAULT_RPC_WEIGHT;

    #[test]
    fn test_new_creates_config_with_default_weight() {
        let url = "https://example.com".to_string();
        let config = RpcConfig::new(url.clone());

        assert_eq!(config.url, url);
        assert_eq!(config.weight, DEFAULT_RPC_WEIGHT);
    }

    #[test]
    fn test_with_weight_creates_config_with_custom_weight() {
        let url = "https://example.com".to_string();
        let weight: u8 = 5;
        let config = RpcConfig::with_weight(url.clone(), weight);

        assert_eq!(config.url, url);
        assert_eq!(config.weight, weight);
    }

    #[test]
    fn test_get_weight_returns_weight_value() {
        let url = "https://example.com".to_string();
        let weight: u8 = 10;
        let config = RpcConfig { url, weight };

        assert_eq!(config.get_weight(), weight);
    }

    #[test]
    fn test_equality_of_configs() {
        let url = "https://example.com".to_string();
        let config1 = RpcConfig::new(url.clone());
        let config2 = RpcConfig::new(url.clone()); // Same as config1
        let config3 = RpcConfig::with_weight(url.clone(), 5u8); // Different weight
        let config4 =
            RpcConfig::with_weight("https://different.com".to_string(), DEFAULT_RPC_WEIGHT); // Different URL

        assert_eq!(config1, config2);
        assert_ne!(config1, config3);
        assert_ne!(config1, config4);
    }
}
