//! Configuration for a Vault signer
//!
//! This module provides configuration for interacting with HashiCorp Vault as a key
//! management system for signing operations. It supports both AppRole authentication.
//!
//! The configuration supports:
//! - Vault server address (URL)
//! - Optional namespace (for Vault Enterprise)
//! - AppRole authentication (role_id and secret_id)
//! - Key name to use for signing operations
//! - Optional mount point override for Transit engine
use crate::{
    config::ConfigFileError,
    models::{validate_plain_or_env_value, PlainOrEnvValue},
};
use serde::{Deserialize, Serialize};
use validator::Validate;

use super::{validate_with_validator, SignerConfigValidate};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Validate)]
#[serde(deny_unknown_fields)]
pub struct VaultSignerFileConfig {
    #[validate(url)]
    pub address: String,
    pub namespace: Option<String>,
    #[validate(custom(function = "validate_plain_or_env_value"))]
    pub role_id: PlainOrEnvValue,
    #[validate(custom(function = "validate_plain_or_env_value"))]
    pub secret_id: PlainOrEnvValue,
    #[validate(length(min = 1, message = "Vault key name cannot be empty"))]
    pub key_name: String,
    pub mount_point: Option<String>,
}

impl SignerConfigValidate for VaultSignerFileConfig {
    fn validate(&self) -> Result<(), ConfigFileError> {
        validate_with_validator(self)
    }
}

#[cfg(test)]
mod tests {
    use crate::models::SecretString;

    use super::*;
    use validator::Validate;

    #[test]
    fn test_vault_signer_file_config_valid() {
        let config = VaultSignerFileConfig {
            address: "https://vault.example.com:8200".to_string(),
            namespace: Some("namespace1".to_string()),
            role_id: PlainOrEnvValue::Plain {
                value: SecretString::new("role-123"),
            },
            secret_id: PlainOrEnvValue::Plain {
                value: SecretString::new("secret-456"),
            },
            key_name: "my-key".to_string(),
            mount_point: Some("transit".to_string()),
        };

        assert!(Validate::validate(&config).is_ok());
        assert!(SignerConfigValidate::validate(&config).is_ok());
    }

    #[test]
    fn test_vault_signer_file_config_invalid_address() {
        let config = VaultSignerFileConfig {
            address: "not-a-url".to_string(),
            namespace: Some("namespace1".to_string()),
            role_id: PlainOrEnvValue::Plain {
                value: SecretString::new("role-123"),
            },
            secret_id: PlainOrEnvValue::Plain {
                value: SecretString::new("secret-456"),
            },
            key_name: "my-key".to_string(),
            mount_point: Some("transit".to_string()),
        };

        let result = SignerConfigValidate::validate(&config);
        assert!(result.is_err());
        if let Err(e) = result {
            let error_message = format!("{:?}", e);
            assert!(error_message.contains("address"));
        }
    }

    #[test]
    fn test_vault_signer_file_config_empty_role_id() {
        let config = VaultSignerFileConfig {
            address: "https://vault.example.com:8200".to_string(),
            namespace: Some("namespace1".to_string()),
            role_id: PlainOrEnvValue::Plain {
                value: SecretString::new(""),
            },
            secret_id: PlainOrEnvValue::Plain {
                value: SecretString::new("secret-456"),
            },
            key_name: "my-key".to_string(),
            mount_point: Some("transit".to_string()),
        };

        let result = SignerConfigValidate::validate(&config);
        assert!(result.is_err());

        if let Err(e) = result {
            let error_message = format!("{:?}", e);
            assert!(error_message.contains("role_id"));
        }
    }

    #[test]
    fn test_vault_signer_file_config_empty_secret_id() {
        let config = VaultSignerFileConfig {
            address: "https://vault.example.com:8200".to_string(),
            namespace: None,
            role_id: PlainOrEnvValue::Plain {
                value: SecretString::new("role-123"),
            },
            secret_id: PlainOrEnvValue::Plain {
                value: SecretString::new(""),
            },
            key_name: "my-key".to_string(),
            mount_point: None,
        };

        let result = SignerConfigValidate::validate(&config);
        assert!(result.is_err());

        if let Err(e) = result {
            let error_message = format!("{:?}", e);
            assert!(error_message.contains("secret_id"));
        }
    }

