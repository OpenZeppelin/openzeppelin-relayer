//! Configuration for Turnkey signer
//!
//! This module provides configuration for using Turnkey
//! as a signing mechanism. Turnkey is a custody platform that offers secure key management
//! and signing services without exposing private keys.
//!
//! The configuration supports:
//! - API credentials (public key and private key) for authenticating with Turnkey
//! - Organization ID to identify the Turnkey organization
//! - Private key ID to identify the specific private key within Turnkey
//! - Public key representation for verification
//!
//! Turnkey allows for secure signing operations where private keys are managed and
//! protected within Turnkey's secure infrastructure.
use crate::{
    config::ConfigFileError,
    models::{validate_plain_or_env_value, PlainOrEnvValue},
};
use serde::{Deserialize, Serialize};
use validator::Validate;

use super::{validate_with_validator, SignerConfigValidate};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Validate)]
#[serde(deny_unknown_fields)]
pub struct TurnkeySignerFileConfig {
    #[validate(length(min = 1, message = "api_public_key field cannot be empty"))]
    pub api_public_key: String,
    #[validate(custom(function = "validate_plain_or_env_value"))]
    pub api_private_key: PlainOrEnvValue,
    #[validate(length(min = 1, message = "organization_id field cannot be empty"))]
    pub organization_id: String,
    #[validate(length(min = 1, message = "private_key_id cannot be empty"))]
    pub private_key_id: String,
    #[validate(length(min = 1, message = "public_key cannot be empty"))]
    pub public_key: String,
}

impl SignerConfigValidate for TurnkeySignerFileConfig {
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
        let config = TurnkeySignerFileConfig {
            api_private_key: PlainOrEnvValue::Plain {
                value: SecretString::new("api_private_key"),
            },
            api_public_key: "public-key".to_string(),
            organization_id: "org-id".to_string(),
            private_key_id: "private-key-id".to_string(),
            public_key: "public-key".to_string(),
        };

