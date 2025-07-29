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
    Vault {
        address: String,
        namespace: Option<String>,
        key_name: String,
        mount_point: Option<String>,
        // role_id: Option<String>, hidden from response due to security concerns
        // secret_id: Option<String>, hidden from response due to security concerns
    },
    #[serde(rename = "vault_transit")]
    VaultTransit {
        key_name: String,
        address: String,
        namespace: Option<String>,
        pubkey: String,
        mount_point: Option<String>,
        // role_id: Option<String>, hidden from response due to security concerns
        // secret_id: Option<String>, hidden from response due to security concerns
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
        // api_private_key: Option<String>, hidden from response due to security concerns
    },
    #[serde(rename = "google_cloud_kms")]
    GoogleCloudKms {
        service_account: GoogleCloudKmsSignerServiceAccountResponseConfig,
        key: GoogleCloudKmsSignerKeyResponseConfig,
    },
    Plain {},
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
    // pub private_key: Option<String>, hidden from response due to security concerns
    // pub private_key_id: Option<String>, hidden from response due to security concerns
    // pub client_email: Option<String>, hidden from response due to security concerns
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
            SignerConfig::Local(_) => SignerConfigResponse::Plain {},
            SignerConfig::Vault(c) => SignerConfigResponse::Vault {
                address: c.address,
                namespace: c.namespace,
                key_name: c.key_name,
                mount_point: c.mount_point,
            },
            SignerConfig::VaultTransit(c) => SignerConfigResponse::VaultTransit {
                key_name: c.key_name,
                address: c.address,
                namespace: c.namespace,
                pubkey: c.pubkey,
                mount_point: c.mount_point,
            },
            SignerConfig::AwsKms(c) => SignerConfigResponse::AwsKms {
                region: c.region,
                key_id: c.key_id,
            },
            SignerConfig::Turnkey(c) => SignerConfigResponse::Turnkey {
                api_public_key: c.api_public_key,
                organization_id: c.organization_id,
                private_key_id: c.private_key_id,
                public_key: c.public_key,
            },
            SignerConfig::GoogleCloudKms(c) => SignerConfigResponse::GoogleCloudKms {
                service_account: GoogleCloudKmsSignerServiceAccountResponseConfig {
                    project_id: c.service_account.project_id,
                    client_id: c.service_account.client_id,
                    auth_uri: c.service_account.auth_uri,
                    token_uri: c.service_account.token_uri,
                    auth_provider_x509_cert_url: c.service_account.auth_provider_x509_cert_url,
                    client_x509_cert_url: c.service_account.client_x509_cert_url,
                    universe_domain: c.service_account.universe_domain,
                },
                key: GoogleCloudKmsSignerKeyResponseConfig {
                    location: c.key.location,
                    key_ring_id: c.key.key_ring_id,
                    key_id: c.key.key_id,
                    key_version: c.key.key_version,
                },
            },
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct SignerResponse {
    /// The unique identifier of the signer
    pub id: String,
    /// The type of signer (local, aws_kms, google_cloud_kms, vault, etc.)
    pub r#type: SignerType,
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
            config: SignerConfigResponse::from(domain_signer.config),
        }
    }
}

impl From<Signer> for SignerResponse {
    fn from(signer: Signer) -> Self {
        Self {
            id: signer.id.clone(),
            r#type: signer.signer_type(),
            config: SignerConfigResponse::from(signer.config),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{LocalSignerConfigStorage, SignerConfigStorage};
    use secrets::SecretVec;

    #[test]
    fn test_signer_response_from_repo_model() {
        let repo_model = SignerRepoModel {
            id: "test-signer".to_string(),
            config: SignerConfigStorage::Local(LocalSignerConfigStorage {
                raw_key: SecretVec::new(32, |v| v.copy_from_slice(&[1; 32])),
            }),
        };

        let response = SignerResponse::from(repo_model);

        assert_eq!(response.id, "test-signer");
        assert_eq!(response.r#type, SignerType::Local);
        assert_eq!(response.config, SignerConfigResponse::Plain {});
    }

    #[test]
    fn test_signer_response_from_domain_model() {
        use crate::models::signer::{AwsKmsSignerConfig, SignerConfig};

        let aws_config = AwsKmsSignerConfig {
            key_id: "test-key-id".to_string(),
            region: Some("us-east-1".to_string()),
        };

        let signer = crate::models::Signer::new(
            "domain-signer".to_string(),
            SignerConfig::AwsKms(aws_config),
        );

        let response = SignerResponse::from(signer);

        assert_eq!(response.id, "domain-signer");
        assert_eq!(response.r#type, SignerType::AwsKms);
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
                SignerConfigStorage::Local(LocalSignerConfigStorage {
                    raw_key: SecretVec::new(32, |v| v.copy_from_slice(&[1; 32])),
                }),
                SignerType::Local,
                SignerConfigResponse::Plain {},
            ),
            (
                SignerConfigStorage::AwsKms(crate::models::AwsKmsSignerConfigStorage {
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
            config: SignerConfigResponse::Plain {},
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"id\":\"test-signer\""));
        assert!(json.contains("\"type\":\"local\""));
    }

    #[test]
    fn test_response_deserialization() {
        let json = r#"{
            "id": "test-signer",
            "type": "aws_kms",
            "config": {
                "region": "us-east-1",
                "key_id": "test-key-id"
            }
        }"#;

        let response: SignerResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.id, "test-signer");
        assert_eq!(response.r#type, SignerType::AwsKms);
        assert_eq!(
            response.config,
            SignerConfigResponse::AwsKms {
                region: Some("us-east-1".to_string()),
                key_id: "test-key-id".to_string(),
            }
        );
    }

    #[test]
    fn test_response_deserialization_all_types() {
        let json = r#"{"id": "test", "type": "google_cloud_kms", "config": {"service_account": {"project_id": "proj", "client_id": "cid", "auth_uri": "auth", "token_uri": "token", "auth_provider_x509_cert_url": "cert", "client_x509_cert_url": "client_cert", "universe_domain": "domain"}, "key": {"location": "loc", "key_ring_id": "ring", "key_id": "key", "key_version": 1}}}"#;

        let response: SignerResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.r#type, SignerType::GoogleCloudKms);
    }
}
