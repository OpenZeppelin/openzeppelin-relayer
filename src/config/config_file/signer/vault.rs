use crate::config::ConfigFileError;
use async_trait::async_trait;
use reqwest::Url;
use serde::{Deserialize, Serialize};

use super::{KeyLoaderTrait, SignerConfigValidate};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct VaultSignerFileConfig {
    pub address: String,
    pub namespace: Option<String>,
    pub role_id: String,
    pub secret_id: String,
    pub key_name: String,
}

impl VaultSignerFileConfig {
    fn validate_address(&self) -> Result<(), ConfigFileError> {
        if self.address.is_empty() {
            return Err(ConfigFileError::InvalidIdLength(
                "Vault address cannot be empty".into(),
            ));
        }
        Url::parse(&self.address).map_err(|err| {
            ConfigFileError::InvalidFormat(format!("Invalid Vault address: {}", err))
        })?;

        Ok(())
    }

    fn validate_role_id(&self) -> Result<(), ConfigFileError> {
        if self.role_id.is_empty() {
            return Err(ConfigFileError::InvalidIdLength(
                "Vault role ID cannot be empty".into(),
            ));
        }
        Ok(())
    }

    fn validate_secret_id(&self) -> Result<(), ConfigFileError> {
        if self.secret_id.is_empty() {
            return Err(ConfigFileError::InvalidIdLength(
                "Vault secret ID cannot be empty".into(),
            ));
        }
        Ok(())
    }

    fn validate_key_name(&self) -> Result<(), ConfigFileError> {
        if self.key_name.is_empty() {
            return Err(ConfigFileError::InvalidIdLength(
                "Vault key name cannot be empty".into(),
            ));
        }
        Ok(())
    }
}

impl SignerConfigValidate for VaultSignerFileConfig {
    fn validate(&self) -> Result<(), ConfigFileError> {
        self.validate_address()?;
        self.validate_secret_id()?;
        self.validate_role_id()?;
        self.validate_key_name()?;
        Ok(())
    }
}

#[async_trait]
impl KeyLoaderTrait for VaultSignerFileConfig {
    async fn load_key(&self) -> Result<Vec<u8>, ConfigFileError> {
        let key_raw = format!("{}:{}", self.role_id, self.secret_id);
        Ok(key_raw.into_bytes())
    }
}
