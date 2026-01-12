//! Common Network Configuration Components
//!
//! This module defines shared configuration structures and utilities common across
//! all network types (EVM, Solana, Stellar) with inheritance and merging support.
//!
//! ## Key Features
//!
//! - **Inheritance support**: Child networks inherit from parents with override capability
//! - **Smart merging**: Collections merge preserving unique items, primitives override
//! - **Validation**: Required field checks and URL format validation

use crate::config::ConfigFileError;
use crate::models::{deserialize_rpc_urls, RpcConfig};
use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Serialize, Clone)]
pub struct NetworkConfigCommon {
    /// Unique network identifier (e.g., "mainnet", "sepolia", "custom-devnet").
    pub network: String,
    /// Optional name of an existing network to inherit configuration from.
    /// If set, this network will use the `from` network's settings as a base,
    /// overriding specific fields as needed.
    pub from: Option<String>,
    /// List of RPC endpoint configurations for connecting to the network.
    /// Supports both simple format (array of strings) and extended format (array of RpcConfig objects).
    #[serde(deserialize_with = "deserialize_rpc_urls")]
    pub rpc_urls: Option<Vec<RpcConfig>>,
    /// List of Explorer endpoint URLs for connecting to the network.
    pub explorer_urls: Option<Vec<String>>,
    /// Estimated average time between blocks in milliseconds.
    pub average_blocktime_ms: Option<u64>,
    /// Flag indicating if the network is a testnet.
    pub is_testnet: Option<bool>,
    /// List of arbitrary tags for categorizing or filtering networks.
    pub tags: Option<Vec<String>>,
}

impl<'de> Deserialize<'de> for NetworkConfigCommon {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct NetworkConfigCommonHelper {
            network: String,
            from: Option<String>,
            #[serde(deserialize_with = "deserialize_rpc_urls")]
            rpc_urls: Option<Vec<RpcConfig>>,
            explorer_urls: Option<Vec<String>>,
            average_blocktime_ms: Option<u64>,
            is_testnet: Option<bool>,
            tags: Option<Vec<String>>,
        }

        let helper = NetworkConfigCommonHelper::deserialize(deserializer)?;
        Ok(NetworkConfigCommon {
            network: helper.network,
            from: helper.from,
            rpc_urls: helper.rpc_urls,
            explorer_urls: helper.explorer_urls,
            average_blocktime_ms: helper.average_blocktime_ms,
            is_testnet: helper.is_testnet,
            tags: helper.tags,
        })
    }
}

impl NetworkConfigCommon {
    /// Validates the common fields for a network configuration.
    ///
    /// # Returns
    /// - `Ok(())` if common fields are valid.
    /// - `Err(ConfigFileError)` if validation fails.
    pub fn validate(&self) -> Result<(), ConfigFileError> {
        // Validate network name
        if self.network.is_empty() {
            return Err(ConfigFileError::MissingField("network name".into()));
        }

        // If this is a base network (not inheriting), validate required fields
        if self.from.is_none() {
            // RPC URLs are required for base networks
            if self.rpc_urls.is_none() || self.rpc_urls.as_ref().unwrap().is_empty() {
                return Err(ConfigFileError::MissingField("rpc_urls".into()));
            }
        }

        // Validate RPC URLs format if provided
        if let Some(configs) = &self.rpc_urls {
            for config in configs {
                reqwest::Url::parse(&config.url).map_err(|_| {
                    ConfigFileError::InvalidFormat(format!("Invalid RPC URL: {}", config.url))
                })?;
            }
        }

        if let Some(urls) = &self.explorer_urls {
            for url in urls {
                reqwest::Url::parse(url).map_err(|_| {
                    ConfigFileError::InvalidFormat(format!("Invalid Explorer URL: {url}"))
                })?;
            }
        }

        Ok(())
    }

