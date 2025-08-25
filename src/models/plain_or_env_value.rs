//! PlainOrEnvValue module for secure configuration value handling
//!
//! This module provides functionality to securely handle configuration values
//! that can either be provided directly in the configuration file ("plain")
//! or retrieved from environment variables ("env").
//!
//! The `PlainOrEnvValue` enum supports two variants:
//! - `Plain`: For values stored directly in the configuration
//! - `Env`: For values that should be retrieved from environment variables
//!
//! When a value is requested, if it's an "env" variant, the module will
//! attempt to retrieve the value from the specified environment variable.
//! All values are wrapped in `SecretString` to ensure secure memory handling.
use serde::{Deserialize, Serialize};
use thiserror::Error;
use validator::ValidationError;
use zeroize::Zeroizing;

use super::SecretString;

#[derive(Error, Debug)]
pub enum PlainOrEnvValueError {
    #[error("Missing env var: {0}")]
    MissingEnvVar(String),
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum PlainOrEnvValue {
    Env { value: String },
    Plain { value: SecretString },
}

impl PlainOrEnvValue {
    pub fn get_value(&self) -> Result<SecretString, PlainOrEnvValueError> {
        match self {
            PlainOrEnvValue::Env { value } => {
                let value = Zeroizing::new(std::env::var(value).map_err(|_| {
                    PlainOrEnvValueError::MissingEnvVar(format!(
                        "Environment variable {} not found",
                        value
                    ))
                })?);
                Ok(SecretString::new(&value))
            }
            PlainOrEnvValue::Plain { value } => Ok(value.clone()),
        }
    }
    pub fn is_empty(&self) -> bool {
        let value = self.get_value();

        match value {
            Ok(v) => v.is_empty(),
            Err(_) => true,
        }
    }
}

pub fn validate_plain_or_env_value(plain_or_env: &PlainOrEnvValue) -> Result<(), ValidationError> {
    let value = plain_or_env.get_value().map_err(|e| {
        let mut err = ValidationError::new("plain_or_env_value_error");
        err.message = Some(format!("plain_or_env_value_error: {}", e).into());
        err
    })?;

    match value.is_empty() {
        true => Err(ValidationError::new(
            "plain_or_env_value_error: value cannot be empty",
        )),
        false => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{env, sync::Mutex};
    use validator::Validate;

    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    #[derive(Validate)]
    struct TestStruct {
        #[validate(custom(function = "validate_plain_or_env_value"))]
        value: PlainOrEnvValue,
    }

    #[test]
    fn test_plain_value_get_value() {
        let plain = PlainOrEnvValue::Plain {
            value: SecretString::new("test-secret"),
        };

        let result = plain.get_value().unwrap();
        result.as_str(|s| {
            assert_eq!(s, "test-secret");
        });
    }

    #[test]
    fn test_env_value_get_value_when_env_exists() {
        let _guard = ENV_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        env::set_var("TEST_ENV_VAR", "env-secret-value");

        let env_value = PlainOrEnvValue::Env {
            value: "TEST_ENV_VAR".to_string(),
        };

        let result = env_value.get_value().unwrap();
        result.as_str(|s| {
            assert_eq!(s, "env-secret-value");
        });

        env::remove_var("TEST_ENV_VAR");
    }

    #[test]
    fn test_env_value_get_value_when_env_missing() {
        let _guard = ENV_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        env::remove_var("NONEXISTENT_VAR");

        let env_value = PlainOrEnvValue::Env {
            value: "NONEXISTENT_VAR".to_string(),
        };

        let result = env_value.get_value();
        assert!(result.is_err());

        match result {
            Err(PlainOrEnvValueError::MissingEnvVar(msg)) => {
                assert!(msg.contains("NONEXISTENT_VAR"));
            }
            _ => panic!("Expected MissingEnvVar error"),
        }
    }

    #[test]
    fn test_is_empty_with_plain_empty_value() {
        let plain = PlainOrEnvValue::Plain {
            value: SecretString::new(""),
        };

        assert!(plain.is_empty());
    }

    #[test]
    fn test_is_empty_with_plain_non_empty_value() {
        let plain = PlainOrEnvValue::Plain {
            value: SecretString::new("non-empty"),
        };

        assert!(!plain.is_empty());
    }

    #[test]
    fn test_is_empty_with_env_missing_var() {
        let _guard = ENV_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        env::remove_var("NONEXISTENT_VAR");

        let env_value = PlainOrEnvValue::Env {
            value: "NONEXISTENT_VAR".to_string(),
        };

        assert!(env_value.is_empty());
    }

    #[test]
    fn test_is_empty_with_env_empty_var() {
        let _guard = ENV_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        env::set_var("EMPTY_ENV_VAR", "");

        let env_value = PlainOrEnvValue::Env {
            value: "EMPTY_ENV_VAR".to_string(),
        };

        assert!(env_value.is_empty());

        env::remove_var("EMPTY_ENV_VAR");
    }

    #[test]
    fn test_is_empty_with_env_non_empty_var() {
        let _guard = ENV_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        env::set_var("TEST_ENV_VAR", "some-value");

        let env_value = PlainOrEnvValue::Env {
            value: "TEST_ENV_VAR".to_string(),
        };

        assert!(!env_value.is_empty());

        env::remove_var("TEST_ENV_VAR");
    }

    #[test]
    fn test_validator_with_plain_empty_value() {
        let test_struct = TestStruct {
            value: PlainOrEnvValue::Plain {
                value: SecretString::new(""),
            },
        };

        let result = test_struct.validate();
        assert!(result.is_err());
    }

    #[test]
    fn test_validator_with_plain_non_empty_value() {
        let test_struct = TestStruct {
            value: PlainOrEnvValue::Plain {
                value: SecretString::new("non-empty"),
            },
        };

        let result = test_struct.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_validator_with_env_missing_var() {
        let _guard = ENV_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        env::remove_var("NONEXISTENT_VAR");

        let test_struct = TestStruct {
            value: PlainOrEnvValue::Env {
                value: "NONEXISTENT_VAR".to_string(),
            },
        };

        let result = test_struct.validate();
        assert!(result.is_err());
    }

    #[test]
    fn test_validator_with_env_empty_var() {
        let _guard = ENV_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        env::set_var("EMPTY_ENV_VAR", "");

        let test_struct = TestStruct {
            value: PlainOrEnvValue::Env {
                value: "EMPTY_ENV_VAR".to_string(),
            },
        };

        let result = test_struct.validate();
        assert!(result.is_err());

        env::remove_var("EMPTY_ENV_VAR");
    }

    #[test]
    fn test_validator_with_env_non_empty_var() {
        let _guard = ENV_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        env::set_var("TEST_ENV_VAR", "some-value");

        let test_struct = TestStruct {
            value: PlainOrEnvValue::Env {
                value: "TEST_ENV_VAR".to_string(),
            },
        };

        let result = test_struct.validate();
        assert!(result.is_ok());

        env::remove_var("TEST_ENV_VAR");
    }

    #[test]
    fn test_serialize_plain_value() {
        let plain = PlainOrEnvValue::Plain {
            value: SecretString::new("test-secret"),
        };

        let serialized = serde_json::to_string(&plain).unwrap();

        assert!(serialized.contains(r#""type":"plain"#));
        // Value should be protected (either REDACTED or base64-encoded)
        assert!(
            serialized.contains(r#""value":"REDACTED"#)
                || (serialized.contains(r#""value":""#) && !serialized.contains("test-secret")),
            "Expected protected value, got: {}",
            serialized
        );
    }

    #[test]
    fn test_serialize_env_value() {
        let env_value = PlainOrEnvValue::Env {
            value: "TEST_ENV_VAR".to_string(),
        };

        let serialized = serde_json::to_string(&env_value).unwrap();

        assert!(serialized.contains(r#""type":"env"#));
        assert!(serialized.contains(r#""value":"TEST_ENV_VAR"#));
    }

    #[test]
    fn test_deserialize_plain_value() {
        let json = r#"{"type":"plain","value":"test-secret"}"#;

        let deserialized: PlainOrEnvValue = serde_json::from_str(json).unwrap();

        match &deserialized {
            PlainOrEnvValue::Plain { value } => {
                value.as_str(|s| {
                    assert_eq!(s, "test-secret");
                });
            }
            _ => panic!("Expected Plain variant"),
        }
    }

    #[test]
    fn test_deserialize_env_value() {
        let json = r#"{"type":"env","value":"TEST_ENV_VAR"}"#;

        let deserialized: PlainOrEnvValue = serde_json::from_str(json).unwrap();

        match &deserialized {
            PlainOrEnvValue::Env { value } => {
                assert_eq!(value, "TEST_ENV_VAR");
            }
            _ => panic!("Expected Env variant"),
        }
    }

    #[test]
    fn test_error_messages() {
        let error = PlainOrEnvValueError::MissingEnvVar("TEST_VAR".to_string());
        let message = format!("{}", error);
        assert_eq!(message, "Missing env var: TEST_VAR");
    }

    #[test]
    fn test_validation_error_messages() {
        let test_struct = TestStruct {
            value: PlainOrEnvValue::Plain {
                value: SecretString::new(""),
            },
        };

        let result = test_struct.validate();
        assert!(result.is_err());

        if let Err(errors) = result {
            let field_errors = errors.field_errors();
            assert!(field_errors.contains_key("value"));

            let error_msgs = &field_errors["value"];
            assert!(!error_msgs.is_empty());

            let has_empty_message = error_msgs
                .iter()
                .any(|e| e.code == "plain_or_env_value_error: value cannot be empty");

            assert!(
                has_empty_message,
                "Validation error should mention empty value"
            );
        }
    }
}
