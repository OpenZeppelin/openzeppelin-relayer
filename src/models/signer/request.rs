//! API request models and validation for signer endpoints.
//!
//! This module handles incoming HTTP requests for signer operations, providing:
//!
//! - **Request Models**: Structures for creating and updating signers via API
//! - **Input Validation**: Sanitization and validation of user-provided data
//! - **Domain Conversion**: Transformation from API requests to domain objects
//!
//! Serves as the entry point for signer data from external clients, ensuring
//! all input is properly validated before reaching the core business logic.

use crate::models::{
    ApiError, AwsKmsSignerConfig, GoogleCloudKmsSignerConfig, GoogleCloudKmsSignerKeyConfig,
    GoogleCloudKmsSignerServiceAccountConfig, LocalSignerConfig, SecretString, Signer,
    SignerConfig, TurnkeySignerConfig, VaultCloudSignerConfig, VaultSignerConfig,
    VaultTransitSignerConfig,
};
use secrets::SecretVec;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use zeroize::Zeroize;

/// AWS KMS signer configuration for API requests
#[derive(Debug, Serialize, Deserialize, ToSchema, Zeroize)]
pub struct PlainSignerRequestConfig {
    pub key: String,
}

/// AWS KMS signer configuration for API requests
#[derive(Debug, Serialize, Deserialize, ToSchema, Zeroize)]
pub struct AwsKmsSignerRequestConfig {
    pub region: String,
    pub key_id: String,
}

/// Vault signer configuration for API requests
#[derive(Debug, Serialize, Deserialize, ToSchema, Zeroize)]
pub struct VaultSignerRequestConfig {
    pub address: String,
    pub namespace: Option<String>,
    pub role_id: String,
    pub secret_id: String,
    pub key_name: String,
    pub mount_point: Option<String>,
}

/// Vault Cloud signer configuration for API requests
#[derive(Debug, Serialize, Deserialize, ToSchema, Zeroize)]
pub struct VaultCloudSignerRequestConfig {
    pub client_id: String,
    pub client_secret: String,
    pub org_id: String,
    pub project_id: String,
    pub app_name: String,
    pub key_name: String,
}

/// Vault Transit signer configuration for API requests
#[derive(Debug, Serialize, Deserialize, ToSchema, Zeroize)]
pub struct VaultTransitSignerRequestConfig {
    pub key_name: String,
    pub address: String,
    pub namespace: Option<String>,
    pub role_id: String,
    pub secret_id: String,
    pub pubkey: String,
    pub mount_point: Option<String>,
}

/// Turnkey signer configuration for API requests
#[derive(Debug, Serialize, Deserialize, ToSchema, Zeroize)]
pub struct TurnkeySignerRequestConfig {
    pub api_public_key: String,
    pub api_private_key: String,
    pub organization_id: String,
    pub private_key_id: String,
    pub public_key: String,
}

/// Google Cloud KMS service account configuration for API requests
#[derive(Debug, Serialize, Deserialize, ToSchema, Zeroize)]
pub struct GoogleCloudKmsSignerServiceAccountRequestConfig {
    pub private_key: String,
    pub private_key_id: String,
    pub project_id: String,
    pub client_email: String,
    pub client_id: String,
    pub auth_uri: String,
    pub token_uri: String,
    pub auth_provider_x509_cert_url: String,
    pub client_x509_cert_url: String,
    pub universe_domain: String,
}

/// Google Cloud KMS key configuration for API requests
#[derive(Debug, Serialize, Deserialize, ToSchema, Zeroize)]
pub struct GoogleCloudKmsSignerKeyRequestConfig {
    pub location: String,
    pub key_ring_id: String,
    pub key_id: String,
    pub key_version: u32,
}

/// Google Cloud KMS signer configuration for API requests
#[derive(Debug, Serialize, Deserialize, ToSchema, Zeroize)]
pub struct GoogleCloudKmsSignerRequestConfig {
    pub service_account: GoogleCloudKmsSignerServiceAccountRequestConfig,
    pub key: GoogleCloudKmsSignerKeyRequestConfig,
}

/// Signer configuration enum for API requests
#[derive(Debug, Serialize, Deserialize, ToSchema, Zeroize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SignerConfigRequest {
    #[serde(rename = "plain")]
    Local {
        config: PlainSignerRequestConfig,
    },
    #[serde(rename = "aws_kms")]
    AwsKms {
        config: AwsKmsSignerRequestConfig,
    },
    Vault {
        config: VaultSignerRequestConfig,
    },
    #[serde(rename = "vault_cloud")]
    VaultCloud {
        config: VaultCloudSignerRequestConfig,
    },
    #[serde(rename = "vault_transit")]
    VaultTransit {
        config: VaultTransitSignerRequestConfig,
    },
    Turnkey {
        config: TurnkeySignerRequestConfig,
    },
    #[serde(rename = "google_cloud_kms")]
    GoogleCloudKms {
        config: GoogleCloudKmsSignerRequestConfig,
    },
}

