//! EVM Network Configuration
//!
//! This module provides configuration support for EVM-compatible blockchain networks
//! such as Ethereum, Polygon, BSC, Avalanche, and other Ethereum-compatible chains.
//!
//! ## Key Features
//!
//! - **Full inheritance support**: EVM networks can inherit from other EVM networks
//! - **Feature merging**: Parent and child features are merged preserving unique items
//! - **Type safety**: Inheritance only allowed between EVM networks

use super::common::{merge_optional_string_vecs, NetworkConfigCommon};
use crate::config::ConfigFileError;
use serde::{Deserialize, Serialize};

/// Default value for gas price cache enabled flag
fn default_gas_cache_enabled() -> bool {
    false
}

/// Default value for gas price cache stale after duration in milliseconds
fn default_gas_cache_stale_after_ms() -> u64 {
    20_000 // 20 seconds
}

/// Default value for gas price cache expire after duration in milliseconds
fn default_gas_cache_expire_after_ms() -> u64 {
    45_000 // 45 seconds
}

/// Configuration for gas price caching
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Hash)]
#[serde(deny_unknown_fields)]
pub struct GasPriceCacheConfig {
    /// Enable gas price caching for this network
    #[serde(default = "default_gas_cache_enabled")]
    pub enabled: bool,

    /// When data becomes stale (milliseconds)
    #[serde(default = "default_gas_cache_stale_after_ms")]
    pub stale_after_ms: u64,

    /// When to expire and force refresh (milliseconds)
    #[serde(default = "default_gas_cache_expire_after_ms")]
    pub expire_after_ms: u64,
}

impl Default for GasPriceCacheConfig {
    fn default() -> Self {
        Self {
            enabled: default_gas_cache_enabled(),
            stale_after_ms: default_gas_cache_stale_after_ms(),
            expire_after_ms: default_gas_cache_expire_after_ms(),
        }
    }
}

impl GasPriceCacheConfig {
    /// Validates the gas price cache configuration
    ///
    /// # Returns
    /// - `Ok(())` if the configuration is valid
    /// - `Err(ConfigFileError)` if validation fails
    pub fn validate(&self) -> Result<(), ConfigFileError> {
        // Check that durations are non-zero
        if self.stale_after_ms == 0 {
            return Err(ConfigFileError::InvalidFormat(
                "Gas price cache stale_after_ms must be greater than zero".into(),
            ));
        }

        if self.expire_after_ms == 0 {
            return Err(ConfigFileError::InvalidFormat(
                "Gas price cache expire_after_ms must be greater than zero".into(),
            ));
        }

        // Check that expire_after_ms > stale_after_ms
        if self.expire_after_ms <= self.stale_after_ms {
            return Err(ConfigFileError::InvalidFormat(
                "Gas price cache expire_after_ms must be greater than stale_after_ms".into(),
            ));
        }

        Ok(())
    }
}

/// Configuration specific to EVM-compatible networks.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct EvmNetworkConfig {
    /// Common network fields.
    #[serde(flatten)]
    pub common: NetworkConfigCommon,

    /// The unique chain identifier (Chain ID) for the EVM network.
    pub chain_id: Option<u64>,
    /// Number of block confirmations required before a transaction is considered final.
    pub required_confirmations: Option<u64>,
    /// List of specific features supported by the network (e.g., "eip1559").
    pub features: Option<Vec<String>>,
    /// The symbol of the network's native currency (e.g., "ETH", "MATIC").
    pub symbol: Option<String>,
    /// Gas price cache configuration
    pub gas_price_cache: Option<GasPriceCacheConfig>,
}

impl EvmNetworkConfig {
    /// Validates the specific configuration fields for an EVM network.
    ///
    /// # Returns
    /// - `Ok(())` if the EVM configuration is valid.
    /// - `Err(ConfigFileError)` if validation fails (e.g., missing fields, invalid URLs).
    pub fn validate(&self) -> Result<(), ConfigFileError> {
        self.common.validate()?;

        // Chain ID is required for non-inherited networks
        if self.chain_id.is_none() {
            return Err(ConfigFileError::MissingField("chain_id".into()));
        }

        if self.required_confirmations.is_none() {
            return Err(ConfigFileError::MissingField(
                "required_confirmations".into(),
            ));
        }

        if self.symbol.is_none() || self.symbol.as_ref().unwrap_or(&String::new()).is_empty() {
            return Err(ConfigFileError::MissingField("symbol".into()));
        }

        // Validate gas price cache configuration if present
        if let Some(gas_price_cache) = &self.gas_price_cache {
            gas_price_cache.validate()?;
        }

        Ok(())
    }

