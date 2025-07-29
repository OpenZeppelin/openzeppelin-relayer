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
    SignerConfig, TurnkeySignerConfig, VaultSignerConfig, VaultTransitSignerConfig,
};
use secrets::SecretVec;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use zeroize::Zeroize;

/// Local signer configuration for API requests
#[derive(Debug, Serialize, Deserialize, ToSchema, Zeroize)]
#[serde(deny_unknown_fields)]
pub struct LocalSignerRequestConfig {
    pub key: String,
}

/// AWS KMS signer configuration for API requests
#[derive(Debug, Serialize, Deserialize, ToSchema, Zeroize)]
#[serde(deny_unknown_fields)]
pub struct AwsKmsSignerRequestConfig {
    pub region: String,
    pub key_id: String,
}

/// Vault signer configuration for API requests
#[derive(Debug, Serialize, Deserialize, ToSchema, Zeroize)]
#[serde(deny_unknown_fields)]
pub struct VaultSignerRequestConfig {
    pub address: String,
    #[schema(nullable = false)]
    pub namespace: Option<String>,
    pub role_id: String,
    pub secret_id: String,
    pub key_name: String,
    #[schema(nullable = false)]
    pub mount_point: Option<String>,
}

/// Vault Transit signer configuration for API requests
#[derive(Debug, Serialize, Deserialize, ToSchema, Zeroize)]
#[serde(deny_unknown_fields)]
pub struct VaultTransitSignerRequestConfig {
    pub key_name: String,
    pub address: String,
    #[schema(nullable = false)]
    pub namespace: Option<String>,
    pub role_id: String,
    pub secret_id: String,
    pub pubkey: String,
    #[schema(nullable = false)]
    pub mount_point: Option<String>,
}

/// Turnkey signer configuration for API requests
#[derive(Debug, Serialize, Deserialize, ToSchema, Zeroize)]
#[serde(deny_unknown_fields)]
pub struct TurnkeySignerRequestConfig {
    pub api_public_key: String,
    pub api_private_key: String,
    pub organization_id: String,
    pub private_key_id: String,
    pub public_key: String,
}

/// Google Cloud KMS service account configuration for API requests
#[derive(Debug, Serialize, Deserialize, ToSchema, Zeroize)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub struct GoogleCloudKmsSignerKeyRequestConfig {
    pub location: String,
    pub key_ring_id: String,
    pub key_id: String,
    pub key_version: u32,
}

/// Google Cloud KMS signer configuration for API requests
#[derive(Debug, Serialize, Deserialize, ToSchema, Zeroize)]
#[serde(deny_unknown_fields)]
pub struct GoogleCloudKmsSignerRequestConfig {
    pub service_account: GoogleCloudKmsSignerServiceAccountRequestConfig,
    pub key: GoogleCloudKmsSignerKeyRequestConfig,
}

/// Signer configuration enum for API requests (without type discriminator)
#[derive(Debug, Serialize, Deserialize, ToSchema, Zeroize)]
#[serde(untagged)]
pub enum SignerConfigRequest {
    Local(LocalSignerRequestConfig),
    AwsKms(AwsKmsSignerRequestConfig),
    Vault(VaultSignerRequestConfig),
    VaultTransit(VaultTransitSignerRequestConfig),
    Turnkey(TurnkeySignerRequestConfig),
    GoogleCloudKms(GoogleCloudKmsSignerRequestConfig),
}

/// Signer type enum for API requests
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum SignerTypeRequest {
    #[serde(rename = "plain")]
    Local,
    #[serde(rename = "aws_kms")]
    AwsKms,
    Vault,
    #[serde(rename = "vault_transit")]
    VaultTransit,
    Turnkey,
    #[serde(rename = "google_cloud_kms")]
    GoogleCloudKms,
}

impl zeroize::Zeroize for SignerTypeRequest {
    fn zeroize(&mut self) {
        // No sensitive data to zeroize in this enum
    }
}

