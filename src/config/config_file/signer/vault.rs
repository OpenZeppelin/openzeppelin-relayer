use crate::config::ConfigFileError;
use serde::{Deserialize, Serialize};
use validator::Validate;

use super::SignerConfigValidate;

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
}

impl SignerConfigValidate for VaultSignerFileConfig {
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
