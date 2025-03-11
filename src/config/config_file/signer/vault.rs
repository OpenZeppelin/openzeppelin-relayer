use crate::config::ConfigFileError;
use serde::{Deserialize, Serialize};
use validator::Validate;

use super::{SignerConfigValidate, ValidatableSignerConfig};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Validate)]
#[serde(deny_unknown_fields)]
pub struct VaultSignerFileConfig {
    #[validate(url)]
    pub address: String,
    pub namespace: Option<String>,
    #[validate(length(min = 1, message = "Vault role ID cannot be empty"))]
    pub role_id: String,
    #[validate(length(min = 1, message = "Vault secret ID cannot be empty"))]
    pub secret_id: String,
    #[validate(length(min = 1, message = "Vault key name cannot be empty"))]
    pub key_name: String,
    pub mount_point: Option<String>,
}

impl SignerConfigValidate for VaultSignerFileConfig {
    fn validate(&self) -> Result<(), ConfigFileError> {
        self.validate_with_validator()
    }
}
