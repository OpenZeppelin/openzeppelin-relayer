//! Configuration for HashiCorp Vault Transit engine signer
//!
//! This module provides configuration for using HashiCorp Vault's Transit engine
//! as a signing mechanism. Transit is Vault's cryptographic backend that allows
//! for signing operations without exposing private keys.
//!
//! The configuration supports:
//! - Key name for the Transit engine key to use
//! - Vault server address (URL)
//! - AppRole authentication (role_id and secret_id)
//! - Public key representation for verification
//! - Optional mount point override for the Transit engine
//! - Optional namespace (for Vault Enterprise)
//!
//! Unlike regular Vault configuration, this specifically targets the Transit
//! engine use case where keys are managed and stored within Vault itself.
use crate::{
    config::ConfigFileError,
    models::{validate_plain_or_env_value, PlainOrEnvValue},
};
use serde::{Deserialize, Serialize};
use validator::Validate;

use super::{validate_with_validator, SignerConfigValidate};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Validate)]
#[serde(deny_unknown_fields)]
pub struct VaultTransitSignerFileConfig {
    #[validate(length(min = 1, message = "Key name cannot be empty"))]
    pub key_name: String,
    #[validate(url)]
    pub address: String,
    #[validate(custom(function = "validate_plain_or_env_value"))]
    pub role_id: PlainOrEnvValue,
    #[validate(custom(function = "validate_plain_or_env_value"))]
    pub secret_id: PlainOrEnvValue,
    #[validate(length(min = 1, message = "pubkey cannot be empty"))]
    pub pubkey: String,
    pub mount_point: Option<String>,
    pub namespace: Option<String>,
}

impl SignerConfigValidate for VaultTransitSignerFileConfig {
    fn validate(&self) -> Result<(), ConfigFileError> {
        validate_with_validator(self)
    }
}
#[cfg(test)]
mod tests {
    use crate::models::SecretString;

    use super::*;

    #[test]
    fn test_vault_transit_signer_file_config_valid() {
        let config = VaultTransitSignerFileConfig {
            key_name: "transit-key".to_string(),
            address: "https://vault.example.com:8200".to_string(),
            role_id: PlainOrEnvValue::Plain {
                value: SecretString::new("role-123"),
            },
            secret_id: PlainOrEnvValue::Plain {
                value: SecretString::new("secret-456"),
            },
            pubkey: "MFYwEAYHKoZIzj0CAQYFK4EEAAoDQgAEd+vn+WOG+lGUiJCzHsj8VItmr7Lmdv/Zr+tIhJM7rM+QT9QEzvEX2jWOPyXrvCwUyvVgWoMwUYIo3hd1PFTy7A==".to_string(),
            mount_point: Some("transit".to_string()),
            namespace: Some("namespace1".to_string()),
        };

        assert!(Validate::validate(&config).is_ok());
        assert!(SignerConfigValidate::validate(&config).is_ok());
    }

    #[test]
    fn test_vault_transit_signer_file_config_invalid_address() {
        let config = VaultTransitSignerFileConfig {
            key_name: "transit-key".to_string(),
            address: "not-a-url".to_string(),
            role_id: PlainOrEnvValue::Plain {
                value: SecretString::new("role-123"),
            },
            secret_id: PlainOrEnvValue::Plain {
                value: SecretString::new("secret-456"),
            },
            pubkey: "MFYwEAYHKoZIzj0CAQYFK4EEAAoDQgAEd+vn+WOG+lGUiJCzHsj8VItmr7Lmdv/Zr+tIhJM7rM+QT9QEzvEX2jWOPyXrvCwUyvVgWoMwUYIo3hd1PFTy7A==".to_string(),
            mount_point: Some("transit".to_string()),
            namespace: None,
        };

        let result = SignerConfigValidate::validate(&config);
        assert!(result.is_err());
        if let Err(e) = result {
            let error_message = format!("{:?}", e);
            assert!(error_message.contains("address"));
        }
    }

