//! String or Environment Variable Value Module
//!
//! This module provides functionality to handle configuration values that can be
//! either plain strings or references to environment variables. This is particularly
//! useful for RPC URLs that may contain sensitive API keys.
//!
//! ## Usage
//!
//! Values can be specified in two formats:
//! - As a plain string: `"https://api.example.com/v3/abc123"`
//! - As an env var reference: `{"type": "env", "value": "RPC_URL_MAINNET"}`
//!
//! For backward compatibility, plain strings are automatically detected during
//! deserialization without requiring a type tag.

use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum StringOrEnvError {
    #[error("Environment variable '{0}' not found")]
    MissingEnvVar(String),
}

/// Represents a value that can be either a plain string or an environment variable reference
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum StringOrEnvValue {
    /// Plain string value
    Plain(String),
    /// Environment variable reference
    Env(EnvRef),
}

/// Reference to an environment variable
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnvRef {
    #[serde(rename = "type")]
    pub type_field: String,
    pub value: String,
}

impl StringOrEnvValue {
    /// Creates a new plain string value
    pub fn plain(value: impl Into<String>) -> Self {
        Self::Plain(value.into())
    }

    /// Creates a new environment variable reference
    pub fn env(var_name: impl Into<String>) -> Self {
        Self::Env(EnvRef {
            type_field: "env".to_string(),
            value: var_name.into(),
        })
    }

    /// Resolves the value, fetching from environment if needed
    pub fn resolve(&self) -> Result<String, StringOrEnvError> {
        match self {
            Self::Plain(value) => Ok(value.clone()),
            Self::Env(env_ref) => {
                std::env::var(&env_ref.value)
                    .map_err(|_| StringOrEnvError::MissingEnvVar(env_ref.value.clone()))
            }
        }
    }

    /// Checks if this is an environment variable reference
    pub fn is_env(&self) -> bool {
        matches!(self, Self::Env(_))
    }

    /// Checks if this is a plain value
    pub fn is_plain(&self) -> bool {
        matches!(self, Self::Plain(_))
    }
}

impl fmt::Display for StringOrEnvValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Plain(value) => write!(f, "{}", value),
            Self::Env(env_ref) => write!(f, "${}", env_ref.value),
        }
    }
}

// Custom deserializer for backward compatibility
impl<'de> Deserialize<'de> for StringOrEnvValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Helper {
            String(String),
            Env(EnvRef),
        }

        let helper = Helper::deserialize(deserializer)?;
        match helper {
            Helper::String(s) => Ok(StringOrEnvValue::Plain(s)),
            Helper::Env(env_ref) => {
                // Validate that type is "env"
                if env_ref.type_field != "env" {
                    return Err(serde::de::Error::custom(format!(
                        "Invalid type '{}', expected 'env'",
                        env_ref.type_field
                    )));
                }
                Ok(StringOrEnvValue::Env(env_ref))
            }
        }
    }
}

/// Helper function to resolve a vector of StringOrEnvValue items
pub fn resolve_all(values: &[StringOrEnvValue]) -> Result<Vec<String>, StringOrEnvError> {
    values.iter().map(|v| v.resolve()).collect()
}

