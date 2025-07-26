//! Configuration file representation and parsing for signers.
//!
//! This module handles the configuration file format for signers, providing:
//!
//! - **Config Models**: Structures that match the configuration file schema
//! - **Conversions**: Bidirectional mapping between config and domain models
//! - **Collections**: Container types for managing multiple signer configurations
//!
//! Used primarily during application startup to parse signer settings from config files.
//! Validation is handled by the domain model in signer.rs to ensure reusability.

use crate::{
    config::ConfigFileError,
    models::signer::{
        AwsKmsSignerConfig, GoogleCloudKmsSignerConfig, GoogleCloudKmsSignerKeyConfig,
        GoogleCloudKmsSignerServiceAccountConfig, LocalSignerConfig, Signer, SignerConfig,
        TurnkeySignerConfig, VaultSignerConfig, VaultTransitSignerConfig,
    },
    models::PlainOrEnvValue,
};
use secrets::SecretVec;
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, path::Path};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LocalSignerFileConfig {
    pub path: String,
    pub passphrase: PlainOrEnvValue,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AwsKmsSignerFileConfig {
    pub region: String,
    pub key_id: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TurnkeySignerFileConfig {
    pub api_public_key: String,
    pub api_private_key: PlainOrEnvValue,
    pub organization_id: String,
    pub private_key_id: String,
    pub public_key: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct VaultSignerFileConfig {
    pub address: String,
    pub namespace: Option<String>,
    pub role_id: PlainOrEnvValue,
    pub secret_id: PlainOrEnvValue,
    pub key_name: String,
    pub mount_point: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct VaultTransitSignerFileConfig {
    pub key_name: String,
    pub address: String,
    pub role_id: PlainOrEnvValue,
    pub secret_id: PlainOrEnvValue,
    pub pubkey: String,
    pub mount_point: Option<String>,
    pub namespace: Option<String>,
}

fn google_cloud_default_auth_uri() -> String {
    "https://accounts.google.com/o/oauth2/auth".to_string()
}

fn google_cloud_default_token_uri() -> String {
    "https://oauth2.googleapis.com/token".to_string()
}

fn google_cloud_default_auth_provider_x509_cert_url() -> String {
    "https://www.googleapis.com/oauth2/v1/certs".to_string()
}

fn google_cloud_default_client_x509_cert_url() -> String {
    "https://www.googleapis.com/robot/v1/metadata/x509/solana-signer%40forward-emitter-459820-r7.iam.gserviceaccount.com".to_string()
}

fn google_cloud_default_universe_domain() -> String {
    "googleapis.com".to_string()
}

fn google_cloud_default_key_version() -> u32 {
    1
}

fn google_cloud_default_location() -> String {
    "global".to_string()
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GoogleCloudKmsServiceAccountFileConfig {
    pub project_id: String,
    pub private_key_id: PlainOrEnvValue,
    pub private_key: PlainOrEnvValue,
    pub client_email: PlainOrEnvValue,
    pub client_id: String,
    #[serde(default = "google_cloud_default_auth_uri")]
    pub auth_uri: String,
    #[serde(default = "google_cloud_default_token_uri")]
    pub token_uri: String,
    #[serde(default = "google_cloud_default_auth_provider_x509_cert_url")]
    pub auth_provider_x509_cert_url: String,
    #[serde(default = "google_cloud_default_client_x509_cert_url")]
    pub client_x509_cert_url: String,
    #[serde(default = "google_cloud_default_universe_domain")]
    pub universe_domain: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GoogleCloudKmsKeyFileConfig {
    #[serde(default = "google_cloud_default_location")]
    pub location: String,
    pub key_ring_id: String,
    pub key_id: String,
    #[serde(default = "google_cloud_default_key_version")]
    pub key_version: u32,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GoogleCloudKmsSignerFileConfig {
    pub service_account: GoogleCloudKmsServiceAccountFileConfig,
    pub key: GoogleCloudKmsKeyFileConfig,
}

/// Main enum for all signer config types
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase", content = "config")]
pub enum SignerFileConfigEnum {
    Local(LocalSignerFileConfig),
    #[serde(rename = "aws_kms")]
    AwsKms(AwsKmsSignerFileConfig),
    Turnkey(TurnkeySignerFileConfig),
    Vault(VaultSignerFileConfig),
    #[serde(rename = "vault_transit")]
    VaultTransit(VaultTransitSignerFileConfig),
    #[serde(rename = "google_cloud_kms")]
    GoogleCloudKms(GoogleCloudKmsSignerFileConfig),
}

/// Individual signer configuration from config file
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct SignerFileConfig {
    pub id: String,
    #[serde(flatten)]
    pub config: SignerFileConfigEnum,
}

/// Collection of signer configurations  
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct SignersFileConfig {
    pub signers: Vec<SignerFileConfig>,
}

impl SignerFileConfig {
    pub fn validate_basic(&self) -> Result<(), ConfigFileError> {
        if self.id.is_empty() {
            return Err(ConfigFileError::InvalidIdLength(
                "Signer ID cannot be empty".into(),
            ));
        }
        Ok(())
    }
}

impl SignersFileConfig {
    pub fn new(signers: Vec<SignerFileConfig>) -> Self {
        Self { signers }
    }

    pub fn validate(&self) -> Result<(), ConfigFileError> {
        if self.signers.is_empty() {
            return Ok(());
        }

        let mut ids = HashSet::new();
        for signer in &self.signers {
            signer.validate_basic()?;
            if !ids.insert(signer.id.clone()) {
                return Err(ConfigFileError::DuplicateId(signer.id.clone()));
            }
        }
        Ok(())
    }
}

impl TryFrom<LocalSignerFileConfig> for LocalSignerConfig {
    type Error = ConfigFileError;

    fn try_from(config: LocalSignerFileConfig) -> Result<Self, Self::Error> {
        if config.path.is_empty() {
            return Err(ConfigFileError::InvalidIdLength(
                "Signer path cannot be empty".into(),
            ));
        }

        let path = Path::new(&config.path);
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

        let passphrase = config.passphrase.get_value().map_err(|e| {
            ConfigFileError::InvalidFormat(format!("Failed to get passphrase value: {}", e))
        })?;

        if passphrase.is_empty() {
            return Err(ConfigFileError::InvalidFormat(
                "Local signer passphrase cannot be empty".into(),
            ));
        }

        let raw_key = SecretVec::new(32, |buffer| {
            let loaded = oz_keystore::LocalClient::load(
                Path::new(&config.path).to_path_buf(),
                passphrase.to_str().as_str().to_string(),
            );
            buffer.copy_from_slice(&loaded);
        });

        Ok(LocalSignerConfig { raw_key })
    }
}

impl TryFrom<AwsKmsSignerFileConfig> for AwsKmsSignerConfig {
    type Error = ConfigFileError;

    fn try_from(config: AwsKmsSignerFileConfig) -> Result<Self, Self::Error> {
        Ok(AwsKmsSignerConfig {
            region: Some(config.region),
            key_id: config.key_id,
        })
    }
}

impl TryFrom<TurnkeySignerFileConfig> for TurnkeySignerConfig {
    type Error = ConfigFileError;

    fn try_from(config: TurnkeySignerFileConfig) -> Result<Self, Self::Error> {
        let api_private_key = config.api_private_key.get_value().map_err(|e| {
            ConfigFileError::InvalidFormat(format!("Failed to get API private key: {}", e))
        })?;

        Ok(TurnkeySignerConfig {
            api_public_key: config.api_public_key,
            api_private_key,
            organization_id: config.organization_id,
            private_key_id: config.private_key_id,
            public_key: config.public_key,
        })
    }
}

impl TryFrom<VaultSignerFileConfig> for VaultSignerConfig {
    type Error = ConfigFileError;

    fn try_from(config: VaultSignerFileConfig) -> Result<Self, Self::Error> {
        let role_id = config
            .role_id
            .get_value()
            .map_err(|e| ConfigFileError::InvalidFormat(format!("Failed to get role ID: {}", e)))?;

        let secret_id = config.secret_id.get_value().map_err(|e| {
            ConfigFileError::InvalidFormat(format!("Failed to get secret ID: {}", e))
        })?;

        Ok(VaultSignerConfig {
            address: config.address,
            namespace: config.namespace,
            role_id,
            secret_id,
            key_name: config.key_name,
            mount_point: config.mount_point,
        })
    }
}

impl TryFrom<VaultTransitSignerFileConfig> for VaultTransitSignerConfig {
    type Error = ConfigFileError;

    fn try_from(config: VaultTransitSignerFileConfig) -> Result<Self, Self::Error> {
        let role_id = config
            .role_id
            .get_value()
            .map_err(|e| ConfigFileError::InvalidFormat(format!("Failed to get role ID: {}", e)))?;

        let secret_id = config.secret_id.get_value().map_err(|e| {
            ConfigFileError::InvalidFormat(format!("Failed to get secret ID: {}", e))
        })?;

        Ok(VaultTransitSignerConfig {
            key_name: config.key_name,
            address: config.address,
            namespace: config.namespace,
            role_id,
            secret_id,
            pubkey: config.pubkey,
            mount_point: config.mount_point,
        })
    }
}

impl TryFrom<GoogleCloudKmsSignerFileConfig> for GoogleCloudKmsSignerConfig {
    type Error = ConfigFileError;

    fn try_from(config: GoogleCloudKmsSignerFileConfig) -> Result<Self, Self::Error> {
        let private_key = config
            .service_account
            .private_key
            .get_value()
            .map_err(|e| {
                ConfigFileError::InvalidFormat(format!("Failed to get private key: {}", e))
            })?;

        let private_key_id = config
            .service_account
            .private_key_id
            .get_value()
            .map_err(|e| {
                ConfigFileError::InvalidFormat(format!("Failed to get private key ID: {}", e))
            })?;

        let client_email = config
            .service_account
            .client_email
            .get_value()
            .map_err(|e| {
                ConfigFileError::InvalidFormat(format!("Failed to get client email: {}", e))
            })?;

        let service_account = GoogleCloudKmsSignerServiceAccountConfig {
            private_key,
            private_key_id,
            project_id: config.service_account.project_id,
            client_email,
            client_id: config.service_account.client_id,
            auth_uri: config.service_account.auth_uri,
            token_uri: config.service_account.token_uri,
            auth_provider_x509_cert_url: config.service_account.auth_provider_x509_cert_url,
            client_x509_cert_url: config.service_account.client_x509_cert_url,
            universe_domain: config.service_account.universe_domain,
        };

        let key = GoogleCloudKmsSignerKeyConfig {
            location: config.key.location,
            key_ring_id: config.key.key_ring_id,
            key_id: config.key.key_id,
            key_version: config.key.key_version,
        };

        Ok(GoogleCloudKmsSignerConfig {
            service_account,
            key,
        })
    }
}

impl TryFrom<SignerFileConfigEnum> for SignerConfig {
    type Error = ConfigFileError;

    fn try_from(config: SignerFileConfigEnum) -> Result<Self, Self::Error> {
        match config {
            SignerFileConfigEnum::Local(local) => {
                Ok(SignerConfig::Local(LocalSignerConfig::try_from(local)?))
            }
            SignerFileConfigEnum::AwsKms(aws_kms) => {
                Ok(SignerConfig::AwsKms(AwsKmsSignerConfig::try_from(aws_kms)?))
            }
            SignerFileConfigEnum::Turnkey(turnkey) => Ok(SignerConfig::Turnkey(
                TurnkeySignerConfig::try_from(turnkey)?,
            )),
            SignerFileConfigEnum::Vault(vault) => {
                Ok(SignerConfig::Vault(VaultSignerConfig::try_from(vault)?))
            }
            SignerFileConfigEnum::VaultTransit(vault_transit) => Ok(SignerConfig::VaultTransit(
                VaultTransitSignerConfig::try_from(vault_transit)?,
            )),
            SignerFileConfigEnum::GoogleCloudKms(gcp_kms) => Ok(SignerConfig::GoogleCloudKms(
                GoogleCloudKmsSignerConfig::try_from(gcp_kms)?,
            )),
        }
    }
}

impl TryFrom<SignerFileConfig> for Signer {
    type Error = ConfigFileError;

    fn try_from(config: SignerFileConfig) -> Result<Self, Self::Error> {
        config.validate_basic()?;

        let signer_config = SignerConfig::try_from(config.config)?;

        // Create core signer with configuration
        let signer = Signer::new(config.id, signer_config);

        // Validate using domain model validation logic
        signer.validate().map_err(|e| match e {
            crate::models::signer::SignerValidationError::EmptyId => {
                ConfigFileError::MissingField("signer id".into())
            }
            crate::models::signer::SignerValidationError::InvalidIdFormat => {
                ConfigFileError::InvalidFormat("Invalid signer ID format".into())
            }
            crate::models::signer::SignerValidationError::InvalidConfig(msg) => {
                ConfigFileError::InvalidFormat(format!("Invalid signer configuration: {}", msg))
            }
        })?;

        Ok(signer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::SecretString;

    #[test]
    fn test_aws_kms_conversion() {
        let config = AwsKmsSignerFileConfig {
            region: "us-east-1".to_string(),
            key_id: "test-key-id".to_string(),
        };

        let result = AwsKmsSignerConfig::try_from(config);
        assert!(result.is_ok());

        let aws_config = result.unwrap();
        assert_eq!(aws_config.region, Some("us-east-1".to_string()));
        assert_eq!(aws_config.key_id, "test-key-id");
    }

    #[test]
    fn test_turnkey_conversion() {
        let config = TurnkeySignerFileConfig {
            api_public_key: "test-public-key".to_string(),
            api_private_key: PlainOrEnvValue::Plain {
                value: SecretString::new("test-private-key"),
            },
            organization_id: "test-org".to_string(),
            private_key_id: "test-private-key-id".to_string(),
            public_key: "test-public-key".to_string(),
        };

        let result = TurnkeySignerConfig::try_from(config);
        assert!(result.is_ok());

        let turnkey_config = result.unwrap();
        assert_eq!(turnkey_config.api_public_key, "test-public-key");
        assert_eq!(turnkey_config.organization_id, "test-org");
    }

    #[test]
    fn test_signer_file_config_validation() {
        let signer_config = SignerFileConfig {
            id: "test-signer".to_string(),
            config: SignerFileConfigEnum::Local(LocalSignerFileConfig {
                path: "test-path".to_string(),
                passphrase: PlainOrEnvValue::Plain {
                    value: SecretString::new("test-passphrase"),
                },
            }),
        };

        assert!(signer_config.validate_basic().is_ok());
    }

    #[test]
    fn test_empty_signer_id() {
        let signer_config = SignerFileConfig {
            id: "".to_string(),
            config: SignerFileConfigEnum::Local(LocalSignerFileConfig {
                path: "test-path".to_string(),
                passphrase: PlainOrEnvValue::Plain {
                    value: SecretString::new("test-passphrase"),
                },
            }),
        };

        assert!(signer_config.validate_basic().is_err());
    }

    #[test]
    fn test_signers_config_validation() {
        let configs = SignersFileConfig::new(vec![
            SignerFileConfig {
                id: "signer1".to_string(),
                config: SignerFileConfigEnum::Local(LocalSignerFileConfig {
                    path: "test-path".to_string(),
                    passphrase: PlainOrEnvValue::Plain {
                        value: SecretString::new("test-passphrase"),
                    },
                }),
            },
            SignerFileConfig {
                id: "signer2".to_string(),
                config: SignerFileConfigEnum::Local(LocalSignerFileConfig {
                    path: "test-path".to_string(),
                    passphrase: PlainOrEnvValue::Plain {
                        value: SecretString::new("test-passphrase"),
                    },
                }),
            },
        ]);

        assert!(configs.validate().is_ok());
    }

    #[test]
    fn test_duplicate_signer_ids() {
        let configs = SignersFileConfig::new(vec![
            SignerFileConfig {
                id: "signer1".to_string(),
                config: SignerFileConfigEnum::Local(LocalSignerFileConfig {
                    path: "test-path".to_string(),
                    passphrase: PlainOrEnvValue::Plain {
                        value: SecretString::new("test-passphrase"),
                    },
                }),
            },
            SignerFileConfig {
                id: "signer1".to_string(), // Duplicate ID
                config: SignerFileConfigEnum::Local(LocalSignerFileConfig {
                    path: "test-path".to_string(),
                    passphrase: PlainOrEnvValue::Plain {
                        value: SecretString::new("test-passphrase"),
                    },
                }),
            },
        ]);

        assert!(matches!(
            configs.validate(),
            Err(ConfigFileError::DuplicateId(_))
        ));
    }

    #[test]
    fn test_local_conversion_invalid_path() {
        let config = LocalSignerFileConfig {
            path: "non-existent-path".to_string(),
            passphrase: PlainOrEnvValue::Plain {
                value: SecretString::new("test-passphrase"),
            },
        };

        let result = LocalSignerConfig::try_from(config);
        assert!(result.is_err());
        if let Err(ConfigFileError::FileNotFound(msg)) = result {
            assert!(msg.contains("Signer file not found"));
        } else {
            panic!("Expected FileNotFound error");
        }
    }

    #[test]
    fn test_vault_conversion() {
        let config = VaultSignerFileConfig {
            address: "https://vault.example.com".to_string(),
            namespace: Some("test-namespace".to_string()),
            role_id: PlainOrEnvValue::Plain {
                value: SecretString::new("test-role"),
            },
            secret_id: PlainOrEnvValue::Plain {
                value: SecretString::new("test-secret"),
            },
            key_name: "test-key".to_string(),
            mount_point: Some("test-mount".to_string()),
        };

        let result = VaultSignerConfig::try_from(config);
        assert!(result.is_ok());

        let vault_config = result.unwrap();
        assert_eq!(vault_config.address, "https://vault.example.com");
        assert_eq!(vault_config.namespace, Some("test-namespace".to_string()));
    }

    #[test]
    fn test_google_cloud_kms_conversion() {
        let config = GoogleCloudKmsSignerFileConfig {
            service_account: GoogleCloudKmsServiceAccountFileConfig {
                project_id: "test-project".to_string(),
                private_key_id: PlainOrEnvValue::Plain {
                    value: SecretString::new("test-key-id"),
                },
                private_key: PlainOrEnvValue::Plain {
                    value: SecretString::new("test-private-key"),
                },
                client_email: PlainOrEnvValue::Plain {
                    value: SecretString::new("test@email.com"),
                },
                client_id: "test-client-id".to_string(),
                auth_uri: google_cloud_default_auth_uri(),
                token_uri: google_cloud_default_token_uri(),
                auth_provider_x509_cert_url: google_cloud_default_auth_provider_x509_cert_url(),
                client_x509_cert_url: google_cloud_default_client_x509_cert_url(),
                universe_domain: google_cloud_default_universe_domain(),
            },
            key: GoogleCloudKmsKeyFileConfig {
                location: google_cloud_default_location(),
                key_ring_id: "test-ring".to_string(),
                key_id: "test-key".to_string(),
                key_version: google_cloud_default_key_version(),
            },
        };

        let result = GoogleCloudKmsSignerConfig::try_from(config);
        assert!(result.is_ok());

        let gcp_config = result.unwrap();
        assert_eq!(gcp_config.key.key_id, "test-key");
        assert_eq!(gcp_config.service_account.project_id, "test-project");
    }
}
