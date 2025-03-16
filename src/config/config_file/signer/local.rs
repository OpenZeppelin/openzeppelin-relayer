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

use crate::{config::ConfigFileError, models::PlainOrEnvValue};

use super::SignerConfigValidate;
use std::path::Path;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LocalSignerFileConfig {
    pub path: String,
    pub passphrase: PlainOrEnvValue,
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
            PlainOrEnvValue::Env { value } => {
                if value.is_empty() {
                    return Err(ConfigFileError::MissingField(
                        "Passphrase environment variable name cannot be empty".into(),
                    ));
                }
                if std::env::var(value).is_err() {
                    return Err(ConfigFileError::MissingEnvVar(format!(
                        "Environment variable {} not found",
                        value
                    )));
                }
            }
            PlainOrEnvValue::Plain { value } => {
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

#[cfg(test)]
mod tests {
    use crate::models::SecretString;

    use super::*;
    use std::env;
    use std::fs::File;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn test_valid_local_signer_config() {
        let temp_dir = tempdir().unwrap();
        let file_path = temp_dir.path().join("test-keystore.json");
        let mut file = File::create(&file_path).unwrap();
        writeln!(file, "{{\"mock\": \"keystore\"}}").unwrap();

        let config = LocalSignerFileConfig {
            path: file_path.to_str().unwrap().to_string(),
            passphrase: PlainOrEnvValue::Plain {
                value: SecretString::new("password123"),
            },
        };

        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_empty_path() {
        let config = LocalSignerFileConfig {
            path: "".to_string(),
            passphrase: PlainOrEnvValue::Plain {
                value: SecretString::new("password123"),
            },
        };

        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(result, Err(ConfigFileError::InvalidIdLength(_))));
    }

    #[test]
    fn test_nonexistent_path() {
        let config = LocalSignerFileConfig {
            path: "/tmp/definitely-doesnt-exist-12345.json".to_string(),
            passphrase: PlainOrEnvValue::Plain {
                value: SecretString::new("password123"),
            },
        };

        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(result, Err(ConfigFileError::FileNotFound(_))));
    }

    #[test]
    fn test_path_is_directory() {
        let temp_dir = tempdir().unwrap();

        let config = LocalSignerFileConfig {
            path: temp_dir.path().to_str().unwrap().to_string(),
            passphrase: PlainOrEnvValue::Plain {
                value: SecretString::new("password123"),
            },
        };

        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(result, Err(ConfigFileError::InvalidFormat(_))));
    }

    #[test]
    fn test_empty_plain_passphrase() {
        let temp_dir = tempdir().unwrap();
        let file_path = temp_dir.path().join("test-keystore.json");
        let mut file = File::create(&file_path).unwrap();
        writeln!(file, "{{\"mock\": \"keystore\"}}").unwrap();

        let config = LocalSignerFileConfig {
            path: file_path.to_str().unwrap().to_string(),
            passphrase: PlainOrEnvValue::Plain {
                value: SecretString::new(""),
            },
        };

        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(result, Err(ConfigFileError::InvalidFormat(_))));
    }

    #[test]
    fn test_empty_env_name() {
        let temp_dir = tempdir().unwrap();
        let file_path = temp_dir.path().join("test-keystore.json");
        let mut file = File::create(&file_path).unwrap();
        writeln!(file, "{{\"mock\": \"keystore\"}}").unwrap();

        let config = LocalSignerFileConfig {
            path: file_path.to_str().unwrap().to_string(),
            passphrase: PlainOrEnvValue::Env {
                value: "".to_string(),
            },
        };

        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(result, Err(ConfigFileError::MissingField(_))));
    }

    #[test]
    fn test_missing_env_var() {
        let temp_dir = tempdir().unwrap();
        let file_path = temp_dir.path().join("test-keystore.json");
        let mut file = File::create(&file_path).unwrap();
        writeln!(file, "{{\"mock\": \"keystore\"}}").unwrap();

        // Make sure this environment variable doesn't exist
        env::remove_var("TEST_SIGNER_PASSPHRASE_THAT_DOESNT_EXIST");

        let config = LocalSignerFileConfig {
            path: file_path.to_str().unwrap().to_string(),
            passphrase: PlainOrEnvValue::Env {
                value: "TEST_SIGNER_PASSPHRASE_THAT_DOESNT_EXIST".to_string(),
            },
        };

        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(result, Err(ConfigFileError::MissingEnvVar(_))));
    }

    #[test]
    fn test_valid_env_var_passphrase() {
        let temp_dir = tempdir().unwrap();
        let file_path = temp_dir.path().join("test-keystore.json");
        let mut file = File::create(&file_path).unwrap();
        writeln!(file, "{{\"mock\": \"keystore\"}}").unwrap();

        env::set_var("TEST_SIGNER_PASSPHRASE", "super-secret-passphrase");

        let config = LocalSignerFileConfig {
            path: file_path.to_str().unwrap().to_string(),
            passphrase: PlainOrEnvValue::Env {
                value: "TEST_SIGNER_PASSPHRASE".to_string(),
            },
        };

        assert!(config.validate().is_ok());

        env::remove_var("TEST_SIGNER_PASSPHRASE");
    }

    #[test]
    fn test_serialize_deserialize() {
        let config = LocalSignerFileConfig {
            path: "/path/to/keystore.json".to_string(),
            passphrase: PlainOrEnvValue::Plain {
                value: SecretString::new("password123"),
            },
        };

        let serialized = serde_json::to_string(&config).unwrap();
        let deserialized: LocalSignerFileConfig = serde_json::from_str(&serialized).unwrap();

        assert_eq!(config.path, deserialized.path);
        assert_ne!(config.passphrase, deserialized.passphrase);
    }

    #[test]
    fn test_deserialize_from_json() {
        let json = r#"{
            "path": "/path/to/keystore.json",
            "passphrase": {
                "type": "plain",
                "value": "password123"
            }
        }"#;

        let config: LocalSignerFileConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.path, "/path/to/keystore.json");

        if let PlainOrEnvValue::Plain { value } = &config.passphrase {
            assert_eq!(value.to_str().as_str(), "password123");
        } else {
            panic!("Expected Plain passphrase variant");
        }
    }

    #[test]
    fn test_deserialize_env_passphrase() {
        let json = r#"{
            "path": "/path/to/keystore.json",
            "passphrase": {
                "type": "env",
                "value": "KEYSTORE_PASSPHRASE"
            }
        }"#;

        let config: LocalSignerFileConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.path, "/path/to/keystore.json");

        if let PlainOrEnvValue::Env { value } = &config.passphrase {
            assert_eq!(value, "KEYSTORE_PASSPHRASE");
        } else {
            panic!("Expected Env passphrase variant");
        }
    }

    #[test]
    fn test_reject_unknown_fields() {
        let json = r#"{
            "path": "/path/to/keystore.json",
            "passphrase": {
                "type": "plain",
                "value": "password123"
            },
            "unexpected_field": "should cause error"
        }"#;

        let result: Result<LocalSignerFileConfig, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_path_and_passphrase_methods() {
        let temp_dir = tempdir().unwrap();
        let file_path = temp_dir.path().join("test-keystore.json");
        let mut file = File::create(&file_path).unwrap();
        writeln!(file, "{{\"mock\": \"keystore\"}}").unwrap();

        let config1 = LocalSignerFileConfig {
            path: file_path.to_str().unwrap().to_string(),
            passphrase: PlainOrEnvValue::Plain {
                value: SecretString::new("password123"),
            },
        };
        assert!(config1.validate_path().is_ok());

        assert!(config1.validate_passphrase().is_ok());

        let config2 = LocalSignerFileConfig {
            path: "/nonexistent/path.json".to_string(),
            passphrase: PlainOrEnvValue::Plain {
                value: SecretString::new("password123"),
            },
        };
        assert!(config2.validate_path().is_err());

        let config3 = LocalSignerFileConfig {
            path: file_path.to_str().unwrap().to_string(),
            passphrase: PlainOrEnvValue::Plain {
                value: SecretString::new(""),
            },
        };
        assert!(config3.validate_passphrase().is_err());
    }
}
