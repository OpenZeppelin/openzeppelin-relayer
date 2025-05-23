//! Configuration for Google Cloud KMS signer
//!
//! This module provides configuration for using Google Cloud KMS as a signing mechanism.
//! Google Cloud KMS allows you to manage cryptographic keys and perform signing operations
//! without exposing private keys directly to your application.
//!
//! The configuration supports:
//! - Service account credentials (project_id, private_key_id, private_key, client_email, etc.)
//! - KMS key identification (key_ring_id, key_id, key_version)
//! - Optional universe domain and other GCP-specific fields
//!
//! This configuration is used to securely interact with Google Cloud KMS for operations
//! such as public key retrieval and message signing.
use crate::{
    config::ConfigFileError,
    models::{validate_plain_or_env_value, PlainOrEnvValue},
};
use serde::{Deserialize, Serialize};
use validator::Validate;

use super::{validate_with_validator, SignerConfigValidate};

pub fn default_auth_uri() -> String {
    "https://accounts.google.com/o/oauth2/auth".to_string()
}
pub fn default_token_uri() -> String {
    "https://oauth2.googleapis.com/token".to_string()
}
fn default_auth_provider_x509_cert_url() -> String {
    "https://www.googleapis.com/oauth2/v1/certs".to_string()
}
fn default_client_x509_cert_url() -> String {
    "https://www.googleapis.com/robot/v1/metadata/x509/solana-signer%40forward-emitter-459820-r7.iam.gserviceaccount.com".to_string()
}

fn default_universe_domain() -> String {
    "googleapis.com".to_string()
}

fn default_key_version() -> u32 {
    1
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Validate)]
#[serde(deny_unknown_fields)]
pub struct ServiceAccountConfig {
    #[validate(length(min = 1, message = "project_id cannot be empty"))]
    pub project_id: String,
    #[validate(custom(function = "validate_plain_or_env_value"))]
    pub private_key_id: PlainOrEnvValue,
    #[validate(custom(function = "validate_plain_or_env_value"))]
    pub private_key: PlainOrEnvValue,
    #[validate(custom(function = "validate_plain_or_env_value"))]
    pub client_email: PlainOrEnvValue,
    #[validate(length(min = 1, message = "client_id cannot be empty"))]
    pub client_id: String,
    #[validate(url)]
    #[serde(default = "default_auth_uri")]
    pub auth_uri: String,
    #[validate(url)]
    #[serde(default = "default_token_uri")]
    pub token_uri: String,
    #[validate(url)]
    #[serde(default = "default_auth_provider_x509_cert_url")]
    pub auth_provider_x509_cert_url: String,
    #[validate(url)]
    #[serde(default = "default_client_x509_cert_url")]
    pub client_x509_cert_url: String,
    #[serde(default = "default_universe_domain")]
    pub universe_domain: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Validate)]
#[serde(deny_unknown_fields)]
pub struct KmsKeyConfig {
    #[validate(length(min = 1, message = "key_ring_id name cannot be empty"))]
    pub key_ring_id: String,
    #[validate(length(min = 1, message = "key_id cannot be empty"))]
    pub key_id: String,
    #[serde(default = "default_key_version")]
    pub key_version: u32,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Validate)]
#[serde(deny_unknown_fields)]
pub struct GoogleCloudKmsSignerFileConfig {
    #[validate(nested)]
    pub service_account: ServiceAccountConfig,
    #[validate(nested)]
    pub key: KmsKeyConfig,
}