    /// Creates a new EVM configuration by merging this config with a parent, where child values override parent defaults.
    ///
    /// # Arguments
    /// * `parent` - The parent EVM configuration to merge with.
    ///
    /// # Returns
    /// A new `EvmNetworkConfig` with merged values where child takes precedence over parent.
    pub fn merge_with_parent(&self, parent: &Self) -> Self {
        Self {
            common: self.common.merge_with_parent(&parent.common),
            chain_id: self.chain_id.or(parent.chain_id),
            required_confirmations: self
                .required_confirmations
                .or(parent.required_confirmations),
            features: merge_optional_string_vecs(&self.features, &parent.features),
            symbol: self.symbol.clone().or_else(|| parent.symbol.clone()),
            gas_price_cache: self
                .gas_price_cache
                .clone()
                .or_else(|| parent.gas_price_cache.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::config_file::network::test_utils::*;

    #[test]
    fn test_validate_success_complete_config() {
        let config = create_evm_network("ethereum-mainnet");
        let result = config.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_success_minimal_config() {
        let mut config = create_evm_network("minimal-evm");
        config.features = None;
        let result = config.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_missing_chain_id() {
        let mut config = create_evm_network("ethereum-mainnet");
        config.chain_id = None;

        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::MissingField(_)
        ));
    }

    #[test]
    fn test_validate_missing_required_confirmations() {
        let mut config = create_evm_network("ethereum-mainnet");
        config.required_confirmations = None;

        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::MissingField(_)
        ));
    }

    #[test]
    fn test_validate_missing_symbol() {
        let mut config = create_evm_network("ethereum-mainnet");
        config.symbol = None;

        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::MissingField(_)
        ));
    }

    #[test]
    fn test_validate_invalid_common_fields() {
        let mut config = create_evm_network("ethereum-mainnet");
        config.common.network = String::new(); // Invalid empty network name

        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::MissingField(_)
        ));
    }

    #[test]
    fn test_validate_invalid_rpc_urls() {
        let mut config = create_evm_network("ethereum-mainnet");
        config.common.rpc_urls = Some(vec!["invalid-url".to_string()]);

        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::InvalidFormat(_)
        ));
    }

    #[test]
    fn test_validate_with_zero_chain_id() {
        let mut config = create_evm_network("ethereum-mainnet");
        config.chain_id = Some(0);

        let result = config.validate();
        assert!(result.is_ok()); // Zero is a valid chain ID
    }

    #[test]
    fn test_validate_with_large_chain_id() {
        let mut config = create_evm_network("ethereum-mainnet");
        config.chain_id = Some(u64::MAX);

        let result = config.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_with_zero_confirmations() {
        let mut config = create_evm_network("ethereum-mainnet");
        config.required_confirmations = Some(0);

        let result = config.validate();
        assert!(result.is_ok()); // Zero confirmations is valid
    }

    #[test]
    fn test_validate_with_empty_features() {
        let mut config = create_evm_network("ethereum-mainnet");
        config.features = Some(vec![]);

        let result = config.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_with_empty_symbol() {
        let mut config = create_evm_network("ethereum-mainnet");
        config.symbol = Some(String::new());

        let result = config.validate();
        assert!(result.is_err());
    }

    #[test]
    fn test_merge_with_parent_child_overrides() {
        let parent = EvmNetworkConfig {
            common: NetworkConfigCommon {
                network: "parent".to_string(),
                from: Some("parent".to_string()),
                rpc_urls: Some(vec!["https://parent-rpc.example.com".to_string()]),
                explorer_urls: Some(vec!["https://parent-explorer.example.com".to_string()]),
                average_blocktime_ms: Some(10000),
                is_testnet: Some(true),
                tags: Some(vec!["parent-tag".to_string()]),
            },
            chain_id: Some(1),
            required_confirmations: Some(6),
            features: Some(vec!["legacy".to_string()]),
            symbol: Some("PETH".to_string()),
            gas_price_cache: Some(GasPriceCacheConfig {
                enabled: true,
                stale_after_ms: 20_000,
                expire_after_ms: 100_000,
            }),
        };

        let child = EvmNetworkConfig {
            common: NetworkConfigCommon {
                network: "child".to_string(),
                from: Some("parent".to_string()),
                rpc_urls: Some(vec!["https://child-rpc.example.com".to_string()]),
                explorer_urls: Some(vec!["https://child-explorer.example.com".to_string()]),
                average_blocktime_ms: Some(15000),
                is_testnet: Some(false),
                tags: Some(vec!["child-tag".to_string()]),
            },
            chain_id: Some(31337),
            required_confirmations: Some(1),
            features: Some(vec!["eip1559".to_string()]),
            symbol: Some("CETH".to_string()),
            gas_price_cache: Some(GasPriceCacheConfig {
                enabled: false,
                stale_after_ms: 40_000,
                expire_after_ms: 200_000,
            }),
        };

        let result = child.merge_with_parent(&parent);

        // Child values should override parent values
        assert_eq!(result.common.network, "child");
        assert_eq!(result.common.from, Some("parent".to_string()));
        assert_eq!(
            result.common.rpc_urls,
            Some(vec!["https://child-rpc.example.com".to_string()])
        );
        assert_eq!(
            result.common.explorer_urls,
            Some(vec!["https://child-explorer.example.com".to_string()])
        );
        assert_eq!(result.common.average_blocktime_ms, Some(15000));
        assert_eq!(result.common.is_testnet, Some(false));
        assert_eq!(
            result.common.tags,
            Some(vec!["parent-tag".to_string(), "child-tag".to_string()])
        );
        assert_eq!(result.chain_id, Some(31337));
        assert_eq!(result.required_confirmations, Some(1));
        assert_eq!(
            result.features,
            Some(vec!["legacy".to_string(), "eip1559".to_string()])
        );
        assert_eq!(result.symbol, Some("CETH".to_string()));
        assert_eq!(
            result.gas_price_cache,
            Some(GasPriceCacheConfig {
                enabled: false,
                stale_after_ms: 40_000,
                expire_after_ms: 200_000,
            })
        );
    }

    #[test]
    fn test_merge_with_parent_child_inherits() {
        let parent = EvmNetworkConfig {
            common: NetworkConfigCommon {
                network: "parent".to_string(),
                from: None,
                rpc_urls: Some(vec!["https://parent-rpc.example.com".to_string()]),
                explorer_urls: Some(vec!["https://parent-explorer.example.com".to_string()]),
                average_blocktime_ms: Some(10000),
                is_testnet: Some(true),
                tags: Some(vec!["parent-tag".to_string()]),
            },
            chain_id: Some(1),
            required_confirmations: Some(6),
            features: Some(vec!["eip1559".to_string()]),
            symbol: Some("ETH".to_string()),
            gas_price_cache: Some(GasPriceCacheConfig {
                enabled: true,
                stale_after_ms: 20_000,
                expire_after_ms: 100_000,
            }),
        };

        let child = create_evm_network_for_inheritance_test("ethereum-testnet", "ethereum-mainnet");

        let result = child.merge_with_parent(&parent);

        // Child should inherit parent values where child has None
        assert_eq!(result.common.network, "ethereum-testnet");
        assert_eq!(result.common.from, Some("ethereum-mainnet".to_string()));
        assert_eq!(
            result.common.rpc_urls,
            Some(vec!["https://parent-rpc.example.com".to_string()])
        );
        assert_eq!(
            result.common.explorer_urls,
            Some(vec!["https://parent-explorer.example.com".to_string()])
        );
        assert_eq!(result.common.average_blocktime_ms, Some(10000));
        assert_eq!(result.common.is_testnet, Some(true));
        assert_eq!(result.common.tags, Some(vec!["parent-tag".to_string()]));
        assert_eq!(result.chain_id, Some(1));
        assert_eq!(result.required_confirmations, Some(6));
        assert_eq!(result.features, Some(vec!["eip1559".to_string()]));
        assert_eq!(result.symbol, Some("ETH".to_string()));
        assert_eq!(
            result.gas_price_cache,
            Some(GasPriceCacheConfig {
                enabled: true,
                stale_after_ms: 20_000,
                expire_after_ms: 100_000,
            })
        );
    }

    #[test]
    fn test_merge_with_parent_mixed_inheritance() {
        let parent = EvmNetworkConfig {
            common: NetworkConfigCommon {
                network: "parent".to_string(),
                from: None,
                rpc_urls: Some(vec!["https://parent-rpc.example.com".to_string()]),
                explorer_urls: Some(vec!["https://parent-explorer.example.com".to_string()]),
                average_blocktime_ms: Some(10000),
                is_testnet: Some(true),
                tags: Some(vec!["parent-tag1".to_string(), "parent-tag2".to_string()]),
            },
            chain_id: Some(1),
            required_confirmations: Some(6),
            features: Some(vec!["eip155".to_string(), "eip1559".to_string()]),
            symbol: Some("ETH".to_string()),
            gas_price_cache: Some(GasPriceCacheConfig {
                enabled: true,
                stale_after_ms: 20_000,
                expire_after_ms: 100_000,
            }),
        };

        let child = EvmNetworkConfig {
            common: NetworkConfigCommon {
                network: "child".to_string(),
                from: Some("parent".to_string()),
                rpc_urls: Some(vec!["https://child-rpc.example.com".to_string()]), // Override
                explorer_urls: Some(vec!["https://child-explorer.example.com".to_string()]), // Override
                average_blocktime_ms: None,                // Inherit
                is_testnet: Some(false),                   // Override
                tags: Some(vec!["child-tag".to_string()]), // Merge
            },
            chain_id: Some(31337),                       // Override
            required_confirmations: None,                // Inherit
            features: Some(vec!["eip2930".to_string()]), // Merge
            symbol: None,                                // Inherit
            gas_price_cache: Some(GasPriceCacheConfig {
                enabled: false,
                stale_after_ms: 40_000,
                expire_after_ms: 200_000,
            }),
        };

        let result = child.merge_with_parent(&parent);

        assert_eq!(result.common.network, "child");
        assert_eq!(
            result.common.rpc_urls,
            Some(vec!["https://child-rpc.example.com".to_string()])
        ); // Overridden
        assert_eq!(
            result.common.explorer_urls,
            Some(vec!["https://child-explorer.example.com".to_string()])
        ); // Overridden
        assert_eq!(result.common.average_blocktime_ms, Some(10000)); // Inherited
        assert_eq!(result.common.is_testnet, Some(false)); // Overridden
        assert_eq!(
            result.common.tags,
            Some(vec![
                "parent-tag1".to_string(),
                "parent-tag2".to_string(),
                "child-tag".to_string()
            ])
        ); // Merged
        assert_eq!(result.chain_id, Some(31337)); // Overridden
        assert_eq!(result.required_confirmations, Some(6)); // Inherited
        assert_eq!(
            result.features,
            Some(vec![
                "eip155".to_string(),
                "eip1559".to_string(),
                "eip2930".to_string()
            ])
        ); // Merged
        assert_eq!(result.symbol, Some("ETH".to_string())); // Inherited
        assert_eq!(
            result.gas_price_cache,
            Some(GasPriceCacheConfig {
                enabled: false,
                stale_after_ms: 40_000,
                expire_after_ms: 200_000,
            })
        );
    }

    #[test]
    fn test_merge_with_parent_both_empty() {
        let parent = EvmNetworkConfig {
            common: NetworkConfigCommon {
                network: "parent".to_string(),
                from: None,
                rpc_urls: None,
                explorer_urls: None,
                average_blocktime_ms: None,
                is_testnet: None,
                tags: None,
            },
            chain_id: None,
            required_confirmations: None,
            features: None,
            symbol: None,
            gas_price_cache: None,
        };

        let child = EvmNetworkConfig {
            common: NetworkConfigCommon {
                network: "child".to_string(),
                from: Some("parent".to_string()),
                rpc_urls: None,
                explorer_urls: None,
                average_blocktime_ms: None,
                is_testnet: None,
                tags: None,
            },
            chain_id: None,
            required_confirmations: None,
            features: None,
            symbol: None,
            gas_price_cache: None,
        };

        let result = child.merge_with_parent(&parent);

        assert_eq!(result.common.network, "child");
        assert_eq!(result.common.from, Some("parent".to_string()));
        assert_eq!(result.common.rpc_urls, None);
        assert_eq!(result.common.average_blocktime_ms, None);
        assert_eq!(result.common.is_testnet, None);
        assert_eq!(result.common.tags, None);
        assert_eq!(result.chain_id, None);
        assert_eq!(result.required_confirmations, None);
        assert_eq!(result.features, None);
        assert_eq!(result.symbol, None);
        assert_eq!(result.gas_price_cache, None);
    }

    #[test]
    fn test_merge_with_parent_complex_features_merging() {
        let parent = EvmNetworkConfig {
            common: NetworkConfigCommon {
                network: "parent".to_string(),
                from: None,
                rpc_urls: Some(vec!["https://rpc.example.com".to_string()]),
                explorer_urls: Some(vec!["https://explorer.example.com".to_string()]),
                average_blocktime_ms: Some(12000),
                is_testnet: Some(false),
                tags: None,
            },
            chain_id: Some(1),
            required_confirmations: Some(12),
            features: Some(vec![
                "eip155".to_string(),
                "eip1559".to_string(),
                "shared".to_string(),
            ]),
            symbol: Some("ETH".to_string()),
            gas_price_cache: Some(GasPriceCacheConfig {
                enabled: true,
                stale_after_ms: 20_000,
                expire_after_ms: 100_000,
            }),
        };

        let child = EvmNetworkConfig {
            common: NetworkConfigCommon {
                network: "child".to_string(),
                from: Some("parent".to_string()),
                rpc_urls: None,
                explorer_urls: None,
                average_blocktime_ms: None,
                is_testnet: None,
                tags: None,
            },
            chain_id: None,
            required_confirmations: None,
            features: Some(vec![
                "shared".to_string(),
                "eip2930".to_string(),
                "custom".to_string(),
            ]),
            symbol: None,
            gas_price_cache: None,
        };

        let result = child.merge_with_parent(&parent);

        // Features should be merged with parent first, then unique child features added
        let expected_features = vec![
            "eip155".to_string(),
            "eip1559".to_string(),
            "shared".to_string(), // Duplicate should not be added again
            "eip2930".to_string(),
            "custom".to_string(),
        ];
        assert_eq!(result.features, Some(expected_features));
        assert_eq!(
            result.gas_price_cache,
            Some(GasPriceCacheConfig {
                enabled: true,
                stale_after_ms: 20_000,
                expire_after_ms: 100_000,
            })
        );
    }

    #[test]
    fn test_merge_with_parent_preserves_child_network_name() {
        let parent = create_evm_network("ethereum-mainnet");
        let mut child =
            create_evm_network_for_inheritance_test("ethereum-testnet", "ethereum-mainnet");
        child.common.network = "custom-child-name".to_string();

        let result = child.merge_with_parent(&parent);

        // Child network name should always be preserved
        assert_eq!(result.common.network, "custom-child-name");
    }

    #[test]
    fn test_merge_with_parent_preserves_child_from_field() {
        let parent = EvmNetworkConfig {
            common: NetworkConfigCommon {
                network: "parent".to_string(),
                from: Some("grandparent".to_string()),
                rpc_urls: Some(vec!["https://parent.example.com".to_string()]),
                explorer_urls: Some(vec!["https://parent-explorer.example.com".to_string()]),
                average_blocktime_ms: Some(10000),
                is_testnet: Some(true),
                tags: None,
            },
            chain_id: Some(1),
            required_confirmations: Some(6),
            features: None,
            symbol: Some("ETH".to_string()),
            gas_price_cache: Some(GasPriceCacheConfig {
                enabled: true,
                stale_after_ms: 20_000,
                expire_after_ms: 100_000,
            }),
        };

        let child = EvmNetworkConfig {
            common: NetworkConfigCommon {
                network: "child".to_string(),
                from: Some("parent".to_string()),
                rpc_urls: None,
                explorer_urls: None,
                average_blocktime_ms: None,
                is_testnet: None,
                tags: None,
            },
            chain_id: None,
            required_confirmations: None,
            features: None,
            symbol: None,
            gas_price_cache: None,
        };

        let result = child.merge_with_parent(&parent);

        // Child's 'from' field should be preserved, not inherited from parent
        assert_eq!(result.common.from, Some("parent".to_string()));
        assert_eq!(
            result.gas_price_cache,
            Some(GasPriceCacheConfig {
                enabled: true,
                stale_after_ms: 20_000,
                expire_after_ms: 100_000,
            })
        );
    }

    #[test]
    fn test_validate_with_unicode_symbol() {
        let mut config = create_evm_network("ethereum-mainnet");
        config.symbol = Some("Ξ".to_string()); // Greek Xi symbol for Ethereum

        let result = config.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_with_unicode_features() {
        let mut config = create_evm_network("ethereum-mainnet");
        config.features = Some(vec!["eip1559".to_string(), "测试功能".to_string()]);

        let result = config.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_merge_with_parent_with_empty_features() {
        let parent = EvmNetworkConfig {
            common: NetworkConfigCommon {
                network: "parent".to_string(),
                from: None,
                rpc_urls: Some(vec!["https://rpc.example.com".to_string()]),
                explorer_urls: Some(vec!["https://explorer.example.com".to_string()]),
                average_blocktime_ms: Some(12000),
                is_testnet: Some(false),
                tags: None,
            },
            chain_id: Some(1),
            required_confirmations: Some(12),
            features: Some(vec![]),
            symbol: Some("ETH".to_string()),
            gas_price_cache: Some(GasPriceCacheConfig {
                enabled: true,
                stale_after_ms: 20_000,
                expire_after_ms: 100_000,
            }),
        };

        let child = EvmNetworkConfig {
            common: NetworkConfigCommon {
                network: "child".to_string(),
                from: Some("parent".to_string()),
                rpc_urls: None,
                explorer_urls: None,
                average_blocktime_ms: None,
                is_testnet: None,
                tags: None,
            },
            chain_id: None,
            required_confirmations: None,
            features: Some(vec!["eip1559".to_string()]),
            symbol: None,
            gas_price_cache: None,
        };

        let result = child.merge_with_parent(&parent);

        // Should merge empty parent features with child features
        assert_eq!(result.features, Some(vec!["eip1559".to_string()]));
        assert_eq!(
            result.gas_price_cache,
            Some(GasPriceCacheConfig {
                enabled: true,
                stale_after_ms: 20_000,
                expire_after_ms: 100_000,
            })
        );
    }

    #[test]
    fn test_validate_with_very_large_confirmations() {
        let mut config = create_evm_network("ethereum-mainnet");
        config.required_confirmations = Some(u64::MAX);

        let result = config.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_merge_with_parent_identical_configs() {
        let config = create_evm_network("ethereum-mainnet");
        let result = config.merge_with_parent(&config);

        // Merging identical configs should result in the same config
        assert_eq!(result.common.network, config.common.network);
        assert_eq!(result.chain_id, config.chain_id);
        assert_eq!(result.required_confirmations, config.required_confirmations);
        assert_eq!(result.features, config.features);
        assert_eq!(result.symbol, config.symbol);
        assert_eq!(result.gas_price_cache, config.gas_price_cache);
    }

    #[test]
    fn test_validate_propagates_common_validation_errors() {
        let mut config = create_evm_network("ethereum-mainnet");
        config.common.rpc_urls = None; // This should cause common validation to fail

        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::MissingField(_)
        ));
    }

    #[test]
    fn test_gas_price_cache_validation_zero_stale_after() {
        let mut config = create_evm_network("ethereum-mainnet");
        config.gas_price_cache = Some(GasPriceCacheConfig {
            enabled: true,
            stale_after_ms: 0, // Invalid: zero value
            expire_after_ms: 45_000,
        });

        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::InvalidFormat(_)
        ));
    }

    #[test]
    fn test_gas_price_cache_validation_zero_expire_after() {
        let mut config = create_evm_network("ethereum-mainnet");
        config.gas_price_cache = Some(GasPriceCacheConfig {
            enabled: true,
            stale_after_ms: 20_000,
            expire_after_ms: 0, // Invalid: zero value
        });

        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::InvalidFormat(_)
        ));
    }

    #[test]
    fn test_gas_price_cache_validation_expire_less_than_stale() {
        let mut config = create_evm_network("ethereum-mainnet");
        config.gas_price_cache = Some(GasPriceCacheConfig {
            enabled: true,
            stale_after_ms: 45_000,
            expire_after_ms: 20_000, // Invalid: less than stale_after_ms
        });

        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::InvalidFormat(_)
        ));
    }

    #[test]
    fn test_gas_price_cache_validation_expire_equal_to_stale() {
        let mut config = create_evm_network("ethereum-mainnet");
        config.gas_price_cache = Some(GasPriceCacheConfig {
            enabled: true,
            stale_after_ms: 20_000,
            expire_after_ms: 20_000, // Invalid: equal to stale_after_ms
        });

        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::InvalidFormat(_)
        ));
    }

    #[test]
    fn test_gas_price_cache_validation_valid_config() {
        let mut config = create_evm_network("ethereum-mainnet");
        config.gas_price_cache = Some(GasPriceCacheConfig {
            enabled: true,
            stale_after_ms: 20_000,
            expire_after_ms: 45_000, // Valid: greater than stale_after_ms
        });

        let result = config.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_gas_price_cache_default_values() {
        let config = GasPriceCacheConfig::default();

        assert_eq!(config.enabled, false);
        assert_eq!(config.stale_after_ms, 20_000);
        assert_eq!(config.expire_after_ms, 45_000);

        // Validation should pass for default values
        assert!(config.validate().is_ok());
    }
}