/// Request model for creating a new signer
#[derive(Debug, Serialize, Deserialize, ToSchema, Zeroize)]
#[serde(deny_unknown_fields)]
pub struct SignerCreateRequest {
    /// Optional ID - if not provided, a UUID will be generated
    #[schema(nullable = false)]
    pub id: Option<String>,
    /// The type of signer
    #[serde(rename = "type")]
    pub signer_type: SignerTypeRequest,
    /// The signer configuration
    pub config: SignerConfigRequest,
}

/// Request model for updating an existing signer
/// At the moment, we don't allow updating signers
#[derive(Debug, Serialize, Deserialize, ToSchema, Zeroize)]
#[serde(deny_unknown_fields)]
pub struct SignerUpdateRequest {}

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

impl TryFrom<SignerConfigRequest> for SignerConfig {
    type Error = ApiError;

    fn try_from(config: SignerConfigRequest) -> Result<Self, Self::Error> {
        let domain_config = match config {
            SignerConfigRequest::Local(local_config) => {
                // Decode hex string to raw bytes for cryptographic key
                let key_bytes = hex::decode(&local_config.key)
                    .map_err(|e| ApiError::BadRequest(format!(
                        "Invalid hex key format: {}. Key must be a 64-character hex string (32 bytes).", e
                    )))?;

                let raw_key = SecretVec::new(key_bytes.len(), |buffer| {
                    buffer.copy_from_slice(&key_bytes);
                });

                SignerConfig::Local(LocalSignerConfig { raw_key })
            }
            SignerConfigRequest::AwsKms(aws_config) => SignerConfig::AwsKms(AwsKmsSignerConfig {
                region: Some(aws_config.region),
                key_id: aws_config.key_id,
            }),
            SignerConfigRequest::Vault(vault_config) => SignerConfig::Vault(VaultSignerConfig {
                address: vault_config.address,
                namespace: vault_config.namespace,
                role_id: SecretString::new(&vault_config.role_id),
                secret_id: SecretString::new(&vault_config.secret_id),
                key_name: vault_config.key_name,
                mount_point: vault_config.mount_point,
            }),
            SignerConfigRequest::VaultTransit(vault_transit_config) => {
                SignerConfig::VaultTransit(VaultTransitSignerConfig {
                    key_name: vault_transit_config.key_name,
                    address: vault_transit_config.address,
                    namespace: vault_transit_config.namespace,
                    role_id: SecretString::new(&vault_transit_config.role_id),
                    secret_id: SecretString::new(&vault_transit_config.secret_id),
                    pubkey: vault_transit_config.pubkey,
                    mount_point: vault_transit_config.mount_point,
                })
            }
            SignerConfigRequest::Turnkey(turnkey_config) => {
                SignerConfig::Turnkey(TurnkeySignerConfig {
                    api_public_key: turnkey_config.api_public_key,
                    api_private_key: SecretString::new(&turnkey_config.api_private_key),
                    organization_id: turnkey_config.organization_id,
                    private_key_id: turnkey_config.private_key_id,
                    public_key: turnkey_config.public_key,
                })
            }
            SignerConfigRequest::GoogleCloudKms(gcp_kms_config) => {
                SignerConfig::GoogleCloudKms(GoogleCloudKmsSignerConfig {
                    service_account: gcp_kms_config.service_account.into(),
                    key: gcp_kms_config.key.into(),
                })
            }
        };

        // Validate the configuration using domain model validation
        domain_config.validate().map_err(ApiError::from)?;

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

        // Validate that the signer type matches the config variant
        let config_matches_type = matches!(
            (&request.signer_type, &request.config),
            (SignerTypeRequest::Local, SignerConfigRequest::Local(_))
                | (SignerTypeRequest::AwsKms, SignerConfigRequest::AwsKms(_))
                | (SignerTypeRequest::Vault, SignerConfigRequest::Vault(_))
                | (
                    SignerTypeRequest::VaultTransit,
                    SignerConfigRequest::VaultTransit(_)
                )
                | (SignerTypeRequest::Turnkey, SignerConfigRequest::Turnkey(_))
                | (
                    SignerTypeRequest::GoogleCloudKms,
                    SignerConfigRequest::GoogleCloudKms(_)
                )
        );

        if !config_matches_type {
            return Err(ApiError::BadRequest(format!(
                "Signer type '{:?}' does not match the provided configuration",
                request.signer_type
            )));
        }

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
    use crate::models::signer::SignerType;

    #[test]
    fn test_json_deserialization_local_signer() {
        let json = r#"{
            "id": "test-local-signer",
            "type": "plain",
            "config": {
                "key": "1111111111111111111111111111111111111111111111111111111111111111"
            }
        }"#;

        let result: Result<SignerCreateRequest, _> = serde_json::from_str(json);

        assert!(
            result.is_ok(),
            "Failed to deserialize local signer: {:?}",
            result.err()
        );

        let request = result.unwrap();
        assert_eq!(request.id, Some("test-local-signer".to_string()));

        match request.config {
            SignerConfigRequest::Local(local_config) => {
                assert_eq!(
                    local_config.key,
                    "1111111111111111111111111111111111111111111111111111111111111111"
                );
            }
            _ => panic!("Expected Local config variant"),
        }
    }