    #[test]
    fn test_vault_signer_file_config_empty_key_name() {
        let config = VaultSignerFileConfig {
            address: "https://vault.example.com:8200".to_string(),
            namespace: None,
            role_id: PlainOrEnvValue::Plain {
                value: SecretString::new("role-123"),
            },
            secret_id: PlainOrEnvValue::Plain {
                value: SecretString::new("secret-456"),
            },
            key_name: "".to_string(),
            mount_point: None,
        };

        let result = SignerConfigValidate::validate(&config);
        assert!(result.is_err());

        if let Err(e) = result {
            let error_message = format!("{:?}", e);
            assert!(error_message.contains("key_name"));
            assert!(error_message.contains("cannot be empty"));
        }
    }

    #[test]
    fn test_vault_signer_file_config_optional_fields() {
        let config = VaultSignerFileConfig {
            address: "https://vault.example.com:8200".to_string(),
            namespace: None,
            role_id: PlainOrEnvValue::Plain {
                value: SecretString::new("role-123"),
            },
            secret_id: PlainOrEnvValue::Plain {
                value: SecretString::new("secret-456"),
            },
            key_name: "my-key".to_string(),
            mount_point: None,
        };

        assert!(SignerConfigValidate::validate(&config).is_ok());
    }

    #[test]
    fn test_vault_signer_file_config_multiple_errors() {
        // Create a config with multiple validation errors
        let config = VaultSignerFileConfig {
            address: "invalid-url".to_string(),
            namespace: None,
            role_id: PlainOrEnvValue::Plain {
                value: SecretString::new(""),
            },
            secret_id: PlainOrEnvValue::Plain {
                value: SecretString::new(""),
            },
            key_name: "".to_string(),
            mount_point: None,
        };

        let result = validate_with_validator(&config);
        assert!(result.is_err());

        if let Err(e) = result {
            if let ConfigFileError::InvalidFormat(msg) = e {
                assert!(msg.contains("address"));
                assert!(msg.contains("role_id"));
                assert!(msg.contains("secret_id"));
                assert!(msg.contains("key_name"));
            } else {
                panic!("Expected ConfigFileError::InvalidFormat, got {:?}", e);
            }
        }
    }

    #[test]
    fn test_serde_deserialize() {
        let json = r#"
        {
            "address": "https://vault.example.com:8200",
            "namespace": "my-namespace",
            "role_id": {
                "type": "plain",
                "value": "role-123"
            },
            "secret_id": { 
                "type": "plain",
                "value": "secret-456"
            },
            "key_name": "my-key",
            "mount_point": "transit"
        }
        "#;

        let config: VaultSignerFileConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.address, "https://vault.example.com:8200");
        assert_eq!(config.namespace, Some("my-namespace".to_string()));
        assert_eq!(
            config.role_id.get_value().unwrap().to_str().as_str(),
            "role-123"
        );
        assert_eq!(
            config.secret_id.get_value().unwrap().to_str().as_str(),
            "secret-456"
        );
        assert_eq!(config.key_name, "my-key");
        assert_eq!(config.mount_point, Some("transit".to_string()));
    }

    #[test]
    fn test_serde_unknown_field() {
        let json = r#"
        {
            "address": "https://vault.example.com:8200",
            "namespace": "my-namespace",
            "role_id": {
                "type": "plain",
                "value": "role-123"
            },
            "secret_id": { 
                "type": "plain",
                "value": "secret-456"
            },
            "key_name": "my-key",
            "mount_point": "transit",
            "unknown_field": "should cause error"
        }
        "#;

        let result: Result<VaultSignerFileConfig, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_serde_serialize_deserialize() {
        let config = VaultSignerFileConfig {
            address: "https://vault.example.com:8200".to_string(),
            namespace: Some("namespace1".to_string()),
            role_id: PlainOrEnvValue::Plain {
                value: SecretString::new("role-123"),
            },
            secret_id: PlainOrEnvValue::Plain {
                value: SecretString::new("secret-456"),
            },
            key_name: "my-key".to_string(),
            mount_point: Some("transit".to_string()),
        };

        let serialized = serde_json::to_string(&config).unwrap();
        let deserialized: VaultSignerFileConfig = serde_json::from_str(&serialized).unwrap();

        assert_eq!(config.address, deserialized.address);
        assert_eq!(config.key_name, deserialized.key_name);
        assert_eq!(config.mount_point, deserialized.mount_point);
        assert_eq!(config.namespace, deserialized.namespace);
        assert_ne!(config.role_id, deserialized.role_id);
        assert_ne!(config.secret_id, deserialized.secret_id);
    }
}