    /// Creates a new configuration by merging this config with a parent, where child values override parent defaults.
    ///
    /// # Arguments
    /// * `parent` - The parent configuration to merge with.
    ///
    /// # Returns
    /// A new `NetworkConfigCommon` with merged values where child takes precedence over parent.
    /// For RPC URLs: if child has RPC URLs, they completely override parent's. If child has no RPC URLs, parent's are inherited.
    pub fn merge_with_parent(&self, parent: &Self) -> Self {
        Self {
            network: self.network.clone(),
            from: self.from.clone(),
            rpc_urls: merge_optional_rpc_config_vecs(&self.rpc_urls, &parent.rpc_urls),
            explorer_urls: self
                .explorer_urls
                .clone()
                .or_else(|| parent.explorer_urls.clone()),
            average_blocktime_ms: self.average_blocktime_ms.or(parent.average_blocktime_ms),
            is_testnet: self.is_testnet.or(parent.is_testnet),
            tags: merge_tags(&self.tags, &parent.tags),
        }
    }
}

/// Combines child and parent RPC config vectors.
///
/// Behavior:
/// - If child has RPC configs: Use child's configs (allows weight specification for child URLs).
/// - If child has no RPC configs: Use parent's configs (inheritance).
///
/// # Arguments
/// * `child` - Optional vector of child RPC configs.
/// * `parent` - Optional vector of parent RPC configs.
///
/// # Returns
/// An optional vector containing child's RPC configs, or parent's if child has none, or `None` if both inputs are `None`.
pub fn merge_optional_rpc_config_vecs(
    child: &Option<Vec<RpcConfig>>,
    parent: &Option<Vec<RpcConfig>>,
) -> Option<Vec<RpcConfig>> {
    match (child, parent) {
        (Some(child), _) => Some(child.clone()), // Child overrides parent
        (None, Some(parent)) => Some(parent.clone()), // Inherit from parent
        (None, None) => None,
    }
}

/// Combines child and parent string vectors, preserving all unique items with child items taking precedence.
///
/// # Arguments
/// * `child` - Optional vector of child items.
/// * `parent` - Optional vector of parent items.
///
/// # Returns
/// An optional vector containing all unique items from both sources, or `None` if both inputs are `None`.
pub fn merge_optional_string_vecs(
    child: &Option<Vec<String>>,
    parent: &Option<Vec<String>>,
) -> Option<Vec<String>> {
    match (child, parent) {
        (Some(child), Some(parent)) => {
            let mut merged = parent.clone();
            for item in child {
                if !merged.contains(item) {
                    merged.push(item.clone());
                }
            }
            Some(merged)
        }
        (Some(items), None) => Some(items.clone()),
        (None, Some(items)) => Some(items.clone()),
        (None, None) => None,
    }
}

