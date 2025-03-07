use async_trait::async_trait;
use oz_keystore::LocalClient;
use serde::{Deserialize, Serialize};

use crate::{config::ConfigFileError, utils::unsafe_generate_random_private_key};

use super::{PlainOrEnvConfigValue, SignerConfig, SignerConfigValidate, SignerFileConfig};
use std::path::Path;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LocalSignerFileConfig {
    pub path: Option<String>,
    pub passphrase: Option<PlainOrEnvConfigValue>,
}

impl LocalSignerFileConfig {
    fn validate_path(&self) -> Result<(), ConfigFileError> {
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
        match &self.passphrase {
            Some(passphrase) => match passphrase {
                PlainOrEnvConfigValue::Env { name } => {
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
                PlainOrEnvConfigValue::Plain { value } => {
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
}

impl SignerConfigValidate for LocalSignerFileConfig {
    fn validate(&self) -> Result<(), ConfigFileError> {
        self.validate_path()?;
        self.validate_passphrase()?;

        Ok(())
    }
}

#[async_trait]
pub trait SignerConfigKeystore {
    fn load_keystore(&self) -> Result<Vec<u8>, ConfigFileError>;
    fn get_passphrase(&self) -> Result<String, ConfigFileError>;
}

#[async_trait]
impl SignerConfigKeystore for SignerFileConfig {
    fn load_keystore(&self) -> Result<Vec<u8>, ConfigFileError> {
        match &self.config {
            SignerConfig::Test(_) => {
                // generate temporary key
                let key_raw = unsafe_generate_random_private_key();
                Ok(key_raw)
            }
            SignerConfig::Local(local_config) => {
                let path = local_config.path.as_ref().ok_or_else(|| {
                    ConfigFileError::MissingField("Signer path is required for local signer".into())
                })?;
                let passphrase = self.get_passphrase()?;
                let key_raw = LocalClient::load(Path::new(path).to_path_buf(), passphrase);
                Ok(key_raw)
            }
            _ => Err(ConfigFileError::InternalError("Not supported".into())),
        }
    }

    fn get_passphrase(&self) -> Result<String, ConfigFileError> {
        let config = self.config.get_local().ok_or_else(|| {
            ConfigFileError::MissingField("Local signer config is required".into())
        })?;
        match &config.passphrase {
            Some(passphrase) => match passphrase {
                PlainOrEnvConfigValue::Env { name } => {
                    let passphrase = std::env::var(name).map_err(|_| {
                        ConfigFileError::MissingEnvVar(format!(
                            "Environment variable {} not found",
                            name
                        ))
                    })?;
                    Ok(passphrase)
                }
                PlainOrEnvConfigValue::Plain { value } => Ok(value.clone()),
            },
            None => Err(ConfigFileError::MissingField(
                "Passphrase cannot be empty".into(),
            )),
        }
    }
}
