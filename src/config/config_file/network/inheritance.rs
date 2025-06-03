//! Network Configuration Inheritance Resolution
//!
//! This module provides inheritance resolution for network configurations, enabling
//! hierarchical configuration management where child networks inherit and override
//! properties from parent networks.
//!
//! ## Key Features
//!
//! - **Type safety**: Ensures inheritance only between compatible network types
//! - **Recursive resolution**: Supports multi-level inheritance chains
//! - **Smart merging**: Child values override parents, collections merge intelligently
//! - **Error handling**: Detailed errors for circular references and type mismatches
//!
//! ## Resolution Process
//!
//! 1. **Validation**: Verify parent exists and types are compatible
//! 2. **Recursive resolution**: Resolve parent's inheritance chain first
//! 3. **Merging**: Combine child with resolved parent configuration

use super::{
    ConfigFileNetworkType, EvmNetworkConfig, NetworkFileConfig, SolanaNetworkConfig,
    StellarNetworkConfig,
};
use crate::config::ConfigFileError;

/// Resolves network configuration inheritance by recursively merging child configurations with their parents.
pub struct InheritanceResolver<'a> {
    /// Function to lookup network configurations by name
    network_lookup: &'a dyn Fn(&str) -> Option<&'a NetworkFileConfig>,
}

/// Macro to generate inheritance resolution methods for different network types.
///
/// Generates: resolve_evm_inheritance, resolve_solana_inheritance, resolve_stellar_inheritance
/// This eliminates code duplication while maintaining type safety across all network types.
macro_rules! impl_inheritance_resolver {
    ($method_name:ident, $config_type:ty, $network_type:ident, $variant:ident, $type_name:expr) => {
        /// Resolves inheritance for network configurations by recursively merging with parent configurations.
        ///
        /// # Arguments
        /// * `config` - The child network configuration to resolve inheritance for
        /// * `network_name` - The name of the child network (used for error reporting)
        /// * `parent_name` - The name of the parent network to inherit from
        ///
        /// # Returns
        /// Configuration with all inheritance applied, or an error if resolution fails
        pub fn $method_name(&self, config: &$config_type, network_name: &str, parent_name: &str) -> Result<$config_type, ConfigFileError> {
            // Get the parent network
            let parent_network = (self.network_lookup)(parent_name).ok_or_else(|| {
                ConfigFileError::InvalidReference(format!(
                    "Network '{}' inherits from non-existent network '{}' in inheritance chain",
                    network_name, parent_name
                ))
            })?;

            // Verify parent is the same type
            if parent_network.network_type() != ConfigFileNetworkType::$network_type {
                return Err(ConfigFileError::IncompatibleInheritanceType(format!(
                    "Network '{}' (type {}) tries to inherit from '{}' (type {:?}) - inheritance chain broken due to type mismatch",
                    network_name, $type_name, parent_name, parent_network.network_type()
                )));
            }

            // Extract the parent configuration
            let parent_config = match parent_network {
                NetworkFileConfig::$variant(config) => config,
                _ => return Err(ConfigFileError::InvalidFormat(format!("Expected {} network configuration", $type_name))),
            };

            // Recursively resolve parent inheritance first
            let resolved_parent = if parent_network.inherits_from().is_some() {
                let grandparent_name = parent_network.inherits_from().unwrap();
                self.$method_name(parent_config, parent_name, grandparent_name)?
            } else {
                parent_config.clone()
            };

            // Merge child with resolved parent
            Ok(config.merge_with_parent(&resolved_parent))
        }
    };
}

impl<'a> InheritanceResolver<'a> {
    /// Creates a new inheritance resolver.
    ///
    /// # Arguments
    /// * `network_lookup` - Function to lookup network configurations by name
    ///
    /// # Returns
    /// A new `InheritanceResolver` instance
    pub fn new(network_lookup: &'a dyn Fn(&str) -> Option<&'a NetworkFileConfig>) -> Self {
        Self { network_lookup }
    }