impl SignerConfigValidate for GoogleCloudKmsSignerFileConfig {
    fn validate(&self) -> Result<(), ConfigFileError> {
        validate_with_validator(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::SecretString;

    #[test]
    fn test_google_cloud_kms_signer_file_config_valid() {
        let config = GoogleCloudKmsSignerFileConfig {
            service_account: ServiceAccountConfig {
                project_id: "project-123".to_string(),
                private_key_id: PlainOrEnvValue::Plain {
                    value: SecretString::new("private-key-id"),
                },
                private_key: PlainOrEnvValue::Plain {
                    value: SecretString::new("private-key"),
                },
                client_email: PlainOrEnvValue::Plain {
                    value: SecretString::new("client@email.com"),
                },
                client_id: "client-id-123".to_string(),
                auth_uri: "https://accounts.google.com/o/oauth2/auth".to_string(),
                token_uri: "https://oauth2.googleapis.com/token".to_string(),
                auth_provider_x509_cert_url: "https://www.googleapis.com/oauth2/v1/certs".to_string(),
                client_x509_cert_url: "https://www.googleapis.com/robot/v1/metadata/x509/solana-signer%40forward-emitter-459820-r7.iam.gserviceaccount.com".to_string(),
                universe_domain: "googleapis.com".to_string(),
            },
            key: KmsKeyConfig {
                key_ring_id: "ring-1".to_string(),
                key_id: "key-1".to_string(),
                key_version: 1,
            },
        };

        assert!(Validate::validate(&config).is_ok());
        assert!(SignerConfigValidate::validate(&config).is_ok());
    }

    #[test]
    fn test_google_cloud_kms_signer_file_config_empty_project_id() {
        let config = GoogleCloudKmsSignerFileConfig {
            service_account: ServiceAccountConfig {
                project_id: "".to_string(),
                private_key_id: PlainOrEnvValue::Plain {
                    value: SecretString::new("private-key-id"),
                },
                private_key: PlainOrEnvValue::Plain {
                    value: SecretString::new("private-key"),
                },
                client_email: PlainOrEnvValue::Plain {
                    value: SecretString::new("client@email.com"),
                },
                client_id: "client-id-123".to_string(),
                auth_uri: default_auth_uri(),
                token_uri: default_token_uri(),
                auth_provider_x509_cert_url: default_auth_provider_x509_cert_url(),
                client_x509_cert_url: default_client_x509_cert_url(),
                universe_domain: default_universe_domain(),
            },
            key: KmsKeyConfig {
                key_ring_id: "ring-1".to_string(),
                key_id: "key-1".to_string(),
                key_version: 1,
            },
        };

        let result = SignerConfigValidate::validate(&config);

        assert!(result.is_err());
        if let Err(e) = result {
            let error_message = format!("{:?}", e);
            assert!(error_message.contains("project_id"));
            assert!(error_message.contains("cannot be empty"));
        }
    }

    #[test]
    fn test_google_cloud_kms_signer_file_config_empty_key_ring_id() {
        let config = GoogleCloudKmsSignerFileConfig {
            service_account: ServiceAccountConfig {
                project_id: "project-123".to_string(),
                private_key_id: PlainOrEnvValue::Plain {
                    value: SecretString::new("private-key-id"),
                },
                private_key: PlainOrEnvValue::Plain {
                    value: SecretString::new("private-key"),
                },
                client_email: PlainOrEnvValue::Plain {
                    value: SecretString::new("client@email.com"),
                },
                client_id: "client-id-123".to_string(),
                auth_uri: default_auth_uri(),
                token_uri: default_token_uri(),
                auth_provider_x509_cert_url: default_auth_provider_x509_cert_url(),
                client_x509_cert_url: default_client_x509_cert_url(),
                universe_domain: default_universe_domain(),
            },
            key: KmsKeyConfig {
                key_ring_id: "".to_string(),
                key_id: "key-1".to_string(),
                key_version: 1,
            },
        };

        let result = SignerConfigValidate::validate(&config);
        assert!(result.is_err());
        if let Err(e) = result {
            let error_message = format!("{:?}", e);
            assert!(error_message.contains("key_ring_id"));
            assert!(error_message.contains("cannot be empty"));
        }
    }

    #[test]
    fn test_google_cloud_kms_signer_file_config_empty_key_id() {
        let config = GoogleCloudKmsSignerFileConfig {
            service_account: ServiceAccountConfig {
                project_id: "project-123".to_string(),
                private_key_id: PlainOrEnvValue::Plain {
                    value: SecretString::new("private-key-id"),
                },
                private_key: PlainOrEnvValue::Plain {
                    value: SecretString::new("private-key"),
                },
                client_email: PlainOrEnvValue::Plain {
                    value: SecretString::new("client@email.com"),
                },
                client_id: "client-id-123".to_string(),
                auth_uri: default_auth_uri(),
                token_uri: default_token_uri(),
                auth_provider_x509_cert_url: default_auth_provider_x509_cert_url(),
                client_x509_cert_url: default_client_x509_cert_url(),
                universe_domain: default_universe_domain(),
            },
            key: KmsKeyConfig {
                key_ring_id: "ring-1".to_string(),
                key_id: "".to_string(),
                key_version: 1,
            },
        };

        let result = SignerConfigValidate::validate(&config);

        assert!(result.is_err());
        if let Err(e) = result {
            let error_message = format!("{:?}", e);
            assert!(error_message.contains("key_id"));
            assert!(error_message.contains("cannot be empty"));
        }
    }

    #[test]
    fn test_serde_deserialize() {
        let json = r#"
        {
            "service_account": {
                "project_id": "project-123",
                "private_key_id": {
                    "type": "plain",
                    "value": "private-key-id"
                },
                "private_key": {
                    "type": "plain",
                    "value": "private-key"
                },
                "client_email": {
                    "type": "plain",
                    "value": "client@email.com"
                },
                "client_id": "client-id-123"
            },
            "key": {
                "key_ring_id": "ring-1",
                "key_id": "key-1",
                "key_version": 1
            }
        }
        "#;

        let config: GoogleCloudKmsSignerFileConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.service_account.project_id, "project-123");
        assert_eq!(
            config
                .service_account
                .private_key_id
                .get_value()
                .unwrap()
                .to_str()
                .as_str(),
            "private-key-id"
        );
        assert_eq!(
            config
                .service_account
                .private_key
                .get_value()
                .unwrap()
                .to_str()
                .as_str(),
            "private-key"
        );
        assert_eq!(
            config
                .service_account
                .client_email
                .get_value()
                .unwrap()
                .to_str()
                .as_str(),
            "client@email.com"
        );
        assert_eq!(config.service_account.client_id, "client-id-123");
        assert_eq!(config.key.key_ring_id, "ring-1");
        assert_eq!(config.key.key_id, "key-1");
        assert_eq!(config.key.key_version, 1);
    }

    #[test]
    fn test_serde_unknown_field() {
        let json = r#"
        {
            "service_account": {
                "project_id": "project-123",
                "private_key_id": {
                    "type": "plain",
                    "value": "private-key-id"
                },
                "private_key": {
                    "type": "plain",
                    "value": "private-key"
                },
                "client_email": {
                    "type": "plain",
                    "value": "client@email.com"
                },
                "client_id": "client-id-123"
            },
            "key": {
                "key_ring_id": "ring-1",
                "key_id": "key-1",
                "key_version": 1
            },
            "unknown_field": "should cause error"
        }
        "#;

        let result: Result<GoogleCloudKmsSignerFileConfig, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_serde_serialize_deserialize() {
        let config = GoogleCloudKmsSignerFileConfig {
            service_account: ServiceAccountConfig {
                project_id: "project-123".to_string(),
                private_key_id: PlainOrEnvValue::Plain {
                    value: SecretString::new("private-key-id"),
                },
                private_key: PlainOrEnvValue::Plain {
                    value: SecretString::new("private-key"),
                },
                client_email: PlainOrEnvValue::Plain {
                    value: SecretString::new("client@email.com"),
                },
                client_id: "client-id-123".to_string(),
                auth_uri: default_auth_uri(),
                token_uri: default_token_uri(),
                auth_provider_x509_cert_url: default_auth_provider_x509_cert_url(),
                client_x509_cert_url: default_client_x509_cert_url(),
                universe_domain: default_universe_domain(),
            },
            key: KmsKeyConfig {
                key_ring_id: "ring-1".to_string(),
                key_id: "key-1".to_string(),
                key_version: 1,
            },
        };

        let serialized = serde_json::to_string(&config).unwrap();
        let deserialized: GoogleCloudKmsSignerFileConfig =
            serde_json::from_str(&serialized).unwrap();

        assert_eq!(
            config.service_account.project_id,
            deserialized.service_account.project_id
        );
        assert_eq!(config.key.key_id, deserialized.key.key_id);
        assert_eq!(config.key.key_ring_id, deserialized.key.key_ring_id);
        assert_eq!(config.key.key_version, deserialized.key.key_version);
    }

    #[test]
    fn test_defaults_applied() {
        let json = r#"
    {
        "service_account": {
            "project_id": "project-123",
            "private_key_id": { "type": "plain", "value": "private-key-id" },
            "private_key": { "type": "plain", "value": "private-key" },
            "client_email": { "type": "plain", "value": "client@email.com" },
            "client_id": "client-id-123"
        },
        "key": {
            "key_ring_id": "ring-1",
            "key_id": "key-1"
        }
    }
    "#;
        let config: GoogleCloudKmsSignerFileConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.service_account.auth_uri, default_auth_uri());
        assert_eq!(config.service_account.token_uri, default_token_uri());
        assert_eq!(config.key.key_version, 1);
    }
}
