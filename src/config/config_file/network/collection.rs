//! Network Configuration Collection Management
//!
//! This module provides collection management for multiple network configurations with
//! inheritance resolution, validation, and flexible loading from JSON arrays or directories.
//!
//! ## Core Features
//!
//! - **Multi-network support**: Manages EVM, Solana, and Stellar networks in a single collection
//! - **Inheritance resolution**: Resolves complex inheritance hierarchies with type safety
//! - **Flexible loading**: Supports JSON arrays and directory-based configuration sources
//! - **Validation**: Comprehensive validation with detailed error reporting

use super::{InheritanceResolver, NetworkFileConfig, NetworkFileLoader, NetworksSource};
use crate::config::config_file::ConfigFileNetworkType;
use crate::config::ConfigFileError;
use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ops::Index;

/// Represents the complete configuration for all defined networks.
///
/// This structure holds configurations loaded from a file or a directory of files
/// and provides methods to validate and process them, including resolving inheritance.
#[derive(Debug, Default, Serialize, Clone)]
pub struct NetworksFileConfig {
    pub networks: Vec<NetworkFileConfig>,
    #[serde(skip)]
    network_map: HashMap<(ConfigFileNetworkType, String), usize>,
}

/// Custom deserialization logic for `NetworksFileConfig`.
///
/// This allows `NetworksFileConfig` to be created from either a direct list of network
/// configurations, a path string pointing to a directory of configuration files, or null/missing
/// for the default path ("./config/networks").
impl<'de> Deserialize<'de> for NetworksFileConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Use Option to handle missing fields gracefully
        let source_option: Option<NetworksSource> = Option::deserialize(deserializer)?;
        let source = source_option.unwrap_or_default();

        let final_networks =
            NetworkFileLoader::load_from_source(source).map_err(de::Error::custom)?;

        // Check if networks is empty and return error
        if final_networks.is_empty() {
            return Err(de::Error::custom(
                "NetworksFileConfig cannot be empty - networks must contain at least one network configuration"
            ));
        }

        // First, create an instance with unflattened networks.
        // This will perform initial validations like duplicate name checks.
        let unflattened_config = NetworksFileConfig::new(final_networks).map_err(|e| {
            de::Error::custom(format!(
                "Error creating initial NetworksFileConfig: {:?}",
                e
            ))
        })?;

        // Now, flatten the configuration. (Resolve inheritance)
        unflattened_config
            .flatten()
            .map_err(|e| de::Error::custom(format!("Error flattening NetworksFileConfig: {:?}", e)))
    }
}

impl NetworksFileConfig {
    /// Creates a new `NetworksFileConfig` instance from a vector of network configurations.
    ///
    /// # Returns
    /// - `Ok(Self)` if all network names are unique within their respective types and the instance is successfully created.
    /// - `Err(ConfigFileError)` if duplicate network names are found within the same network type.
    pub fn new(networks: Vec<NetworkFileConfig>) -> Result<Self, ConfigFileError> {
        let mut network_map = HashMap::new();

        // Build the network map for efficient lookups
        for (index, network) in networks.iter().enumerate() {
            let name = network.network_name();
            let network_type = network.network_type();
            let key = (network_type, name.to_string());

            if network_map.insert(key, index).is_some() {
                // Return an error if we find a duplicate within the same network type
                return Err(ConfigFileError::DuplicateId(format!(
                    "{:?} network '{}'",
                    network_type, name
                )));
            }
        }

        let instance = Self {
            networks,
            network_map,
        };

        // Check inheritance references and types
        for network in &instance.networks {
            if network.inherits_from().is_some() {
                instance.trace_inheritance(network.network_name(), network.network_type())?;
            }
        }

        Ok(instance)
    }

    /// Retrieves a network configuration by its network type and name.
    ///
    /// # Arguments
    /// * `network_type` - The type of the network to retrieve.
    /// * `name` - The name of the network to retrieve.
    ///
    /// # Returns
    /// - `Some(&NetworkFileConfig)` if a network with the given type and name exists.
    /// - `None` if no network with the given type and name is found.
    pub fn get_network(
        &self,
        network_type: ConfigFileNetworkType,
        name: &str,
    ) -> Option<&NetworkFileConfig> {
        let key = (network_type, name.to_string());
        self.network_map
            .get(&key)
            .map(|&index| &self.networks[index])
    }

