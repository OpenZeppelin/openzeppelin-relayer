//! Network Configuration Module
//!
//! This module provides network configuration support for EVM, Solana, and Stellar networks
//! with inheritance, validation, and flexible loading mechanisms.
//!
//! ## Key Features
//!
//! - **Multi-blockchain support**: EVM, Solana, and Stellar network configurations
//! - **Inheritance system**: Networks can inherit from parents with type safety
//! - **Flexible loading**: JSON arrays or directory-based configuration files
//! - **Comprehensive validation**: URL validation, required fields, inheritance integrity
//!
//! ## Core Types
//!
//! - [`NetworkFileConfig`] - Unified enum for all network types
//! - [`NetworksFileConfig`] - Collection managing multiple networks
//! - [`NetworkConfigCommon`] - Shared configuration fields
//! - [`InheritanceResolver`] - Handles inheritance resolution
//! - [`NetworkFileLoader`] - Loads configurations from files/directories

pub mod collection;
pub mod common;
pub mod evm;
pub mod file_loading;
pub mod inheritance;
pub mod solana;
pub mod stellar;
#[cfg(test)]
pub mod test_utils;

pub use collection::*;
pub use common::*;
pub use evm::*;
pub use file_loading::*;
pub use inheritance::*;
pub use solana::*;
pub use stellar::*;

use super::ConfigFileNetworkType;
use crate::config::ConfigFileError;
use serde::{Deserialize, Serialize};

/// Represents the configuration for a specific network, which can be EVM, Solana, or Stellar.
///
/// During deserialization, the `type` field in the configuration source determines which variant is expected.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum NetworkFileConfig {
    /// Configuration for an EVM-compatible network.
    Evm(EvmNetworkConfig),
    /// Configuration for a Solana network.
    Solana(SolanaNetworkConfig),
    /// Configuration for a Stellar network.
    Stellar(StellarNetworkConfig),
}

impl NetworkFileConfig {
    /// Validates the network configuration based on its type.
    ///
    /// # Returns
    /// - `Ok(())` if the configuration is valid.
    /// - `Err(ConfigFileError)` with details about the validation failure.
    pub fn validate(&self) -> Result<(), ConfigFileError> {
        match self {
            NetworkFileConfig::Evm(network) => network.validate(),
            NetworkFileConfig::Solana(network) => network.validate(),
            NetworkFileConfig::Stellar(network) => network.validate(),
        }
    }

    /// Returns the unique identifier (name) of the network.
    ///
    /// # Returns
    /// - `&str` containing the network name.
    pub fn network_name(&self) -> &str {
        match self {
            NetworkFileConfig::Evm(network) => &network.common.network,
            NetworkFileConfig::Solana(network) => &network.common.network,
            NetworkFileConfig::Stellar(network) => &network.common.network,
        }
    }

    /// Returns the type of the network (EVM, Solana, or Stellar).
    ///
    /// # Returns
    /// - `ConfigFileNetworkType` enum variant corresponding to the network type.
    pub fn network_type(&self) -> ConfigFileNetworkType {
        match self {
            NetworkFileConfig::Evm(_) => ConfigFileNetworkType::Evm,
            NetworkFileConfig::Solana(_) => ConfigFileNetworkType::Solana,
            NetworkFileConfig::Stellar(_) => ConfigFileNetworkType::Stellar,
        }
    }

    /// Returns true if the network is a testnet, false otherwise.
    ///
    /// # Returns
    /// - `true` if the network is a testnet.
    /// - `false` if the network is a mainnet.
    pub fn is_testnet(&self) -> bool {
        match self {
            NetworkFileConfig::Evm(network) => network.common.is_testnet.unwrap_or(false),
            NetworkFileConfig::Solana(network) => network.common.is_testnet.unwrap_or(false),
            NetworkFileConfig::Stellar(network) => network.common.is_testnet.unwrap_or(false),
        }
    }

