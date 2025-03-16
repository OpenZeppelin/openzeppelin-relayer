//! Configuration for HashiCorp Vault Cloud signer
//!
//! This module provides configuration for integrating with HashiCorp Cloud Platform (HCP) Vault,
//! which is the managed service offering of Vault. The configuration handles the OAuth2 client
//! credentials flow required for authenticating with HCP.
//!
//! The configuration supports:
//! - Client ID and Secret for OAuth2 authentication
//! - Organization ID for the HCP account
//! - Project ID within the organization
//! - Application name for identification in logs and metrics
//! - Key name to use for signing operations
//!
//! HCP Vault differs from self-hosted Vault by requiring OAuth-based authentication
//! instead of token or AppRole based authentication methods.
use crate::config::ConfigFileError;
use serde::{Deserialize, Serialize};
use validator::Validate;

use super::{validate_with_validator, SignerConfigValidate};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Validate)]
#[serde(deny_unknown_fields)]
pub struct VaultCloudSignerFileConfig {
    #[validate(length(min = 1, message = "Client ID cannot be empty"))]
    pub client_id: String,
    #[validate(length(min = 1, message = "Client secret cannot be empty"))]
    pub client_secret: String,
    #[validate(length(min = 1, message = "Organization ID cannot be empty"))]
    pub org_id: String,
    #[validate(length(min = 1, message = "Project ID cannot be empty"))]
    pub project_id: String,
    #[validate(length(min = 1, message = "Application name cannot be empty"))]
    pub app_name: String,
    #[validate(length(min = 1, message = "Key name cannot be empty"))]
    pub key_name: String,
}

