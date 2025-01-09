use crate::models::{EvmNetwork, SolanaNetwork, StellarNetwork};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, fs};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ConfigError {
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
    InvalidNetworkType(String),
    #[error("Invalid network name for {network_type}: {name}")]
    InvalidNetwork { network_type: String, name: String },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    pub relayers: Vec<RelayerConfig>,
    // pub networks: Vec<String>,
    // pub accounts: Vec<String>,
    // notifications
    // signers
}

impl Config {
    fn validate_relayer_id_uniqueness(relayers: &[RelayerConfig]) -> Result<(), ConfigError> {
        let mut seen_ids = HashSet::new();
        for relayer in relayers {
            if !seen_ids.insert(&relayer.id) {
                return Err(ConfigError::DuplicateId(format!(
                    "Duplicate relayer ID found: {}",
                    relayer.id
                )));
            }
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        // Validate relayers field
        if self.relayers.is_empty() {
            return Err(ConfigError::MissingField("relayers".into()));
        }
        // Check for duplicate IDs
        Self::validate_relayer_id_uniqueness(&self.relayers)?;

        for relayer in &self.relayers {
            relayer.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum NetworkType {
    Evm,
    Stellar,
    Solana,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RelayerConfig {
    pub id: String,
    pub name: String,
    pub network: String,
    pub paused: bool,
    pub network_type: NetworkType,
}

impl RelayerConfig {
    const MAX_ID_LENGTH: usize = 36;

    fn validate_network(&self) -> Result<(), ConfigError> {
        match self.network_type {
            NetworkType::Evm => {
                if EvmNetwork::from_network_str(&self.network).is_err() {
                    return Err(ConfigError::InvalidNetwork {
                        network_type: "EVM".to_string(),
                        name: self.network.clone(),
                    });
                }
            }
            NetworkType::Stellar => {
                if StellarNetwork::from_network_str(&self.network).is_err() {
                    return Err(ConfigError::InvalidNetwork {
                        network_type: "Stellar".to_string(),
                        name: self.network.clone(),
                    });
                }
            }
            NetworkType::Solana => {
                if SolanaNetwork::from_network_str(&self.network).is_err() {
                    return Err(ConfigError::InvalidNetwork {
                        network_type: "Solana".to_string(),
                        name: self.network.clone(),
                    });
                }
            }
        }
        Ok(())
    }

    // TODO add networks validation
    // TODO add validation that multiple relayers on same network cannot use same signer
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.id.is_empty() {
            return Err(ConfigError::MissingField("relayer id".into()));
        }
        if !Regex::new(r"^[a-zA-Z0-9-_]+$").unwrap().is_match(&self.id) {
            return Err(ConfigError::InvalidIdFormat(
                "ID must contain only letters, numbers, dashes and underscores".into(),
            ));
        }
        if self.id.len() > Self::MAX_ID_LENGTH {
            return Err(ConfigError::InvalidIdLength(format!(
                "ID length must not exceed {} characters",
                Self::MAX_ID_LENGTH
            )));
        }
        if self.name.is_empty() {
            return Err(ConfigError::MissingField("relayer name".into()));
        }
        if self.network.is_empty() {
            return Err(ConfigError::MissingField("network".into()));
        }
        if self.network.is_empty() {
            return Err(ConfigError::MissingField("paused".into()));
        }

        self.validate_network()?;

        Ok(())
    }
}

pub fn load_config() -> Result<Config, ConfigError> {
    let config_str = fs::read_to_string("config.json")?;
    let config: Config = serde_json::from_str(&config_str)?;
    config.validate()?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_valid_config() -> Config {
        Config {
            relayers: vec![RelayerConfig {
                id: "test-1".to_string(),
                name: "Test Relayer".to_string(),
                network: "testnet".to_string(),
                paused: false,
                network_type: NetworkType::Evm,
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
        let config = Config { relayers: vec![] };
        assert!(matches!(
            config.validate(),
            Err(ConfigError::MissingField(_))
        ));
    }

    #[test]
    fn test_invalid_id_format() {
        let mut config = create_valid_config();
        config.relayers[0].id = "invalid@id".to_string();
        assert!(matches!(
            config.validate(),
            Err(ConfigError::InvalidIdFormat(_))
        ));
    }

    #[test]
    fn test_id_too_long() {
        let mut config = create_valid_config();
        config.relayers[0].id = "a".repeat(37);
        assert!(matches!(
            config.validate(),
            Err(ConfigError::InvalidIdLength(_))
        ));
    }

    #[test]
    fn test_duplicate_ids() {
        let mut config = create_valid_config();
        config.relayers.push(config.relayers[0].clone());
        assert!(matches!(
            config.validate(),
            Err(ConfigError::DuplicateId(_))
        ));
    }

    #[test]
    fn test_missing_name() {
        let mut config = create_valid_config();
        config.relayers[0].name = "".to_string();
        assert!(matches!(
            config.validate(),
            Err(ConfigError::MissingField(_))
        ));
    }

    #[test]
    fn test_missing_network() {
        let mut config = create_valid_config();
        config.relayers[0].network = "".to_string();
        assert!(matches!(
            config.validate(),
            Err(ConfigError::MissingField(_))
        ));
    }
}
