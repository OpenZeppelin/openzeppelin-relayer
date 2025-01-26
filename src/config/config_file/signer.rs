//! Configuration file definitions for signer services.
//!
//! Provides configuration structures and validation for different signer types:
//! - Local keystore (encrypted JSON files)
//! - AWS KMS integration [NOT IMPLEMENTED]
//! - HashiCorp Vault integration [NOT IMPLEMENTED]
use super::ConfigFileError;
use async_trait::async_trait;
use oz_keystore::LocalClient;
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, path::Path};

#[async_trait]
pub trait SignerConfigKeystore {
    async fn load_keystore(&self) -> Result<Vec<u8>, ConfigFileError>;
    async fn get_passphrase(&self) -> Result<String, ConfigFileError>;
}

#[derive(Debug, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct SignerFileConfig {
    pub id: String,
    pub r#type: SignerFileConfigType,
    pub path: Option<String>,
    pub passphrase: Option<SignerFileConfigPassphrase>,
}

#[async_trait]
impl SignerConfigKeystore for SignerFileConfig {
    async fn load_keystore(&self) -> Result<Vec<u8>, ConfigFileError> {
        match &self.r#type {
            SignerFileConfigType::Local => {
                let path = self.path.as_ref().ok_or_else(|| {
                    ConfigFileError::MissingField("Signer path is required for local signer".into())
                })?;
                let passphrase = self.get_passphrase().await?;
                let key_raw = LocalClient::load(Path::new(path).to_path_buf(), passphrase);
                Ok(key_raw)
            }
            SignerFileConfigType::AwsKms => {
                Err(ConfigFileError::InternalError("Not implemented".into()))
            }
            SignerFileConfigType::Vault => {
                Err(ConfigFileError::InternalError("Not implemented".into()))
            }
        }
    }

    async fn get_passphrase(&self) -> Result<String, ConfigFileError> {
        match &self.passphrase {
            Some(passphrase) => match passphrase {
                SignerFileConfigPassphrase::Env { name } => {
                    let passphrase = std::env::var(name).map_err(|_| {
                        ConfigFileError::MissingEnvVar(format!(
                            "Environment variable {} not found",
                            name
                        ))
                    })?;
                    Ok(passphrase)
                }
                SignerFileConfigPassphrase::Plain { value } => Ok(value.clone()),
            },
            None => Err(ConfigFileError::MissingField(
                "Passphrase cannot be empty".into(),
            )),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SignerFileConfigType {
    Local,
    AwsKms,
    Vault,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SignerFileConfigPassphrase {
    Env { name: String },
    Plain { value: String },
}

impl SignerFileConfig {
    fn validate_path(&self) -> Result<(), ConfigFileError> {
        if self.r#type != SignerFileConfigType::Local {
            return Ok(());
        }

        let path = self.path.as_ref().ok_or_else(|| {
            ConfigFileError::MissingField("Signer path is required for local signer".into())
        })?;

        if path.is_empty() {
            return Err(ConfigFileError::InvalidIdLength(
                "Signer path cannot be empty".into(),
            ));
        }

        let path = Path::new(path);
        if !path.exists() {
            return Err(ConfigFileError::FileNotFound(format!(
                "Signer file not found at path: {}",
                path.display()
            )));
        }

        if !path.is_file() {
            return Err(ConfigFileError::InvalidFormat(format!(
                "Path exists but is not a file: {}",
                path.display()
            )));
        }

        Ok(())
    }

    fn validate_passphrase(&self) -> Result<(), ConfigFileError> {
        if self.r#type != SignerFileConfigType::Local {
            return Ok(());
        }
        match &self.passphrase {
            Some(passphrase) => match passphrase {
                SignerFileConfigPassphrase::Env { name } => {
                    if name.is_empty() {
                        return Err(ConfigFileError::MissingField(
                            "Passphrase environment variable name cannot be empty".into(),
                        ));
                    }
                    if std::env::var(name).is_err() {
                        return Err(ConfigFileError::MissingEnvVar(format!(
                            "Environment variable {} not found",
                            name
                        )));
                    }
                }
                SignerFileConfigPassphrase::Plain { value } => {
                    if value.is_empty() {
                        return Err(ConfigFileError::InvalidFormat(
                            "Passphrase value cannot be empty".into(),
                        ));
                    }
                }
            },
            None => {
                return Err(ConfigFileError::MissingField(
                    "Passphrase cannot be empty".into(),
                ));
            }
        }

        Ok(())
    }

    fn validate_local(&self) -> Result<(), ConfigFileError> {
        self.validate_path()?;
        self.validate_passphrase()?;

        Ok(())
    }

    pub fn validate_signer(&self) -> Result<(), ConfigFileError> {
        if self.id.is_empty() {
            return Err(ConfigFileError::InvalidIdLength(
                "Signer ID cannot be empty".into(),
            ));
        }

        match &self.r#type {
            SignerFileConfigType::Local => {
                return self.validate_local();
            }
            SignerFileConfigType::AwsKms => {
                return Err(ConfigFileError::InternalError("Not implemented".into()));
            }
            SignerFileConfigType::Vault => {
                if self.path.is_some() {
                    return Err(ConfigFileError::InternalError("Not implemented".into()));
                }
            }
        }
        Ok(())
    }
}

use serde::{de, Deserializer};
use serde_json::Value;

impl<'de> Deserialize<'de> for SignerFileConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Deserialize as a generic JSON object
        let value: Value = Deserialize::deserialize(deserializer)?;
        // Extract and validate required fields
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| de::Error::missing_field("id"))?
            .to_string();

        // Deserialize `signer_type`
        let r#type: SignerFileConfigType = serde_json::from_value(
            value
                .get("type")
                .cloned()
                .ok_or_else(|| de::Error::missing_field("type"))?,
        )
        .map_err(|_| de::Error::unknown_field("type", &["type"]))?;

        // Construct and return the struct
        Ok(SignerFileConfig {
            id,
            r#type,
            path: value
                .get("path")
                .and_then(Value::as_str)
                .map(|s| s.to_string()),
            passphrase: value
                .get("passphrase")
                .map(|v| serde_json::from_value(v.clone()).map_err(de::Error::custom))
                .transpose()?,
        })
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
            "path": "examples/basic-example/keys/local-signer.json",
            "passphrase": {
                "type": "plain",
                "value": "secret",
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
            "path": "examples/basic-example/keys/local-signer.json",
            "passphrase": {
                "type": "env",
                "name": "LOCAL_SIGNER_KEY_PASSPHRASE"
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
                  "path": "examples/basic-example/keys/local-signer.json",
                  "passphrase": {
                      "type": "plain",
                      "value": "secret",
                  }
                },
                {
                  "id": "local-signer",
                  "type": "local",
                  "path": "examples/basic-example/keys/local-signer.json",
                  "passphrase": {
                      "type": "plain",
                      "value": "secret",
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
                  "path": "examples/basic-example/keys/local-signer.json",
                  "passphrase": {
                      "type": "plain",
                      "value": "secret",
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
