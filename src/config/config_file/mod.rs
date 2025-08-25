//! This module provides functionality for loading and validating configuration files
//! for a blockchain relayer application. It includes definitions for configuration
//! structures, error handling, and validation logic to ensure that the configuration
//! is correct and complete before use.
//!
//! The module supports configuration for different network types, including EVM, Solana,
//! and Stellar, and ensures that test signers are only used with test networks.
//!
//! # Modules
//! - `relayer`: Handles relayer-specific configuration.
//! - `signer`: Manages signer-specific configuration.
//! - `notification`: Deals with notification-specific configuration.
//! - `network`: Handles network configuration, including network overrides and custom networks.
//!
//! # Errors
//! The module defines a comprehensive set of errors to handle various issues that might
//! arise during configuration loading and validation, such as missing fields, invalid
//! formats, and invalid references.
//!
//! # Usage
//! To use this module, load a configuration file using `load_config`, which will parse
//! the file and validate its contents. If the configuration is valid, it can be used
//! to initialize the application components.
use crate::{
    config::ConfigFileError,
    models::{
        relayer::{RelayerFileConfig, RelayersFileConfig},
        signer::{SignerFileConfig, SignersFileConfig},
        NotificationConfig, NotificationConfigs,
    },
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs::{self},
};

mod plugin;
pub use plugin::*;

pub mod network;
pub use network::{
    EvmNetworkConfig, NetworkConfigCommon, NetworkFileConfig, NetworksFileConfig,
    SolanaNetworkConfig, StellarNetworkConfig,
};

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum ConfigFileNetworkType {
    Evm,
    Stellar,
    Solana,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
    pub relayers: Vec<RelayerFileConfig>,
    pub signers: Vec<SignerFileConfig>,
    pub notifications: Vec<NotificationConfig>,
    pub networks: NetworksFileConfig,
    pub plugins: Option<Vec<PluginFileConfig>>,
}

impl Config {
    /// Validates the configuration by checking the validity of relayers, signers, and
    /// notifications.
    ///
    /// This method ensures that all references between relayers, signers, and notifications are
    /// valid. It also checks that test signers are only used with test networks.
    ///
    /// # Errors
    /// Returns a `ConfigFileError` if any validation checks fail.
    pub fn validate(&self) -> Result<(), ConfigFileError> {
        self.validate_networks()?;
        self.validate_relayers(&self.networks)?;
        self.validate_signers()?;
        self.validate_notifications()?;
        self.validate_plugins()?;

        self.validate_relayer_signer_refs()?;
        self.validate_relayer_notification_refs()?;

        Ok(())
    }

    /// Validates that all relayer references to signers are valid.
    ///
    /// This method checks that each relayer references an existing signer and that test signers
    /// are only used with test networks.
    ///
    /// # Errors
    /// Returns a `ConfigFileError::InvalidReference` if a relayer references a non-existent signer.
    /// Returns a `ConfigFileError::TestSigner` if a test signer is used on a production network.
    fn validate_relayer_signer_refs(&self) -> Result<(), ConfigFileError> {
        let signer_ids: HashSet<_> = self.signers.iter().map(|s| &s.id).collect();

        for relayer in &self.relayers {
            if !signer_ids.contains(&relayer.signer_id) {
                return Err(ConfigFileError::InvalidReference(format!(
                    "Relayer '{}' references non-existent signer '{}'",
                    relayer.id, relayer.signer_id
                )));
            }
        }

        Ok(())
    }

    /// Validates that all relayer references to notifications are valid.
    ///
    /// This method checks that each relayer references an existing notification, if specified.
    ///
    /// # Errors
    /// Returns a `ConfigFileError::InvalidReference` if a relayer references a non-existent
    /// notification.
    fn validate_relayer_notification_refs(&self) -> Result<(), ConfigFileError> {
        let notification_ids: HashSet<_> = self.notifications.iter().map(|s| &s.id).collect();

        for relayer in &self.relayers {
            if let Some(notification_id) = &relayer.notification_id {
                if !notification_ids.contains(notification_id) {
                    return Err(ConfigFileError::InvalidReference(format!(
                        "Relayer '{}' references non-existent notification '{}'",
                        relayer.id, notification_id
                    )));
                }
            }
        }

        Ok(())
    }

    /// Validates that all relayers are valid and have unique IDs.
    fn validate_relayers(&self, networks: &NetworksFileConfig) -> Result<(), ConfigFileError> {
        RelayersFileConfig::new(self.relayers.clone()).validate(networks)
    }

    /// Validates that all signers are valid and have unique IDs.
    fn validate_signers(&self) -> Result<(), ConfigFileError> {
        SignersFileConfig::new(self.signers.clone()).validate()
    }

    /// Validates that all notifications are valid and have unique IDs.
    fn validate_notifications(&self) -> Result<(), ConfigFileError> {
        NotificationConfigs::new(self.notifications.clone()).validate()
    }

    /// Validates that all networks are valid and have unique IDs.
    fn validate_networks(&self) -> Result<(), ConfigFileError> {
        if self.networks.is_empty() {
            return Ok(()); // No networks to validate
        }

        self.networks.validate()
    }

    /// Validates that all plugins are valid and have unique IDs.
    fn validate_plugins(&self) -> Result<(), ConfigFileError> {
        if let Some(plugins) = &self.plugins {
            PluginsFileConfig::new(plugins.clone()).validate()
        } else {
            Ok(())
        }
    }
}