    /// Returns the name of the network this configuration inherits from, if any.
    ///
    /// # Returns
    /// - `Some(&str)` containing the source network name if the `from` field is set.
    /// - `None` otherwise.
    pub fn inherits_from(&self) -> Option<&str> {
        match self {
            NetworkFileConfig::Evm(network) => network.common.from.as_deref(),
            NetworkFileConfig::Solana(network) => network.common.from.as_deref(),
            NetworkFileConfig::Stellar(network) => network.common.from.as_deref(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::config_file::network::test_utils::*;

    #[test]
    fn test_validate_evm_network_success() {
        let config = create_evm_network_wrapped("test-evm");
        let result = config.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_solana_network_success() {
        let config = create_solana_network_wrapped("test-solana");
        let result = config.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_stellar_network_success() {
        let config = create_stellar_network_wrapped("test-stellar");
        let result = config.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_evm_network_failure() {
        let mut config = create_evm_network_wrapped("test-evm");
        if let NetworkFileConfig::Evm(ref mut evm_config) = config {
            evm_config.common.network = "".to_string(); // Invalid empty network name
        }

        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::MissingField(_)
        ));
    }

    #[test]
    fn test_validate_solana_network_failure() {
        let mut config = create_solana_network_wrapped("test-solana");
        if let NetworkFileConfig::Solana(ref mut solana_config) = config {
            solana_config.common.rpc_urls = None; // Missing required RPC URLs
        }

        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::MissingField(_)
        ));
    }

    #[test]
    fn test_validate_stellar_network_failure() {
        let mut config = create_stellar_network_wrapped("test-stellar");
        if let NetworkFileConfig::Stellar(ref mut stellar_config) = config {
            stellar_config.common.network = "".to_string(); // Invalid empty network name
        }

        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::MissingField(_)
        ));
    }