        assert!(Validate::validate(&config).is_ok());
        assert!(SignerConfigValidate::validate(&config).is_ok());
    }

    #[test]
    fn test_turnkey_signer_file_config_empty_api_public_key() {
        let config = TurnkeySignerFileConfig {
            api_private_key: PlainOrEnvValue::Plain {
                value: SecretString::new("api_private_key"),
            },
            api_public_key: "".to_string(),
            organization_id: "org-id".to_string(),
            private_key_id: "private-key-id".to_string(),
            public_key: "public-key".to_string(),
        };

        let result = SignerConfigValidate::validate(&config);
        assert!(result.is_err());
        if let Err(e) = result {
            let error_message = format!("{:?}", e);
            assert!(error_message.contains("api_public_key"));
            assert!(error_message.contains("cannot be empty"));
        }
    }

    #[test]
    fn test_turnkey_signer_file_config_empty_api_private_key() {
        let config = TurnkeySignerFileConfig {
            api_private_key: PlainOrEnvValue::Plain {
                value: SecretString::new(""),
            },
            api_public_key: "public-key".to_string(),
            organization_id: "org-id".to_string(),
            private_key_id: "private-key-id".to_string(),
            public_key: "public-key".to_string(),
        };

        let result = SignerConfigValidate::validate(&config);
        assert!(result.is_err());
        if let Err(e) = result {
            let error_message = format!("{:?}", e);
            assert!(error_message.contains("api_private_key"));
        }
    }

    #[test]
    fn test_turnkey_signer_file_config_empty_organization_id() {
        let config = TurnkeySignerFileConfig {
            api_private_key: PlainOrEnvValue::Plain {
                value: SecretString::new("api_private_key"),
            },
            api_public_key: "public-key".to_string(),
            organization_id: "".to_string(),
            private_key_id: "private-key-id".to_string(),
            public_key: "public-key".to_string(),
        };

        let result = SignerConfigValidate::validate(&config);
        assert!(result.is_err());
        if let Err(e) = result {
            let error_message = format!("{:?}", e);
            assert!(error_message.contains("organization_id"));
            assert!(error_message.contains("cannot be empty"));
        }
    }

    #[test]
    fn test_turnkey_signer_file_config_empty_private_key_id() {
        let config = TurnkeySignerFileConfig {
            api_private_key: PlainOrEnvValue::Plain {
                value: SecretString::new("api_private_key"),
            },
            api_public_key: "public-key".to_string(),
            organization_id: "org-id".to_string(),
            private_key_id: "".to_string(),
            public_key: "public-key".to_string(),
        };

        let result = SignerConfigValidate::validate(&config);
        assert!(result.is_err());
        if let Err(e) = result {
            let error_message = format!("{:?}", e);
            assert!(error_message.contains("private_key_id"));
            assert!(error_message.contains("cannot be empty"));
        }
    }

    #[test]
    fn test_turnkey_signer_file_config_empty_public_key() {
        let config = TurnkeySignerFileConfig {
            api_private_key: PlainOrEnvValue::Plain {
                value: SecretString::new("api_private_key"),
            },
            api_public_key: "public-key".to_string(),
            organization_id: "org-id".to_string(),
            private_key_id: "private-key-id".to_string(),
            public_key: "".to_string(),
        };

        let result = SignerConfigValidate::validate(&config);
        assert!(result.is_err());
        if let Err(e) = result {
            let error_message = format!("{:?}", e);
            assert!(error_message.contains("public_key"));
            assert!(error_message.contains("cannot be empty"));
        }
    }

    #[test]
    fn test_turnkey_signer_file_config_multiple_errors() {
        let config = TurnkeySignerFileConfig {
            api_private_key: PlainOrEnvValue::Plain {
                value: SecretString::new(""),
            },
            api_public_key: "".to_string(),
            organization_id: "".to_string(),
            private_key_id: "".to_string(),
            public_key: "".to_string(),
        };

        let result = validate_with_validator(&config);
        assert!(result.is_err());

        if let Err(e) = result {
            if let ConfigFileError::InvalidFormat(msg) = e {
                assert!(msg.contains("api_public_key"));
                assert!(msg.contains("api_private_key"));
                assert!(msg.contains("organization_id"));
                assert!(msg.contains("private_key_id"));
                assert!(msg.contains("public_key"));
            } else {
                panic!("Expected ConfigFileError::InvalidFormat, got {:?}", e);
            }
        }
    }

    #[test]
    fn test_serde_deserialize() {
        let json = r#"
        {
            "api_public_key": "turnkey-api-public-key",
            "api_private_key": {
                "type": "plain",
                "value": "turnkey-api-private-key"
            },
            "organization_id": "org-123456",
            "private_key_id": "key-123456",
            "public_key": "MFYwEAYHKoZIzj0CAQYFK4EEAAoDQgAEd+vn+WOG+lGUiJCzHsj8VItmr7Lmdv/Zr+tIhJM7rM+QT9QEzvEX2jWOPyXrvCwUyvVgWoMwUYIo3hd1PFTy7A=="
        }
        "#;

        let config: TurnkeySignerFileConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.api_public_key, "turnkey-api-public-key");
        assert_eq!(
            config
                .api_private_key
                .get_value()
                .unwrap()
                .to_str()
                .as_str(),
            "turnkey-api-private-key"
        );
        assert_eq!(config.organization_id, "org-123456");
        assert_eq!(config.private_key_id, "key-123456");
        assert_eq!(config.public_key, "MFYwEAYHKoZIzj0CAQYFK4EEAAoDQgAEd+vn+WOG+lGUiJCzHsj8VItmr7Lmdv/Zr+tIhJM7rM+QT9QEzvEX2jWOPyXrvCwUyvVgWoMwUYIo3hd1PFTy7A==");
    }

    #[test]
    fn test_serde_unknown_field() {
        let json = r#"
        {
            "api_public_key": "turnkey-api-public-key",
            "api_private_key": {
                "type": "plain",
                "value": "turnkey-api-private-key"
            },
            "organization_id": "org-123456",
            "private_key_id": "key-123456",
            "public_key": "MFYwEAYHKoZIzj0CAQYFK4EEAAoDQgAEd+vn+WOG+lGUiJCzHsj8VItmr7Lmdv/Zr+tIhJM7rM+QT9QEzvEX2jWOPyXrvCwUyvVgWoMwUYIo3hd1PFTy7A==",
            "unknown_field": "should cause error"
        }
        "#;

        let result: Result<TurnkeySignerFileConfig, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_serde_serialize_deserialize() {
        let config = TurnkeySignerFileConfig {
            api_private_key: PlainOrEnvValue::Plain {
                value: SecretString::new("api_private_key"),
            },
            api_public_key: "public-key".to_string(),
            organization_id: "org-id".to_string(),
            private_key_id: "private-key-id".to_string(),
            public_key: "MFYwEAYHKoZIzj0CAQYFK4EEAAoDQgAEd+vn+WOG+lGUiJCzHsj8VItmr7Lmdv/Zr+tIhJM7rM+QT9QEzvEX2jWOPyXrvCwUyvVgWoMwUYIo3hd1PFTy7A==".to_string(),
        };

        let serialized = serde_json::to_string(&config).unwrap();
        let deserialized: TurnkeySignerFileConfig = serde_json::from_str(&serialized).unwrap();

        assert_eq!(config.api_public_key, deserialized.api_public_key);
        assert_eq!(config.organization_id, deserialized.organization_id);
        assert_eq!(config.private_key_id, deserialized.private_key_id);
        assert_eq!(config.public_key, deserialized.public_key);
    }

    #[test]
    fn test_turnkey_signer_file_config_env_variable() {
        let env_var_name = "TEST_API_PRIVATE_KEY";
        std::env::set_var(env_var_name, "env-api-private-key");

        let config = TurnkeySignerFileConfig {
            api_private_key: PlainOrEnvValue::Env {
                value: env_var_name.to_string(),
            },
            api_public_key: "public-key".to_string(),
            organization_id: "org-id".to_string(),
            private_key_id: "private-key-id".to_string(),
            public_key: "public-key".to_string(),
        };

        assert!(SignerConfigValidate::validate(&config).is_ok());
        assert_eq!(
            config
                .api_private_key
                .get_value()
                .unwrap()
                .to_str()
                .as_str(),
            "env-api-private-key"
        );

        std::env::remove_var(env_var_name);
    }
}
