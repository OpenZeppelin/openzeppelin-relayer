use serde::{Deserialize, Serialize};
use std::{collections::HashSet, fs};
use thiserror::Error;

mod relayer;
pub use relayer::*;

mod signer;
pub use signer::*;

mod notification;
pub use notification::*;

use crate::models::{EvmNetwork, SolanaNetwork, StellarNetwork};

#[derive(Error, Debug)]
pub enum ConfigFileError {
    #[error("Invalid ID length: {0}")]
    InvalidIdLength(String),
    #[error("Invalid ID format: {0}")]
    InvalidIdFormat(String),
    #[error("Missing required field: {0}")]
    MissingField(String),
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),
    #[error("Duplicate id error: {0}")]
    DuplicateId(String),
    #[error("Invalid network type: {0}")]
    InvalidConfigFileNetworkType(String),
    #[error("Invalid network name for {network_type}: {name}")]
    InvalidNetwork { network_type: String, name: String },
    #[error("Invalid policy: {0}")]
    InvalidPolicy(String),
    #[error("Internal error: {0}")]
    InternalError(String),
    #[error("Missing env var: {0}")]
    MissingEnvVar(String),
    #[error("Invalid format: {0}")]
    InvalidFormat(String),
    #[error("File not found: {0}")]
    FileNotFound(String),
    #[error("Invalid reference: {0}")]
    InvalidReference(String),
    #[error("File read error: {0}")]
    FileRead(String),
    #[error("Test Signer error: {0}")]
    TestSigner(String),
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ConfigFileNetworkType {
    Evm,
    Stellar,
    Solana,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    pub relayers: Vec<RelayerFileConfig>,
    pub signers: Vec<SignerFileConfig>,
    pub notifications: Vec<NotificationFileConfig>,
    // pub networks: Vec<String>,
    // pub accounts: Vec<String>,
}

impl Config {
    pub fn validate(&self) -> Result<(), ConfigFileError> {
        RelayersFileConfig::new(self.relayers.clone()).validate()?;
        SignersFileConfig::new(self.signers.clone()).validate()?;
        NotificationsFileConfig::new(self.notifications.clone()).validate()?;
        self.validate_relayer_signer_refs()?;
        self.validate_relayer_notification_refs()?;
        Ok(())
    }

    fn validate_relayer_signer_refs(&self) -> Result<(), ConfigFileError> {
        let signer_ids: HashSet<_> = self.signers.iter().map(|s| &s.id).collect();

        for relayer in &self.relayers {
            if !signer_ids.contains(&relayer.signer_id) {
                return Err(ConfigFileError::InvalidReference(format!(
                    "Relayer '{}' references non-existent signer '{}'",
                    relayer.id, relayer.signer_id
                )));
            }
            let signer_config = self
                .signers
                .iter()
                .find(|s| s.id == relayer.signer_id)
                .ok_or_else(|| {
                    ConfigFileError::InternalError(format!(
                        "Signer '{}' not found for relayer '{}'",
                        relayer.signer_id, relayer.id
                    ))
                })?;

            if signer_config.r#type == SignerFileConfigType::Test {
                // ensure that only testnets are used with test signers
                match relayer.network_type {
                    ConfigFileNetworkType::Evm => {
                        let network =
                            EvmNetwork::from_network_str(&relayer.network).map_err(|_| {
                                ConfigFileError::InvalidNetwork {
                                    network_type: "EVM".to_string(),
                                    name: relayer.network.clone(),
                                }
                            })?;
                        if !network.is_testnet() {
                            return Err(ConfigFileError::TestSigner(
                                "Test signer type cannot be used on production networks"
                                    .to_string(),
                            ));
                        }
                    }
                    ConfigFileNetworkType::Solana => {
                        let network =
                            SolanaNetwork::from_network_str(&relayer.network).map_err(|_| {
                                ConfigFileError::InvalidNetwork {
                                    network_type: "EVM".to_string(),
                                    name: relayer.network.clone(),
                                }
                            })?;
                        if !network.is_testnet() {
                            return Err(ConfigFileError::TestSigner(
                                "Test signer type cannot be used on production networks"
                                    .to_string(),
                            ));
                        }
                    }
                    ConfigFileNetworkType::Stellar => {
                        let network =
                            StellarNetwork::from_network_str(&relayer.network).map_err(|_| {
                                ConfigFileError::InvalidNetwork {
                                    network_type: "EVM".to_string(),
                                    name: relayer.network.clone(),
                                }
                            })?;
                        if !network.is_testnet() {
                            return Err(ConfigFileError::TestSigner(
                                "Test signer type cannot be used on production networks"
                                    .to_string(),
                            ));
                        }
                    }
                }
            }
        }

