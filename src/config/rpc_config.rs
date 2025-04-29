//! Configuration for RPC endpoints.
//!
//! This module provides configuration structures for RPC endpoints,
//! including URLs and weights for load balancing.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Configuration for an RPC endpoint.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, ToSchema)]
pub struct RpcConfig {
    /// The RPC endpoint URL.
    pub url: String,
    /// The weight of this endpoint in the weighted round-robin selection.
    /// If not specified, a default weight of 1 is used.
    pub weight: Option<u32>,
}

impl RpcConfig {
    /// Creates a new RPC configuration with the given URL and default weight (1).
    pub fn new(url: String) -> Self {
        Self {
            url,
            weight: Some(1),
        }
    }

    /// Creates a new RPC configuration with the given URL and weight.
    pub fn with_weight(url: String, weight: u32) -> Self {
        Self {
            url,
            weight: Some(weight),
        }
    }

    /// Gets the weight of this RPC endpoint, defaulting to 1 if not specified.
    pub fn get_weight(&self) -> u32 {
        self.weight.unwrap_or(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_creates_config_with_default_weight() {
        let url = "https://example.com".to_string();
        let config = RpcConfig::new(url.clone());

        assert_eq!(config.url, url);
        assert_eq!(config.weight, Some(1));
    }

    #[test]
    fn test_with_weight_creates_config_with_custom_weight() {
        let url = "https://example.com".to_string();
        let weight = 5;
        let config = RpcConfig::with_weight(url.clone(), weight);

        assert_eq!(config.url, url);
        assert_eq!(config.weight, Some(weight));
    }

    #[test]
    fn test_get_weight_returns_weight_value_when_specified() {
        let url = "https://example.com".to_string();
        let weight = 10;
        let config = RpcConfig {
            url,
            weight: Some(weight),
        };

        assert_eq!(config.get_weight(), weight);
    }

    #[test]
    fn test_get_weight_returns_default_when_none() {
        let url = "https://example.com".to_string();
        let config = RpcConfig { url, weight: None };

        assert_eq!(config.get_weight(), 1);
    }

    #[test]
    fn test_equality_of_configs() {
        let url = "https://example.com".to_string();
        let config1 = RpcConfig::new(url.clone());
        let config2 = RpcConfig::new(url.clone());
        let config3 = RpcConfig::with_weight(url, 5);

        assert_eq!(config1, config2);
        assert_ne!(config1, config3);
    }
}
