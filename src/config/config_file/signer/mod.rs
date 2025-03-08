//! Configuration file definitions for signer services.
//!
//! Provides configuration structures and validation for different signer types:
//! - Test (temporary private keys)
//! - Local keystore (encrypted JSON files)
//! - AWS KMS integration [NOT IMPLEMENTED]
//! - HashiCorp Vault integration [NOT IMPLEMENTED]
use super::ConfigFileError;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

mod local;
pub use local::*;

mod vault;
pub use vault::*;

mod vault_cloud;
pub use vault_cloud::*;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum PlainOrEnvConfigValue {
    Env { name: String },
    Plain { value: String },
}

impl PlainOrEnvConfigValue {
    pub fn get_value(&self) -> Result<String, ConfigFileError> {
        match self {
            PlainOrEnvConfigValue::Env { name } => {
                let value = std::env::var(name).map_err(|_| {
                    ConfigFileError::MissingEnvVar(format!(
                        "Environment variable {} not found",
                        name
                    ))
                })?;
                Ok(value)
            }
            PlainOrEnvConfigValue::Plain { value } => Ok(value.clone()),
        }
    }
}

pub trait SignerConfigValidate {
    fn validate(&self) -> Result<(), ConfigFileError>;
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TestSignerFileConfig {}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AwsKmsSignerFileConfig {}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase", content = "config")]
pub enum SignerConfig {
    Test(TestSignerFileConfig),
    Local(LocalSignerFileConfig),
    AwsKms(AwsKmsSignerFileConfig),
    Vault(VaultSignerFileConfig),
    #[serde(rename = "vault_cloud")]
    VaultCloud(VaultCloudSignerFileConfig),
}

impl SignerConfig {
    pub fn get_local(&self) -> Option<&LocalSignerFileConfig> {
        match self {
            SignerConfig::Local(local) => Some(local),
            _ => None,
        }
    }

    pub fn get_vault(&self) -> Option<&VaultSignerFileConfig> {
        match self {
            SignerConfig::Vault(vault) => Some(vault),
            _ => None,
        }
    }

    pub fn get_vault_cloud(&self) -> Option<&VaultCloudSignerFileConfig> {
        match self {
            SignerConfig::VaultCloud(vault_cloud) => Some(vault_cloud),
            _ => None,
        }
    }

    pub fn get_test(&self) -> Option<&TestSignerFileConfig> {
        match self {
            SignerConfig::Test(test) => Some(test),
            _ => None,
        }
    }

    pub fn get_aws_kms(&self) -> Option<&AwsKmsSignerFileConfig> {
        match self {
            SignerConfig::AwsKms(aws_kms) => Some(aws_kms),
            _ => None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct SignerFileConfig {
    pub id: String,
    #[serde(flatten)]
    pub config: SignerConfig,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SignerFileConfigPassphrase {
    Env { name: String },
    Plain { value: String },
}

impl SignerFileConfig {
    pub fn validate_signer(&self) -> Result<(), ConfigFileError> {
        if self.id.is_empty() {
            return Err(ConfigFileError::InvalidIdLength(
                "Signer ID cannot be empty".into(),
            ));
        }

        match &self.config {
            SignerConfig::Test(_) => Ok(()),
            SignerConfig::Local(local_config) => local_config.validate(),
            SignerConfig::AwsKms(_) => {
                Err(ConfigFileError::InternalError("Not implemented".into()))
            }
            SignerConfig::Vault(vault_config) => vault_config.validate(),
            SignerConfig::VaultCloud(vault_cloud_config) => vault_cloud_config.validate(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct SignersFileConfig {
    pub signers: Vec<SignerFileConfig>,
}

impl SignersFileConfig {
    pub fn new(signers: Vec<SignerFileConfig>) -> Self {
        Self { signers }
    }

    pub fn validate(&self) -> Result<(), ConfigFileError> {
        if self.signers.is_empty() {
            return Err(ConfigFileError::MissingField("signers".into()));
        }

        let mut ids = HashSet::new();
        for signer in &self.signers {
            signer.validate_signer()?;
            if !ids.insert(signer.id.clone()) {
                return Err(ConfigFileError::DuplicateId(signer.id.clone()));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::env;

    #[test]
    fn test_valid_signer_config() {
        let config = json!({
            "id": "local-signer",
            "type": "local",
            "config": {
                "path": "examples/basic-example/config/keys/local-signer.json",
                "passphrase": {
                    "type": "plain",
                    "value": "secret",
                }
            }
        });

        let signer_config: SignerFileConfig = serde_json::from_value(config).unwrap();
        assert!(signer_config.validate_signer().is_ok());
    }

    #[test]
    fn test_valid_signer_config_env() {
        env::set_var("LOCAL_SIGNER_KEY_PASSPHRASE", "mocked_value");

        let config = json!({
            "id": "local-signer",
            "type": "local",
            "config": {
                "path": "examples/basic-example/config/keys/local-signer.json",
                "passphrase": {
                    "type": "env",
                    "name": "LOCAL_SIGNER_KEY_PASSPHRASE"
                }
            }
        });

        let signer_config: SignerFileConfig = serde_json::from_value(config).unwrap();
        assert!(signer_config.validate_signer().is_ok());
        env::remove_var("LOCAL_SIGNER_KEY_PASSPHRASE");
    }

    #[test]
    fn test_duplicate_signer_ids() {
        let config = json!({
            "signers": [
                {
                  "id": "local-signer",
                  "type": "local",
                  "config": {
                      "path": "examples/basic-example/config/keys/local-signer.json",
                      "passphrase": {
                          "type": "plain",
                          "value": "secret",
                      }
                  }
                },
                {
                  "id": "local-signer",
                  "type": "local",
                  "config": {
                      "path": "examples/basic-example/config/keys/local-signer.json",
                      "passphrase": {
                          "type": "plain",
                          "value": "secret",
                      }
                  }
                }
            ]
        });

        let signer_config: SignersFileConfig = serde_json::from_value(config).unwrap();
        assert!(matches!(
            signer_config.validate(),
            Err(ConfigFileError::DuplicateId(_))
        ));
    }

    #[test]
    fn test_empty_signer_id() {
        let config = json!({
            "signers": [
                {
                  "id": "",
                  "type": "local",
                  "config": {
                    "path": "examples/basic-example/config/keys/local-signer.json",
                    "passphrase": {
                        "type": "plain",
                        "value": "secret",
                    }
                }

                }
            ]
        });

        let signer_config: SignersFileConfig = serde_json::from_value(config).unwrap();
        assert!(matches!(
            signer_config.validate(),
            Err(ConfigFileError::InvalidIdLength(_))
        ));
    }
}