    // Generate the three inheritance resolution methods using the macro
    impl_inheritance_resolver!(resolve_evm_inheritance, EvmNetworkConfig, Evm, Evm, "EVM");
    impl_inheritance_resolver!(
        resolve_solana_inheritance,
        SolanaNetworkConfig,
        Solana,
        Solana,
        "Solana"
    );
    impl_inheritance_resolver!(
        resolve_stellar_inheritance,
        StellarNetworkConfig,
        Stellar,
        Stellar,
        "Stellar"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::config_file::network::common::NetworkConfigCommon;
    use crate::config::config_file::network::test_utils::*;
    use std::collections::HashMap;

    #[test]
    fn test_inheritance_resolver_new() {
        let networks: HashMap<String, NetworkFileConfig> = HashMap::new();
        let lookup_fn = |name: &str| networks.get(name);
        let resolver = InheritanceResolver::new(&lookup_fn);

        // Test that the resolver was created successfully
        // We can't directly test the function pointer, but we can test that it works
        assert!((resolver.network_lookup)("nonexistent").is_none());
    }

    #[test]
    fn test_resolve_evm_inheritance_simple_success() {
        let mut networks = HashMap::new();

        // Create parent network
        let parent_config = create_evm_network("parent");
        networks.insert(
            "parent".to_string(),
            NetworkFileConfig::Evm(parent_config.clone()),
        );

        let lookup_fn = |name: &str| networks.get(name);
        let resolver = InheritanceResolver::new(&lookup_fn);

        // Create child network that inherits from parent
        let child_config = EvmNetworkConfig {
            common: NetworkConfigCommon {
                network: "child".to_string(),
                from: Some("parent".to_string()),
                rpc_urls: None,                    // Will inherit from parent
                explorer_urls: None,               // Will inherit from parent
                average_blocktime_ms: Some(15000), // Override parent value
                is_testnet: Some(false),           // Override parent value
                tags: None,
            },
            chain_id: None,                  // Will inherit from parent
            required_confirmations: Some(2), // Override parent value
            features: None,
            symbol: None, // Will inherit from parent
        };

        let result = resolver.resolve_evm_inheritance(&child_config, "child", "parent");
        assert!(result.is_ok());

        let resolved = result.unwrap();
        assert_eq!(resolved.common.network, "child");
        assert_eq!(resolved.common.rpc_urls, parent_config.common.rpc_urls); // Inherited
        assert_eq!(
            resolved.common.explorer_urls,
            parent_config.common.explorer_urls
        ); // Inherited
        assert_eq!(resolved.common.average_blocktime_ms, Some(15000)); // Overridden
        assert_eq!(resolved.common.is_testnet, Some(false)); // Overridden
        assert_eq!(resolved.chain_id, parent_config.chain_id); // Inherited
        assert_eq!(resolved.required_confirmations, Some(2)); // Overridden
        assert_eq!(resolved.symbol, parent_config.symbol); // Inherited
    }

    #[test]
    fn test_resolve_evm_inheritance_multi_level() {
        let mut networks = HashMap::new();

        // Create grandparent network
        let grandparent_config = create_evm_network("grandparent");
        networks.insert(
            "grandparent".to_string(),
            NetworkFileConfig::Evm(grandparent_config.clone()),
        );

        // Create parent network that inherits from grandparent
        let parent_config = EvmNetworkConfig {
            common: NetworkConfigCommon {
                network: "parent".to_string(),
                from: Some("grandparent".to_string()),
                rpc_urls: None,
                explorer_urls: None,
                average_blocktime_ms: Some(10000), // Override grandparent
                is_testnet: None,
                tags: None,
            },
            chain_id: None,
            required_confirmations: Some(3), // Override grandparent
            features: None,
            symbol: None,
        };
        networks.insert(
            "parent".to_string(),
            NetworkFileConfig::Evm(parent_config.clone()),
        );

        let lookup_fn = |name: &str| networks.get(name);
        let resolver = InheritanceResolver::new(&lookup_fn);

        // Create child network that inherits from parent
        let child_config = EvmNetworkConfig {
            common: NetworkConfigCommon {
                network: "child".to_string(),
                from: Some("parent".to_string()),
                rpc_urls: None,
                explorer_urls: None,
                average_blocktime_ms: None,
                is_testnet: Some(false), // Override
                tags: None,
            },
            chain_id: Some(42), // Override
            required_confirmations: None,
            features: None,
            symbol: None,
        };

        let result = resolver.resolve_evm_inheritance(&child_config, "child", "parent");
        assert!(result.is_ok());

        let resolved = result.unwrap();
        assert_eq!(resolved.common.network, "child");
        assert_eq!(resolved.common.rpc_urls, grandparent_config.common.rpc_urls); // From grandparent
        assert_eq!(
            resolved.common.explorer_urls,
            grandparent_config.common.explorer_urls
        ); // From grandparent
        assert_eq!(resolved.common.average_blocktime_ms, Some(10000)); // From parent
        assert_eq!(resolved.common.is_testnet, Some(false)); // From child
        assert_eq!(resolved.chain_id, Some(42)); // From child
        assert_eq!(resolved.required_confirmations, Some(3)); // From parent
        assert_eq!(resolved.symbol, grandparent_config.symbol); // From grandparent
    }

    #[test]
    fn test_resolve_evm_inheritance_nonexistent_parent() {
        let networks: HashMap<String, NetworkFileConfig> = HashMap::new();
        let lookup_fn = |name: &str| networks.get(name);
        let resolver = InheritanceResolver::new(&lookup_fn);

        let child_config = create_evm_network_with_parent("child", "nonexistent");

        let result = resolver.resolve_evm_inheritance(&child_config, "child", "nonexistent");
        assert!(result.is_err());

        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::InvalidReference(_)
        ));
    }

    #[test]
    fn test_resolve_evm_inheritance_type_mismatch() {
        let mut networks = HashMap::new();

        // Create a Solana parent network
        let parent_config = create_solana_network("parent");
        networks.insert(
            "parent".to_string(),
            NetworkFileConfig::Solana(parent_config),
        );

        let lookup_fn = |name: &str| networks.get(name);
        let resolver = InheritanceResolver::new(&lookup_fn);

        let child_config = create_evm_network_with_parent("child", "parent");

        let result = resolver.resolve_evm_inheritance(&child_config, "child", "parent");
        assert!(result.is_err());

        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::IncompatibleInheritanceType(_)
        ));
    }

    #[test]
    fn test_resolve_evm_inheritance_no_inheritance() {
        let mut networks = HashMap::new();

        // Create parent network with no inheritance
        let parent_config = create_evm_network("parent");
        networks.insert(
            "parent".to_string(),
            NetworkFileConfig::Evm(parent_config.clone()),
        );

        let lookup_fn = |name: &str| networks.get(name);
        let resolver = InheritanceResolver::new(&lookup_fn);

        let child_config = create_evm_network_with_parent("child", "parent");

        let result = resolver.resolve_evm_inheritance(&child_config, "child", "parent");
        assert!(result.is_ok());

        let resolved = result.unwrap();
        // Should merge child with parent (parent has no inheritance)
        assert_eq!(resolved.common.network, "child");
        assert_eq!(
            resolved.chain_id,
            child_config.chain_id.or(parent_config.chain_id)
        );
    }

    // Solana Inheritance Tests
    #[test]
    fn test_resolve_solana_inheritance_simple_success() {
        let mut networks = HashMap::new();

        let parent_config = create_solana_network("parent");
        networks.insert(
            "parent".to_string(),
            NetworkFileConfig::Solana(parent_config.clone()),
        );

        let lookup_fn = |name: &str| networks.get(name);
        let resolver = InheritanceResolver::new(&lookup_fn);

        let child_config = SolanaNetworkConfig {
            common: NetworkConfigCommon {
                network: "child".to_string(),
                from: Some("parent".to_string()),
                rpc_urls: None,                  // Will inherit
                explorer_urls: None,             // Will inherit
                average_blocktime_ms: Some(500), // Override
                is_testnet: None,
                tags: None,
            },
        };

        let result = resolver.resolve_solana_inheritance(&child_config, "child", "parent");
        assert!(result.is_ok());

        let resolved = result.unwrap();
        assert_eq!(resolved.common.network, "child");
        assert_eq!(resolved.common.rpc_urls, parent_config.common.rpc_urls); // Inherited
        assert_eq!(
            resolved.common.explorer_urls,
            parent_config.common.explorer_urls
        ); // Inherited
        assert_eq!(resolved.common.average_blocktime_ms, Some(500)); // Overridden
    }

    #[test]
    fn test_resolve_solana_inheritance_type_mismatch() {
        let mut networks = HashMap::new();

        // Create an EVM parent network
        let parent_config = create_evm_network("parent");
        networks.insert("parent".to_string(), NetworkFileConfig::Evm(parent_config));

        let lookup_fn = |name: &str| networks.get(name);
        let resolver = InheritanceResolver::new(&lookup_fn);

        let child_config = create_solana_network_with_parent("child", "parent");

        let result = resolver.resolve_solana_inheritance(&child_config, "child", "parent");
        assert!(result.is_err());

        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::IncompatibleInheritanceType(_)
        ));
    }

    #[test]
    fn test_resolve_solana_inheritance_nonexistent_parent() {
        let networks: HashMap<String, NetworkFileConfig> = HashMap::new();
        let lookup_fn = |name: &str| networks.get(name);
        let resolver = InheritanceResolver::new(&lookup_fn);

        let child_config = create_solana_network_with_parent("child", "nonexistent");

        let result = resolver.resolve_solana_inheritance(&child_config, "child", "nonexistent");
        assert!(result.is_err());

        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::InvalidReference(_)
        ));
    }

    #[test]
    fn test_resolve_stellar_inheritance_simple_success() {
        let mut networks = HashMap::new();

        let parent_config = create_stellar_network("parent");
        networks.insert(
            "parent".to_string(),
            NetworkFileConfig::Stellar(parent_config.clone()),
        );

        let lookup_fn = |name: &str| networks.get(name);
        let resolver = InheritanceResolver::new(&lookup_fn);

        let child_config = StellarNetworkConfig {
            common: NetworkConfigCommon {
                network: "child".to_string(),
                from: Some("parent".to_string()),
                rpc_urls: None,                   // Will inherit
                explorer_urls: None,              // Will inherit
                average_blocktime_ms: Some(6000), // Override
                is_testnet: None,
                tags: None,
            },
            passphrase: None, // Will inherit from parent
        };

        let result = resolver.resolve_stellar_inheritance(&child_config, "child", "parent");
        assert!(result.is_ok());

        let resolved = result.unwrap();
        assert_eq!(resolved.common.network, "child");
        assert_eq!(resolved.common.rpc_urls, parent_config.common.rpc_urls); // Inherited
        assert_eq!(resolved.common.average_blocktime_ms, Some(6000)); // Overridden
        assert_eq!(resolved.passphrase, parent_config.passphrase); // Inherited
    }

    #[test]
    fn test_resolve_stellar_inheritance_type_mismatch() {
        let mut networks = HashMap::new();

        // Create a Solana parent network
        let parent_config = create_solana_network("parent");
        networks.insert(
            "parent".to_string(),
            NetworkFileConfig::Solana(parent_config),
        );

        let lookup_fn = |name: &str| networks.get(name);
        let resolver = InheritanceResolver::new(&lookup_fn);

        let child_config = create_stellar_network_with_parent("child", "parent");

        let result = resolver.resolve_stellar_inheritance(&child_config, "child", "parent");
        assert!(result.is_err());

        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::IncompatibleInheritanceType(_)
        ));
    }

    #[test]
    fn test_resolve_stellar_inheritance_nonexistent_parent() {
        let networks: HashMap<String, NetworkFileConfig> = HashMap::new();
        let lookup_fn = |name: &str| networks.get(name);
        let resolver = InheritanceResolver::new(&lookup_fn);

        let child_config = create_stellar_network_with_parent("child", "nonexistent");

        let result = resolver.resolve_stellar_inheritance(&child_config, "child", "nonexistent");
        assert!(result.is_err());

        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::InvalidReference(_)
        ));
    }

    #[test]
    fn test_resolve_inheritance_deep_chain() {
        let mut networks = HashMap::new();

        // Create a 4-level inheritance chain: great-grandparent -> grandparent -> parent -> child
        let great_grandparent_config = create_evm_network("great-grandparent");
        networks.insert(
            "great-grandparent".to_string(),
            NetworkFileConfig::Evm(great_grandparent_config.clone()),
        );

        let grandparent_config = EvmNetworkConfig {
            common: NetworkConfigCommon {
                network: "grandparent".to_string(),
                from: Some("great-grandparent".to_string()),
                rpc_urls: None,
                explorer_urls: None,
                average_blocktime_ms: Some(11000),
                is_testnet: None,
                tags: None,
            },
            chain_id: None,
            required_confirmations: None,
            features: Some(vec!["eip1559".to_string(), "london".to_string()]),
            symbol: None,
        };
        networks.insert(
            "grandparent".to_string(),
            NetworkFileConfig::Evm(grandparent_config),
        );

        let parent_config = EvmNetworkConfig {
            common: NetworkConfigCommon {
                network: "parent".to_string(),
                from: Some("grandparent".to_string()),
                rpc_urls: None,
                explorer_urls: None,
                average_blocktime_ms: None,
                is_testnet: Some(false),
                tags: Some(vec!["production".to_string()]),
            },
            chain_id: Some(100),
            required_confirmations: None,
            features: None,
            symbol: None,
        };
        networks.insert("parent".to_string(), NetworkFileConfig::Evm(parent_config));

        let lookup_fn = |name: &str| networks.get(name);
        let resolver = InheritanceResolver::new(&lookup_fn);

        let child_config = EvmNetworkConfig {
            common: NetworkConfigCommon {
                network: "child".to_string(),
                from: Some("parent".to_string()),
                rpc_urls: Some(vec!["https://custom-rpc.example.com".to_string()]),
                explorer_urls: Some(vec!["https://custom-explorer.example.com".to_string()]),
                average_blocktime_ms: None,
                is_testnet: None,
                tags: None,
            },
            chain_id: None,
            required_confirmations: Some(5),
            features: None,
            symbol: Some("CUSTOM".to_string()),
        };

        let result = resolver.resolve_evm_inheritance(&child_config, "child", "parent");
        assert!(result.is_ok());

        let resolved = result.unwrap();
        assert_eq!(resolved.common.network, "child");
        assert_eq!(
            resolved.common.rpc_urls,
            Some(vec!["https://custom-rpc.example.com".to_string()])
        ); // From child
        assert_eq!(
            resolved.common.explorer_urls,
            Some(vec!["https://custom-explorer.example.com".to_string()])
        ); // From child
        assert_eq!(resolved.common.average_blocktime_ms, Some(11000)); // From grandparent
        assert_eq!(resolved.common.is_testnet, Some(false)); // From parent
        assert_eq!(
            resolved.common.tags,
            Some(vec!["test".to_string(), "production".to_string()])
        ); // Merged from great-grandparent and parent
        assert_eq!(resolved.chain_id, Some(100)); // From parent
        assert_eq!(resolved.required_confirmations, Some(5)); // From child
        assert_eq!(resolved.symbol, Some("CUSTOM".to_string())); // From child
        assert_eq!(
            resolved.features,
            Some(vec!["eip1559".to_string(), "london".to_string()])
        );
    }

    #[test]
    fn test_resolve_inheritance_with_empty_network_name() {
        let mut networks = HashMap::new();
        let parent_config = create_evm_network("parent");
        networks.insert("parent".to_string(), NetworkFileConfig::Evm(parent_config));

        let lookup_fn = |name: &str| networks.get(name);
        let resolver = InheritanceResolver::new(&lookup_fn);

        let child_config = create_evm_network_with_parent("child", "parent");

        // Test with empty network name - this should succeed since parent exists
        let result = resolver.resolve_evm_inheritance(&child_config, "", "parent");
        assert!(result.is_ok());

        // The resolved config should have the child's network name (empty string in this case)
        let resolved = result.unwrap();
        assert_eq!(resolved.common.network, "child"); // Network name comes from the child config, not the parameter
    }

    #[test]
    fn test_resolve_inheritance_with_empty_parent_name() {
        let networks: HashMap<String, NetworkFileConfig> = HashMap::new();
        let lookup_fn = |name: &str| networks.get(name);
        let resolver = InheritanceResolver::new(&lookup_fn);

        let child_config = create_evm_network_with_parent("child", "");

        // Test with empty parent name
        let result = resolver.resolve_evm_inheritance(&child_config, "child", "");
        assert!(result.is_err());

        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::InvalidReference(_)
        ));
    }

    #[test]
    fn test_all_network_types_coverage() {
        let mut networks = HashMap::new();

        // Create parent networks for all types
        let evm_parent = create_evm_network("evm-parent");
        let solana_parent = create_solana_network("solana-parent");
        let stellar_parent = create_stellar_network("stellar-parent");

        networks.insert("evm-parent".to_string(), NetworkFileConfig::Evm(evm_parent));
        networks.insert(
            "solana-parent".to_string(),
            NetworkFileConfig::Solana(solana_parent),
        );
        networks.insert(
            "stellar-parent".to_string(),
            NetworkFileConfig::Stellar(stellar_parent),
        );

        let lookup_fn = |name: &str| networks.get(name);
        let resolver = InheritanceResolver::new(&lookup_fn);

        // Test EVM inheritance
        let evm_child = create_evm_network_with_parent("evm-child", "evm-parent");
        let evm_result = resolver.resolve_evm_inheritance(&evm_child, "evm-child", "evm-parent");
        assert!(evm_result.is_ok());

        // Test Solana inheritance
        let solana_child = create_solana_network_with_parent("solana-child", "solana-parent");
        let solana_result =
            resolver.resolve_solana_inheritance(&solana_child, "solana-child", "solana-parent");
        assert!(solana_result.is_ok());

        // Test Stellar inheritance
        let stellar_child = create_stellar_network_with_parent("stellar-child", "stellar-parent");
        let stellar_result =
            resolver.resolve_stellar_inheritance(&stellar_child, "stellar-child", "stellar-parent");
        assert!(stellar_result.is_ok());
    }

    #[test]
    fn test_inheritance_with_complex_merging() {
        let mut networks = HashMap::new();

        // Create parent with comprehensive configuration
        let parent_config = EvmNetworkConfig {
            common: NetworkConfigCommon {
                network: "parent".to_string(),
                from: None,
                rpc_urls: Some(vec![
                    "https://parent-rpc1.example.com".to_string(),
                    "https://parent-rpc2.example.com".to_string(),
                ]),
                explorer_urls: Some(vec![
                    "https://parent-explorer1.example.com".to_string(),
                    "https://parent-explorer2.example.com".to_string(),
                ]),
                average_blocktime_ms: Some(12000),
                is_testnet: Some(true),
                tags: Some(vec!["parent-tag1".to_string(), "parent-tag2".to_string()]),
            },
            chain_id: Some(1),
            required_confirmations: Some(1),
            features: Some(vec!["eip1559".to_string(), "london".to_string()]),
            symbol: Some("ETH".to_string()),
        };
        networks.insert("parent".to_string(), NetworkFileConfig::Evm(parent_config));

        let lookup_fn = |name: &str| networks.get(name);
        let resolver = InheritanceResolver::new(&lookup_fn);

        // Create child that partially overrides parent
        let child_config = EvmNetworkConfig {
            common: NetworkConfigCommon {
                network: "child".to_string(),
                from: Some("parent".to_string()),
                rpc_urls: Some(vec!["https://child-rpc.example.com".to_string()]), // Override
                explorer_urls: Some(vec!["https://child-explorer.example.com".to_string()]), // Override
                average_blocktime_ms: None,                // Inherit
                is_testnet: Some(false),                   // Override
                tags: Some(vec!["child-tag".to_string()]), // Override (merge behavior depends on implementation)
            },
            chain_id: Some(42),                         // Override
            required_confirmations: None,               // Inherit
            features: Some(vec!["berlin".to_string()]), // Override (merge behavior depends on implementation)
            symbol: None,                               // Inherit
        };

        let result = resolver.resolve_evm_inheritance(&child_config, "child", "parent");
        assert!(result.is_ok());

        let resolved = result.unwrap();
        assert_eq!(resolved.common.network, "child");
        assert_eq!(
            resolved.common.rpc_urls,
            Some(vec!["https://child-rpc.example.com".to_string()])
        ); // Child override
        assert_eq!(
            resolved.common.explorer_urls,
            Some(vec!["https://child-explorer.example.com".to_string()])
        ); // Child override
        assert_eq!(resolved.common.average_blocktime_ms, Some(12000)); // Inherited from parent
        assert_eq!(resolved.common.is_testnet, Some(false)); // Child override
        assert_eq!(resolved.chain_id, Some(42)); // Child override
        assert_eq!(resolved.required_confirmations, Some(1)); // Inherited from parent
        assert_eq!(resolved.symbol, Some("ETH".to_string())); // Inherited from parent
    }
}
