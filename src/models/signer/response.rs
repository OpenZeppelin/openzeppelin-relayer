//! API response models for signer endpoints.
//!
//! This module handles outgoing HTTP responses for signer operations, providing:
//!
//! - **Response Models**: Structures for returning signer data via API
//! - **Data Sanitization**: Ensures sensitive information is not exposed
//! - **Domain Conversion**: Transformation from domain/repository objects to API responses
//!
//! Serves as the exit point for signer data to external clients, ensuring
//! proper data formatting and security considerations.

use crate::models::{Signer, SignerConfig, SignerRepoModel, SignerType};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Signer configuration response
/// Does not include sensitive information like private keys
#[derive(Debug, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(untagged)]
#[serde(rename_all = "lowercase")]
pub enum SignerConfigResponse {
    #[serde(rename = "plain")]
    Plain {
        has_key: bool,
    },
    Vault {
        address: String,
        namespace: Option<String>,
        key_name: String,
        mount_point: Option<String>,
        has_role_id: bool,
        has_secret_id: bool,
    },
    #[serde(rename = "vault_cloud")]
    VaultCloud {
        client_id: String,
        org_id: String,
        project_id: String,
        app_name: String,
        key_name: String,
        has_client_secret: bool,
    },
    #[serde(rename = "vault_transit")]
    VaultTransit {
        key_name: String,
        address: String,
        namespace: Option<String>,
        pubkey: String,
        mount_point: Option<String>,
        has_role_id: bool,
        has_secret_id: bool,
    },
    #[serde(rename = "aws_kms")]
    AwsKms {
        region: Option<String>,
        key_id: String,
    },
    Turnkey {
        api_public_key: String,
        organization_id: String,
        private_key_id: String,
        public_key: String,
        has_api_private_key: bool,
    },
    #[serde(rename = "google_cloud_kms")]
    GoogleCloudKms {
        service_account: GoogleCloudKmsSignerServiceAccountResponseConfig,
        key: GoogleCloudKmsSignerKeyResponseConfig,
    },
}

#[derive(Debug, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
pub struct GoogleCloudKmsSignerServiceAccountResponseConfig {
    pub project_id: String,
    pub client_id: String,
    pub auth_uri: String,
    pub token_uri: String,
    pub auth_provider_x509_cert_url: String,
    pub client_x509_cert_url: String,
    pub universe_domain: String,
    pub has_private_key: bool,
    pub has_private_key_id: bool,
    pub has_client_email: bool,
}

#[derive(Debug, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
pub struct GoogleCloudKmsSignerKeyResponseConfig {
    pub location: String,
    pub key_ring_id: String,
    pub key_id: String,
    pub key_version: u32,
}