        Ok(())
    }

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
}

pub fn load_config(config_file_path: &str) -> Result<Config, ConfigFileError> {
    let config_str = fs::read_to_string(config_file_path)?;
    let config: Config = serde_json::from_str(&config_str)?;
    config.validate()?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_valid_config() -> Config {
        Config {
            relayers: vec![RelayerFileConfig {
                id: "test-1".to_string(),
                name: "Test Relayer".to_string(),
                network: "sepolia".to_string(),
                paused: false,
                network_type: ConfigFileNetworkType::Evm,
                policies: None,
                signer_id: "test-1".to_string(),
                notification_id: Some("test-1".to_string()),
            }],
            signers: vec![
                SignerFileConfig {
                    id: "test-1".to_string(),
                    path: Some("examples/basic-example/config/keys/local-signer.json".to_string()),
                    r#type: SignerFileConfigType::Local,
                    passphrase: Some(SignerFileConfigPassphrase::Plain {
                        value: "test".to_string(),
                    }),
                },
                SignerFileConfig {
                    id: "test-type".to_string(),
                    path: None,
                    r#type: SignerFileConfigType::Test,
                    passphrase: None,
                },
            ],
            notifications: vec![NotificationFileConfig {
                id: "test-1".to_string(),
                r#type: NotificationFileConfigType::Webhook,
                url: "https://api.example.com/notifications".to_string(),
                signing_key: None,
            }],
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
        };
        assert!(matches!(
            config.validate(),
            Err(ConfigFileError::MissingField(_))
        ));
    }

    #[test]
    fn test_empty_signers() {
        let config = Config {
            relayers: Vec::new(),
            signers: Vec::new(),
            notifications: Vec::new(),
        };
        assert!(matches!(
            config.validate(),
            Err(ConfigFileError::MissingField(_))
        ));
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
            Err(ConfigFileError::MissingField(_))
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
    fn test_evm_mainnet_not_allowed_for_signer_type_test() {
        let mut config = create_valid_config();
        config.relayers[0].network = "mainnet".to_string();
        config.relayers[0].signer_id = "test-type".to_string();

        let result = config.validate();
        assert!(matches!(
            result,
            Err(ConfigFileError::TestSigner(msg)) if msg.contains("production networks")
        ));
    }

    #[test]
    fn test_evm_sepolia_allowed_for_signer_type_test() {
        let mut config = create_valid_config();
        config.relayers[0].network = "sepolia".to_string();
        config.relayers[0].signer_id = "test-type".to_string();

        let result = config.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_solana_mainnet_not_allowed_for_signer_type_test() {
        let mut config = create_valid_config();
        config.relayers[0].network_type = ConfigFileNetworkType::Solana;
        config.relayers[0].network = "mainnet-beta".to_string();
        config.relayers[0].signer_id = "test-type".to_string();

        let result = config.validate();
        assert!(matches!(
            result,
            Err(ConfigFileError::TestSigner(msg)) if msg.contains("production networks")
        ));
    }

    #[test]
    fn test_solana_devnet_allowed_for_signer_type_test() {
        let mut config = create_valid_config();
        config.relayers[0].network_type = ConfigFileNetworkType::Solana;
        config.relayers[0].network = "devnet".to_string();
        config.relayers[0].signer_id = "test-type".to_string();

        let result = config.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_stellar_mainnet_not_allowed_for_signer_type_test() {
        let mut config = create_valid_config();
        config.relayers[0].network_type = ConfigFileNetworkType::Stellar;
        config.relayers[0].network = "mainnet".to_string();
        config.relayers[0].signer_id = "test-type".to_string();

        let result = config.validate();
        assert!(matches!(
            result,
            Err(ConfigFileError::TestSigner(msg)) if msg.contains("production networks")
        ));
    }

    #[test]
    fn test_stellar_testnet_allowed_for_signer_type_test() {
        let mut config = create_valid_config();
        config.relayers[0].network_type = ConfigFileNetworkType::Stellar;
        config.relayers[0].network = "testnet".to_string();
        config.relayers[0].signer_id = "test-type".to_string();

        let result = config.validate();
        assert!(result.is_ok());
    }
}