    #[test]
    fn test_json_deserialization_aws_kms_signer() {
        let json = r#"{
            "id": "test-aws-signer",
            "type": "aws_kms",
            "config": {
                "region": "us-east-1",
                "key_id": "test-key-id"
            }
        }"#;

        let result: Result<SignerCreateRequest, _> = serde_json::from_str(json);

        assert!(
            result.is_ok(),
            "Failed to deserialize AWS KMS signer: {:?}",
            result.err()
        );

        let request = result.unwrap();
        assert_eq!(request.id, Some("test-aws-signer".to_string()));

        match request.config {
            SignerConfigRequest::AwsKms(aws_config) => {
                assert_eq!(aws_config.region, "us-east-1");
                assert_eq!(aws_config.key_id, "test-key-id");
            }
            _ => panic!("Expected AwsKms config variant"),
        }
    }

    #[test]
    fn test_json_deserialization_vault_signer() {
        let json = r#"{
            "id": "test-vault-signer",
            "type": "vault",
            "config": {
                "address": "https://vault.example.com",
                "namespace": null,
                "role_id": "test-role-id",
                "secret_id": "test-secret-id",
                "key_name": "test-key",
                "mount_point": null
            }
        }"#;

        let result: Result<SignerCreateRequest, _> = serde_json::from_str(json);

        assert!(
            result.is_ok(),
            "Failed to deserialize Vault signer: {:?}",
            result.err()
        );

        let request = result.unwrap();
        assert_eq!(request.id, Some("test-vault-signer".to_string()));

        match request.config {
            SignerConfigRequest::Vault(vault_config) => {
                assert_eq!(vault_config.address, "https://vault.example.com");
                assert_eq!(vault_config.namespace, None);
                assert_eq!(vault_config.role_id, "test-role-id");
                assert_eq!(vault_config.secret_id, "test-secret-id");
                assert_eq!(vault_config.key_name, "test-key");
                assert_eq!(vault_config.mount_point, None);
            }
            _ => panic!("Expected Vault config variant"),
        }
    }

    #[test]
    fn test_json_deserialization_turnkey_signer() {
        let json = r#"{
            "id": "test-turnkey-signer",
            "type": "turnkey",
            "config": {
                "api_public_key": "test-public-key",
                "api_private_key": "test-private-key",
                "organization_id": "test-org",
                "private_key_id": "test-private-key-id",
                "public_key": "test-public-key"
            }
        }"#;

        let result: Result<SignerCreateRequest, _> = serde_json::from_str(json);

        assert!(
            result.is_ok(),
            "Failed to deserialize Turnkey signer: {:?}",
            result.err()
        );

        let request = result.unwrap();
        assert_eq!(request.id, Some("test-turnkey-signer".to_string()));

        match request.config {
            SignerConfigRequest::Turnkey(turnkey_config) => {
                assert_eq!(turnkey_config.api_public_key, "test-public-key");
                assert_eq!(turnkey_config.api_private_key, "test-private-key");
                assert_eq!(turnkey_config.organization_id, "test-org");
                assert_eq!(turnkey_config.private_key_id, "test-private-key-id");
                assert_eq!(turnkey_config.public_key, "test-public-key");
            }
            _ => panic!("Expected Turnkey config variant"),
        }
    }

    #[test]
    fn test_json_serialization_local_signer() {
        let request = SignerCreateRequest {
            id: Some("test-local-signer".to_string()),
            signer_type: SignerTypeRequest::Local,
            config: SignerConfigRequest::Local(LocalSignerRequestConfig {
                key: "1111111111111111111111111111111111111111111111111111111111111111".to_string(),
            }),
        };

        let json_result = serde_json::to_string_pretty(&request);

        assert!(
            json_result.is_ok(),
            "Failed to serialize local signer: {:?}",
            json_result.err()
        );

        let json = json_result.unwrap();

        // Verify it can be deserialized back
        let deserialize_result: Result<SignerCreateRequest, _> = serde_json::from_str(&json);
        assert!(
            deserialize_result.is_ok(),
            "Failed to deserialize back: {:?}",
            deserialize_result.err()
        );
    }

    #[test]
    fn test_json_serialization_aws_kms_signer() {
        let request = SignerCreateRequest {
            id: Some("test-aws-signer".to_string()),
            signer_type: SignerTypeRequest::AwsKms,
            config: SignerConfigRequest::AwsKms(AwsKmsSignerRequestConfig {
                region: "us-east-1".to_string(),
                key_id: "test-key-id".to_string(),
            }),
        };

        let json_result = serde_json::to_string_pretty(&request);

        assert!(
            json_result.is_ok(),
            "Failed to serialize AWS KMS signer: {:?}",
            json_result.err()
        );

        let json = json_result.unwrap();

        // Verify it can be deserialized back
        let deserialize_result: Result<SignerCreateRequest, _> = serde_json::from_str(&json);
        assert!(
            deserialize_result.is_ok(),
            "Failed to deserialize back: {:?}",
            deserialize_result.err()
        );
    }

    #[test]
    fn test_type_config_mismatch_validation() {
        // Create a request where the type doesn't match the config
        let json = r#"{
            "id": "test-mismatch-signer",
            "type": "aws_kms",
            "config": {
                "key": "1111111111111111111111111111111111111111111111111111111111111111"
            }
        }"#;

        let result: Result<SignerCreateRequest, _> = serde_json::from_str(json);

        // This should deserialize successfully due to untagged enum
        assert!(result.is_ok(), "JSON deserialization should succeed");

        let request = result.unwrap();

        // But the conversion to Signer should fail due to type mismatch validation
        let signer_result = Signer::try_from(request);
        assert!(
            signer_result.is_err(),
            "Type mismatch should cause validation error"
        );

        if let Err(ApiError::BadRequest(msg)) = signer_result {
            assert!(
                msg.contains("does not match"),
                "Error should mention type mismatch: {}",
                msg
            );
        } else {
            panic!("Expected BadRequest error for type mismatch");
        }
    }

    // Keep existing tests for backward compatibility
    #[test]
    fn test_valid_aws_kms_create_request() {
        let request = SignerCreateRequest {
            id: Some("test-aws-signer".to_string()),
            signer_type: SignerTypeRequest::AwsKms,
            config: SignerConfigRequest::AwsKms(AwsKmsSignerRequestConfig {
                region: "us-east-1".to_string(),
                key_id: "test-key-id".to_string(),
            }),
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
            signer_type: SignerTypeRequest::Vault,
            config: SignerConfigRequest::Vault(VaultSignerRequestConfig {
                address: "https://vault.example.com".to_string(),
                namespace: None,
                role_id: "test-role-id".to_string(),
                secret_id: "test-secret-id".to_string(),
                key_name: "test-key".to_string(),
                mount_point: None,
            }),
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
            signer_type: SignerTypeRequest::AwsKms,
            config: SignerConfigRequest::AwsKms(AwsKmsSignerRequestConfig {
                region: "us-east-1".to_string(),
                key_id: "".to_string(), // Empty key ID should fail validation
            }),
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
            signer_type: SignerTypeRequest::Vault,
            config: SignerConfigRequest::Vault(VaultSignerRequestConfig {
                address: "".to_string(), // Empty address should fail validation
                namespace: None,
                role_id: "test-role".to_string(),
                secret_id: "test-secret".to_string(),
                key_name: "test-key".to_string(),
                mount_point: None,
            }),
        };

        let result = Signer::try_from(request);
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_vault_invalid_url() {
        let request = SignerCreateRequest {
            id: Some("test-signer".to_string()),
            signer_type: SignerTypeRequest::Vault,
            config: SignerConfigRequest::Vault(VaultSignerRequestConfig {
                address: "not-a-url".to_string(), // Invalid URL should fail validation
                namespace: None,
                role_id: "test-role".to_string(),
                secret_id: "test-secret".to_string(),
                key_name: "test-key".to_string(),
                mount_point: None,
            }),
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
            signer_type: SignerTypeRequest::Local,
            config: SignerConfigRequest::Local(LocalSignerRequestConfig {
                key: "1111111111111111111111111111111111111111111111111111111111111111".to_string(), // 32 bytes as hex
            }),
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
            signer_type: SignerTypeRequest::Local,
            config: SignerConfigRequest::Local(LocalSignerRequestConfig {
                key: "2222222222222222222222222222222222222222222222222222222222222222".to_string(), // 32 bytes as hex
            }),
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
            signer_type: SignerTypeRequest::Local,
            config: SignerConfigRequest::Local(LocalSignerRequestConfig {
                key: "3333333333333333333333333333333333333333333333333333333333333333".to_string(), // 32 bytes as hex
            }),
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
            signer_type: SignerTypeRequest::Local,
            config: SignerConfigRequest::Local(LocalSignerRequestConfig {
                key: "4444444444444444444444444444444444444444444444444444444444444444".to_string(), // 32 bytes as hex
            }),
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
            signer_type: SignerTypeRequest::Turnkey,
            config: SignerConfigRequest::Turnkey(TurnkeySignerRequestConfig {
                api_public_key: "test-public-key".to_string(),
                api_private_key: "test-private-key".to_string(),
                organization_id: "test-org".to_string(),
                private_key_id: "test-private-key-id".to_string(),
                public_key: "test-public-key".to_string(),
            }),
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
    fn test_valid_vault_transit_create_request() {
        let request = SignerCreateRequest {
            id: Some("test-vault-transit-signer".to_string()),
            signer_type: SignerTypeRequest::VaultTransit,
            config: SignerConfigRequest::VaultTransit(VaultTransitSignerRequestConfig {
                key_name: "test-key".to_string(),
                address: "https://vault.example.com".to_string(),
                namespace: None,
                role_id: "test-role".to_string(),
                secret_id: "test-secret".to_string(),
                pubkey: "test-pubkey".to_string(),
                mount_point: None,
            }),
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
            signer_type: SignerTypeRequest::GoogleCloudKms,
            config: SignerConfigRequest::GoogleCloudKms(GoogleCloudKmsSignerRequestConfig {
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
            }),
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
            signer_type: SignerTypeRequest::Local,
            config: SignerConfigRequest::Local(LocalSignerRequestConfig {
                key: "invalid-hex".to_string(), // Invalid hex
            }),
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
            signer_type: SignerTypeRequest::Turnkey,
            config: SignerConfigRequest::Turnkey(TurnkeySignerRequestConfig {
                api_public_key: "".to_string(), // Empty
                api_private_key: "test-private-key".to_string(),
                organization_id: "test-org".to_string(),
                private_key_id: "test-private-key-id".to_string(),
                public_key: "test-public-key".to_string(),
            }),
        };

        let result = Signer::try_from(request);
        assert!(result.is_err());
        if let Err(ApiError::BadRequest(msg)) = result {
            assert!(msg.contains("API public key cannot be empty"));
        }
    }
}
