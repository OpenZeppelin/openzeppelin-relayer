//! Configuration file definitions for signer services.
//!
//! Provides configuration structures and validation for different signer types:
//! - Test (temporary private keys)
//! - Local keystore (encrypted JSON files)
//! - HashiCorp Vault integration
//! - AWS KMS integration [NOT IMPLEMENTED]
use super::ConfigFileError;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use validator::Validate;

mod local;
pub use local::*;

mod vault;
pub use vault::*;

mod vault_cloud;
pub use vault_cloud::*;

mod vault_transit;
pub use vault_transit::*;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum PlainOrEnvConfigValue {
    Env { name: String },
    Plain { value: String },
}

impl PlainOrEnvConfigValue {
    pub fn get_value(&self) -> Result<String, ConfigFileError> {
        match self {
            PlainOrEnvConfigValue::Env { name } => {
                let value = std::env::var(name).map_err(|_| {
                    ConfigFileError::MissingEnvVar(format!(
                        "Environment variable {} not found",
                        name
                    ))
                })?;
                Ok(value)
            }
            PlainOrEnvConfigValue::Plain { value } => Ok(value.clone()),
        }
    }
}

pub trait SignerConfigValidate {
    fn validate(&self) -> Result<(), ConfigFileError>;
}

pub trait ValidatableSignerConfig: SignerConfigValidate + Validate {
    // Default implementation that uses validator::Validate
    fn validate_with_validator(&self) -> Result<(), ConfigFileError> {
        match Validate::validate(self) {
            Ok(_) => Ok(()),
            Err(errors) => {
                // Convert validator::ValidationErrors to your ConfigFileError
                let error_message = errors
                    .field_errors()
                    .iter()
                    .map(|(field, errors)| {
                        let messages: Vec<String> = errors
                            .iter()
                            .map(|error| error.message.clone().unwrap_or_default().to_string())
                            .collect();
                        format!("{}: {}", field, messages.join(", "))
                    })
                    .collect::<Vec<String>>()
                    .join("; ");

                Err(ConfigFileError::InvalidFormat(error_message))
            }
        }
    }
}

impl<T> ValidatableSignerConfig for T where T: SignerConfigValidate + Validate {}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TestSignerFileConfig {}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AwsKmsSignerFileConfig {}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase", content = "config")]
pub enum SignerConfig {
    Test(TestSignerFileConfig),
    Local(LocalSignerFileConfig),
    AwsKms(AwsKmsSignerFileConfig),
    Vault(VaultSignerFileConfig),
    #[serde(rename = "vault_cloud")]
    VaultCloud(VaultCloudSignerFileConfig),
    #[serde(rename = "vault_transit")]
    VaultTransit(VaultTransitSignerFileConfig),
}

impl SignerConfig {
    pub fn get_local(&self) -> Option<&LocalSignerFileConfig> {
        match self {
            SignerConfig::Local(local) => Some(local),
            _ => None,
        }
    }

    pub fn get_vault(&self) -> Option<&VaultSignerFileConfig> {
        match self {
            SignerConfig::Vault(vault) => Some(vault),
            _ => None,
        }
    }

    pub fn get_vault_cloud(&self) -> Option<&VaultCloudSignerFileConfig> {
        match self {
            SignerConfig::VaultCloud(vault_cloud) => Some(vault_cloud),
            _ => None,
        }
    }

    pub fn get_vault_transit(&self) -> Option<&VaultTransitSignerFileConfig> {
        match self {
            SignerConfig::VaultTransit(vault_transit) => Some(vault_transit),
            _ => None,
        }
    }

    pub fn get_test(&self) -> Option<&TestSignerFileConfig> {
        match self {
            SignerConfig::Test(test) => Some(test),
            _ => None,
        }
    }