/// Request model for creating a new signer
#[derive(Debug, Serialize, Deserialize, ToSchema, Zeroize)]
pub struct SignerCreateRequest {
    /// Optional ID - if not provided, a UUID will be generated
    pub id: Option<String>,
    /// The signer configuration including type and config data
    #[serde(flatten)]
    pub config: SignerConfigRequest,
}

/// Request model for updating an existing signer
/// At the moment, we don't allow updating signers
#[derive(Debug, Serialize, Deserialize, ToSchema, Zeroize)]
pub struct SignerUpdateRequest {}

impl From<AwsKmsSignerRequestConfig> for AwsKmsSignerConfig {
    fn from(config: AwsKmsSignerRequestConfig) -> Self {
        Self {
            region: Some(config.region),
            key_id: config.key_id,
        }
    }
}

impl From<VaultSignerRequestConfig> for VaultSignerConfig {
    fn from(config: VaultSignerRequestConfig) -> Self {
        Self {
            address: config.address,
            namespace: config.namespace,
            role_id: SecretString::new(&config.role_id),
            secret_id: SecretString::new(&config.secret_id),
            key_name: config.key_name,
            mount_point: config.mount_point,
        }
    }
}

impl From<VaultCloudSignerRequestConfig> for VaultCloudSignerConfig {
    fn from(config: VaultCloudSignerRequestConfig) -> Self {
        Self {
            client_id: config.client_id,
            client_secret: SecretString::new(&config.client_secret),
            org_id: config.org_id,
            project_id: config.project_id,
            app_name: config.app_name,
            key_name: config.key_name,
        }
    }
}

impl From<VaultTransitSignerRequestConfig> for VaultTransitSignerConfig {
    fn from(config: VaultTransitSignerRequestConfig) -> Self {
        Self {
            key_name: config.key_name,
            address: config.address,
            namespace: config.namespace,
            role_id: SecretString::new(&config.role_id),
            secret_id: SecretString::new(&config.secret_id),
            pubkey: config.pubkey,
            mount_point: config.mount_point,
        }
    }
}

impl From<TurnkeySignerRequestConfig> for TurnkeySignerConfig {
    fn from(config: TurnkeySignerRequestConfig) -> Self {
        Self {
            api_public_key: config.api_public_key,
            api_private_key: SecretString::new(&config.api_private_key),
            organization_id: config.organization_id,
            private_key_id: config.private_key_id,
            public_key: config.public_key,
        }
    }
}

impl From<GoogleCloudKmsSignerServiceAccountRequestConfig>
    for GoogleCloudKmsSignerServiceAccountConfig
{
    fn from(config: GoogleCloudKmsSignerServiceAccountRequestConfig) -> Self {
        Self {
            private_key: SecretString::new(&config.private_key),
            private_key_id: SecretString::new(&config.private_key_id),
            project_id: config.project_id,
            client_email: SecretString::new(&config.client_email),
            client_id: config.client_id,
            auth_uri: config.auth_uri,
            token_uri: config.token_uri,
            auth_provider_x509_cert_url: config.auth_provider_x509_cert_url,
            client_x509_cert_url: config.client_x509_cert_url,
            universe_domain: config.universe_domain,
        }
    }
}

impl From<GoogleCloudKmsSignerKeyRequestConfig> for GoogleCloudKmsSignerKeyConfig {
    fn from(config: GoogleCloudKmsSignerKeyRequestConfig) -> Self {
        Self {
            location: config.location,
            key_ring_id: config.key_ring_id,
            key_id: config.key_id,
            key_version: config.key_version,
        }
    }
}

impl From<GoogleCloudKmsSignerRequestConfig> for GoogleCloudKmsSignerConfig {
    fn from(config: GoogleCloudKmsSignerRequestConfig) -> Self {
        Self {
            service_account: config.service_account.into(),
            key: config.key.into(),
        }
    }
}

impl TryFrom<SignerConfigRequest> for SignerConfig {
    type Error = ApiError;