    /// Builds a new set of networks with all inheritance chains resolved and flattened.
    ///
    /// This method processes all networks and their inheritance relationships to produce
    /// a set of fully expanded network configurations where each network includes all properties
    /// from its parent networks, with any overrides applied.
    ///
    /// # Returns
    /// - `Result<NetworksFileConfig, ConfigFileError>` containing either the flattened configuration
    ///   or an error if any inheritance issues are encountered.
    pub fn flatten(&self) -> Result<NetworksFileConfig, ConfigFileError> {
        // Process each network to resolve inheritance
        let resolved_networks = self
            .networks
            .iter()
            .map(|network| self.resolve_inheritance(network))
            .collect::<Result<Vec<NetworkFileConfig>, ConfigFileError>>()?;

        NetworksFileConfig::new(resolved_networks)
    }

    /// Creates a fully resolved network configuration by merging properties from its inheritance chain.
    ///
    /// # Arguments
    /// * `network` - A reference to the `NetworkFileConfig` to resolve.
    ///
    /// # Returns
    /// - `Ok(NetworkFileConfig)` containing the fully resolved network configuration.
    /// - `Err(ConfigFileError)` if any issues are encountered during inheritance resolution.
    fn resolve_inheritance(
        &self,
        network: &NetworkFileConfig,
    ) -> Result<NetworkFileConfig, ConfigFileError> {
        // If no inheritance, return a clone of the original
        if network.inherits_from().is_none() {
            return Ok(network.clone());
        }

        let parent_name = network.inherits_from().unwrap();
        let network_name = network.network_name();
        let network_type = network.network_type();

        // Create the inheritance resolver with a lookup function that uses network type
        let lookup_fn = move |name: &str| self.get_network(network_type, name);
        let resolver = InheritanceResolver::new(&lookup_fn);

        match network {
            NetworkFileConfig::Evm(config) => {
                let resolved_config =
                    resolver.resolve_evm_inheritance(config, network_name, parent_name)?;
                Ok(NetworkFileConfig::Evm(resolved_config))
            }
            NetworkFileConfig::Solana(config) => {
                let resolved_config =
                    resolver.resolve_solana_inheritance(config, network_name, parent_name)?;
                Ok(NetworkFileConfig::Solana(resolved_config))
            }
            NetworkFileConfig::Stellar(config) => {
                let resolved_config =
                    resolver.resolve_stellar_inheritance(config, network_name, parent_name)?;
                Ok(NetworkFileConfig::Stellar(resolved_config))
            }
        }
    }

    /// Validates the entire networks configuration structure.
    ///
    /// # Returns
    /// - `Ok(())` if the entire configuration is valid.
    /// - `Err(ConfigFileError)` if any validation fails (duplicate names, invalid inheritance,
    ///   incompatible inheritance types, or errors from individual network validations).
    pub fn validate(&self) -> Result<(), ConfigFileError> {
        for network in &self.networks {
            network.validate()?;
        }
        Ok(())
    }

    /// Traces the inheritance path for a given network to check for cycles or invalid references.
    ///
    /// # Arguments
    /// - `start_network_name` - The name of the network to trace inheritance for.
    /// - `network_type` - The type of the network to trace inheritance for.
    ///
    /// # Returns
    /// - `Ok(())` if the inheritance chain is valid.
    /// - `Err(ConfigFileError)` if a cycle or invalid reference is detected.
    fn trace_inheritance(
        &self,
        start_network_name: &str,
        network_type: ConfigFileNetworkType,
    ) -> Result<(), ConfigFileError> {
        let mut current_path_names = Vec::new();
        let mut current_name = start_network_name;

        loop {
            // Check cycle first
            if current_path_names.contains(&current_name) {
                let cycle_path_str = current_path_names.join(" -> ");
                return Err(ConfigFileError::CircularInheritance(format!(
                    "Circular inheritance detected: {} -> {}",
                    cycle_path_str, current_name
                )));
            }

            current_path_names.push(current_name);

            let current_network =
                self.get_network(network_type, current_name)
                    .ok_or_else(|| {
                        ConfigFileError::InvalidReference(format!(
                            "{:?} network '{}' not found in configuration",
                            network_type, current_name
                        ))
                    })?;

            if let Some(source_name) = current_network.inherits_from() {
                let derived_type = current_network.network_type();

                if source_name == current_name {
                    return Err(ConfigFileError::InvalidReference(format!(
                        "Network '{}' cannot inherit from itself",
                        current_name
                    )));
                }

                let source_network =
                    self.get_network(network_type, source_name).ok_or_else(|| {
                        ConfigFileError::InvalidReference(format!(
                            "{:?} network '{}' inherits from non-existent network '{}'",
                            network_type, current_name, source_name
                        ))
                    })?;

                let source_type = source_network.network_type();

                if derived_type != source_type {
                    return Err(ConfigFileError::IncompatibleInheritanceType(format!(
                        "Network '{}' (type {:?}) tries to inherit from '{}' (type {:?})",
                        current_name, derived_type, source_name, source_type
                    )));
                }
                current_name = source_name;
            } else {
                break;
            }
        }

        Ok(())
    }

