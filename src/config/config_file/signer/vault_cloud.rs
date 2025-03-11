use crate::config::ConfigFileError;
use serde::{Deserialize, Serialize};
use validator::Validate;

use super::{SignerConfigValidate, ValidatableSignerConfig};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Validate)]
#[serde(deny_unknown_fields)]
pub struct VaultCloudSignerFileConfig {
    #[validate(length(min = 1, message = "Client ID cannot be empty"))]
    pub client_id: String,
    #[validate(length(min = 1, message = "Client secret cannot be empty"))]
    pub client_secret: String,
    #[validate(length(min = 1, message = "Organization ID cannot be empty"))]
    pub org_id: String,
    #[validate(length(min = 1, message = "Project ID cannot be empty"))]
    pub project_id: String,
    #[validate(length(min = 1, message = "Application name cannot be empty"))]
    pub app_name: String,
    #[validate(length(min = 1, message = "Key name cannot be empty"))]
    pub key_name: String,
}

impl SignerConfigValidate for VaultCloudSignerFileConfig {
    fn validate(&self) -> Result<(), ConfigFileError> {
        self.validate_with_validator()
    }
}