    fn try_from(config: SignerConfigRequest) -> Result<Self, Self::Error> {
        let domain_config = match config {
            SignerConfigRequest::Local { config } => {
                // Decode hex string to raw bytes for cryptographic key
                let key_bytes = hex::decode(&config.key)
                    .map_err(|e| ApiError::BadRequest(format!(
                        "Invalid hex key format: {}. Key must be a 64-character hex string (32 bytes).", e
                    )))?;

                let raw_key = SecretVec::new(key_bytes.len(), |buffer| {
                    buffer.copy_from_slice(&key_bytes);
                });

                SignerConfig::Local(LocalSignerConfig { raw_key })
            }
            SignerConfigRequest::AwsKms { config } => SignerConfig::AwsKms(config.into()),
            SignerConfigRequest::Vault { config } => SignerConfig::Vault(config.into()),
            SignerConfigRequest::VaultCloud { config } => SignerConfig::VaultCloud(config.into()),
            SignerConfigRequest::VaultTransit { config } => {
                SignerConfig::VaultTransit(config.into())
            }
            SignerConfigRequest::Turnkey { config } => SignerConfig::Turnkey(config.into()),
            SignerConfigRequest::GoogleCloudKms { config } => {
                SignerConfig::GoogleCloudKms(config.into())
            }
        };

        // Validate the configuration using domain model validation
        domain_config.validate().map_err(|e| ApiError::from(e))?;

        Ok(domain_config)
    }
}

impl TryFrom<SignerCreateRequest> for Signer {
    type Error = ApiError;