impl From<SignerConfig> for SignerConfigResponse {
    fn from(config: SignerConfig) -> Self {
        match config {
            SignerConfig::Local(c) => SignerConfigResponse::Plain {
                has_key: !c.raw_key.is_empty(),
            },
            SignerConfig::Vault(c) => SignerConfigResponse::Vault {
                address: c.address,
                namespace: c.namespace,
                key_name: c.key_name,
                mount_point: c.mount_point,
                has_role_id: !c.role_id.is_empty(),
                has_secret_id: !c.secret_id.is_empty(),
            },
            SignerConfig::VaultCloud(c) => {
                SignerConfigResponse::VaultCloud {
                    client_id: c.client_id,
                    org_id: c.org_id,
                    project_id: c.project_id,
                    app_name: c.app_name,
                    key_name: c.key_name,
                    has_client_secret: !c.client_secret.is_empty(),
                }
            }
            SignerConfig::VaultTransit(c) => {
                SignerConfigResponse::VaultTransit {
                    key_name: c.key_name,
                    address: c.address,
                    namespace: c.namespace,
                    pubkey: c.pubkey,
                    mount_point: c.mount_point,
                    has_role_id: !c.role_id.is_empty(),
                    has_secret_id: !c.secret_id.is_empty(),
                }
            }
            SignerConfig::AwsKms(c) => {
                SignerConfigResponse::AwsKms {
                    region: c.region,
                    key_id: c.key_id,
                }
            }
            SignerConfig::Turnkey(c) => {
                SignerConfigResponse::Turnkey {
                    api_public_key: c.api_public_key,
                    organization_id: c.organization_id,
                    private_key_id: c.private_key_id,
                    public_key: c.public_key,
                    has_api_private_key: !c.api_private_key.is_empty(),
                }
            }
            SignerConfig::GoogleCloudKms(c) => {
                SignerConfigResponse::GoogleCloudKms {
                    service_account: GoogleCloudKmsSignerServiceAccountResponseConfig {
                        project_id: c.service_account.project_id,
                        client_id: c.service_account.client_id,
                        auth_uri: c.service_account.auth_uri,
                        token_uri: c.service_account.token_uri,
                        auth_provider_x509_cert_url: c.service_account.auth_provider_x509_cert_url,
                        client_x509_cert_url: c.service_account.client_x509_cert_url,
                        universe_domain: c.service_account.universe_domain,
                        has_private_key: !c.service_account.private_key.is_empty(),
                        has_private_key_id: !c.service_account.private_key_id.is_empty(),
                        has_client_email: !c.service_account.client_email.is_empty(),
                    },
                    key: GoogleCloudKmsSignerKeyResponseConfig {
                        location: c.key.location,
                        key_ring_id: c.key.key_ring_id,
                        key_id: c.key.key_id,
                        key_version: c.key.key_version,
                    },
                }
            }
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct SignerResponse {
    /// The unique identifier of the signer
    pub id: String,
    /// The type of signer (local, aws_kms, google_cloud_kms, vault, etc.)
    pub r#type: SignerType,
    /// Optional human-readable name for the signer
    pub name: Option<String>,
    /// Optional description of the signer's purpose
    pub description: Option<String>,
    /// Non-secret configuration details
    pub config: SignerConfigResponse,
}

impl From<SignerRepoModel> for SignerResponse {
    fn from(repo_model: SignerRepoModel) -> Self {
        // Convert to domain model
        let domain_signer = Signer::from(repo_model);

        Self {
            id: domain_signer.id.clone(),
            r#type: domain_signer.signer_type(),
            name: domain_signer.name,
            description: domain_signer.description,
            config: SignerConfigResponse::from(domain_signer.config),
        }
    }
}

impl From<Signer> for SignerResponse {
    fn from(signer: Signer) -> Self {
        Self {
            id: signer.id.clone(),
            r#type: signer.signer_type(),
            name: signer.name,
            description: signer.description,
            config: SignerConfigResponse::from(signer.config),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{LocalSignerConfig, SignerConfig};
    use secrets::SecretVec;

    #[test]
    fn test_signer_response_from_repo_model() {
        let repo_model = SignerRepoModel {
            id: "test-signer".to_string(),
            config: SignerConfig::Local(LocalSignerConfig {
                raw_key: SecretVec::new(32, |v| v.copy_from_slice(&[1; 32])),
            }),
        };

        let response = SignerResponse::from(repo_model);

        assert_eq!(response.id, "test-signer");
        assert_eq!(response.r#type, SignerType::Local);
        assert_eq!(response.name, None);
        assert_eq!(response.description, None);
        assert_eq!(response.config, SignerConfigResponse::Plain {
            has_key: true,
        });
    }

    #[test]
    fn test_signer_response_from_domain_model() {
        use crate::models::signer::signer::{AwsKmsSignerConfig, SignerConfig};

        let aws_config = AwsKmsSignerConfig {
            key_id: "test-key-id".to_string(),
            region: Some("us-east-1".to_string()),
        };

        let signer = crate::models::Signer::new(
            "domain-signer".to_string(),
            SignerConfig::AwsKms(aws_config),
            Some("AWS KMS Signer".to_string()),
            Some("Production AWS KMS signer".to_string()),
        );

        let response = SignerResponse::from(signer);

        assert_eq!(response.id, "domain-signer");
        assert_eq!(response.r#type, SignerType::AwsKms);
        assert_eq!(response.name, Some("AWS KMS Signer".to_string()));
        assert_eq!(
            response.description,
            Some("Production AWS KMS signer".to_string())
        );
        assert_eq!(
            response.config,
            SignerConfigResponse::AwsKms {
                region: Some("us-east-1".to_string()),
                key_id: "test-key-id".to_string(),
            }
        );
    }

    #[test]
    fn test_signer_type_mapping_from_config() {
        let test_cases = vec![
            (
                SignerConfig::Local(LocalSignerConfig {
                    raw_key: SecretVec::new(32, |v| v.copy_from_slice(&[1; 32])),
                }),
                SignerType::Local,
                SignerConfigResponse::Plain {
                    has_key: true,
                },
            ),
            (
                SignerConfig::AwsKms(crate::models::AwsKmsSignerConfig {
                    region: Some("us-east-1".to_string()),
                    key_id: "test-key".to_string(),
                }),
                SignerType::AwsKms,
                SignerConfigResponse::AwsKms {
                    region: Some("us-east-1".to_string()),
                    key_id: "test-key".to_string(),
                },
            ),
        ];

        for (config, expected_type, expected_config) in test_cases {
            let repo_model = SignerRepoModel {
                id: "test".to_string(),
                config,
            };

            let response = SignerResponse::from(repo_model);
            assert_eq!(
                response.r#type, expected_type,
                "Type mapping failed for {:?}",
                expected_type
            );
            assert_eq!(response.config, expected_config);
        }
    }

    #[test]
    fn test_response_serialization() {
        let response = SignerResponse {
            id: "test-signer".to_string(),
            r#type: SignerType::Local,
            name: Some("Test Signer".to_string()),
            description: Some("A test signer".to_string()),
            config: SignerConfigResponse::Plain {
                has_key: true,
            },
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"id\":\"test-signer\""));
        assert!(json.contains("\"type\":\"local\""));
        assert!(json.contains("\"name\":\"Test Signer\""));
        assert!(json.contains("\"has_key\":true")); // Updated to match actual format
    }

    #[test]
    fn test_response_deserialization() {
        let json = r#"{
            "id": "test-signer",
            "type": "aws_kms",
            "name": "AWS KMS Signer",
            "description": "Production signer",
            "config": {
                "region": "us-east-1",
                "key_id": "test-key-id"
            }
        }"#;

        let response: SignerResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.id, "test-signer");
        assert_eq!(response.r#type, SignerType::AwsKms);
        assert_eq!(response.name, Some("AWS KMS Signer".to_string()));
        assert_eq!(response.description, Some("Production signer".to_string()));
        assert_eq!(
            response.config,
            SignerConfigResponse::AwsKms {
                region: Some("us-east-1".to_string()),
                key_id: "test-key-id".to_string(),
            }
        );
    }
}
