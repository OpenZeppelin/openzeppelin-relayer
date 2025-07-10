//! Midnight Network Configuration
//!
//! This module provides configuration support for Midnight blockchain networks.

use super::common::NetworkConfigCommon;
use crate::config::ConfigFileError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Hash)]
pub struct IndexerUrls {
    pub http: String,
    pub ws: String,
}

/// Configuration specific to Midnight networks.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct MidnightNetworkConfig {
    /// Common network fields.
    #[serde(flatten)]
    pub common: NetworkConfigCommon,
    // Midnight-specific fields
    pub indexer_urls: IndexerUrls, // URL for the indexer server (ws, http)
    pub prover_url: String,        // URL for the prover server
    pub commitment_tree_ttl: Option<u64>, // How long to cache Merkle roots
}

impl MidnightNetworkConfig {
    /// Validates the specific configuration fields for a Midnight network.
    ///
    /// # Returns
    /// - `Ok(())` if the Midnight configuration is valid.
    /// - `Err(ConfigFileError)` if validation fails (e.g., missing fields, invalid URLs).
    pub fn validate(&self) -> Result<(), ConfigFileError> {
        self.common.validate()?;

        // Validate indexer URLs
        reqwest::Url::parse(&self.indexer_urls.http).map_err(|_| {
            ConfigFileError::InvalidFormat(format!(
                "Invalid indexer HTTP URL: {}",
                self.indexer_urls.http
            ))
        })?;

        reqwest::Url::parse(&self.indexer_urls.ws).map_err(|_| {
            ConfigFileError::InvalidFormat(format!(
                "Invalid indexer WebSocket URL: {}",
                self.indexer_urls.ws
            ))
        })?;

        // Validate prover URL if provided
        reqwest::Url::parse(&self.prover_url).map_err(|_| {
            ConfigFileError::InvalidFormat(format!("Invalid prover URL: {}", self.prover_url))
        })?;

        // Validate network_id if provided
        match self.common.network.as_str() {
            "mainnet" | "testnet" | "devnet" => {}
            _ => {
                return Err(ConfigFileError::InvalidFormat(format!(
                    "Invalid network_id: {}. Must be one of: mainnet, testnet, devnet",
                    self.common.network
                )))
            }
        }

        // Validate commitment_tree_ttl is reasonable if provided
        if let Some(ttl) = self.commitment_tree_ttl {
            if ttl == 0 {
                return Err(ConfigFileError::InvalidFormat(
                    "commitment_tree_ttl must be greater than 0".to_string(),
                ));
            }
        }

        Ok(())
    }