    fn try_from(request: SignerCreateRequest) -> Result<Self, Self::Error> {
        // Generate UUID if no ID provided
        let id = request
            .id
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        // Convert request config to domain config (with validation)
        let config = SignerConfig::try_from(request.config)?;

        // Create the signer
        let signer = Signer::new(id, config);

        // Validate using domain model validation (this will also validate the config)
        signer.validate().map_err(ApiError::from)?;

        Ok(signer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::signer::signer::SignerType;

    #[test]
    fn test_valid_aws_kms_create_request() {
        let request = SignerCreateRequest {
            id: Some("test-aws-signer".to_string()),
            config: SignerConfigRequest::AwsKms {
                config: AwsKmsSignerRequestConfig {
                    region: "us-east-1".to_string(),
                    key_id: "test-key-id".to_string(),
                },
            },
        };

        let result = Signer::try_from(request);
        assert!(result.is_ok());

        let signer = result.unwrap();
        assert_eq!(signer.id, "test-aws-signer");
        assert_eq!(signer.signer_type(), SignerType::AwsKms);

        // Verify the config was properly converted
        if let Some(aws_config) = signer.config.get_aws_kms() {
            assert_eq!(aws_config.region, Some("us-east-1".to_string()));
            assert_eq!(aws_config.key_id, "test-key-id");
        } else {
            panic!("Expected AWS KMS config");
        }
    }

    #[test]
    fn test_valid_vault_create_request() {
        let request = SignerCreateRequest {
            id: Some("test-vault-signer".to_string()),
            config: SignerConfigRequest::Vault {
                config: VaultSignerRequestConfig {
                    address: "https://vault.example.com".to_string(),
                    namespace: None,
                    role_id: "test-role-id".to_string(),
                    secret_id: "test-secret-id".to_string(),
                    key_name: "test-key".to_string(),
                    mount_point: None,
                },
            },
        };

        let result = Signer::try_from(request);
        assert!(result.is_ok());

        let signer = result.unwrap();
        assert_eq!(signer.id, "test-vault-signer");
        assert_eq!(signer.signer_type(), SignerType::Vault);
    }

    #[test]
    fn test_invalid_aws_kms_empty_key_id() {
        let request = SignerCreateRequest {
            id: Some("test-signer".to_string()),
            config: SignerConfigRequest::AwsKms {
                config: AwsKmsSignerRequestConfig {
                    region: "us-east-1".to_string(),
                    key_id: "".to_string(), // Empty key ID should fail validation
                },
            },
        };

        let result = Signer::try_from(request);
        assert!(result.is_err());

        if let Err(ApiError::BadRequest(msg)) = result {
            assert!(msg.contains("Key ID cannot be empty"));
        } else {
            panic!("Expected BadRequest error for empty key ID");
        }
    }

    #[test]
    fn test_invalid_vault_empty_address() {
        let request = SignerCreateRequest {
            id: Some("test-signer".to_string()),
            config: SignerConfigRequest::Vault {
                config: VaultSignerRequestConfig {
                    address: "".to_string(), // Empty address should fail validation
                    namespace: None,
                    role_id: "test-role".to_string(),
                    secret_id: "test-secret".to_string(),
                    key_name: "test-key".to_string(),
                    mount_point: None,
                },
            },
        };

        let result = Signer::try_from(request);
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_vault_invalid_url() {
        let request = SignerCreateRequest {
            id: Some("test-signer".to_string()),
            config: SignerConfigRequest::Vault {
                config: VaultSignerRequestConfig {
                    address: "not-a-url".to_string(), // Invalid URL should fail validation
                    namespace: None,
                    role_id: "test-role".to_string(),
                    secret_id: "test-secret".to_string(),
                    key_name: "test-key".to_string(),
                    mount_point: None,
                },
            },
        };

        let result = Signer::try_from(request);
        assert!(result.is_err());

        if let Err(ApiError::BadRequest(msg)) = result {
            assert!(msg.contains("Address must be a valid URL"));
        } else {
            panic!("Expected BadRequest error for invalid URL");
        }
    }

    #[test]
    fn test_create_request_generates_uuid_when_no_id() {
        let request = SignerCreateRequest {
            id: None,
            config: SignerConfigRequest::Local {
                config: PlainSignerRequestConfig {
                    key: "1111111111111111111111111111111111111111111111111111111111111111"
                        .to_string(), // 32 bytes as hex
                },
            },
        };

        let result = Signer::try_from(request);
        assert!(result.is_ok());

        let signer = result.unwrap();
        assert!(!signer.id.is_empty());
        assert_eq!(signer.signer_type(), SignerType::Local);

        // Verify it's a valid UUID format
        assert!(uuid::Uuid::parse_str(&signer.id).is_ok());
    }

    #[test]
    fn test_invalid_id_format() {
        let request = SignerCreateRequest {
            id: Some("invalid@id".to_string()), // Invalid characters
            config: SignerConfigRequest::Local {
                config: PlainSignerRequestConfig {
                    key: "2222222222222222222222222222222222222222222222222222222222222222"
                        .to_string(), // 32 bytes as hex
                },
            },
        };

        let result = Signer::try_from(request);
        assert!(result.is_err());

        if let Err(ApiError::BadRequest(msg)) = result {
            assert!(msg.contains("ID must contain only letters, numbers, dashes and underscores"));
        } else {
            panic!("Expected BadRequest error with validation message");
        }
    }

    #[test]
    fn test_test_signer_creation() {
        let request = SignerCreateRequest {
            id: Some("test-signer".to_string()),
            config: SignerConfigRequest::Local {
                config: PlainSignerRequestConfig {
                    key: "3333333333333333333333333333333333333333333333333333333333333333"
                        .to_string(), // 32 bytes as hex
                },
            },
        };

        let result = Signer::try_from(request);
        assert!(result.is_ok());

        let signer = result.unwrap();
        assert_eq!(signer.id, "test-signer");
        assert_eq!(signer.signer_type(), SignerType::Local);
    }

    #[test]
    fn test_local_signer_creation() {
        let request = SignerCreateRequest {
            id: Some("local-signer".to_string()),
            config: SignerConfigRequest::Local {
                config: PlainSignerRequestConfig {
                    key: "4444444444444444444444444444444444444444444444444444444444444444"
                        .to_string(), // 32 bytes as hex
                },
            },
        };

        let result = Signer::try_from(request);
        assert!(result.is_ok());

        let signer = result.unwrap();
        assert_eq!(signer.id, "local-signer");
        assert_eq!(signer.signer_type(), SignerType::Local);
    }

    #[test]
    fn test_valid_turnkey_create_request() {
        let request = SignerCreateRequest {
            id: Some("test-turnkey-signer".to_string()),
            config: SignerConfigRequest::Turnkey {
                config: TurnkeySignerRequestConfig {
                    api_public_key: "test-public-key".to_string(),
                    api_private_key: "test-private-key".to_string(),
                    organization_id: "test-org".to_string(),
                    private_key_id: "test-private-key-id".to_string(),
                    public_key: "test-public-key".to_string(),
                },
            },
        };

        let result = Signer::try_from(request);
        assert!(result.is_ok());

        let signer = result.unwrap();
        assert_eq!(signer.id, "test-turnkey-signer");
        assert_eq!(signer.signer_type(), SignerType::Turnkey);

        if let Some(turnkey_config) = signer.config.get_turnkey() {
            assert_eq!(turnkey_config.api_public_key, "test-public-key");
            assert_eq!(turnkey_config.organization_id, "test-org");
        } else {
            panic!("Expected Turnkey config");
        }
    }

    #[test]
    fn test_valid_vault_cloud_create_request() {
        let request = SignerCreateRequest {
            id: Some("test-vault-cloud-signer".to_string()),
            config: SignerConfigRequest::VaultCloud {
                config: VaultCloudSignerRequestConfig {
                    client_id: "test-client-id".to_string(),
                    client_secret: "test-client-secret".to_string(),
                    org_id: "test-org".to_string(),
                    project_id: "test-project".to_string(),
                    app_name: "test-app".to_string(),
                    key_name: "test-key".to_string(),
                },
            },
        };

        let result = Signer::try_from(request);
        assert!(result.is_ok());

        let signer = result.unwrap();
        assert_eq!(signer.id, "test-vault-cloud-signer");
        assert_eq!(signer.signer_type(), SignerType::VaultCloud);
    }

    #[test]
    fn test_valid_vault_transit_create_request() {
        let request = SignerCreateRequest {
            id: Some("test-vault-transit-signer".to_string()),
            config: SignerConfigRequest::VaultTransit {
                config: VaultTransitSignerRequestConfig {
                    key_name: "test-key".to_string(),
                    address: "https://vault.example.com".to_string(),
                    namespace: None,
                    role_id: "test-role".to_string(),
                    secret_id: "test-secret".to_string(),
                    pubkey: "test-pubkey".to_string(),
                    mount_point: None,
                },
            },
        };

        let result = Signer::try_from(request);
        assert!(result.is_ok());

        let signer = result.unwrap();
        assert_eq!(signer.id, "test-vault-transit-signer");
        assert_eq!(signer.signer_type(), SignerType::VaultTransit);
    }

    #[test]
    fn test_valid_google_cloud_kms_create_request() {
        let request = SignerCreateRequest {
            id: Some("test-gcp-kms-signer".to_string()),
            config: SignerConfigRequest::GoogleCloudKms {
                config: GoogleCloudKmsSignerRequestConfig {
                    service_account: GoogleCloudKmsSignerServiceAccountRequestConfig {
                        private_key: "test-private-key".to_string(),
                        private_key_id: "test-key-id".to_string(),
                        project_id: "test-project".to_string(),
                        client_email: "test@email.com".to_string(),
                        client_id: "test-client-id".to_string(),
                        auth_uri: "https://accounts.google.com/o/oauth2/auth".to_string(),
                        token_uri: "https://oauth2.googleapis.com/token".to_string(),
                        auth_provider_x509_cert_url: "https://www.googleapis.com/oauth2/v1/certs".to_string(),
                        client_x509_cert_url: "https://www.googleapis.com/robot/v1/metadata/x509/test%40test.iam.gserviceaccount.com".to_string(),
                        universe_domain: "googleapis.com".to_string(),
                    },
                    key: GoogleCloudKmsSignerKeyRequestConfig {
                        location: "global".to_string(),
                        key_ring_id: "test-ring".to_string(),
                        key_id: "test-key".to_string(),
                        key_version: 1,
                    },
                },
            },
        };

        let result = Signer::try_from(request);
        assert!(result.is_ok());

        let signer = result.unwrap();
        assert_eq!(signer.id, "test-gcp-kms-signer");
        assert_eq!(signer.signer_type(), SignerType::GoogleCloudKms);
    }

    #[test]
    fn test_invalid_local_hex_key() {
        let request = SignerCreateRequest {
            id: Some("test-signer".to_string()),
            config: SignerConfigRequest::Local {
                config: PlainSignerRequestConfig {
                    key: "invalid-hex".to_string(), // Invalid hex
                },
            },
        };

        let result = Signer::try_from(request);
        assert!(result.is_err());
        if let Err(ApiError::BadRequest(msg)) = result {
            assert!(msg.contains("Invalid hex key format"));
        }
    }

    #[test]
    fn test_invalid_turnkey_empty_key() {
        let request = SignerCreateRequest {
            id: Some("test-signer".to_string()),
            config: SignerConfigRequest::Turnkey {
                config: TurnkeySignerRequestConfig {
                    api_public_key: "".to_string(), // Empty
                    api_private_key: "test-private-key".to_string(),
                    organization_id: "test-org".to_string(),
                    private_key_id: "test-private-key-id".to_string(),
                    public_key: "test-public-key".to_string(),
                },
            },
        };

        let result = Signer::try_from(request);
        assert!(result.is_err());
        if let Err(ApiError::BadRequest(msg)) = result {
            assert!(msg.contains("API public key cannot be empty"));
        }
    }
}