/// Combines child and parent tag vectors, preserving all unique tags with child tags taking precedence.
///
/// # Arguments
/// * `child_tags` - Optional vector of child tags.
/// * `parent_tags` - Optional vector of parent tags.
///
/// # Returns
/// An optional vector containing all unique tags from both sources, or `None` if both inputs are `None`.
fn merge_tags(
    child_tags: &Option<Vec<String>>,
    parent_tags: &Option<Vec<String>>,
) -> Option<Vec<String>> {
    merge_optional_string_vecs(child_tags, parent_tags)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::config_file::network::test_utils::*;

    #[test]
    fn test_validate_success_base_network() {
        let config = create_network_common("test-network");
        let result = config.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_success_inheriting_network() {
        let config = create_network_common_with_parent("child-network", "parent-network");
        let result = config.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_empty_network_name() {
        let mut config = create_network_common("test-network");
        config.network = String::new();

        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::MissingField(_)
        ));
    }

    #[test]
    fn test_validate_base_network_missing_rpc_urls() {
        let mut config = create_network_common("test-network");
        config.rpc_urls = None;

        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::MissingField(_)
        ));
    }

    #[test]
    fn test_validate_base_network_empty_rpc_urls() {
        let mut config = create_network_common("test-network");
        config.rpc_urls = Some(vec![]);

        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::MissingField(_)
        ));
    }

    #[test]
    fn test_validate_invalid_rpc_url_format() {
        use crate::models::RpcConfig;
        let mut config = create_network_common("test-network");
        config.rpc_urls = Some(vec![RpcConfig::new("invalid-url".to_string())]);

        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::InvalidFormat(_)
        ));
    }

    #[test]
    fn test_validate_multiple_invalid_rpc_urls() {
        use crate::models::RpcConfig;
        let mut config = create_network_common("test-network");
        config.rpc_urls = Some(vec![
            RpcConfig::new("https://valid.example.com".to_string()),
            RpcConfig::new("invalid-url".to_string()),
            RpcConfig::new("also-invalid".to_string()),
        ]);

        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::InvalidFormat(_)
        ));
    }

    #[test]
    fn test_validate_various_valid_rpc_url_formats() {
        use crate::models::RpcConfig;
        let mut config = create_network_common("test-network");
        config.rpc_urls = Some(vec![
            RpcConfig::new("https://mainnet.infura.io/v3/key".to_string()),
            RpcConfig::new("http://localhost:8545".to_string()),
            RpcConfig::new("wss://ws.example.com".to_string()),
            RpcConfig::new("https://rpc.example.com:8080/path".to_string()),
        ]);

        let result = config.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_inheriting_network_with_rpc_urls() {
        use crate::models::RpcConfig;
        let mut config = create_network_common_with_parent("child-network", "parent-network");
        config.rpc_urls = Some(vec![RpcConfig::new(
            "https://override.example.com".to_string(),
        )]);

        let result = config.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_inheriting_network_with_invalid_rpc_urls() {
        use crate::models::RpcConfig;
        let mut config = create_network_common_with_parent("child-network", "parent-network");
        config.rpc_urls = Some(vec![RpcConfig::new("invalid-url".to_string())]);

        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::InvalidFormat(_)
        ));
    }

    #[test]
    fn test_merge_with_parent_child_overrides() {
        use crate::models::RpcConfig;
        let parent = NetworkConfigCommon {
            network: "parent".to_string(),
            from: None,
            rpc_urls: Some(vec![RpcConfig::new(
                "https://parent-rpc.example.com".to_string(),
            )]),
            explorer_urls: Some(vec!["https://parent-explorer.example.com".to_string()]),
            average_blocktime_ms: Some(10000),
            is_testnet: Some(true),
            tags: Some(vec!["parent-tag".to_string()]),
        };

        let child = NetworkConfigCommon {
            network: "child".to_string(),
            from: Some("parent".to_string()),
            rpc_urls: Some(vec![RpcConfig::new(
                "https://child-rpc.example.com".to_string(),
            )]),
            explorer_urls: Some(vec!["https://child-explorer.example.com".to_string()]),
            average_blocktime_ms: Some(15000),
            is_testnet: Some(false),
            tags: Some(vec!["child-tag".to_string()]),
        };

        let result = child.merge_with_parent(&parent);

        assert_eq!(result.network, "child");
        assert_eq!(result.from, Some("parent".to_string()));
        // Child's RPC URLs override parent's
        assert_eq!(
            result.rpc_urls,
            Some(vec![RpcConfig::new(
                "https://child-rpc.example.com".to_string()
            )])
        );
        assert_eq!(result.average_blocktime_ms, Some(15000));
        assert_eq!(result.is_testnet, Some(false));
        assert_eq!(
            result.tags,
            Some(vec!["parent-tag".to_string(), "child-tag".to_string()])
        );
    }

    #[test]
    fn test_merge_with_parent_child_inherits() {
        use crate::models::RpcConfig;
        let parent = NetworkConfigCommon {
            network: "parent".to_string(),
            from: None,
            rpc_urls: Some(vec![RpcConfig::new(
                "https://parent-rpc.example.com".to_string(),
            )]),
            explorer_urls: Some(vec!["https://parent-explorer.example.com".to_string()]),
            average_blocktime_ms: Some(10000),
            is_testnet: Some(true),
            tags: Some(vec!["parent-tag".to_string()]),
        };

        let child = NetworkConfigCommon {
            network: "child".to_string(),
            from: Some("parent".to_string()),
            rpc_urls: None,             // Will inherit
            explorer_urls: None,        // Will inherit
            average_blocktime_ms: None, // Will inherit
            is_testnet: None,           // Will inherit
            tags: None,                 // Will inherit
        };

        let result = child.merge_with_parent(&parent);

        assert_eq!(result.network, "child");
        assert_eq!(result.from, Some("parent".to_string()));
        assert_eq!(
            result.rpc_urls,
            Some(vec![RpcConfig::new(
                "https://parent-rpc.example.com".to_string()
            )])
        );
        assert_eq!(
            result.explorer_urls,
            Some(vec!["https://parent-explorer.example.com".to_string()])
        );
        assert_eq!(result.average_blocktime_ms, Some(10000));
        assert_eq!(result.is_testnet, Some(true));
        assert_eq!(result.tags, Some(vec!["parent-tag".to_string()]));
    }

    #[test]
    fn test_merge_with_parent_mixed_inheritance() {
        use crate::models::RpcConfig;
        let parent = NetworkConfigCommon {
            network: "parent".to_string(),
            from: None,
            rpc_urls: Some(vec![RpcConfig::new(
                "https://parent-rpc.example.com".to_string(),
            )]),
            explorer_urls: Some(vec!["https://parent-explorer.example.com".to_string()]),
            average_blocktime_ms: Some(10000),
            is_testnet: Some(true),
            tags: Some(vec!["parent-tag1".to_string(), "parent-tag2".to_string()]),
        };

        let child = NetworkConfigCommon {
            network: "child".to_string(),
            from: Some("parent".to_string()),
            rpc_urls: Some(vec![RpcConfig::new(
                "https://child-rpc.example.com".to_string(),
            )]), // Override
            explorer_urls: Some(vec!["https://child-explorer.example.com".to_string()]), // Override
            average_blocktime_ms: None,                                                  // Inherit
            is_testnet: Some(false),                                                     // Override
            tags: Some(vec!["child-tag".to_string()]),                                   // Merge
        };

        let result = child.merge_with_parent(&parent);

        assert_eq!(result.network, "child");
        // Child's RPC URLs override parent's (complete override)
        assert_eq!(
            result.rpc_urls,
            Some(vec![RpcConfig::new(
                "https://child-rpc.example.com".to_string()
            )])
        );
        assert_eq!(
            result.explorer_urls,
            Some(vec!["https://child-explorer.example.com".to_string()])
        );
        assert_eq!(result.average_blocktime_ms, Some(10000)); // Inherited
        assert_eq!(result.is_testnet, Some(false)); // Overridden
        assert_eq!(
            result.tags,
            Some(vec![
                "parent-tag1".to_string(),
                "parent-tag2".to_string(),
                "child-tag".to_string()
            ])
        );
    }

    #[test]
    fn test_merge_with_parent_both_empty() {
        let parent = NetworkConfigCommon {
            network: "parent".to_string(),
            from: None,
            rpc_urls: None,
            explorer_urls: None,
            average_blocktime_ms: None,
            is_testnet: None,
            tags: None,
        };

        let child = NetworkConfigCommon {
            network: "child".to_string(),
            from: Some("parent".to_string()),
            rpc_urls: None,
            explorer_urls: None,
            average_blocktime_ms: None,
            is_testnet: None,
            tags: None,
        };

        let result = child.merge_with_parent(&parent);

        assert_eq!(result.network, "child");
        assert_eq!(result.from, Some("parent".to_string()));
        assert_eq!(result.rpc_urls, None);
        assert_eq!(result.explorer_urls, None);
        assert_eq!(result.average_blocktime_ms, None);
        assert_eq!(result.is_testnet, None);
        assert_eq!(result.tags, None);
    }

    #[test]
    fn test_merge_with_parent_complex_tag_merging() {
        use crate::models::RpcConfig;
        let parent = NetworkConfigCommon {
            network: "parent".to_string(),
            from: None,
            rpc_urls: Some(vec![RpcConfig::new("https://rpc.example.com".to_string())]),
            explorer_urls: Some(vec!["https://explorer.example.com".to_string()]),
            average_blocktime_ms: Some(12000),
            is_testnet: Some(true),
            tags: Some(vec![
                "production".to_string(),
                "mainnet".to_string(),
                "shared".to_string(),
            ]),
        };

        let child = NetworkConfigCommon {
            network: "child".to_string(),
            from: Some("parent".to_string()),
            rpc_urls: None,
            explorer_urls: None,
            average_blocktime_ms: None,
            is_testnet: None,
            tags: Some(vec![
                "shared".to_string(),
                "custom".to_string(),
                "override".to_string(),
            ]),
        };

        let result = child.merge_with_parent(&parent);

        // Tags should be merged with parent first, then unique child tags added
        let expected_tags = vec![
            "production".to_string(),
            "mainnet".to_string(),
            "shared".to_string(), // Duplicate should not be added again
            "custom".to_string(),
            "override".to_string(),
        ];
        assert_eq!(result.tags, Some(expected_tags));
    }

    #[test]
    fn test_merge_optional_string_vecs_both_some() {
        let child = Some(vec!["child1".to_string(), "child2".to_string()]);
        let parent = Some(vec!["parent1".to_string(), "parent2".to_string()]);
        let result = merge_optional_string_vecs(&child, &parent);
        assert_eq!(
            result,
            Some(vec![
                "parent1".to_string(),
                "parent2".to_string(),
                "child1".to_string(),
                "child2".to_string()
            ])
        );
    }

    #[test]
    fn test_merge_optional_string_vecs_child_some_parent_none() {
        let child = Some(vec!["child1".to_string()]);
        let parent = None;
        let result = merge_optional_string_vecs(&child, &parent);
        assert_eq!(result, Some(vec!["child1".to_string()]));
    }

    #[test]
    fn test_merge_optional_string_vecs_child_none_parent_some() {
        let child = None;
        let parent = Some(vec!["parent1".to_string()]);
        let result = merge_optional_string_vecs(&child, &parent);
        assert_eq!(result, Some(vec!["parent1".to_string()]));
    }

    #[test]
    fn test_merge_optional_string_vecs_both_none() {
        let child = None;
        let parent = None;
        let result = merge_optional_string_vecs(&child, &parent);
        assert_eq!(result, None);
    }

    #[test]
    fn test_merge_optional_string_vecs_duplicate_handling() {
        // Test duplicate handling
        let child = Some(vec!["duplicate".to_string(), "child1".to_string()]);
        let parent = Some(vec!["duplicate".to_string(), "parent1".to_string()]);
        let result = merge_optional_string_vecs(&child, &parent);
        assert_eq!(
            result,
            Some(vec![
                "duplicate".to_string(),
                "parent1".to_string(),
                "child1".to_string()
            ])
        );
    }

    #[test]
    fn test_merge_optional_string_vecs_empty_vectors() {
        // Test empty child vector
        let child = Some(vec![]);
        let parent = Some(vec!["parent1".to_string()]);
        let result = merge_optional_string_vecs(&child, &parent);
        assert_eq!(result, Some(vec!["parent1".to_string()]));

        // Test empty parent vector
        let child = Some(vec!["child1".to_string()]);
        let parent = Some(vec![]);
        let result = merge_optional_string_vecs(&child, &parent);
        assert_eq!(result, Some(vec!["child1".to_string()]));

        // Test both empty vectors
        let child = Some(vec![]);
        let parent = Some(vec![]);
        let result = merge_optional_string_vecs(&child, &parent);
        assert_eq!(result, Some(vec![]));
    }

    #[test]
    fn test_merge_optional_string_vecs_multiple_duplicates() {
        let child = Some(vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "a".to_string(),
        ]);
        let parent = Some(vec!["b".to_string(), "d".to_string(), "a".to_string()]);
        let result = merge_optional_string_vecs(&child, &parent);

        // Should preserve parent order, then add unique child items
        let expected = vec![
            "b".to_string(),
            "d".to_string(),
            "a".to_string(),
            "c".to_string(),
        ];
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn test_merge_optional_string_vecs_single_item_vectors() {
        let child = Some(vec!["child".to_string()]);
        let parent = Some(vec!["parent".to_string()]);
        let result = merge_optional_string_vecs(&child, &parent);
        assert_eq!(
            result,
            Some(vec!["parent".to_string(), "child".to_string()])
        );
    }

    #[test]
    fn test_merge_optional_string_vecs_identical_vectors() {
        let child = Some(vec!["same1".to_string(), "same2".to_string()]);
        let parent = Some(vec!["same1".to_string(), "same2".to_string()]);
        let result = merge_optional_string_vecs(&child, &parent);
        assert_eq!(result, Some(vec!["same1".to_string(), "same2".to_string()]));
    }

    // Edge Cases and Integration Tests
    #[test]
    fn test_network_config_common_clone() {
        let config = create_network_common("test-network");
        let cloned = config.clone();

        assert_eq!(config.network, cloned.network);
        assert_eq!(config.from, cloned.from);
        assert_eq!(config.rpc_urls, cloned.rpc_urls);
        assert_eq!(config.average_blocktime_ms, cloned.average_blocktime_ms);
        assert_eq!(config.is_testnet, cloned.is_testnet);
        assert_eq!(config.tags, cloned.tags);
    }

    #[test]
    fn test_network_config_common_debug() {
        let config = create_network_common("test-network");
        let debug_str = format!("{:?}", config);

        assert!(debug_str.contains("NetworkConfigCommon"));
        assert!(debug_str.contains("test-network"));
    }

    #[test]
    fn test_validate_with_unicode_network_name() {
        let mut config = create_network_common("test-network");
        config.network = "测试网络".to_string();

        let result = config.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_with_unicode_rpc_urls() {
        use crate::models::RpcConfig;
        let mut config = create_network_common("test-network");
        config.rpc_urls = Some(vec![RpcConfig::new("https://测试.example.com".to_string())]);

        let result = config.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_merge_with_parent_preserves_child_network_name() {
        use crate::models::RpcConfig;
        let parent = NetworkConfigCommon {
            network: "parent-name".to_string(),
            from: None,
            rpc_urls: Some(vec![RpcConfig::new(
                "https://parent.example.com".to_string(),
            )]),
            explorer_urls: Some(vec!["https://parent.example.com".to_string()]),
            average_blocktime_ms: Some(10000),
            is_testnet: Some(true),
            tags: None,
        };

        let child = NetworkConfigCommon {
            network: "child-name".to_string(),
            from: Some("parent-name".to_string()),
            rpc_urls: None,
            explorer_urls: None,
            average_blocktime_ms: None,
            is_testnet: None,
            tags: None,
        };

        let result = child.merge_with_parent(&parent);

        // Child network name should always be preserved
        assert_eq!(result.network, "child-name");
        assert_eq!(result.from, Some("parent-name".to_string()));
    }

    #[test]
    fn test_merge_with_parent_preserves_child_from_field() {
        use crate::models::RpcConfig;
        let parent = NetworkConfigCommon {
            network: "parent".to_string(),
            from: Some("grandparent".to_string()),
            rpc_urls: Some(vec![RpcConfig::new(
                "https://parent.example.com".to_string(),
            )]),
            explorer_urls: Some(vec!["https://parent.example.com".to_string()]),
            average_blocktime_ms: Some(10000),
            is_testnet: Some(true),
            tags: None,
        };

        let child = NetworkConfigCommon {
            network: "child".to_string(),
            from: Some("parent".to_string()),
            rpc_urls: None,
            explorer_urls: None,
            average_blocktime_ms: None,
            is_testnet: None,
            tags: None,
        };

        let result = child.merge_with_parent(&parent);

        // Child's 'from' field should be preserved, not inherited from parent
        assert_eq!(result.from, Some("parent".to_string()));
    }

    #[test]
    fn test_deserialize_simple_string_array_format() {
        // Test that simple format (array of strings) is correctly converted to RpcConfig
        let json = r#"{
            "network": "test-network",
            "rpc_urls": ["https://rpc1.example.com", "https://rpc2.example.com"]
        }"#;

        let config: NetworkConfigCommon = serde_json::from_str(json).unwrap();
        assert!(config.rpc_urls.is_some());
        let rpc_configs = config.rpc_urls.unwrap();
        assert_eq!(rpc_configs.len(), 2);
        assert_eq!(rpc_configs[0].url, "https://rpc1.example.com");
        assert_eq!(rpc_configs[0].weight, crate::constants::DEFAULT_RPC_WEIGHT);
        assert_eq!(rpc_configs[1].url, "https://rpc2.example.com");
        assert_eq!(rpc_configs[1].weight, crate::constants::DEFAULT_RPC_WEIGHT);
    }

    #[test]
    fn test_deserialize_extended_object_array_format() {
        // Test that extended format (array of RpcConfig objects) works correctly
        let json = r#"{
            "network": "test-network",
            "rpc_urls": [
                {"url": "https://rpc1.example.com", "weight": 50},
                {"url": "https://rpc2.example.com", "weight": 100}
            ]
        }"#;

        let config: NetworkConfigCommon = serde_json::from_str(json).unwrap();
        assert!(config.rpc_urls.is_some());
        let rpc_configs = config.rpc_urls.unwrap();
        assert_eq!(rpc_configs.len(), 2);
        assert_eq!(rpc_configs[0].url, "https://rpc1.example.com");
        assert_eq!(rpc_configs[0].weight, 50);
        assert_eq!(rpc_configs[1].url, "https://rpc2.example.com");
        assert_eq!(rpc_configs[1].weight, 100);
    }

    #[test]
    fn test_deserialize_object_array_with_default_weight() {
        // Test that RpcConfig objects without weight get default weight
        let json = r#"{
            "network": "test-network",
            "rpc_urls": [
                {"url": "https://rpc1.example.com"}
            ]
        }"#;

        let config: NetworkConfigCommon = serde_json::from_str(json).unwrap();
        assert!(config.rpc_urls.is_some());
        let rpc_configs = config.rpc_urls.unwrap();
        assert_eq!(rpc_configs.len(), 1);
        assert_eq!(rpc_configs[0].url, "https://rpc1.example.com");
        assert_eq!(rpc_configs[0].weight, crate::constants::DEFAULT_RPC_WEIGHT);
    }

    #[test]
    fn test_serialize_preserves_weights() {
        // Test that serialization preserves weights
        use crate::models::RpcConfig;
        let config = NetworkConfigCommon {
            network: "test-network".to_string(),
            from: None,
            rpc_urls: Some(vec![
                RpcConfig::with_weight("https://rpc1.example.com".to_string(), 50).unwrap(),
                RpcConfig::new("https://rpc2.example.com".to_string()),
            ]),
            explorer_urls: None,
            average_blocktime_ms: None,
            is_testnet: None,
            tags: None,
        };

        let serialized = serde_json::to_string(&config).unwrap();
        let deserialized: NetworkConfigCommon = serde_json::from_str(&serialized).unwrap();

        assert!(deserialized.rpc_urls.is_some());
        let rpc_configs = deserialized.rpc_urls.unwrap();
        assert_eq!(rpc_configs.len(), 2);
        assert_eq!(rpc_configs[0].url, "https://rpc1.example.com");
        assert_eq!(rpc_configs[0].weight, 50);
        assert_eq!(rpc_configs[1].url, "https://rpc2.example.com");
        assert_eq!(rpc_configs[1].weight, crate::constants::DEFAULT_RPC_WEIGHT);
    }

    #[test]
    fn test_roundtrip_simple_to_extended_format() {
        // Test that simple format can be read and then serialized in extended format
        let simple_json = r#"{
            "network": "test-network",
            "rpc_urls": ["https://rpc1.example.com", "https://rpc2.example.com"]
        }"#;

        let config: NetworkConfigCommon = serde_json::from_str(simple_json).unwrap();
        let serialized = serde_json::to_string(&config).unwrap();

        // The serialized version should be in extended format (with weights)
        assert!(serialized.contains("\"url\""));
        assert!(serialized.contains("\"weight\""));

        // Deserialize again to verify it still works
        let deserialized: NetworkConfigCommon = serde_json::from_str(&serialized).unwrap();
        assert!(deserialized.rpc_urls.is_some());
        let rpc_configs = deserialized.rpc_urls.unwrap();
        assert_eq!(rpc_configs.len(), 2);
        assert_eq!(rpc_configs[0].url, "https://rpc1.example.com");
        assert_eq!(rpc_configs[1].url, "https://rpc2.example.com");
    }

    #[test]
    fn test_merge_rpc_configs_override_behavior() {
        // Test that child RPC configs completely override parent configs
        use crate::models::RpcConfig;
        let parent = NetworkConfigCommon {
            network: "parent".to_string(),
            from: None,
            rpc_urls: Some(vec![
                RpcConfig::with_weight("https://rpc1.example.com".to_string(), 100).unwrap(),
                RpcConfig::with_weight("https://rpc2.example.com".to_string(), 100).unwrap(),
            ]),
            explorer_urls: None,
            average_blocktime_ms: None,
            is_testnet: None,
            tags: None,
        };

        let child = NetworkConfigCommon {
            network: "child".to_string(),
            from: Some("parent".to_string()),
            // Child completely overrides parent's RPC URLs
            rpc_urls: Some(vec![
                RpcConfig::with_weight("https://child-rpc1.example.com".to_string(), 50).unwrap(),
                RpcConfig::with_weight("https://child-rpc2.example.com".to_string(), 75).unwrap(),
            ]),
            explorer_urls: None,
            average_blocktime_ms: None,
            is_testnet: None,
            tags: None,
        };

        let result = child.merge_with_parent(&parent);

        // Should have only child's RPC configs (complete override)
        assert!(result.rpc_urls.is_some());
        let rpc_configs = result.rpc_urls.unwrap();
        assert_eq!(rpc_configs.len(), 2);

        // Find each config by URL
        let child_rpc1 = rpc_configs
            .iter()
            .find(|c| c.url == "https://child-rpc1.example.com")
            .unwrap();
        let child_rpc2 = rpc_configs
            .iter()
            .find(|c| c.url == "https://child-rpc2.example.com")
            .unwrap();

        // Should have child's weights
        assert_eq!(child_rpc1.weight, 50);
        assert_eq!(child_rpc2.weight, 75);

        // Should not have any parent URLs
        assert!(rpc_configs
            .iter()
            .find(|c| c.url == "https://rpc1.example.com")
            .is_none());
        assert!(rpc_configs
            .iter()
            .find(|c| c.url == "https://rpc2.example.com")
            .is_none());
    }
}