    #[test]
    fn test_validate_evm_network_missing_chain_id() {
        let mut config = create_evm_network_wrapped("test-evm");
        if let NetworkFileConfig::Evm(ref mut evm_config) = config {
            evm_config.chain_id = None; // Missing required chain_id
        }

        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::MissingField(_)
        ));
    }

    #[test]
    fn test_validate_evm_network_missing_confirmations() {
        let mut config = create_evm_network_wrapped("test-evm");
        if let NetworkFileConfig::Evm(ref mut evm_config) = config {
            evm_config.required_confirmations = None; // Missing required confirmations
        }

        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::MissingField(_)
        ));
    }

    #[test]
    fn test_validate_evm_network_missing_symbol() {
        let mut config = create_evm_network_wrapped("test-evm");
        if let NetworkFileConfig::Evm(ref mut evm_config) = config {
            evm_config.symbol = None; // Missing required symbol
        }

        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::MissingField(_)
        ));
    }

    // NetworkFileConfig::network_name() tests
    #[test]
    fn test_network_name_evm() {
        let config = create_evm_network_wrapped("test-evm");
        assert_eq!(config.network_name(), "test-evm");
    }

    #[test]
    fn test_network_name_solana() {
        let config = create_solana_network_wrapped("test-solana");
        assert_eq!(config.network_name(), "test-solana");
    }

    #[test]
    fn test_network_name_stellar() {
        let config = create_stellar_network_wrapped("test-stellar");
        assert_eq!(config.network_name(), "test-stellar");
    }

    #[test]
    fn test_network_name_with_unicode() {
        let mut config = create_evm_network_wrapped("test-evm");
        if let NetworkFileConfig::Evm(ref mut evm_config) = config {
            evm_config.common.network = "测试网络".to_string();
        }
        assert_eq!(config.network_name(), "测试网络");
    }

    #[test]
    fn test_network_name_with_special_characters() {
        let mut config = create_solana_network_wrapped("test-solana");
        if let NetworkFileConfig::Solana(ref mut solana_config) = config {
            solana_config.common.network = "test-network_123-dev".to_string();
        }
        assert_eq!(config.network_name(), "test-network_123-dev");
    }

    #[test]
    fn test_network_name_empty_string() {
        let mut config = create_stellar_network_wrapped("test-stellar");
        if let NetworkFileConfig::Stellar(ref mut stellar_config) = config {
            stellar_config.common.network = "".to_string();
        }
        assert_eq!(config.network_name(), "");
    }

    #[test]
    fn test_network_type_evm() {
        let config = create_evm_network_wrapped("test-evm");
        assert_eq!(config.network_type(), ConfigFileNetworkType::Evm);
    }

    #[test]
    fn test_network_type_solana() {
        let config = create_solana_network_wrapped("test-solana");
        assert_eq!(config.network_type(), ConfigFileNetworkType::Solana);
    }

    #[test]
    fn test_network_type_stellar() {
        let config = create_stellar_network_wrapped("test-stellar");
        assert_eq!(config.network_type(), ConfigFileNetworkType::Stellar);
    }

    #[test]
    fn test_network_type_consistency() {
        let evm_config = create_evm_network_wrapped("test-evm");
        let solana_config = create_solana_network_wrapped("test-solana");
        let stellar_config = create_stellar_network_wrapped("test-stellar");

        // Ensure each type returns the correct enum variant
        assert!(matches!(
            evm_config.network_type(),
            ConfigFileNetworkType::Evm
        ));
        assert!(matches!(
            solana_config.network_type(),
            ConfigFileNetworkType::Solana
        ));
        assert!(matches!(
            stellar_config.network_type(),
            ConfigFileNetworkType::Stellar
        ));
    }

    #[test]
    fn test_inherits_from_none() {
        let config = create_evm_network_wrapped("test-evm");
        assert_eq!(config.inherits_from(), None);
    }

    #[test]
    fn test_inherits_from_some_evm() {
        let config = create_evm_network_wrapped_with_parent("child-evm", "parent-evm");
        assert_eq!(config.inherits_from(), Some("parent-evm"));
    }

    #[test]
    fn test_inherits_from_some_solana() {
        let mut config = create_solana_network_wrapped("test-solana");
        if let NetworkFileConfig::Solana(ref mut solana_config) = config {
            solana_config.common.from = Some("parent-solana".to_string());
        }
        assert_eq!(config.inherits_from(), Some("parent-solana"));
    }

    #[test]
    fn test_inherits_from_some_stellar() {
        let mut config = create_stellar_network_wrapped("test-stellar");
        if let NetworkFileConfig::Stellar(ref mut stellar_config) = config {
            stellar_config.common.from = Some("parent-stellar".to_string());
        }
        assert_eq!(config.inherits_from(), Some("parent-stellar"));
    }

    #[test]
    fn test_inherits_from_empty_string() {
        let mut config = create_evm_network_wrapped("test-evm");
        if let NetworkFileConfig::Evm(ref mut evm_config) = config {
            evm_config.common.from = Some("".to_string());
        }
        assert_eq!(config.inherits_from(), Some(""));
    }

    #[test]
    fn test_inherits_from_with_unicode() {
        let mut config = create_solana_network_wrapped("test-solana");
        if let NetworkFileConfig::Solana(ref mut solana_config) = config {
            solana_config.common.from = Some("父网络".to_string());
        }
        assert_eq!(config.inherits_from(), Some("父网络"));
    }

    #[test]
    fn test_serialize_deserialize_evm() {
        let original = create_evm_network_wrapped("test-evm");
        let serialized = serde_json::to_string(&original).unwrap();
        let deserialized: NetworkFileConfig = serde_json::from_str(&serialized).unwrap();

        assert_eq!(original.network_name(), deserialized.network_name());
        assert_eq!(original.network_type(), deserialized.network_type());
        assert_eq!(original.inherits_from(), deserialized.inherits_from());
    }

    #[test]
    fn test_serialize_deserialize_solana() {
        let original = create_solana_network_wrapped("test-solana");
        let serialized = serde_json::to_string(&original).unwrap();
        let deserialized: NetworkFileConfig = serde_json::from_str(&serialized).unwrap();

        assert_eq!(original.network_name(), deserialized.network_name());
        assert_eq!(original.network_type(), deserialized.network_type());
        assert_eq!(original.inherits_from(), deserialized.inherits_from());
    }

    #[test]
    fn test_serialize_deserialize_stellar() {
        let original = create_stellar_network_wrapped("test-stellar");
        let serialized = serde_json::to_string(&original).unwrap();
        let deserialized: NetworkFileConfig = serde_json::from_str(&serialized).unwrap();

        assert_eq!(original.network_name(), deserialized.network_name());
        assert_eq!(original.network_type(), deserialized.network_type());
        assert_eq!(original.inherits_from(), deserialized.inherits_from());
    }

    #[test]
    fn test_deserialize_evm_from_json() {
        let json = r#"{
            "type": "evm",
            "network": "test-evm-json",
            "chain_id": 1337,
            "required_confirmations": 2,
            "symbol": "ETH",
            "rpc_urls": ["https://rpc.example.com"]
        }"#;

        let config: NetworkFileConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.network_name(), "test-evm-json");
        assert_eq!(config.network_type(), ConfigFileNetworkType::Evm);
        assert_eq!(config.inherits_from(), None);
    }

    #[test]
    fn test_deserialize_solana_from_json() {
        let json = r#"{
            "type": "solana",
            "network": "test-solana-json",
            "rpc_urls": ["https://api.devnet.solana.com"]
        }"#;

        let config: NetworkFileConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.network_name(), "test-solana-json");
        assert_eq!(config.network_type(), ConfigFileNetworkType::Solana);
        assert_eq!(config.inherits_from(), None);
    }

    #[test]
    fn test_deserialize_stellar_from_json() {
        let json = r#"{
            "type": "stellar",
            "network": "test-stellar-json",
            "rpc_urls": ["https://horizon-testnet.stellar.org"]
        }"#;

        let config: NetworkFileConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.network_name(), "test-stellar-json");
        assert_eq!(config.network_type(), ConfigFileNetworkType::Stellar);
        assert_eq!(config.inherits_from(), None);
    }

    #[test]
    fn test_deserialize_with_inheritance() {
        let json = r#"{
            "type": "evm",
            "network": "child-network",
            "from": "parent-network",
            "chain_id": 1337,
            "required_confirmations": 1,
            "symbol": "ETH"
        }"#;

        let config: NetworkFileConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.network_name(), "child-network");
        assert_eq!(config.inherits_from(), Some("parent-network"));
    }

    #[test]
    fn test_deserialize_invalid_type() {
        let json = r#"{
            "type": "invalid",
            "network": "test-network"
        }"#;

        let result: Result<NetworkFileConfig, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_deserialize_missing_type() {
        let json = r#"{
            "network": "test-network",
            "chain_id": 1337
        }"#;

        let result: Result<NetworkFileConfig, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_deserialize_missing_network_field() {
        let json = r#"{
            "type": "evm",
            "chain_id": 1337
        }"#;

        let result: Result<NetworkFileConfig, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_all_network_types_in_collection() {
        let configs = vec![
            create_evm_network_wrapped("test-evm"),
            create_solana_network_wrapped("test-solana"),
            create_stellar_network_wrapped("test-stellar"),
        ];

        let types: Vec<ConfigFileNetworkType> = configs.iter().map(|c| c.network_type()).collect();
        assert!(types.contains(&ConfigFileNetworkType::Evm));
        assert!(types.contains(&ConfigFileNetworkType::Solana));
        assert!(types.contains(&ConfigFileNetworkType::Stellar));
    }

    #[test]
    fn test_network_names_uniqueness() {
        let configs = vec![
            create_evm_network_wrapped("test-evm"),
            create_solana_network_wrapped("test-solana"),
            create_stellar_network_wrapped("test-stellar"),
        ];

        let names: Vec<&str> = configs.iter().map(|c| c.network_name()).collect();
        assert_eq!(names.len(), 3);
        assert!(names.contains(&"test-evm"));
        assert!(names.contains(&"test-solana"));
        assert!(names.contains(&"test-stellar"));
    }

    #[test]
    fn test_inheritance_patterns() {
        let mut configs = vec![
            create_evm_network_wrapped("test-evm"),
            create_evm_network_wrapped_with_parent("child-evm", "parent-evm"),
        ];

        let mut solana_with_inheritance = create_solana_network_wrapped("test-solana");
        if let NetworkFileConfig::Solana(ref mut solana_config) = solana_with_inheritance {
            solana_config.common.from = Some("parent-solana".to_string());
        }
        configs.push(solana_with_inheritance);

        let inheritance_info: Vec<Option<&str>> =
            configs.iter().map(|c| c.inherits_from()).collect();
        assert_eq!(inheritance_info[0], None); // Base EVM config
        assert_eq!(inheritance_info[1], Some("parent-evm")); // Child EVM config
        assert_eq!(inheritance_info[2], Some("parent-solana")); // Child Solana config
    }

    #[test]
    fn test_validation_error_propagation() {
        let mut config = create_evm_network_wrapped("test-evm");
        if let NetworkFileConfig::Evm(ref mut evm_config) = config {
            evm_config.common.rpc_urls = Some(vec!["invalid-url".to_string()]);
        }

        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::InvalidFormat(_)
        ));
    }

    #[test]
    fn test_serialization_preserves_all_fields() {
        let config = create_evm_network_wrapped("test-evm");
        let serialized = serde_json::to_string(&config).unwrap();

        // Check that important fields are present in serialized JSON
        assert!(serialized.contains("\"type\":\"evm\""));
        assert!(serialized.contains("\"network\":\"test-evm\""));
        assert!(serialized.contains("\"chain_id\":31337"));
        assert!(serialized.contains("\"required_confirmations\":1"));
        assert!(serialized.contains("\"symbol\":\"ETH\""));
    }

    #[test]
    fn test_deserialization_with_extra_fields() {
        let json = r#"{
            "type": "evm",
            "network": "test-network",
            "chain_id": 1337,
            "required_confirmations": 1,
            "symbol": "ETH",
            "rpc_urls": ["https://rpc.example.com"],
            "extra_field": "should_be_ignored"
        }"#;

        let result: Result<NetworkFileConfig, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_method_consistency_across_types() {
        let configs = vec![
            create_evm_network_wrapped("test-evm"),
            create_solana_network_wrapped("test-solana"),
            create_stellar_network_wrapped("test-stellar"),
        ];

        // Ensure all methods work consistently across all network types
        for config in configs {
            // All should have non-empty network names
            assert!(!config.network_name().is_empty());

            // All should have valid network types
            let network_type = config.network_type();
            assert!(matches!(
                network_type,
                ConfigFileNetworkType::Evm
                    | ConfigFileNetworkType::Solana
                    | ConfigFileNetworkType::Stellar
            ));

            // All should validate successfully
            assert!(config.validate().is_ok());

            // All should have None inheritance by default
            assert_eq!(config.inherits_from(), None);
        }
    }
}