/// Helper function to merge optional vectors of StringOrEnvValue
pub fn merge_optional_string_or_env_vecs(
    child: &Option<Vec<StringOrEnvValue>>,
    parent: &Option<Vec<StringOrEnvValue>>,
) -> Option<Vec<StringOrEnvValue>> {
    match (child, parent) {
        (Some(child), Some(_parent)) => {
            // For URLs, child completely overrides parent (no merging)
            Some(child.clone())
        }
        (Some(items), None) => Some(items.clone()),
        (None, Some(items)) => Some(items.clone()),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plain_value_creation() {
        let value = StringOrEnvValue::plain("https://api.example.com");
        assert!(value.is_plain());
        assert!(!value.is_env());
        assert_eq!(value.resolve().unwrap(), "https://api.example.com");
    }

    #[test]
    fn test_env_value_creation() {
        let value = StringOrEnvValue::env("RPC_URL");
        assert!(!value.is_plain());
        assert!(value.is_env());
    }

    #[test]
    fn test_env_resolution_success() {
        std::env::set_var("TEST_RPC_URL", "https://test.example.com");
        let value = StringOrEnvValue::env("TEST_RPC_URL");
        assert_eq!(value.resolve().unwrap(), "https://test.example.com");
        std::env::remove_var("TEST_RPC_URL");
    }

    #[test]
    fn test_env_resolution_failure() {
        std::env::remove_var("NONEXISTENT_VAR");
        let value = StringOrEnvValue::env("NONEXISTENT_VAR");
        assert!(value.resolve().is_err());
    }

    #[test]
    fn test_deserialize_plain_string() {
        let json = r#""https://api.example.com""#;
        let value: StringOrEnvValue = serde_json::from_str(json).unwrap();
        assert!(value.is_plain());
        assert_eq!(value.resolve().unwrap(), "https://api.example.com");
    }

    #[test]
    fn test_deserialize_env_reference() {
        let json = r#"{"type": "env", "value": "RPC_URL"}"#;
        let value: StringOrEnvValue = serde_json::from_str(json).unwrap();
        assert!(value.is_env());
        if let StringOrEnvValue::Env(env_ref) = value {
            assert_eq!(env_ref.value, "RPC_URL");
        }
    }

    #[test]
    fn test_deserialize_array_mixed() {
        let json = r#"[
            "https://plain.example.com",
            {"type": "env", "value": "RPC_URL_1"},
            "https://another.example.com"
        ]"#;
        let values: Vec<StringOrEnvValue> = serde_json::from_str(json).unwrap();
        assert_eq!(values.len(), 3);
        assert!(values[0].is_plain());
        assert!(values[1].is_env());
        assert!(values[2].is_plain());
    }

    #[test]
    fn test_serialize_plain() {
        let value = StringOrEnvValue::plain("https://api.example.com");
        let json = serde_json::to_string(&value).unwrap();
        assert_eq!(json, r#""https://api.example.com""#);
    }

    #[test]
    fn test_serialize_env() {
        let value = StringOrEnvValue::env("RPC_URL");
        let json = serde_json::to_string(&value).unwrap();
        assert!(json.contains(r#""type":"env""#));
        assert!(json.contains(r#""value":"RPC_URL""#));
    }

    #[test]
    fn test_display_format() {
        let plain = StringOrEnvValue::plain("https://api.example.com");
        assert_eq!(format!("{}", plain), "https://api.example.com");

        let env = StringOrEnvValue::env("RPC_URL");
        assert_eq!(format!("{}", env), "$RPC_URL");
    }

    #[test]
    fn test_resolve_all() {
        std::env::set_var("TEST_URL_1", "https://test1.example.com");
        std::env::set_var("TEST_URL_2", "https://test2.example.com");

        let values = vec![
            StringOrEnvValue::plain("https://plain.example.com"),
            StringOrEnvValue::env("TEST_URL_1"),
            StringOrEnvValue::env("TEST_URL_2"),
        ];

        let resolved = resolve_all(&values).unwrap();
        assert_eq!(resolved.len(), 3);
        assert_eq!(resolved[0], "https://plain.example.com");
        assert_eq!(resolved[1], "https://test1.example.com");
        assert_eq!(resolved[2], "https://test2.example.com");

        std::env::remove_var("TEST_URL_1");
        std::env::remove_var("TEST_URL_2");
    }

    #[test]
    fn test_merge_vecs_child_overrides() {
        let parent = Some(vec![
            StringOrEnvValue::plain("https://parent1.example.com"),
            StringOrEnvValue::plain("https://parent2.example.com"),
        ]);
        let child = Some(vec![StringOrEnvValue::plain("https://child.example.com")]);

        let merged = merge_optional_string_or_env_vecs(&child, &parent);
        assert!(merged.is_some());
        let merged = merged.unwrap();
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].resolve().unwrap(), "https://child.example.com");
    }

    #[test]
    fn test_backward_compatibility() {
        // Test that existing JSON configs still work
        let old_config = r#"{
            "rpc_urls": [
                "https://eth.drpc.org",
                "https://1rpc.io/eth"
            ]
        }"#;

        #[derive(Deserialize)]
        struct TestConfig {
            rpc_urls: Vec<StringOrEnvValue>,
        }

        let config: TestConfig = serde_json::from_str(old_config).unwrap();
        assert_eq!(config.rpc_urls.len(), 2);
        assert!(config.rpc_urls[0].is_plain());
        assert!(config.rpc_urls[1].is_plain());
    }
}