    /// Returns an iterator over all networks.
    pub fn iter(&self) -> impl Iterator<Item = &NetworkFileConfig> {
        self.networks.iter()
    }

    /// Returns the number of networks in the configuration.
    pub fn len(&self) -> usize {
        self.networks.len()
    }

    /// Returns true if there are no networks in the configuration.
    pub fn is_empty(&self) -> bool {
        self.networks.is_empty()
    }

    /// Filters networks by type.
    pub fn networks_by_type(
        &self,
        network_type: crate::config::config_file::ConfigFileNetworkType,
    ) -> impl Iterator<Item = &NetworkFileConfig> {
        self.networks
            .iter()
            .filter(move |network| network.network_type() == network_type)
    }

    /// Gets all network names.
    pub fn network_names(&self) -> impl Iterator<Item = &str> {
        self.networks.iter().map(|network| network.network_name())
    }

    /// Returns the first network in the configuration.
    ///
    /// # Returns
    /// - `Some(&NetworkFileConfig)` if there is at least one network.
    /// - `None` if the configuration is empty.
    pub fn first(&self) -> Option<&NetworkFileConfig> {
        self.networks.first()
    }

    /// Returns a reference to the network at the given index.
    ///
    /// # Arguments
    /// * `index` - The index of the network to retrieve.
    ///
    /// # Returns
    /// - `Some(&NetworkFileConfig)` if a network exists at the given index.
    /// - `None` if the index is out of bounds.
    pub fn get(&self, index: usize) -> Option<&NetworkFileConfig> {
        self.networks.get(index)
    }
}

// Implementation of Index trait for array-like access (config[0])
impl Index<usize> for NetworksFileConfig {
    type Output = NetworkFileConfig;