    #[test]
    fn test_vault_transit_signer_file_config_empty_key_name() {
        let config = VaultTransitSignerFileConfig {
            key_name: "".to_string(),
            address: "https://vault.example.com:8200".to_string(),
            role_id: PlainOrEnvValue::Plain {
                value: SecretString::new("role-123"),
            },
            secret_id: PlainOrEnvValue::Plain {
                value: SecretString::new("secret-456"),
            },
            pubkey: "MFYwEAYHKoZIzj0CAQYFK4EEAAoDQgAEd+vn+WOG+lGUiJCzHsj8VItmr7Lmdv/Zr+tIhJM7rM+QT9QEzvEX2jWOPyXrvCwUyvVgWoMwUYIo3hd1PFTy7A==".to_string(),
            mount_point: Some("transit".to_string()),
            namespace: None,
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
    fn test_vault_transit_signer_file_config_empty_role_id() {
        let config = VaultTransitSignerFileConfig {
            key_name: "transit-key".to_string(),
            address: "https://vault.example.com:8200".to_string(),
            role_id: PlainOrEnvValue::Plain {
                value: SecretString::new(""),
            },
            secret_id: PlainOrEnvValue::Plain {
                value: SecretString::new("secret-456"),
            },
            pubkey: "MFYwEAYHKoZIzj0CAQYFK4EEAAoDQgAEd+vn+WOG+lGUiJCzHsj8VItmr7Lmdv/Zr+tIhJM7rM+QT9QEzvEX2jWOPyXrvCwUyvVgWoMwUYIo3hd1PFTy7A==".to_string(),
            mount_point: Some("transit".to_string()),
            namespace: None,
        };

        let result = SignerConfigValidate::validate(&config);
        assert!(result.is_err());
        if let Err(e) = result {
            let error_message = format!("{:?}", e);
            assert!(error_message.contains("role_id"));
        }
    }

    #[test]
    fn test_vault_transit_signer_file_config_empty_secret_id() {
        let config = VaultTransitSignerFileConfig {
            key_name: "transit-key".to_string(),
            address: "https://vault.example.com:8200".to_string(),
            role_id: PlainOrEnvValue::Plain {
                value: SecretString::new("role-123"),
            },
            secret_id: PlainOrEnvValue::Plain {
                value: SecretString::new(""),
            },
            pubkey: "MFYwEAYHKoZIzj0CAQYFK4EEAAoDQgAEd+vn+WOG+lGUiJCzHsj8VItmr7Lmdv/Zr+tIhJM7rM+QT9QEzvEX2jWOPyXrvCwUyvVgWoMwUYIo3hd1PFTy7A==".to_string(),
            mount_point: Some("transit".to_string()),
            namespace: None,
        };

        let result = SignerConfigValidate::validate(&config);
        assert!(result.is_err());
        if let Err(e) = result {
            let error_message = format!("{:?}", e);
            assert!(error_message.contains("secret_id"));
        }
    }

    #[test]
    fn test_vault_transit_signer_file_config_empty_pubkey() {
        let config = VaultTransitSignerFileConfig {
            key_name: "transit-key".to_string(),
            address: "https://vault.example.com:8200".to_string(),
            role_id: PlainOrEnvValue::Plain {
                value: SecretString::new("role-123"),
            },
            secret_id: PlainOrEnvValue::Plain {
                value: SecretString::new("secret-456"),
            },
            pubkey: "".to_string(),
            mount_point: Some("transit".to_string()),
            namespace: None,
        };

        let result = SignerConfigValidate::validate(&config);
        assert!(result.is_err());
        if let Err(e) = result {
            let error_message = format!("{:?}", e);
            assert!(error_message.contains("pubkey"));
            assert!(error_message.contains("cannot be empty"));
        }
    }

    #[test]
    fn test_vault_transit_signer_file_config_optional_fields() {
        let config = VaultTransitSignerFileConfig {
            key_name: "transit-key".to_string(),
            address: "https://vault.example.com:8200".to_string(),
            role_id: PlainOrEnvValue::Plain {
                value: SecretString::new("role-123"),
            },
            secret_id: PlainOrEnvValue::Plain {
                value: SecretString::new("secret-456"),
            },
            pubkey: "MFYwEAYHKoZIzj0CAQYFK4EEAAoDQgAEd+vn+WOG+lGUiJCzHsj8VItmr7Lmdv/Zr+tIhJM7rM+QT9QEzvEX2jWOPyXrvCwUyvVgWoMwUYIo3hd1PFTy7A==".to_string(),
            mount_point: None,
            namespace: None,
        };

        assert!(SignerConfigValidate::validate(&config).is_ok());
    }

    #[test]
    fn test_vault_transit_signer_file_config_multiple_errors() {
        let config = VaultTransitSignerFileConfig {
            key_name: "".to_string(),
            address: "invalid-url".to_string(),
            role_id: PlainOrEnvValue::Plain {
                value: SecretString::new("role-123"),
            },
            secret_id: PlainOrEnvValue::Plain {
                value: SecretString::new("secret-456"),
            },
            pubkey: "".to_string(),
            mount_point: None,
            namespace: None,
        };

        let result = validate_with_validator(&config);
        assert!(result.is_err());

        if let Err(e) = result {
            if let ConfigFileError::InvalidFormat(msg) = e {
                assert!(msg.contains("key_name"));
                assert!(msg.contains("address"));
                assert!(msg.contains("pubkey"));
            } else {
                panic!("Expected ConfigFileError::InvalidFormat, got {:?}", e);
            }
        }
    }

    #[test]
    fn test_serde_deserialize() {
        let json = r#"
        {
            "key_name": "transit-key",
            "address": "https://vault.example.com:8200",
            "role_id": {
                "type": "plain",
                "value": "role-123"
            },
            "secret_id": {
                "type": "plain",
                "value": "secret-456"
            },
            "pubkey": "MFYwEAYHKoZIzj0CAQYFK4EEAAoDQgAEd+vn+WOG+lGUiJCzHsj8VItmr7Lmdv/Zr+tIhJM7rM+QT9QEzvEX2jWOPyXrvCwUyvVgWoMwUYIo3hd1PFTy7A==",
            "mount_point": "transit",
            "namespace": "my-namespace"
        }
        "#;

        let config: VaultTransitSignerFileConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.key_name, "transit-key");
        assert_eq!(config.address, "https://vault.example.com:8200");
        assert_eq!(
            config.role_id.get_value().unwrap().to_str().as_str(),
            "role-123"
        );
        assert_eq!(
            config.secret_id.get_value().unwrap().to_str().as_str(),
            "secret-456"
        );
        assert_eq!(config.pubkey, "MFYwEAYHKoZIzj0CAQYFK4EEAAoDQgAEd+vn+WOG+lGUiJCzHsj8VItmr7Lmdv/Zr+tIhJM7rM+QT9QEzvEX2jWOPyXrvCwUyvVgWoMwUYIo3hd1PFTy7A==");
        assert_eq!(config.mount_point, Some("transit".to_string()));
        assert_eq!(config.namespace, Some("my-namespace".to_string()));
    }

    #[test]
    fn test_serde_unknown_field() {
        let json = r#"
        {
            "key_name": "transit-key",
            "address": "https://vault.example.com:8200",
            "role_id": "role-123",
            "secret_id": "secret-456",
            "pubkey": "MFYwEAYHKoZIzj0CAQYFK4EEAAoDQgAEd+vn+WOG+lGUiJCzHsj8VItmr7Lmdv/Zr+tIhJM7rM+QT9QEzvEX2jWOPyXrvCwUyvVgWoMwUYIo3hd1PFTy7A==",
            "mount_point": "transit",
            "namespace": "my-namespace",
            "unknown_field": "should cause error"
        }
        "#;

        let result: Result<VaultTransitSignerFileConfig, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_serde_serialize_deserialize() {
        let config = VaultTransitSignerFileConfig {
            key_name: "transit-key".to_string(),
            address: "https://vault.example.com:8200".to_string(),
            role_id: PlainOrEnvValue::Plain {
                value: SecretString::new("role-123"),
            },
            secret_id: PlainOrEnvValue::Plain {
                value: SecretString::new("secret-456"),
            },
            pubkey: "MFYwEAYHKoZIzj0CAQYFK4EEAAoDQgAEd+vn+WOG+lGUiJCzHsj8VItmr7Lmdv/Zr+tIhJM7rM+QT9QEzvEX2jWOPyXrvCwUyvVgWoMwUYIo3hd1PFTy7A==".to_string(),
            mount_point: Some("transit".to_string()),
            namespace: Some("namespace1".to_string()),
        };

        let serialized = serde_json::to_string(&config).unwrap();
        let deserialized: VaultTransitSignerFileConfig = serde_json::from_str(&serialized).unwrap();

        assert_eq!(config.address, deserialized.address);
        assert_eq!(config.key_name, deserialized.key_name);
        assert_eq!(config.mount_point, deserialized.mount_point);
        assert_eq!(config.namespace, deserialized.namespace);
        assert_eq!(config.pubkey, deserialized.pubkey);
        assert_ne!(config.role_id, deserialized.role_id);
        assert_ne!(config.secret_id, deserialized.secret_id);
    }
}
