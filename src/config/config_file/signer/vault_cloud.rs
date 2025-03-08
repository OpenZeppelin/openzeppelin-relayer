use crate::config::ConfigFileError;
use serde::{Deserialize, Serialize};
use validator::Validate;

use super::SignerConfigValidate;

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
        // Use the validator crate's validate method
        match validator::Validate::validate(self) {
            Ok(_) => Ok(()),
            Err(errors) => {
                // Convert validator::ValidationErrors to your ConfigFileError
                let error_message = errors
                    .field_errors()
                    .iter()
                    .map(|(field, errors)| {
                        let messages: Vec<String> = errors
                            .iter()
                            .map(|error| error.message.clone().unwrap_or_default().to_string())
                            .collect();
                        format!("{}: {}", field, messages.join(", "))
                    })
                    .collect::<Vec<String>>()
                    .join("; ");

                Err(ConfigFileError::InvalidFormat(error_message))
            }
        }
    }
}