    fn index(&self, index: usize) -> &Self::Output {
        &self.networks[index]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::config_file::network::test_utils::*;
    use crate::config::config_file::ConfigFileNetworkType;
    use std::fs::File;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn test_new_with_single_network() {
        let networks = vec![create_evm_network_wrapped("test-evm")];
        let config = NetworksFileConfig::new(networks);

        assert!(config.is_ok());
        let config = config.unwrap();
        assert_eq!(config.networks.len(), 1);
        assert_eq!(config.network_map.len(), 1);
        assert!(config
            .network_map
            .contains_key(&(ConfigFileNetworkType::Evm, "test-evm".to_string())));
    }

    #[test]
    fn test_new_with_multiple_networks() {
        let networks = vec![
            create_evm_network_wrapped("evm-1"),
            create_solana_network_wrapped("solana-1"),
            create_stellar_network_wrapped("stellar-1"),
        ];
        let config = NetworksFileConfig::new(networks);

        assert!(config.is_ok());
        let config = config.unwrap();
        assert_eq!(config.networks.len(), 3);
        assert_eq!(config.network_map.len(), 3);
        assert!(config
            .network_map
            .contains_key(&(ConfigFileNetworkType::Evm, "evm-1".to_string())));
        assert!(config
            .network_map
            .contains_key(&(ConfigFileNetworkType::Solana, "solana-1".to_string())));
        assert!(config
            .network_map
            .contains_key(&(ConfigFileNetworkType::Stellar, "stellar-1".to_string())));
    }

    #[test]
    fn test_new_with_empty_networks() {
        let networks = vec![];
        let config = NetworksFileConfig::new(networks);

        assert!(config.is_ok());
        let config = config.unwrap();
        assert_eq!(config.networks.len(), 0);
        assert_eq!(config.network_map.len(), 0);
    }

    #[test]
    fn test_new_with_valid_inheritance() {
        let networks = vec![
            create_evm_network_wrapped("parent"),
            create_evm_network_wrapped_with_parent("child", "parent"),
        ];
        let config = NetworksFileConfig::new(networks);

        assert!(config.is_ok());
        let config = config.unwrap();
        assert_eq!(config.networks.len(), 2);
    }

    #[test]
    fn test_new_with_invalid_inheritance_reference() {
        let networks = vec![create_evm_network_wrapped_with_parent(
            "child",
            "non-existent",
        )];
        let result = NetworksFileConfig::new(networks);

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::InvalidReference(_)
        ));
    }

    #[test]
    fn test_new_with_self_inheritance() {
        let networks = vec![create_evm_network_wrapped_with_parent(
            "self-ref", "self-ref",
        )];
        let result = NetworksFileConfig::new(networks);

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::InvalidReference(_)
        ));
    }

    #[test]
    fn test_new_with_circular_inheritance() {
        let networks = vec![
            create_evm_network_wrapped_with_parent("a", "b"),
            create_evm_network_wrapped_with_parent("b", "c"),
            create_evm_network_wrapped_with_parent("c", "a"),
        ];
        let result = NetworksFileConfig::new(networks);

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::CircularInheritance(_)
        ));
    }

    #[test]
    fn test_new_with_incompatible_inheritance_types() {
        let networks = vec![
            create_evm_network_wrapped("evm-parent"),
            create_solana_network_wrapped_with_parent("solana-child", "evm-parent"),
        ];
        let result = NetworksFileConfig::new(networks);

        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(matches!(error, ConfigFileError::InvalidReference(_)));
    }

    #[test]
    fn test_new_with_deep_inheritance_chain() {
        let networks = vec![
            create_evm_network_wrapped("root"),
            create_evm_network_wrapped_with_parent("level1", "root"),
            create_evm_network_wrapped_with_parent("level2", "level1"),
            create_evm_network_wrapped_with_parent("level3", "level2"),
        ];
        let config = NetworksFileConfig::new(networks);

        assert!(config.is_ok());
        let config = config.unwrap();
        assert_eq!(config.networks.len(), 4);
    }

    #[test]
    fn test_get_network_existing() {
        let networks = vec![
            create_evm_network_wrapped("test-evm"),
            create_solana_network_wrapped("test-solana"),
        ];
        let config = NetworksFileConfig::new(networks).unwrap();

        let network = config.get_network(ConfigFileNetworkType::Evm, "test-evm");
        assert!(network.is_some());
        assert_eq!(network.unwrap().network_name(), "test-evm");

        let network = config.get_network(ConfigFileNetworkType::Solana, "test-solana");
        assert!(network.is_some());
        assert_eq!(network.unwrap().network_name(), "test-solana");
    }

    #[test]
    fn test_get_network_non_existent() {
        let networks = vec![create_evm_network_wrapped("test-evm")];
        let config = NetworksFileConfig::new(networks).unwrap();

        let network = config.get_network(ConfigFileNetworkType::Evm, "non-existent");
        assert!(network.is_none());
    }

    #[test]
    fn test_get_network_empty_config() {
        let config = NetworksFileConfig::new(vec![]).unwrap();

        let network = config.get_network(ConfigFileNetworkType::Evm, "any-name");
        assert!(network.is_none());
    }

    #[test]
    fn test_get_network_case_sensitive() {
        let networks = vec![create_evm_network_wrapped("Test-Network")];
        let config = NetworksFileConfig::new(networks).unwrap();

        assert!(config
            .get_network(ConfigFileNetworkType::Evm, "Test-Network")
            .is_some());
        assert!(config
            .get_network(ConfigFileNetworkType::Evm, "test-network")
            .is_none());
        assert!(config
            .get_network(ConfigFileNetworkType::Evm, "TEST-NETWORK")
            .is_none());
    }

    #[test]
    fn test_validate_success() {
        let networks = vec![
            create_evm_network_wrapped("evm-1"),
            create_solana_network_wrapped("solana-1"),
        ];
        let config = NetworksFileConfig::new(networks).unwrap();

        let result = config.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_with_invalid_network() {
        let networks = vec![
            create_evm_network_wrapped("valid"),
            create_invalid_evm_network_wrapped("invalid"),
        ];
        let config = NetworksFileConfig::new(networks).unwrap();

        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::MissingField(_)
        ));
    }

    #[test]
    fn test_validate_empty_config() {
        let config = NetworksFileConfig::new(vec![]).unwrap();

        let result = config.validate();
        assert!(result.is_ok()); // Empty config is valid for validation
    }

    #[test]
    fn test_flatten_without_inheritance() {
        let networks = vec![
            create_evm_network_wrapped("evm-1"),
            create_solana_network_wrapped("solana-1"),
        ];
        let config = NetworksFileConfig::new(networks).unwrap();

        let flattened = config.flatten();
        assert!(flattened.is_ok());
        let flattened = flattened.unwrap();
        assert_eq!(flattened.networks.len(), 2);
    }

    #[test]
    fn test_flatten_with_simple_inheritance() {
        let networks = vec![
            create_evm_network_wrapped("parent"),
            create_evm_network_wrapped_with_parent("child", "parent"),
        ];
        let config = NetworksFileConfig::new(networks).unwrap();

        let flattened = config.flatten();
        assert!(flattened.is_ok());
        let flattened = flattened.unwrap();
        assert_eq!(flattened.networks.len(), 2);

        // Child should still exist with inheritance information preserved
        let child = flattened.get_network(ConfigFileNetworkType::Evm, "child");
        assert!(child.is_some());
        // The from field is preserved to show inheritance source, but inheritance is resolved
        assert_eq!(child.unwrap().inherits_from(), Some("parent"));
    }

    #[test]
    fn test_flatten_with_multi_level_inheritance() {
        let networks = vec![
            create_evm_network_wrapped("root"),
            create_evm_network_wrapped_with_parent("middle", "root"),
            create_evm_network_wrapped_with_parent("leaf", "middle"),
        ];
        let config = NetworksFileConfig::new(networks).unwrap();

        let flattened = config.flatten();
        assert!(flattened.is_ok());
        let flattened = flattened.unwrap();
        assert_eq!(flattened.networks.len(), 3);
    }

    #[test]
    fn test_validation_after_flatten_with_failure() {
        let networks = vec![
            create_evm_network_wrapped("valid"),
            create_invalid_evm_network_wrapped("invalid"),
        ];
        let config = NetworksFileConfig::new(networks).unwrap();

        let flattened = config.flatten();
        assert!(flattened.is_ok());
        let flattened = flattened.unwrap();

        let result = flattened.validate();

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::MissingField(_)
        ));
    }

    #[test]
    fn test_flatten_with_mixed_network_types() {
        let networks = vec![
            create_evm_network_wrapped("evm-parent"),
            create_evm_network_wrapped_with_parent("evm-child", "evm-parent"),
            create_solana_network_wrapped("solana-parent"),
            create_solana_network_wrapped_with_parent("solana-child", "solana-parent"),
            create_stellar_network_wrapped("stellar-standalone"),
        ];
        let config = NetworksFileConfig::new(networks).unwrap();

        let flattened = config.flatten();
        assert!(flattened.is_ok());
        let flattened = flattened.unwrap();
        assert_eq!(flattened.networks.len(), 5);
    }

    #[test]
    fn test_iter() {
        let networks = vec![
            create_evm_network_wrapped("evm-1"),
            create_solana_network_wrapped("solana-1"),
        ];
        let config = NetworksFileConfig::new(networks).unwrap();

        let collected: Vec<_> = config.iter().collect();
        assert_eq!(collected.len(), 2);
        assert_eq!(collected[0].network_name(), "evm-1");
        assert_eq!(collected[1].network_name(), "solana-1");
    }

    #[test]
    fn test_len() {
        let config = NetworksFileConfig::new(vec![]).unwrap();
        assert_eq!(config.len(), 0);

        let networks = vec![create_evm_network_wrapped("test")];
        let config = NetworksFileConfig::new(networks).unwrap();
        assert_eq!(config.len(), 1);

        let networks = vec![
            create_evm_network_wrapped("test1"),
            create_solana_network_wrapped("test2"),
            create_stellar_network_wrapped("test3"),
        ];
        let config = NetworksFileConfig::new(networks).unwrap();
        assert_eq!(config.len(), 3);
    }

    #[test]
    fn test_is_empty() {
        let config = NetworksFileConfig::new(vec![]).unwrap();
        assert!(config.is_empty());

        let networks = vec![create_evm_network_wrapped("test")];
        let config = NetworksFileConfig::new(networks).unwrap();
        assert!(!config.is_empty());
    }

    #[test]
    fn test_networks_by_type() {
        let networks = vec![
            create_evm_network_wrapped("evm-1"),
            create_evm_network_wrapped("evm-2"),
            create_solana_network_wrapped("solana-1"),
            create_stellar_network_wrapped("stellar-1"),
        ];
        let config = NetworksFileConfig::new(networks).unwrap();

        let evm_networks: Vec<_> = config
            .networks_by_type(ConfigFileNetworkType::Evm)
            .collect();
        assert_eq!(evm_networks.len(), 2);

        let solana_networks: Vec<_> = config
            .networks_by_type(ConfigFileNetworkType::Solana)
            .collect();
        assert_eq!(solana_networks.len(), 1);

        let stellar_networks: Vec<_> = config
            .networks_by_type(ConfigFileNetworkType::Stellar)
            .collect();
        assert_eq!(stellar_networks.len(), 1);
    }

    #[test]
    fn test_networks_by_type_empty_result() {
        let networks = vec![create_evm_network_wrapped("evm-only")];
        let config = NetworksFileConfig::new(networks).unwrap();

        let solana_networks: Vec<_> = config
            .networks_by_type(ConfigFileNetworkType::Solana)
            .collect();
        assert_eq!(solana_networks.len(), 0);
    }

    #[test]
    fn test_network_names() {
        let networks = vec![
            create_evm_network_wrapped("alpha"),
            create_solana_network_wrapped("beta"),
            create_stellar_network_wrapped("gamma"),
        ];
        let config = NetworksFileConfig::new(networks).unwrap();

        let names: Vec<_> = config.network_names().collect();
        assert_eq!(names.len(), 3);
        assert!(names.contains(&"alpha"));
        assert!(names.contains(&"beta"));
        assert!(names.contains(&"gamma"));
    }

    #[test]
    fn test_network_names_empty() {
        let config = NetworksFileConfig::new(vec![]).unwrap();

        let names: Vec<_> = config.network_names().collect();
        assert_eq!(names.len(), 0);
    }

    // Tests for Default implementation
    #[test]
    fn test_default() {
        let config = NetworksFileConfig::default();

        assert_eq!(config.networks.len(), 0);
        assert_eq!(config.network_map.len(), 0);
        assert!(config.is_empty());
    }

    #[test]
    fn test_deserialize_from_array() {
        let json = r#"[
            {
                "type": "evm",
                "network": "test-evm",
                "chain_id": 31337,
                "required_confirmations": 1,
                "symbol": "ETH",
                "rpc_urls": ["https://rpc.test.example.com"]
            }
        ]"#;

        let result: Result<NetworksFileConfig, _> = serde_json::from_str(json);
        assert!(result.is_ok());
        let config = result.unwrap();
        assert_eq!(config.len(), 1);
        assert!(config
            .get_network(ConfigFileNetworkType::Evm, "test-evm")
            .is_some());
    }

    #[test]
    fn test_deserialize_empty_array_returns_error() {
        let json = r#"[]"#;
        let result: Result<NetworksFileConfig, _> = serde_json::from_str(json);

        assert!(result.is_err());
        let error_message = result.unwrap_err().to_string();
        assert!(error_message.contains("NetworksFileConfig cannot be empty"));
    }

    #[test]
    fn test_deserialize_from_directory() {
        let dir = tempdir().expect("Failed to create temp dir");
        let network_dir_path = dir.path();

        // Create test network files
        let evm_file = network_dir_path.join("evm.json");
        let mut file = File::create(&evm_file).expect("Failed to create EVM file");
        writeln!(file, r#"{{"networks": [{{"type": "evm", "network": "test-evm-from-file", "chain_id": 31337, "required_confirmations": 1, "symbol": "ETH", "rpc_urls": ["https://rpc.test.example.com"]}}]}}"#).expect("Failed to write EVM file");

        let solana_file = network_dir_path.join("solana.json");
        let mut file = File::create(&solana_file).expect("Failed to create Solana file");
        writeln!(file, r#"{{"networks": [{{"type": "solana", "network": "test-solana-from-file", "rpc_urls": ["https://rpc.solana.example.com"]}}]}}"#).expect("Failed to write Solana file");

        let json = format!(r#""{}""#, network_dir_path.to_str().unwrap());

        let result: Result<NetworksFileConfig, _> = serde_json::from_str(&json);
        assert!(result.is_ok());
        let config = result.unwrap();
        assert_eq!(config.len(), 2);
        assert!(config
            .get_network(ConfigFileNetworkType::Evm, "test-evm-from-file")
            .is_some());
        assert!(config
            .get_network(ConfigFileNetworkType::Solana, "test-solana-from-file")
            .is_some());
    }

    #[test]
    fn test_deserialize_invalid_directory() {
        let json = r#""/non/existent/directory""#;

        let result: Result<NetworksFileConfig, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_deserialize_with_inheritance_resolution() {
        let json = r#"[
            {
                "type": "evm",
                "network": "parent",
                "chain_id": 31337,
                "required_confirmations": 1,
                "symbol": "ETH",
                "rpc_urls": ["https://rpc.parent.example.com"]
            },
            {
                "type": "evm",
                "network": "child",
                "from": "parent",
                "chain_id": 31338,
                "required_confirmations": 1,
                "symbol": "ETH"
            }
        ]"#;

        let result: Result<NetworksFileConfig, _> = serde_json::from_str(json);
        assert!(result.is_ok());
        let config = result.unwrap();
        assert_eq!(config.len(), 2);

        // After deserialization, inheritance should be resolved but from field preserved
        let child = config
            .get_network(ConfigFileNetworkType::Evm, "child")
            .unwrap();
        assert_eq!(child.inherits_from(), Some("parent")); // From field preserved

        // Verify that child has inherited properties from parent
        if let NetworkFileConfig::Evm(child_evm) = child {
            assert!(child_evm.common.rpc_urls.is_some()); // Should have inherited RPC URLs
            assert_eq!(child_evm.chain_id, Some(31338)); // Should have overridden chain_id
        }
    }

    #[test]
    fn test_deserialize_with_invalid_inheritance() {
        let json = r#"[
            {
                "type": "evm",
                "network": "child",
                "from": "non-existent-parent",
                "chain_id": 31337,
                "required_confirmations": 1,
                "symbol": "ETH",
                "rpc_urls": ["https://rpc.test.example.com"]
            }
        ]"#;

        let result: Result<NetworksFileConfig, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    // Edge cases and stress tests
    #[test]
    fn test_large_number_of_networks() {
        let mut networks = Vec::new();
        for i in 0..100 {
            networks.push(create_evm_network_wrapped(&format!("network-{}", i)));
        }

        let config = NetworksFileConfig::new(networks);
        assert!(config.is_ok());
        let config = config.unwrap();
        assert_eq!(config.len(), 100);

        // Test that all networks are accessible
        for i in 0..100 {
            assert!(config
                .get_network(ConfigFileNetworkType::Evm, &format!("network-{}", i))
                .is_some());
        }
    }

    #[test]
    fn test_unicode_network_names() {
        let networks = vec![
            create_evm_network_wrapped("测试网络"),
            create_solana_network_wrapped("тестовая-сеть"),
            create_stellar_network_wrapped("réseau-test"),
        ];

        let config = NetworksFileConfig::new(networks);
        assert!(config.is_ok());
        let config = config.unwrap();
        assert_eq!(config.len(), 3);
        assert!(config
            .get_network(ConfigFileNetworkType::Evm, "测试网络")
            .is_some());
        assert!(config
            .get_network(ConfigFileNetworkType::Solana, "тестовая-сеть")
            .is_some());
        assert!(config
            .get_network(ConfigFileNetworkType::Stellar, "réseau-test")
            .is_some());
    }

    #[test]
    fn test_special_characters_in_network_names() {
        let networks = vec![
            create_evm_network_wrapped("test-network_123"),
            create_solana_network_wrapped("test.network.with.dots"),
            create_stellar_network_wrapped("test@network#with$symbols"),
        ];

        let config = NetworksFileConfig::new(networks);
        assert!(config.is_ok());
        let config = config.unwrap();
        assert_eq!(config.len(), 3);
    }

    #[test]
    fn test_very_long_network_names() {
        let long_name = "a".repeat(1000);
        let networks = vec![create_evm_network_wrapped(&long_name)];

        let config = NetworksFileConfig::new(networks);
        assert!(config.is_ok());
        let config = config.unwrap();
        assert!(config
            .get_network(ConfigFileNetworkType::Evm, &long_name)
            .is_some());
    }

    #[test]
    fn test_complex_inheritance_scenario() {
        let networks = vec![
            // Root networks
            create_evm_network_wrapped("evm-root"),
            create_solana_network_wrapped("solana-root"),
            // First level children
            create_evm_network_wrapped_with_parent("evm-child1", "evm-root"),
            create_evm_network_wrapped_with_parent("evm-child2", "evm-root"),
            create_solana_network_wrapped_with_parent("solana-child1", "solana-root"),
            // Second level children
            create_evm_network_wrapped_with_parent("evm-grandchild", "evm-child1"),
        ];

        let config = NetworksFileConfig::new(networks);
        assert!(config.is_ok());
        let config = config.unwrap();
        assert_eq!(config.len(), 6);

        let flattened = config.flatten();
        assert!(flattened.is_ok());
        let flattened = flattened.unwrap();
        assert_eq!(flattened.len(), 6);
    }

    #[test]
    fn test_new_with_duplicate_network_names_across_types() {
        // Should allow same name across different network types
        let networks = vec![
            create_evm_network_wrapped("mainnet"),
            create_solana_network_wrapped("mainnet"),
            create_stellar_network_wrapped("mainnet"),
        ];
        let result = NetworksFileConfig::new(networks);

        assert!(result.is_ok());
        let config = result.unwrap();
        assert_eq!(config.networks.len(), 3);
        assert_eq!(config.network_map.len(), 3);

        // Verify we can retrieve each network by type and name
        assert!(config
            .get_network(ConfigFileNetworkType::Evm, "mainnet")
            .is_some());
        assert!(config
            .get_network(ConfigFileNetworkType::Solana, "mainnet")
            .is_some());
        assert!(config
            .get_network(ConfigFileNetworkType::Stellar, "mainnet")
            .is_some());
    }

    #[test]
    fn test_new_with_duplicate_network_names_within_same_type() {
        let networks = vec![
            create_evm_network_wrapped("duplicate-evm"),
            create_evm_network_wrapped("duplicate-evm"),
        ];
        let result = NetworksFileConfig::new(networks);

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::DuplicateId(_)
        ));
    }

    #[test]
    fn test_get_with_empty_config() {
        let config = NetworksFileConfig::new(vec![]).unwrap();

        let network_0 = config.get(0);
        assert!(network_0.is_none());
    }

    #[test]
    fn test_get_and_first_equivalence() {
        let networks = vec![create_evm_network_wrapped("test-network")];
        let config = NetworksFileConfig::new(networks).unwrap();

        // Both methods should return the same result
        let network_via_get = config.get(0);
        let network_via_first = config.first();

        assert!(network_via_get.is_some());
        assert!(network_via_first.is_some());
        assert_eq!(
            network_via_get.unwrap().network_name(),
            network_via_first.unwrap().network_name()
        );
        assert_eq!(network_via_get.unwrap().network_name(), "test-network");
    }

    #[test]
    #[allow(clippy::get_first)]
    fn test_different_access_methods() {
        let networks = vec![
            create_evm_network_wrapped("network-0"),
            create_solana_network_wrapped("network-1"),
        ];
        let config = NetworksFileConfig::new(networks).unwrap();

        // Method 1: Using .get())
        let net_0_get = config.get(0);
        assert!(net_0_get.is_some());
        assert_eq!(net_0_get.unwrap().network_name(), "network-0");

        // Method 2: Using .first()
        let net_0_first = config.first();
        assert!(net_0_first.is_some());
        assert_eq!(net_0_first.unwrap().network_name(), "network-0");

        // Method 3: Using indexing [0] (Index trait)
        let net_0_index = &config[0];
        assert_eq!(net_0_index.network_name(), "network-0");

        // Method 4: Using direct field access
        let net_0_direct = config.networks.get(0);
        assert!(net_0_direct.is_some());
        assert_eq!(net_0_direct.unwrap().network_name(), "network-0");

        // All should reference the same network
        assert_eq!(
            net_0_get.unwrap().network_name(),
            net_0_first.unwrap().network_name()
        );
        assert_eq!(
            net_0_get.unwrap().network_name(),
            net_0_index.network_name()
        );
        assert_eq!(
            net_0_get.unwrap().network_name(),
            net_0_direct.unwrap().network_name()
        );
    }

    #[test]
    fn test_first_with_non_empty_config() {
        let networks = vec![
            create_evm_network_wrapped("first-network"),
            create_solana_network_wrapped("second-network"),
        ];
        let config = NetworksFileConfig::new(networks).unwrap();

        let first_network = config.first();
        assert!(first_network.is_some());
        assert_eq!(first_network.unwrap().network_name(), "first-network");
    }

    #[test]
    fn test_first_with_empty_config() {
        let config = NetworksFileConfig::new(vec![]).unwrap();

        let first_network = config.first();
        assert!(first_network.is_none());
    }

    #[test]
    fn test_get_with_valid_index() {
        let networks = vec![
            create_evm_network_wrapped("network-0"),
            create_solana_network_wrapped("network-1"),
            create_stellar_network_wrapped("network-2"),
        ];
        let config = NetworksFileConfig::new(networks).unwrap();

        let network_0 = config.get(0);
        assert!(network_0.is_some());
        assert_eq!(network_0.unwrap().network_name(), "network-0");

        let network_1 = config.get(1);
        assert!(network_1.is_some());
        assert_eq!(network_1.unwrap().network_name(), "network-1");

        let network_2 = config.get(2);
        assert!(network_2.is_some());
        assert_eq!(network_2.unwrap().network_name(), "network-2");
    }

    #[test]
    fn test_get_with_invalid_index() {
        let networks = vec![create_evm_network_wrapped("only-network")];
        let config = NetworksFileConfig::new(networks).unwrap();

        let network_out_of_bounds = config.get(1);
        assert!(network_out_of_bounds.is_none());

        let network_large_index = config.get(100);
        assert!(network_large_index.is_none());
    }

    #[test]
    fn test_networks_source_default() {
        let default_source = NetworksSource::default();
        match default_source {
            NetworksSource::Path(path) => {
                assert_eq!(path, "./config/networks");
            }
            _ => panic!("Default should be a Path variant"),
        }
    }
}