impl SignerConfigValidate for VaultCloudSignerFileConfig {
    fn validate(&self) -> Result<(), ConfigFileError> {
        validate_with_validator(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vault_cloud_signer_file_config_valid() {
        let config = VaultCloudSignerFileConfig {
            client_id: "client-123".to_string(),
            client_secret: "secret-abc".to_string(),
            org_id: "org-456".to_string(),
            project_id: "proj-789".to_string(),
            app_name: "my-cloud-app".to_string(),
            key_name: "hcp-key".to_string(),
        };

        assert!(Validate::validate(&config).is_ok());
        assert!(SignerConfigValidate::validate(&config).is_ok());
    }

    #[test]
    fn test_vault_cloud_signer_file_config_empty_client_id() {
        let config = VaultCloudSignerFileConfig {
            client_id: "".to_string(),
            client_secret: "secret-abc".to_string(),
            org_id: "org-456".to_string(),
            project_id: "proj-789".to_string(),
            app_name: "my-cloud-app".to_string(),
            key_name: "hcp-key".to_string(),
        };

        let result = SignerConfigValidate::validate(&config);
        assert!(result.is_err());
        if let Err(e) = result {
            let error_message = format!("{:?}", e);
            assert!(error_message.contains("client_id"));
            assert!(error_message.contains("cannot be empty"));
        }
    }

    #[test]
    fn test_vault_cloud_signer_file_config_empty_client_secret() {
        let config = VaultCloudSignerFileConfig {
            client_id: "client-123".to_string(),
            client_secret: "".to_string(),
            org_id: "org-456".to_string(),
            project_id: "proj-789".to_string(),
            app_name: "my-cloud-app".to_string(),
            key_name: "hcp-key".to_string(),
        };

        let result = SignerConfigValidate::validate(&config);
        assert!(result.is_err());
        if let Err(e) = result {
            let error_message = format!("{:?}", e);
            assert!(error_message.contains("client_secret"));
            assert!(error_message.contains("cannot be empty"));
        }
    }

    #[test]
    fn test_vault_cloud_signer_file_config_empty_org_id() {
        let config = VaultCloudSignerFileConfig {
            client_id: "client-123".to_string(),
            client_secret: "secret-abc".to_string(),
            org_id: "".to_string(),
            project_id: "proj-789".to_string(),
            app_name: "my-cloud-app".to_string(),
            key_name: "hcp-key".to_string(),
        };

        let result = SignerConfigValidate::validate(&config);
        assert!(result.is_err());
        if let Err(e) = result {
            let error_message = format!("{:?}", e);
            assert!(error_message.contains("org_id"));
            assert!(error_message.contains("cannot be empty"));
        }
    }

    #[test]
    fn test_vault_cloud_signer_file_config_empty_project_id() {
        let config = VaultCloudSignerFileConfig {
            client_id: "client-123".to_string(),
            client_secret: "secret-abc".to_string(),
            org_id: "org-456".to_string(),
            project_id: "".to_string(),
            app_name: "my-cloud-app".to_string(),
            key_name: "hcp-key".to_string(),
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
    fn test_vault_cloud_signer_file_config_empty_app_name() {
        let config = VaultCloudSignerFileConfig {
            client_id: "client-123".to_string(),
            client_secret: "secret-abc".to_string(),
            org_id: "org-456".to_string(),
            project_id: "proj-789".to_string(),
            app_name: "".to_string(),
            key_name: "hcp-key".to_string(),
        };

        let result = SignerConfigValidate::validate(&config);
        assert!(result.is_err());
        if let Err(e) = result {
            let error_message = format!("{:?}", e);
            assert!(error_message.contains("app_name"));
            assert!(error_message.contains("cannot be empty"));
        }
    }

    #[test]
    fn test_vault_cloud_signer_file_config_empty_key_name() {
        let config = VaultCloudSignerFileConfig {
            client_id: "client-123".to_string(),
            client_secret: "secret-abc".to_string(),
            org_id: "org-456".to_string(),
            project_id: "proj-789".to_string(),
            app_name: "my-cloud-app".to_string(),
            key_name: "".to_string(),
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
    fn test_vault_cloud_signer_file_config_multiple_errors() {
        // Config with multiple validation errors
        let config = VaultCloudSignerFileConfig {
            client_id: "".to_string(),
            client_secret: "".to_string(),
            org_id: "".to_string(),
            project_id: "".to_string(),
            app_name: "".to_string(),
            key_name: "".to_string(),
        };

        let result = validate_with_validator(&config);
        assert!(result.is_err());

        if let Err(e) = result {
            if let ConfigFileError::InvalidFormat(msg) = e {
                assert!(msg.contains("client_id"));
                assert!(msg.contains("client_secret"));
                assert!(msg.contains("org_id"));
                assert!(msg.contains("project_id"));
                assert!(msg.contains("app_name"));
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
            "client_id": "client-123",
            "client_secret": "secret-abc",
            "org_id": "org-456",
            "project_id": "proj-789",
            "app_name": "my-cloud-app",
            "key_name": "hcp-key"
        }
        "#;

        let config: VaultCloudSignerFileConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.client_id, "client-123");
        assert_eq!(config.client_secret, "secret-abc");
        assert_eq!(config.org_id, "org-456");
        assert_eq!(config.project_id, "proj-789");
        assert_eq!(config.app_name, "my-cloud-app");
        assert_eq!(config.key_name, "hcp-key");
    }

    #[test]
    fn test_serde_unknown_field() {
        let json = r#"
        {
            "client_id": "client-123",
            "client_secret": "secret-abc",
            "org_id": "org-456",
            "project_id": "proj-789",
            "app_name": "my-cloud-app",
            "key_name": "hcp-key",
            "unknown_field": "should cause error"
        }
        "#;

        let result: Result<VaultCloudSignerFileConfig, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_serde_serialize_deserialize() {
        let config = VaultCloudSignerFileConfig {
            client_id: "client-123".to_string(),
            client_secret: "secret-abc".to_string(),
            org_id: "org-456".to_string(),
            project_id: "proj-789".to_string(),
            app_name: "my-cloud-app".to_string(),
            key_name: "hcp-key".to_string(),
        };

        let serialized = serde_json::to_string(&config).unwrap();
        let deserialized: VaultCloudSignerFileConfig = serde_json::from_str(&serialized).unwrap();

        assert_eq!(config, deserialized);
    }

    #[test]
    fn test_serde_pretty_json() {
        let json = r#"{
        "client_id": "client-123",
        "client_secret": "secret-abc",
        "org_id": "org-456",
        "project_id": "proj-789",
        "app_name": "my-cloud-app",
        "key_name": "hcp-key"
        }"#;

        let config: VaultCloudSignerFileConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.client_id, "client-123");
        assert_eq!(config.client_secret, "secret-abc");
    }

    #[test]
    fn test_validate_with_validator() {
        let valid_config = VaultCloudSignerFileConfig {
            client_id: "client-123".to_string(),
            client_secret: "secret-abc".to_string(),
            org_id: "org-456".to_string(),
            project_id: "proj-789".to_string(),
            app_name: "my-cloud-app".to_string(),
            key_name: "hcp-key".to_string(),
        };

        let invalid_config = VaultCloudSignerFileConfig {
            client_id: "".to_string(),
            client_secret: "secret-abc".to_string(),
            org_id: "org-456".to_string(),
            project_id: "proj-789".to_string(),
            app_name: "my-cloud-app".to_string(),
            key_name: "hcp-key".to_string(),
        };

        assert!(Validate::validate(&valid_config).is_ok());
        assert!(Validate::validate(&invalid_config).is_err());
    }
}
