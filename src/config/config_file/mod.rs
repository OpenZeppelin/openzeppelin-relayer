use serde::{Deserialize, Serialize};
use std::{collections::HashSet, fs};
use thiserror::Error;

mod relayer;
pub use relayer::*;

mod signer;
pub use signer::*;

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
    // pub networks: Vec<String>,
    // pub accounts: Vec<String>,
    // notifications
}

impl Config {
    pub fn validate(&self) -> Result<(), ConfigFileError> {
        RelayersFileConfig::new(self.relayers.clone()).validate()?;
        SignersFileConfig::new(self.signers.clone()).validate()?;

        self.validate_relayer_signer_refs()?;

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
            }],
            signers: vec![SignerFileConfig {
                id: "test-1".to_string(),
                path: Some("examples/basic-example/keys/local-signer.json".to_string()),
                r#type: SignerFileConfigType::Local,
                passphrase: Some(SignerFileConfigPassphrase::Plain {
                    value: "test".to_string(),
                }),
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
}