    /// Merges this Midnight configuration with a parent Midnight configuration.
    /// Parent values are used as defaults, child values take precedence.
    pub fn merge_with_parent(&self, parent: &Self) -> Self {
        Self {
            common: self.common.merge_with_parent(&parent.common),
            // For required fields, we always use the child's values since they must be present
            indexer_urls: self.indexer_urls.clone(),
            prover_url: self.prover_url.clone(),
            // For optional fields, use child's value if present, otherwise parent's
            commitment_tree_ttl: self.commitment_tree_ttl.or(parent.commitment_tree_ttl),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_valid_config() -> MidnightNetworkConfig {
        MidnightNetworkConfig {
            common: NetworkConfigCommon {
                network: "testnet".to_string(),
                from: Some("0x1234567890abcdef".to_string()),
                rpc_urls: Some(vec!["http://localhost:9944".to_string()]),
                explorer_urls: Some(vec!["https://explorer.midnight.network".to_string()]),
                average_blocktime_ms: Some(5000),
                is_testnet: Some(true),
                tags: Some(vec!["test".to_string()]),
            },
            indexer_urls: IndexerUrls {
                http: "https://indexer.midnight.network".to_string(),
                ws: "wss://indexer.midnight.network".to_string(),
            },
            prover_url: "https://prover.midnight.network".to_string(),
            commitment_tree_ttl: Some(3600),
        }
    }

    #[test]
    fn test_valid_midnight_config() {
        let config = create_valid_config();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_indexer_urls_equality() {
        let urls1 = IndexerUrls {
            http: "http://localhost:8080".to_string(),
            ws: "ws://localhost:8080".to_string(),
        };
        let urls2 = IndexerUrls {
            http: "http://localhost:8080".to_string(),
            ws: "ws://localhost:8080".to_string(),
        };
        assert_eq!(urls1, urls2);
    }

    #[test]
    fn test_indexer_urls_serialization() {
        let urls = IndexerUrls {
            http: "http://localhost:8080".to_string(),
            ws: "ws://localhost:8080".to_string(),
        };

        let json = serde_json::to_string(&urls).unwrap();
        let deserialized: IndexerUrls = serde_json::from_str(&json).unwrap();

        assert_eq!(urls, deserialized);
    }

    #[test]
    fn test_invalid_http_indexer_url() {
        let mut config = create_valid_config();
        config.indexer_urls.http = "not-a-valid-url".to_string();

        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid indexer HTTP URL"));
    }

    #[test]
    fn test_invalid_ws_indexer_url() {
        let mut config = create_valid_config();
        config.indexer_urls.ws = "not-a-valid-ws-url".to_string();

        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid indexer WebSocket URL"));
    }

    #[test]
    fn test_invalid_prover_url() {
        let mut config = create_valid_config();
        config.prover_url = "invalid-url".to_string();

        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid prover URL"));
    }

    #[test]
    fn test_valid_network_ids() {
        for network_id in &["mainnet", "testnet", "devnet"] {
            let mut config = create_valid_config();
            config.common.network = network_id.to_string();
            assert!(config.validate().is_ok());
        }
    }

    #[test]
    fn test_invalid_network_id() {
        let mut config = create_valid_config();
        config.common.network = "invalidnet".to_string();

        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid network_id"));
    }

    #[test]
    fn test_commitment_tree_ttl_zero() {
        let mut config = create_valid_config();
        config.commitment_tree_ttl = Some(0);

        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("commitment_tree_ttl must be greater than 0"));
    }

    #[test]
    fn test_commitment_tree_ttl_valid() {
        let mut config = create_valid_config();
        config.commitment_tree_ttl = Some(3600);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_commitment_tree_ttl_none() {
        let mut config = create_valid_config();
        config.commitment_tree_ttl = None;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_merge_with_parent() {
        let parent = MidnightNetworkConfig {
            common: NetworkConfigCommon {
                network: "mainnet".to_string(),
                from: Some("0xparent".to_string()),
                rpc_urls: Some(vec!["http://parent:9944".to_string()]),
                explorer_urls: None,
                average_blocktime_ms: Some(6000),
                is_testnet: Some(false),
                tags: Some(vec!["parent".to_string()]),
            },
            indexer_urls: IndexerUrls {
                http: "http://parent-indexer".to_string(),
                ws: "ws://parent-indexer".to_string(),
            },
            prover_url: "http://parent-prover".to_string(),
            commitment_tree_ttl: Some(7200),
        };

        let child = MidnightNetworkConfig {
            common: NetworkConfigCommon {
                network: "testnet".to_string(),
                from: None,
                rpc_urls: Some(vec!["http://child:9944".to_string()]),
                explorer_urls: Some(vec!["https://child-explorer".to_string()]),
                average_blocktime_ms: None,
                is_testnet: None,
                tags: None,
            },
            indexer_urls: IndexerUrls {
                http: "http://child-indexer".to_string(),
                ws: "ws://child-indexer".to_string(),
            },
            prover_url: "http://child-prover".to_string(),
            commitment_tree_ttl: None,
        };

        let merged = child.merge_with_parent(&parent);

        // Child values take precedence
        assert_eq!(merged.common.network, "testnet");
        assert_eq!(merged.indexer_urls.http, "http://child-indexer");
        assert_eq!(merged.prover_url, "http://child-prover");

        // Parent values used as defaults
        assert_eq!(merged.common.from, Some("0xparent".to_string()));
        assert_eq!(merged.common.average_blocktime_ms, Some(6000));
        assert_eq!(merged.common.is_testnet, Some(false));
        assert_eq!(merged.common.tags, Some(vec!["parent".to_string()]));
        assert_eq!(merged.commitment_tree_ttl, Some(7200));
    }

    #[test]
    fn test_deserialization_with_missing_optional_fields() {
        let json = r#"{
            "network": "testnet",
            "rpc_urls": ["http://localhost:9944"],
            "indexer_urls": {
                "http": "http://indexer",
                "ws": "ws://indexer"
            },
            "prover_url": "http://prover"
        }"#;

        let config: Result<MidnightNetworkConfig, _> = serde_json::from_str(json);
        assert!(config.is_ok());

        let config = config.unwrap();
        assert_eq!(config.common.network, "testnet");
        assert!(config.commitment_tree_ttl.is_none());
        assert!(config.common.from.is_none());
        assert!(config.common.explorer_urls.is_none());
    }

    #[test]
    fn test_serialization_roundtrip() {
        let config = create_valid_config();
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: MidnightNetworkConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(config.common.network, deserialized.common.network);
        assert_eq!(config.indexer_urls, deserialized.indexer_urls);
        assert_eq!(config.prover_url, deserialized.prover_url);
        assert_eq!(config.commitment_tree_ttl, deserialized.commitment_tree_ttl);
    }

    #[test]
    fn test_deny_unknown_fields() {
        let json = r#"{
            "network": "testnet",
            "rpc_urls": ["http://localhost:9944"],
            "indexer_urls": {
                "http": "http://indexer",
                "ws": "ws://indexer"
            },
            "prover_url": "http://prover",
            "unknown_field": "should fail"
        }"#;

        let config: Result<MidnightNetworkConfig, _> = serde_json::from_str(json);
        assert!(config.is_err());
    }
}
