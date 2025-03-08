//! Local signer configuration for the OpenZeppelin Relayer.
//!
//! This module provides functionality for managing and validating local signer configurations
//! that use filesystem-based keystores. It handles loading keystores from disk with passphrase
//! protection, supporting both plain text and environment variable-based passphrases.
//!
//! # Features
//!
//! * Validation of signer file paths and passphrases
//! * Support for environment variable-based passphrase retrieval
use serde::{Deserialize, Serialize};

use crate::config::ConfigFileError;

use super::{PlainOrEnvConfigValue, SignerConfigValidate};
use std::path::Path;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LocalSignerFileConfig {
    pub path: String,
    pub passphrase: PlainOrEnvConfigValue,
}

impl LocalSignerFileConfig {
    fn validate_path(&self) -> Result<(), ConfigFileError> {
        if self.path.is_empty() {
            return Err(ConfigFileError::InvalidIdLength(
                "Signer path cannot be empty".into(),
            ));
        }

        let path = Path::new(&self.path);
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
