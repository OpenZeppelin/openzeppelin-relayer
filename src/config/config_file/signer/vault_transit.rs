use crate::config::ConfigFileError;
use serde::{Deserialize, Serialize};
use validator::Validate;

use super::{SignerConfigValidate, ValidatableSignerConfig};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Validate)]
#[serde(deny_unknown_fields)]
pub struct VaultTransitSignerFileConfig {
    #[validate(length(min = 1, message = "Key name cannot be empty"))]
    pub key_name: String,
    #[validate(url)]
    pub address: String,
    #[validate(length(min = 1, message = "role_id cannot be empty"))]
    pub role_id: String,
    #[validate(length(min = 1, message = "secret_id cannot be empty"))]
    pub secret_id: String,
    #[validate(length(min = 1, message = "pubkey cannot be empty"))]
    pub pubkey: String,
    pub mount_point: Option<String>,
    pub namespace: Option<String>,
}

impl SignerConfigValidate for VaultTransitSignerFileConfig {
    fn validate(&self) -> Result<(), ConfigFileError> {
        self.validate_with_validator()
    }
}