    pub fn get_aws_kms(&self) -> Option<&AwsKmsSignerFileConfig> {
        match self {
            SignerConfig::AwsKms(aws_kms) => Some(aws_kms),
            _ => None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct SignerFileConfig {
    pub id: String,
    #[serde(flatten)]
    pub config: SignerConfig,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SignerFileConfigPassphrase {
    Env { name: String },
    Plain { value: String },
}

impl SignerFileConfig {
    pub fn validate_signer(&self) -> Result<(), ConfigFileError> {
        if self.id.is_empty() {
            return Err(ConfigFileError::InvalidIdLength(
                "Signer ID cannot be empty".into(),
            ));
        }

        match &self.config {
            SignerConfig::Test(_) => Ok(()),
            SignerConfig::Local(local_config) => local_config.validate(),
            SignerConfig::AwsKms(_) => {
                Err(ConfigFileError::InternalError("Not implemented".into()))
            }
            SignerConfig::Vault(vault_config) => SignerConfigValidate::validate(vault_config),
            SignerConfig::VaultCloud(vault_cloud_config) => {
                SignerConfigValidate::validate(vault_cloud_config)
            }
            SignerConfig::VaultTransit(vault_transit_config) => {
                SignerConfigValidate::validate(vault_transit_config)
            }
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct SignersFileConfig {
    pub signers: Vec<SignerFileConfig>,
}

impl SignersFileConfig {
    pub fn new(signers: Vec<SignerFileConfig>) -> Self {
        Self { signers }
    }

    pub fn validate(&self) -> Result<(), ConfigFileError> {
        if self.signers.is_empty() {
            return Err(ConfigFileError::MissingField("signers".into()));
        }

        let mut ids = HashSet::new();
        for signer in &self.signers {
            signer.validate_signer()?;
            if !ids.insert(signer.id.clone()) {
                return Err(ConfigFileError::DuplicateId(signer.id.clone()));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::env;

    #[test]
    fn test_plain_or_env_config_value_plain() {
        let plain = PlainOrEnvConfigValue::Plain {
            value: "test-value".to_string(),
        };

        assert_eq!(plain.get_value().unwrap(), "test-value");
    }

    #[test]
    fn test_plain_or_env_config_value_env_exists() {
        env::set_var("TEST_ENV_VAR", "env-test-value");

        let env_value = PlainOrEnvConfigValue::Env {
            name: "TEST_ENV_VAR".to_string(),
        };

        assert_eq!(env_value.get_value().unwrap(), "env-test-value");
        env::remove_var("TEST_ENV_VAR");
    }

    #[test]
    fn test_plain_or_env_config_value_env_missing() {
        env::remove_var("NONEXISTENT_TEST_VAR");

        let env_value = PlainOrEnvConfigValue::Env {
            name: "NONEXISTENT_TEST_VAR".to_string(),
        };

        let result = env_value.get_value();
        assert!(result.is_err());
        assert!(matches!(result, Err(ConfigFileError::MissingEnvVar(_))));
    }

    #[test]
    fn test_valid_signer_config() {
        let config = json!({
            "id": "local-signer",
            "type": "local",
            "config": {
                "path": "examples/basic-example/config/keys/local-signer.json",
                "passphrase": {
                    "type": "plain",
                    "value": "secret",
                }
            }
        });

        let signer_config: SignerFileConfig = serde_json::from_value(config).unwrap();
        assert!(signer_config.validate_signer().is_ok());
    }

    #[test]
    fn test_valid_signer_config_env() {
        env::set_var("LOCAL_SIGNER_KEY_PASSPHRASE", "mocked_value");

        let config = json!({
            "id": "local-signer",
            "type": "local",
            "config": {
                "path": "examples/basic-example/config/keys/local-signer.json",
                "passphrase": {
                    "type": "env",
                    "name": "LOCAL_SIGNER_KEY_PASSPHRASE"
                }
            }
        });

        let signer_config: SignerFileConfig = serde_json::from_value(config).unwrap();
        assert!(signer_config.validate_signer().is_ok());
        env::remove_var("LOCAL_SIGNER_KEY_PASSPHRASE");
    }

    #[test]
    fn test_duplicate_signer_ids() {
        let config = json!({
            "signers": [
                {
                  "id": "local-signer",
                  "type": "local",
                  "config": {
                      "path": "examples/basic-example/config/keys/local-signer.json",
                      "passphrase": {
                          "type": "plain",
                          "value": "secret",
                      }
                  }
                },
                {
                  "id": "local-signer",
                  "type": "local",
                  "config": {
                      "path": "examples/basic-example/config/keys/local-signer.json",
                      "passphrase": {
                          "type": "plain",
                          "value": "secret",
                      }
                  }
                }
            ]
        });

        let signer_config: SignersFileConfig = serde_json::from_value(config).unwrap();
        assert!(matches!(
            signer_config.validate(),
            Err(ConfigFileError::DuplicateId(_))
        ));
    }

    #[test]
    fn test_empty_signer_id() {
        let config = json!({
            "signers": [
                {
                  "id": "",
                  "type": "local",
                  "config": {
                    "path": "examples/basic-example/config/keys/local-signer.json",
                    "passphrase": {
                        "type": "plain",
                        "value": "secret",
                    }
                }

                }
            ]
        });

        let signer_config: SignersFileConfig = serde_json::from_value(config).unwrap();
        assert!(matches!(
            signer_config.validate(),
            Err(ConfigFileError::InvalidIdLength(_))
        ));
    }

    #[test]
    fn test_validate_test_signer() {
        let config = json!({
            "id": "test-signer",
            "type": "test",
            "config": {}
        });

        let signer_config: SignerFileConfig = serde_json::from_value(config).unwrap();
        assert!(signer_config.validate_signer().is_ok());
    }

    #[test]
    fn test_validate_vault_signer() {
        let config = json!({
            "id": "vault-signer",
            "type": "vault",
            "config": {
                "address": "https://vault.example.com",
                "role_id": "role-123",
                "secret_id": "secret-456",
                "key_name": "test-key"
            }
        });

        let signer_config: SignerFileConfig = serde_json::from_value(config).unwrap();
        assert!(signer_config.validate_signer().is_ok());
    }

    #[test]
    fn test_validate_vault_cloud_signer() {
        let config = json!({
            "id": "vault-cloud-signer",
            "type": "vault_cloud",
            "config": {
                "client_id": "client-123",
                "client_secret": "secret-abc",
                "org_id": "org-456",
                "project_id": "proj-789",
                "app_name": "my-app",
                "key_name": "cloud-key"
            }
        });

        let signer_config: SignerFileConfig = serde_json::from_value(config).unwrap();
        assert!(signer_config.validate_signer().is_ok());
    }

    #[test]
    fn test_validate_vault_transit_signer() {
        let config = json!({
            "id": "vault-transit-signer",
            "type": "vault_transit",
            "config": {
                "key_name": "transit-key",
                "address": "https://vault.example.com",
                "role_id": "role-123",
                "secret_id": "secret-456",
                "pubkey": "test-pubkey"
            }
        });

        let signer_config: SignerFileConfig = serde_json::from_value(config).unwrap();
        assert!(signer_config.validate_signer().is_ok());
    }

    #[test]
    fn test_validate_vault_transit_signer_invalid() {
        let config = json!({
            "id": "vault-transit-signer",
            "type": "vault_transit",
            "config": {
                "key_name": "",
                "address": "https://vault.example.com",
                "role_id": "role-123",
                "secret_id": "secret-456",
                "pubkey": "test-pubkey"
            }
        });

        let signer_config: SignerFileConfig = serde_json::from_value(config).unwrap();
        assert!(signer_config.validate_signer().is_err());
    }

    #[test]
    fn test_empty_signers_array() {
        let config = json!({
            "signers": []
        });

        let signer_config: SignersFileConfig = serde_json::from_value(config).unwrap();
        let result = signer_config.validate();
        assert!(result.is_err());
        assert!(matches!(result, Err(ConfigFileError::MissingField(_))));
    }

    #[test]
    fn test_signers_file_config_new() {
        let signer = SignerFileConfig {
            id: "test-signer".to_string(),
            config: SignerConfig::Test(TestSignerFileConfig {}),
        };

        let config = SignersFileConfig::new(vec![signer.clone()]);
        assert_eq!(config.signers.len(), 1);
        assert_eq!(config.signers[0].id, "test-signer");
        assert!(matches!(config.signers[0].config, SignerConfig::Test(_)));
    }

    #[test]
    fn test_serde_for_enum_variants() {
        let test_config = json!({
            "type": "test",
            "config": {}
        });
        let parsed: SignerConfig = serde_json::from_value(test_config).unwrap();
        assert!(matches!(parsed, SignerConfig::Test(_)));

        let local_config = json!({
            "type": "local",
            "config": {
                "path": "test-path",
                "passphrase": {
                    "type": "plain",
                    "value": "test-passphrase"
                }
            }
        });
        let parsed: SignerConfig = serde_json::from_value(local_config).unwrap();
        assert!(matches!(parsed, SignerConfig::Local(_)));

        let vault_config = json!({
            "type": "vault",
            "config": {
                "address": "https://vault.example.com",
                "role_id": "role-123",
                "secret_id": "secret-456",
                "key_name": "test-key"
            }
        });
        let parsed: SignerConfig = serde_json::from_value(vault_config).unwrap();
        assert!(matches!(parsed, SignerConfig::Vault(_)));

        let vault_cloud_config = json!({
            "type": "vault_cloud",
            "config": {
                "client_id": "client-123",
                "client_secret": "secret-abc",
                "org_id": "org-456",
                "project_id": "proj-789",
                "app_name": "my-app",
                "key_name": "cloud-key"
            }
        });
        let parsed: SignerConfig = serde_json::from_value(vault_cloud_config).unwrap();
        assert!(matches!(parsed, SignerConfig::VaultCloud(_)));

        let vault_transit_config = json!({
            "type": "vault_transit",
            "config": {
                "key_name": "transit-key",
                "address": "https://vault.example.com",
                "role_id": "role-123",
                "secret_id": "secret-456",
                "pubkey": "test-pubkey"
            }
        });
        let parsed: SignerConfig = serde_json::from_value(vault_transit_config).unwrap();
        assert!(matches!(parsed, SignerConfig::VaultTransit(_)));

        let aws_kms_config = json!({
            "type": "awskms",
            "config": {}
        });
        let parsed: SignerConfig = serde_json::from_value(aws_kms_config).unwrap();
        assert!(matches!(parsed, SignerConfig::AwsKms(_)));
    }

    #[test]
    fn test_get_methods_for_signer_config() {
        let test_config = SignerConfig::Test(TestSignerFileConfig {});
        assert!(test_config.get_test().is_some());
        assert!(test_config.get_local().is_none());
        assert!(test_config.get_vault().is_none());
        assert!(test_config.get_vault_cloud().is_none());
        assert!(test_config.get_vault_transit().is_none());
        assert!(test_config.get_aws_kms().is_none());

        let local_config = SignerConfig::Local(LocalSignerFileConfig {
            path: "test-path".to_string(),
            passphrase: PlainOrEnvConfigValue::Plain {
                value: "test-passphrase".to_string(),
            },
        });
        assert!(local_config.get_test().is_none());
        assert!(local_config.get_local().is_some());
        assert!(local_config.get_vault().is_none());
        assert!(local_config.get_vault_cloud().is_none());
        assert!(local_config.get_vault_transit().is_none());
        assert!(local_config.get_aws_kms().is_none());

        let vault_config = SignerConfig::Vault(VaultSignerFileConfig {
            address: "https://vault.example.com".to_string(),
            namespace: None,
            role_id: "role-123".to_string(),
            secret_id: "secret-456".to_string(),
            key_name: "test-key".to_string(),
            mount_point: None,
        });
        assert!(vault_config.get_test().is_none());
        assert!(vault_config.get_local().is_none());
        assert!(vault_config.get_vault().is_some());
        assert!(vault_config.get_vault_cloud().is_none());
        assert!(vault_config.get_vault_transit().is_none());
        assert!(vault_config.get_aws_kms().is_none());

        let vault_cloud_config = SignerConfig::VaultCloud(VaultCloudSignerFileConfig {
            client_id: "client-123".to_string(),
            client_secret: "secret-abc".to_string(),
            org_id: "org-456".to_string(),
            project_id: "proj-789".to_string(),
            app_name: "my-app".to_string(),
            key_name: "cloud-key".to_string(),
        });
        assert!(vault_cloud_config.get_test().is_none());
        assert!(vault_cloud_config.get_local().is_none());
        assert!(vault_cloud_config.get_vault().is_none());
        assert!(vault_cloud_config.get_vault_cloud().is_some());
        assert!(vault_cloud_config.get_vault_transit().is_none());
        assert!(vault_cloud_config.get_aws_kms().is_none());

        let vault_transit_config = SignerConfig::VaultTransit(VaultTransitSignerFileConfig {
            key_name: "transit-key".to_string(),
            address: "https://vault.example.com".to_string(),
            role_id: "role-123".to_string(),
            secret_id: "secret-456".to_string(),
            pubkey: "test-pubkey".to_string(),
            mount_point: None,
            namespace: None,
        });
        assert!(vault_transit_config.get_test().is_none());
        assert!(vault_transit_config.get_local().is_none());
        assert!(vault_transit_config.get_vault().is_none());
        assert!(vault_transit_config.get_vault_cloud().is_none());
        assert!(vault_transit_config.get_vault_transit().is_some());
        assert!(vault_transit_config.get_aws_kms().is_none());

        let aws_kms_config = SignerConfig::AwsKms(AwsKmsSignerFileConfig {});
        assert!(aws_kms_config.get_test().is_none());
        assert!(aws_kms_config.get_local().is_none());
        assert!(aws_kms_config.get_vault().is_none());
        assert!(aws_kms_config.get_vault_cloud().is_none());
        assert!(aws_kms_config.get_vault_transit().is_none());
        assert!(aws_kms_config.get_aws_kms().is_some());
    }
}