/// Loads and validates a configuration file from the specified path.
///
/// This function reads the configuration file, parses it as JSON, and validates its contents.
/// If the configuration is valid, it returns a `Config` object.
///
/// # Arguments
/// * `config_file_path` - A string slice that holds the path to the configuration file.
///
/// # Errors
/// Returns a `ConfigFileError` if the file cannot be read, parsed, or if the configuration is
/// invalid.
pub fn load_config(config_file_path: &str) -> Result<Config, ConfigFileError> {
    let config_str = fs::read_to_string(config_file_path)?;
    let config: Config = serde_json::from_str(&config_str)?;
    config.validate()?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use crate::models::{
        signer::{LocalSignerFileConfig, SignerFileConfig, SignerFileConfigEnum},
        NotificationType, PlainOrEnvValue, SecretString,
    };
    use std::path::Path;

    use super::*;

    fn create_valid_config() -> Config {
        Config {
            relayers: vec![RelayerFileConfig {
                id: "test-1".to_string(),
                name: "Test Relayer".to_string(),
                network: "test-network".to_string(),
                paused: false,
                network_type: ConfigFileNetworkType::Evm,
                policies: None,
                signer_id: "test-1".to_string(),
                notification_id: Some("test-1".to_string()),
                custom_rpc_urls: None,
            }],
            signers: vec![SignerFileConfig {
                id: "test-1".to_string(),
                config: SignerFileConfigEnum::Local(LocalSignerFileConfig {
                    path: "tests/utils/test_keys/unit-test-local-signer.json".to_string(),
                    passphrase: PlainOrEnvValue::Plain {
                        value: SecretString::new("test"),
                    },
                }),
            }],
            notifications: vec![NotificationConfig {
                id: "test-1".to_string(),
                r#type: NotificationType::Webhook,
                url: "https://api.example.com/notifications".to_string(),
                signing_key: None,
            }],
            networks: NetworksFileConfig::new(vec![NetworkFileConfig::Evm(EvmNetworkConfig {
                common: NetworkConfigCommon {
                    network: "test-network".to_string(),
                    from: None,
                    rpc_urls: Some(vec!["https://rpc.test.example.com".to_string()]),
                    explorer_urls: Some(vec!["https://explorer.test.example.com".to_string()]),
                    average_blocktime_ms: Some(12000),
                    is_testnet: Some(true),
                    tags: Some(vec!["test".to_string()]),
                },
                chain_id: Some(31337),
                required_confirmations: Some(1),
                features: None,
                symbol: Some("ETH".to_string()),
            })])
            .expect("Failed to create NetworksFileConfig for test"),
            plugins: Some(vec![PluginFileConfig {
                id: "test-1".to_string(),
                path: "/app/plugins/test-plugin.ts".to_string(),
                timeout: None,
            }]),
        }
    }

    #[test]
    fn test_valid_config_validation() {
        let config = create_valid_config();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_empty_relayers() {
        let config = Config {
            relayers: Vec::new(),
            signers: Vec::new(),
            notifications: Vec::new(),
            networks: NetworksFileConfig::new(vec![NetworkFileConfig::Evm(EvmNetworkConfig {
                common: NetworkConfigCommon {
                    network: "test-network".to_string(),
                    from: None,
                    rpc_urls: Some(vec!["https://rpc.test.example.com".to_string()]),
                    explorer_urls: Some(vec!["https://explorer.test.example.com".to_string()]),
                    average_blocktime_ms: Some(12000),
                    is_testnet: Some(true),
                    tags: Some(vec!["test".to_string()]),
                },
                chain_id: Some(31337),
                required_confirmations: Some(1),
                features: None,
                symbol: Some("ETH".to_string()),
            })])
            .unwrap(),
            plugins: Some(vec![]),
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_empty_signers() {
        let config = Config {
            relayers: Vec::new(),
            signers: Vec::new(),
            notifications: Vec::new(),
            networks: NetworksFileConfig::new(vec![NetworkFileConfig::Evm(EvmNetworkConfig {
                common: NetworkConfigCommon {
                    network: "test-network".to_string(),
                    from: None,
                    rpc_urls: Some(vec!["https://rpc.test.example.com".to_string()]),
                    explorer_urls: Some(vec!["https://explorer.test.example.com".to_string()]),
                    average_blocktime_ms: Some(12000),
                    is_testnet: Some(true),
                    tags: Some(vec!["test".to_string()]),
                },
                chain_id: Some(31337),
                required_confirmations: Some(1),
                features: None,
                symbol: Some("ETH".to_string()),
            })])
            .unwrap(),
            plugins: Some(vec![]),
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_invalid_id_format() {
        let mut config = create_valid_config();
        config.relayers[0].id = "invalid@id".to_string();
        assert!(matches!(
            config.validate(),
            Err(ConfigFileError::InvalidIdFormat(_))
        ));
    }

    #[test]
    fn test_id_too_long() {
        let mut config = create_valid_config();
        config.relayers[0].id = "a".repeat(37);
        assert!(matches!(
            config.validate(),
            Err(ConfigFileError::InvalidIdLength(_))
        ));
    }

    #[test]
    fn test_relayers_duplicate_ids() {
        let mut config = create_valid_config();
        config.relayers.push(config.relayers[0].clone());
        assert!(matches!(
            config.validate(),
            Err(ConfigFileError::DuplicateId(_))
        ));
    }

    #[test]
    fn test_signers_duplicate_ids() {
        let mut config = create_valid_config();
        config.signers.push(config.signers[0].clone());

        assert!(matches!(
            config.validate(),
            Err(ConfigFileError::DuplicateId(_))
        ));
    }

    #[test]
    fn test_missing_name() {
        let mut config = create_valid_config();
        config.relayers[0].name = "".to_string();
        assert!(matches!(
            config.validate(),
            Err(ConfigFileError::MissingField(_))
        ));
    }

    #[test]
    fn test_missing_network() {
        let mut config = create_valid_config();
        config.relayers[0].network = "".to_string();
        assert!(matches!(
            config.validate(),
            Err(ConfigFileError::InvalidFormat(_))
        ));
    }

    #[test]
    fn test_invalid_signer_id_reference() {
        let mut config = create_valid_config();
        config.relayers[0].signer_id = "invalid@id".to_string();
        assert!(matches!(
            config.validate(),
            Err(ConfigFileError::InvalidReference(_))
        ));
    }

    #[test]
    fn test_invalid_notification_id_reference() {
        let mut config = create_valid_config();
        config.relayers[0].notification_id = Some("invalid@id".to_string());
        assert!(matches!(
            config.validate(),
            Err(ConfigFileError::InvalidReference(_))
        ));
    }

    #[test]
    fn test_config_with_networks() {
        let mut config = create_valid_config();
        config.relayers[0].network = "custom-evm".to_string();

        let network_items = vec![serde_json::from_value(serde_json::json!({
            "type": "evm",
            "network": "custom-evm",
            "required_confirmations": 1,
            "chain_id": 1234,
            "rpc_urls": ["https://rpc.example.com"],
            "symbol": "ETH"
        }))
        .unwrap()];
        config.networks = NetworksFileConfig::new(network_items).unwrap();

        assert!(
            config.validate().is_ok(),
            "Error validating config: {:?}",
            config.validate().err()
        );
    }

    #[test]
    fn test_config_with_invalid_networks() {
        let mut config = create_valid_config();
        let network_items = vec![serde_json::from_value(serde_json::json!({
            "type": "evm",
            "network": "invalid-network",
            "rpc_urls": ["https://rpc.example.com"]
        }))
        .unwrap()];
        config.networks = NetworksFileConfig::new(network_items.clone())
            .expect("Should allow creation, validation happens later or should fail here");

        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result,
            Err(ConfigFileError::MissingField(_)) | Err(ConfigFileError::InvalidFormat(_))
        ));
    }

    #[test]
    fn test_config_with_duplicate_network_names() {
        let mut config = create_valid_config();
        let network_items = vec![
            serde_json::from_value(serde_json::json!({
                "type": "evm",
                "network": "custom-evm",
                "chain_id": 1234,
                "rpc_urls": ["https://rpc1.example.com"]
            }))
            .unwrap(),
            serde_json::from_value(serde_json::json!({
                "type": "evm",
                "network": "custom-evm",
                "chain_id": 5678,
                "rpc_urls": ["https://rpc2.example.com"]
            }))
            .unwrap(),
        ];
        let networks_config_result = NetworksFileConfig::new(network_items);
        assert!(
            networks_config_result.is_err(),
            "NetworksFileConfig::new should detect duplicate IDs"
        );

        if let Ok(parsed_networks) = networks_config_result {
            config.networks = parsed_networks;
            let result = config.validate();
            assert!(result.is_err());
            assert!(matches!(result, Err(ConfigFileError::DuplicateId(_))));
        } else if let Err(e) = networks_config_result {
            assert!(matches!(e, ConfigFileError::DuplicateId(_)));
        }
    }

    #[test]
    fn test_config_with_invalid_network_inheritance() {
        let mut config = create_valid_config();
        let network_items = vec![serde_json::from_value(serde_json::json!({
            "type": "evm",
            "network": "custom-evm",
            "from": "non-existent-network",
            "rpc_urls": ["https://rpc.example.com"]
        }))
        .unwrap()];
        let networks_config_result = NetworksFileConfig::new(network_items);

        match networks_config_result {
            Ok(parsed_networks) => {
                config.networks = parsed_networks;
                let validation_result = config.validate();
                assert!(
                    validation_result.is_err(),
                    "Validation should fail due to invalid inheritance reference"
                );
                assert!(matches!(
                    validation_result,
                    Err(ConfigFileError::InvalidReference(_))
                ));
            }
            Err(e) => {
                assert!(
                    matches!(e, ConfigFileError::InvalidReference(_)),
                    "Expected InvalidReference from new or flatten"
                );
            }
        }
    }

    #[test]
    fn test_deserialize_config_with_evm_network() {
        let config_str = r#"
        {
            "relayers": [],
            "signers": [],
            "notifications": [],
            "plugins": [],
            "networks": [
                {
                    "type": "evm",
                    "network": "custom-evm",
                    "chain_id": 1234,
                    "required_confirmations": 1,
                    "symbol": "ETH",
                    "rpc_urls": ["https://rpc.example.com"]
                }
            ]
        }
        "#;
        let result: Result<Config, _> = serde_json::from_str(config_str);
        assert!(result.is_ok());
        let config = result.unwrap();
        assert_eq!(config.networks.len(), 1);

        let network_config = config.networks.first().expect("Should have one network");
        assert!(matches!(network_config, NetworkFileConfig::Evm(_)));
        if let NetworkFileConfig::Evm(evm_config) = network_config {
            assert_eq!(evm_config.common.network, "custom-evm");
            assert_eq!(evm_config.chain_id, Some(1234));
        }
    }

    #[test]
    fn test_deserialize_config_with_solana_network() {
        let config_str = r#"
        {
            "relayers": [],
            "signers": [],
            "notifications": [],
            "plugins": [],
            "networks": [
                {
                    "type": "solana",
                    "network": "custom-solana",
                    "rpc_urls": ["https://rpc.solana.example.com"]
                }
            ]
        }
        "#;
        let result: Result<Config, _> = serde_json::from_str(config_str);
        assert!(result.is_ok());
        let config = result.unwrap();
        assert_eq!(config.networks.len(), 1);

        let network_config = config.networks.first().expect("Should have one network");
        assert!(matches!(network_config, NetworkFileConfig::Solana(_)));
        if let NetworkFileConfig::Solana(sol_config) = network_config {
            assert_eq!(sol_config.common.network, "custom-solana");
        }
    }

    #[test]
    fn test_deserialize_config_with_stellar_network() {
        let config_str = r#"
        {
            "relayers": [],
            "signers": [],
            "notifications": [],
            "plugins": [],
            "networks": [
                {
                    "type": "stellar",
                    "network": "custom-stellar",
                    "rpc_urls": ["https://rpc.stellar.example.com"]
                }
            ]
        }
        "#;
        let result: Result<Config, _> = serde_json::from_str(config_str);
        assert!(result.is_ok());
        let config = result.unwrap();
        assert_eq!(config.networks.len(), 1);

        let network_config = config.networks.first().expect("Should have one network");
        assert!(matches!(network_config, NetworkFileConfig::Stellar(_)));
        if let NetworkFileConfig::Stellar(stl_config) = network_config {
            assert_eq!(stl_config.common.network, "custom-stellar");
        }
    }

    #[test]
    fn test_deserialize_config_with_mixed_networks() {
        let config_str = r#"
        {
            "relayers": [],
            "signers": [],
            "notifications": [],
            "plugins": [],
            "networks": [
                {
                    "type": "evm",
                    "network": "custom-evm",
                    "chain_id": 1234,
                    "required_confirmations": 1,
                    "symbol": "ETH",
                    "rpc_urls": ["https://rpc.example.com"]
                },
                {
                    "type": "solana",
                    "network": "custom-solana",
                    "rpc_urls": ["https://rpc.solana.example.com"]
                }
            ]
        }
        "#;
        let result: Result<Config, _> = serde_json::from_str(config_str);
        assert!(result.is_ok());
        let config = result.unwrap();
        assert_eq!(config.networks.len(), 2);
    }

    #[test]
    #[should_panic(
        expected = "NetworksFileConfig cannot be empty - networks must contain at least one network configuration"
    )]
    fn test_deserialize_config_with_empty_networks_array() {
        let config_str = r#"
        {
            "relayers": [],
            "signers": [],
            "notifications": [],
            "networks": []
        }
        "#;
        let _result: Config = serde_json::from_str(config_str).unwrap();
    }

    #[test]
    fn test_deserialize_config_without_networks_field() {
        let config_str = r#"
        {
            "relayers": [],
            "signers": [],
            "notifications": []
        }
        "#;
        let result: Result<Config, _> = serde_json::from_str(config_str);
        assert!(result.is_ok());
    }

    use std::fs::File;
    use std::io::Write;
    use tempfile::tempdir;

    fn setup_network_file(dir_path: &Path, file_name: &str, content: &str) {
        let file_path = dir_path.join(file_name);
        let mut file = File::create(&file_path).expect("Failed to create temp network file");
        writeln!(file, "{}", content).expect("Failed to write to temp network file");
    }

    #[test]
    fn test_deserialize_config_with_networks_from_directory() {
        let dir = tempdir().expect("Failed to create temp dir");
        let network_dir_path = dir.path();

        setup_network_file(
            network_dir_path,
            "evm_net.json",
            r#"{"networks": [{"type": "evm", "network": "custom-evm-file", "required_confirmations": 1, "symbol": "ETH", "chain_id": 5678, "rpc_urls": ["https://rpc.file-evm.com"]}]}"#,
        );
        setup_network_file(
            network_dir_path,
            "sol_net.json",
            r#"{"networks": [{"type": "solana", "network": "custom-solana-file", "rpc_urls": ["https://rpc.file-solana.com"]}]}"#,
        );

        let config_json = serde_json::json!({
            "relayers": [],
            "signers": [],
            "notifications": [],
            "plugins": [],
            "networks": network_dir_path.to_str().expect("Path should be valid UTF-8")
        });
        let config_str =
            serde_json::to_string(&config_json).expect("Failed to serialize test config to string");

        let result: Result<Config, _> = serde_json::from_str(&config_str);
        assert!(result.is_ok(), "Deserialization failed: {:?}", result.err());

        if let Ok(config) = result {
            assert_eq!(
                config.networks.len(),
                2,
                "Incorrect number of networks loaded"
            );
            let has_evm = config.networks.iter().any(|n| matches!(n, NetworkFileConfig::Evm(evm) if evm.common.network == "custom-evm-file"));
            let has_solana = config.networks.iter().any(|n| matches!(n, NetworkFileConfig::Solana(sol) if sol.common.network == "custom-solana-file"));
            assert!(has_evm, "EVM network from file not found or incorrect");
            assert!(
                has_solana,
                "Solana network from file not found or incorrect"
            );
        }
    }

    #[test]
    fn test_deserialize_config_with_empty_networks_directory() {
        let dir = tempdir().expect("Failed to create temp dir");
        let network_dir_path = dir.path();

        let config_json = serde_json::json!({
            "relayers": [],
            "signers": [],
            "notifications": [],
            "networks": network_dir_path.to_str().expect("Path should be valid UTF-8")
        });
        let config_str =
            serde_json::to_string(&config_json).expect("Failed to serialize test config to string");

        let result: Result<Config, _> = serde_json::from_str(&config_str);
        assert!(
            result.is_err(),
            "Deserialization should fail for empty directory"
        );
    }

    #[test]
    fn test_deserialize_config_with_non_existent_networks_directory() {
        let dir = tempdir().expect("Failed to create temp dir");
        let non_existent_path = dir.path().join("non_existent_sub_dir");

        let config_json = serde_json::json!({
            "relayers": [],
            "signers": [],
            "notifications": [],
            "networks": non_existent_path.to_str().expect("Path should be valid UTF-8")
        });
        let config_str =
            serde_json::to_string(&config_json).expect("Failed to serialize test config to string");

        let result: Result<Config, _> = serde_json::from_str(&config_str);
        assert!(
            result.is_err(),
            "Deserialization should fail for non-existent directory"
        );
    }

    #[test]
    fn test_deserialize_config_with_networks_path_as_file() {
        let dir = tempdir().expect("Failed to create temp dir");
        let network_file_path = dir.path().join("im_a_file.json");
        File::create(&network_file_path).expect("Failed to create temp file");

        let config_json = serde_json::json!({
            "relayers": [],
            "signers": [],
            "notifications": [],
            "networks": network_file_path.to_str().expect("Path should be valid UTF-8")
        });
        let config_str =
            serde_json::to_string(&config_json).expect("Failed to serialize test config to string");

        let result: Result<Config, _> = serde_json::from_str(&config_str);
        assert!(
            result.is_err(),
            "Deserialization should fail if path is a file, not a directory"
        );
    }

    #[test]
    fn test_deserialize_config_network_dir_with_invalid_json_file() {
        let dir = tempdir().expect("Failed to create temp dir");
        let network_dir_path = dir.path();
        setup_network_file(
            network_dir_path,
            "invalid.json",
            r#"{"networks": [{"type": "evm", "network": "broken""#,
        ); // Malformed JSON

        let config_json = serde_json::json!({
            "relayers": [], "signers": [], "notifications": [],
            "networks": network_dir_path.to_str().expect("Path should be valid UTF-8")
        });
        let config_str = serde_json::to_string(&config_json).expect("Failed to serialize");

        let result: Result<Config, _> = serde_json::from_str(&config_str);
        assert!(
            result.is_err(),
            "Deserialization should fail with invalid JSON in network file"
        );
    }

    #[test]
    fn test_deserialize_config_network_dir_with_non_network_config_json_file() {
        let dir = tempdir().expect("Failed to create temp dir");
        let network_dir_path = dir.path();
        setup_network_file(network_dir_path, "not_a_network.json", r#"{"foo": "bar"}"#); // Valid JSON, but not NetworkFileConfig

        let config_json = serde_json::json!({
            "relayers": [], "signers": [], "notifications": [],
            "networks": network_dir_path.to_str().expect("Path should be valid UTF-8")
        });
        let config_str = serde_json::to_string(&config_json).expect("Failed to serialize");

        let result: Result<Config, _> = serde_json::from_str(&config_str);
        assert!(
            result.is_err(),
            "Deserialization should fail if file is not a valid NetworkFileConfig"
        );
    }

    #[test]
    fn test_deserialize_config_still_works_with_array_of_networks() {
        let config_str = r#"
        {
            "relayers": [],
            "signers": [],
            "notifications": [],
            "plugins": [],
            "networks": [
                {
                    "type": "evm",
                    "network": "custom-evm-array",
                    "chain_id": 1234,
                    "required_confirmations": 1,
                    "symbol": "ETH",
                    "rpc_urls": ["https://rpc.example.com"]
                }
            ]
        }
        "#;
        let result: Result<Config, _> = serde_json::from_str(config_str);
        assert!(
            result.is_ok(),
            "Deserialization with array failed: {:?}",
            result.err()
        );
        if let Ok(config) = result {
            assert_eq!(config.networks.len(), 1);

            let network_config = config.networks.first().expect("Should have one network");
            assert!(matches!(network_config, NetworkFileConfig::Evm(_)));
            if let NetworkFileConfig::Evm(evm_config) = network_config {
                assert_eq!(evm_config.common.network, "custom-evm-array");
            }
        }
    }

    #[test]
    fn test_create_valid_networks_file_config_works() {
        let networks = vec![NetworkFileConfig::Evm(EvmNetworkConfig {
            common: NetworkConfigCommon {
                network: "test-network".to_string(),
                from: None,
                rpc_urls: Some(vec!["https://rpc.test.example.com".to_string()]),
                explorer_urls: Some(vec!["https://explorer.test.example.com".to_string()]),
                average_blocktime_ms: Some(12000),
                is_testnet: Some(true),
                tags: Some(vec!["test".to_string()]),
            },
            chain_id: Some(31337),
            required_confirmations: Some(1),
            features: None,
            symbol: Some("ETH".to_string()),
        })];

        let config = NetworksFileConfig::new(networks).unwrap();
        assert_eq!(config.len(), 1);
        assert_eq!(config.first().unwrap().network_name(), "test-network");
    }

    fn setup_config_file(dir_path: &Path, file_name: &str, content: &str) {
        let file_path = dir_path.join(file_name);
        let mut file = File::create(&file_path).expect("Failed to create temp config file");
        write!(file, "{}", content).expect("Failed to write to temp config file");
    }

    #[test]
    fn test_load_config_success() {
        let dir = tempdir().expect("Failed to create temp dir");
        let config_path = dir.path().join("valid_config.json");

        let config_content = serde_json::json!({
            "relayers": [{
                "id": "test-relayer",
                "name": "Test Relayer",
                "network": "test-network",
                "paused": false,
                "network_type": "evm",
                "signer_id": "test-signer"
            }],
            "signers": [{
                "id": "test-signer",
                "type": "local",
                "config": {
                    "path": "tests/utils/test_keys/unit-test-local-signer.json",
                    "passphrase": {
                        "value": "test",
                        "type": "plain"
                    }
                }
            }],
            "notifications": [{
                "id": "test-notification",
                "type": "webhook",
                "url": "https://api.example.com/notifications"
            }],
            "networks": [{
                "type": "evm",
                "network": "test-network",
                "chain_id": 31337,
                "required_confirmations": 1,
                "symbol": "ETH",
                "rpc_urls": ["https://rpc.test.example.com"],
                "is_testnet": true
            }],
            "plugins": [{
                "id": "plugin-id",
                "path": "/app/plugins/plugin.ts",
                "timeout": 12
            }],
        });

        setup_config_file(dir.path(), "valid_config.json", &config_content.to_string());

        let result = load_config(config_path.to_str().unwrap());
        assert!(result.is_ok());

        let config = result.unwrap();
        assert_eq!(config.relayers.len(), 1);
        assert_eq!(config.signers.len(), 1);
        assert_eq!(config.networks.len(), 1);
        assert_eq!(config.plugins.unwrap().len(), 1);
    }

    #[test]
    fn test_load_config_file_not_found() {
        let result = load_config("non_existent_file.json");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ConfigFileError::IoError(_)));
    }

    #[test]
    fn test_load_config_invalid_json() {
        let dir = tempdir().expect("Failed to create temp dir");
        let config_path = dir.path().join("invalid.json");

        setup_config_file(dir.path(), "invalid.json", "{ invalid json }");

        let result = load_config(config_path.to_str().unwrap());
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ConfigFileError::JsonError(_)));
    }

    #[test]
    fn test_load_config_invalid_config_structure() {
        let dir = tempdir().expect("Failed to create temp dir");
        let config_path = dir.path().join("invalid_structure.json");

        let invalid_config = serde_json::json!({
            "relayers": "not_an_array",
            "signers": [],
            "notifications": [],
            "networks": [{
                "type": "evm",
                "network": "test-network",
                "chain_id": 31337,
                "required_confirmations": 1,
                "symbol": "ETH",
                "rpc_urls": ["https://rpc.test.example.com"]
            }]
        });

        setup_config_file(
            dir.path(),
            "invalid_structure.json",
            &invalid_config.to_string(),
        );

        let result = load_config(config_path.to_str().unwrap());
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ConfigFileError::JsonError(_)));
    }

    #[test]
    fn test_load_config_with_unicode_content() {
        let dir = tempdir().expect("Failed to create temp dir");
        let config_path = dir.path().join("unicode_config.json");

        // Use ASCII-compatible IDs since the validation might reject Unicode in IDs
        let config_content = serde_json::json!({
            "relayers": [{
                "id": "test-relayer-unicode",
                "name": "Test Relayer 测试",
                "network": "test-network-unicode",
                "paused": false,
                "network_type": "evm",
                "signer_id": "test-signer-unicode"
            }],
            "signers": [{
                "id": "test-signer-unicode",
                "type": "local",
                "config": {
                    "path": "tests/utils/test_keys/unit-test-local-signer.json",
                    "passphrase": {
                        "value": "test",
                        "type": "plain"
                    }
                }
            }],
            "notifications": [{
                "id": "test-notification-unicode",
                "type": "webhook",
                "url": "https://api.example.com/notifications"
            }],
            "networks": [{
                "type": "evm",
                "network": "test-network-unicode",
                "chain_id": 31337,
                "required_confirmations": 1,
                "symbol": "ETH",
                "rpc_urls": ["https://rpc.test.example.com"],
                "is_testnet": true
            }],
            "plugins": []
        });

        setup_config_file(
            dir.path(),
            "unicode_config.json",
            &config_content.to_string(),
        );

        let result = load_config(config_path.to_str().unwrap());
        assert!(result.is_ok());

        let config = result.unwrap();
        assert_eq!(config.relayers[0].id, "test-relayer-unicode");
        assert_eq!(config.signers[0].id, "test-signer-unicode");
    }

    #[test]
    fn test_load_config_with_empty_file() {
        let dir = tempdir().expect("Failed to create temp dir");
        let config_path = dir.path().join("empty.json");

        setup_config_file(dir.path(), "empty.json", "");

        let result = load_config(config_path.to_str().unwrap());
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ConfigFileError::JsonError(_)));
    }

    #[test]
    fn test_config_serialization_works() {
        let config = create_valid_config();

        let serialized = serde_json::to_string(&config);
        assert!(serialized.is_ok());

        // Just test that serialization works, not round-trip due to complex serde structure
        let serialized_str = serialized.unwrap();
        assert!(!serialized_str.is_empty());
        assert!(serialized_str.contains("relayers"));
        assert!(serialized_str.contains("signers"));
        assert!(serialized_str.contains("networks"));
    }

    #[test]
    fn test_config_serialization_contains_expected_fields() {
        let config = create_valid_config();

        let serialized = serde_json::to_string(&config);
        assert!(serialized.is_ok());

        let serialized_str = serialized.unwrap();

        // Check that important fields are present in serialized JSON
        assert!(serialized_str.contains("\"id\":\"test-1\""));
        assert!(serialized_str.contains("\"name\":\"Test Relayer\""));
        assert!(serialized_str.contains("\"network\":\"test-network\""));
        assert!(serialized_str.contains("\"type\":\"evm\""));
    }

    #[test]
    fn test_validate_relayers_method() {
        let config = create_valid_config();
        let result = config.validate_relayers(&config.networks);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_signers_method() {
        let config = create_valid_config();
        let result = config.validate_signers();
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_notifications_method() {
        let config = create_valid_config();
        let result = config.validate_notifications();
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_networks_method() {
        let config = create_valid_config();
        let result = config.validate_networks();
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_plugins_method() {
        let config = create_valid_config();
        let result = config.validate_plugins();
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_plugins_method_with_empty_plugins() {
        let config = Config {
            relayers: vec![],
            signers: vec![],
            notifications: vec![],
            networks: NetworksFileConfig::new(vec![]).unwrap(),
            plugins: Some(vec![]),
        };
        let result = config.validate_plugins();
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_plugins_method_with_invalid_plugin_extension() {
        let config = Config {
            relayers: vec![],
            signers: vec![],
            notifications: vec![],
            networks: NetworksFileConfig::new(vec![]).unwrap(),
            plugins: Some(vec![PluginFileConfig {
                id: "id".to_string(),
                path: "/app/plugins/test-plugin.js".to_string(),
                timeout: None,
            }]),
        };
        let result = config.validate_plugins();
        assert!(result.is_err());
    }

    #[test]
    fn test_config_with_maximum_length_ids() {
        let mut config = create_valid_config();
        let max_length_id = "a".repeat(36); // Maximum allowed length
        config.relayers[0].id = max_length_id.clone();
        config.relayers[0].signer_id = config.signers[0].id.clone();

        let result = config.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_config_with_special_characters_in_names() {
        let mut config = create_valid_config();
        config.relayers[0].name = "Test-Relayer_123!@#$%^&*()".to_string();

        let result = config.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_config_with_very_long_urls() {
        let mut config = create_valid_config();
        let long_url = format!(
            "https://very-long-domain-name-{}.example.com/api/v1/endpoint",
            "x".repeat(100)
        );
        config.notifications[0].url = long_url;

        let result = config.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_config_with_only_signers_validation() {
        let config = Config {
            relayers: vec![],
            signers: vec![SignerFileConfig {
                id: "test-signer".to_string(),
                config: SignerFileConfigEnum::Local(LocalSignerFileConfig {
                    path: "test-path".to_string(),
                    passphrase: PlainOrEnvValue::Plain {
                        value: SecretString::new("test-passphrase"),
                    },
                }),
            }],
            notifications: vec![],
            networks: NetworksFileConfig::new(vec![NetworkFileConfig::Evm(EvmNetworkConfig {
                common: NetworkConfigCommon {
                    network: "test-network".to_string(),
                    from: None,
                    rpc_urls: Some(vec!["https://rpc.test.example.com".to_string()]),
                    explorer_urls: Some(vec!["https://explorer.test.example.com".to_string()]),
                    average_blocktime_ms: Some(12000),
                    is_testnet: Some(true),
                    tags: Some(vec!["test".to_string()]),
                },
                chain_id: Some(31337),
                required_confirmations: Some(1),
                features: None,
                symbol: Some("ETH".to_string()),
            })])
            .unwrap(),
            plugins: Some(vec![]),
        };

        let result = config.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_config_with_only_notifications() {
        let config = Config {
            relayers: vec![],
            signers: vec![],
            notifications: vec![NotificationConfig {
                id: "test-notification".to_string(),
                r#type: NotificationType::Webhook,
                url: "https://api.example.com/notifications".to_string(),
                signing_key: None,
            }],
            networks: NetworksFileConfig::new(vec![NetworkFileConfig::Evm(EvmNetworkConfig {
                common: NetworkConfigCommon {
                    network: "test-network".to_string(),
                    from: None,
                    rpc_urls: Some(vec!["https://rpc.test.example.com".to_string()]),
                    explorer_urls: Some(vec!["https://explorer.test.example.com".to_string()]),
                    average_blocktime_ms: Some(12000),
                    is_testnet: Some(true),
                    tags: Some(vec!["test".to_string()]),
                },
                chain_id: Some(31337),
                required_confirmations: Some(1),
                features: None,
                symbol: Some("ETH".to_string()),
            })])
            .unwrap(),
            plugins: Some(vec![]),
        };

        let result = config.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_config_with_mixed_network_types_in_relayers() {
        let mut config = create_valid_config();

        // Add Solana relayer
        config.relayers.push(RelayerFileConfig {
            id: "solana-relayer".to_string(),
            name: "Solana Test Relayer".to_string(),
            network: "devnet".to_string(),
            paused: false,
            network_type: ConfigFileNetworkType::Solana,
            policies: None,
            signer_id: "test-1".to_string(),
            notification_id: None,
            custom_rpc_urls: None,
        });

        // Add Stellar relayer
        config.relayers.push(RelayerFileConfig {
            id: "stellar-relayer".to_string(),
            name: "Stellar Test Relayer".to_string(),
            network: "testnet".to_string(),
            paused: true,
            network_type: ConfigFileNetworkType::Stellar,
            policies: None,
            signer_id: "test-1".to_string(),
            notification_id: Some("test-1".to_string()),
            custom_rpc_urls: None,
        });

        let devnet_network = NetworkFileConfig::Solana(SolanaNetworkConfig {
            common: NetworkConfigCommon {
                network: "devnet".to_string(),
                from: None,
                rpc_urls: Some(vec!["https://api.devnet.solana.com".to_string()]),
                explorer_urls: Some(vec!["https://explorer.solana.com".to_string()]),
                average_blocktime_ms: Some(400),
                is_testnet: Some(true),
                tags: Some(vec!["test".to_string()]),
            },
        });

        let testnet_network = NetworkFileConfig::Stellar(StellarNetworkConfig {
            common: NetworkConfigCommon {
                network: "testnet".to_string(),
                from: None,
                rpc_urls: Some(vec!["https://soroban-testnet.stellar.org".to_string()]),
                explorer_urls: Some(vec!["https://stellar.expert/explorer/testnet".to_string()]),
                average_blocktime_ms: Some(5000),
                is_testnet: Some(true),
                tags: Some(vec!["test".to_string()]),
            },
            passphrase: Some("Test SDF Network ; September 2015".to_string()),
        });

        let mut networks = config.networks.networks;
        networks.push(devnet_network);
        networks.push(testnet_network);
        config.networks =
            NetworksFileConfig::new(networks).expect("Failed to create NetworksFileConfig");

        let result = config.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_config_with_all_network_types() {
        let mut config = create_valid_config();

        // Add Solana network
        let solana_network = NetworkFileConfig::Solana(SolanaNetworkConfig {
            common: NetworkConfigCommon {
                network: "solana-test".to_string(),
                from: None,
                rpc_urls: Some(vec!["https://api.devnet.solana.com".to_string()]),
                explorer_urls: Some(vec!["https://explorer.test.example.com".to_string()]),
                average_blocktime_ms: Some(400),
                is_testnet: Some(true),
                tags: Some(vec!["solana".to_string()]),
            },
        });

        // Add Stellar network
        let stellar_network = NetworkFileConfig::Stellar(StellarNetworkConfig {
            common: NetworkConfigCommon {
                network: "stellar-test".to_string(),
                from: None,
                rpc_urls: Some(vec!["https://horizon-testnet.stellar.org".to_string()]),
                explorer_urls: Some(vec!["https://explorer.test.example.com".to_string()]),
                average_blocktime_ms: Some(5000),
                is_testnet: Some(true),
                tags: Some(vec!["stellar".to_string()]),
            },
            passphrase: Some("Test Network ; September 2015".to_string()),
        });

        // Get the existing networks and add new ones
        let mut existing_networks = Vec::new();
        for network in config.networks.iter() {
            existing_networks.push(network.clone());
        }
        existing_networks.push(solana_network);
        existing_networks.push(stellar_network);

        config.networks = NetworksFileConfig::new(existing_networks).unwrap();

        let result = config.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_config_error_propagation_from_relayers() {
        let mut config = create_valid_config();
        config.relayers[0].id = "".to_string(); // Invalid empty ID

        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::MissingField(_)
        ));
    }

    #[test]
    fn test_config_error_propagation_from_signers() {
        let mut config = create_valid_config();
        config.signers[0].id = "".to_string(); // Invalid empty ID

        let result = config.validate();
        assert!(result.is_err());
        // The error should be InvalidIdLength since empty ID is caught by signer validation
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::InvalidIdLength(_)
        ));
    }

    #[test]
    fn test_config_error_propagation_from_notifications() {
        let mut config = create_valid_config();
        config.notifications[0].id = "".to_string(); // Invalid empty ID

        let result = config.validate();
        assert!(result.is_err());

        let error = result.unwrap_err();
        assert!(matches!(error, ConfigFileError::InvalidFormat(_)));
    }

    #[test]
    fn test_config_with_paused_relayers() {
        let mut config = create_valid_config();
        config.relayers[0].paused = true;

        let result = config.validate();
        assert!(result.is_ok()); // Paused relayers should still be valid
    }

    #[test]
    fn test_config_with_none_notification_id() {
        let mut config = create_valid_config();
        config.relayers[0].notification_id = None;

        let result = config.validate();
        assert!(result.is_ok()); // None notification_id should be valid
    }

    #[test]
    fn test_config_file_network_type_display() {
        let evm = ConfigFileNetworkType::Evm;
        let solana = ConfigFileNetworkType::Solana;
        let stellar = ConfigFileNetworkType::Stellar;

        // Test that Debug formatting works (which is what we have)
        let evm_str = format!("{:?}", evm);
        let solana_str = format!("{:?}", solana);
        let stellar_str = format!("{:?}", stellar);

        assert!(evm_str.contains("Evm"));
        assert!(solana_str.contains("Solana"));
        assert!(stellar_str.contains("Stellar"));
    }

    #[test]
    fn test_config_file_plugins_validation_with_empty_plugins() {
        let config = Config {
            relayers: vec![],
            signers: vec![],
            notifications: vec![],
            networks: NetworksFileConfig::new(vec![]).unwrap(),
            plugins: None,
        };
        let result = config.validate_plugins();
        assert!(result.is_ok());
    }

    #[test]
    fn test_config_file_without_plugins() {
        let dir = tempdir().expect("Failed to create temp dir");
        let config_path = dir.path().join("valid_config.json");

        let config_content = serde_json::json!({
            "relayers": [{
                "id": "test-relayer",
                "name": "Test Relayer",
                "network": "test-network",
                "paused": false,
                "network_type": "evm",
                "signer_id": "test-signer"
            }],
            "signers": [{
                "id": "test-signer",
                "type": "local",
                "config": {
                    "path": "tests/utils/test_keys/unit-test-local-signer.json",
                    "passphrase": {
                        "value": "test",
                        "type": "plain"
                    }
                }
            }],
            "notifications": [{
                "id": "test-notification",
                "type": "webhook",
                "url": "https://api.example.com/notifications"
            }],
            "networks": [{
                "type": "evm",
                "network": "test-network",
                "chain_id": 31337,
                "required_confirmations": 1,
                "symbol": "ETH",
                "rpc_urls": ["https://rpc.test.example.com"],
                "is_testnet": true
            }]
        });

        setup_config_file(dir.path(), "valid_config.json", &config_content.to_string());

        let result = load_config(config_path.to_str().unwrap());
        assert!(result.is_ok());

        let config = result.unwrap();
        assert_eq!(config.relayers.len(), 1);
        assert_eq!(config.signers.len(), 1);
        assert_eq!(config.networks.len(), 1);
        assert!(config.plugins.is_none());
    }
}
