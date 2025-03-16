use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SignerType {
    Test,
    Local,
    AwsKms,
    Vault,
}

#[derive(Debug, Clone, Serialize)]
pub struct SignerRepoModel {
    pub id: String,
    pub config: SignerConfig,
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalSignerConfig {
    pub raw_key: Vec<u8>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AwsKmsSignerConfig {}

#[derive(Debug, Clone, Serialize)]
pub struct VaultTransitSignerConfig {
    pub key_name: String,
    pub address: String,
    pub namespace: Option<String>,
    pub role_id: String,
    pub secret_id: String,
    pub pubkey: String,
    pub mount_point: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub enum SignerConfig {
    Test(LocalSignerConfig),
    Local(LocalSignerConfig),
    Vault(LocalSignerConfig),
    VaultCloud(LocalSignerConfig),
    VaultTransit(VaultTransitSignerConfig),
    AwsKms(AwsKmsSignerConfig),
}

impl SignerConfig {
    pub fn get_local(&self) -> Option<&LocalSignerConfig> {
        match self {
            Self::Local(config)
            | Self::Test(config)
            | Self::Vault(config)
            | Self::VaultCloud(config) => Some(config),
            Self::VaultTransit(_) | Self::AwsKms(_) => None,
        }
    }

    pub fn get_aws_kms(&self) -> Option<&AwsKmsSignerConfig> {
        let SignerConfig::AwsKms(config) = self else {
            return None;
        };

        Some(config)
    }

    pub fn get_vault_transit(&self) -> Option<&VaultTransitSignerConfig> {
        let SignerConfig::VaultTransit(config) = self else {
            return None;
        };

        Some(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{from_str, to_string};

    #[test]
    fn test_signer_type_serialization() {
        assert_eq!(to_string(&SignerType::Test).unwrap(), "\"test\"");
        assert_eq!(to_string(&SignerType::Local).unwrap(), "\"local\"");
        assert_eq!(to_string(&SignerType::AwsKms).unwrap(), "\"awskms\"");
        assert_eq!(to_string(&SignerType::Vault).unwrap(), "\"vault\"");
    }

    #[test]
    fn test_signer_type_deserialization() {
        assert_eq!(
            from_str::<SignerType>("\"test\"").unwrap(),
            SignerType::Test
        );
        assert_eq!(
            from_str::<SignerType>("\"local\"").unwrap(),
            SignerType::Local
        );
        assert_eq!(
            from_str::<SignerType>("\"awskms\"").unwrap(),
            SignerType::AwsKms
        );
        assert_eq!(
            from_str::<SignerType>("\"vault\"").unwrap(),
            SignerType::Vault
        );
    }

    #[test]
    fn test_signer_repo_model_creation() {
        let model = SignerRepoModel {
            id: "test-signer".to_string(),
            config: SignerConfig::Test(LocalSignerConfig {
                raw_key: vec![1, 2, 3, 4],
            }),
        };

        assert_eq!(model.id, "test-signer");
        assert!(matches!(model.config, SignerConfig::Test(_)));
    }

    #[test]
    fn test_local_signer_config() {
        let private_key = vec![0, 1, 2, 3, 4, 5];
        let config = LocalSignerConfig {
            raw_key: private_key.clone(),
        };

        assert_eq!(config.raw_key, private_key);
    }

    #[test]
    fn test_vault_transit_signer_config() {
        let config = VaultTransitSignerConfig {
            key_name: "transit-key".to_string(),
            address: "https://vault.example.com".to_string(),
            namespace: Some("ns1".to_string()),
            role_id: "role-123".to_string(),
            secret_id: "secret-456".to_string(),
            pubkey: "mypubkey123".to_string(),
            mount_point: Some("transit".to_string()),
        };

        assert_eq!(config.key_name, "transit-key");
        assert_eq!(config.address, "https://vault.example.com");
        assert_eq!(config.namespace, Some("ns1".to_string()));
        assert_eq!(config.role_id, "role-123");
        assert_eq!(config.secret_id, "secret-456");
        assert_eq!(config.pubkey, "mypubkey123");
        assert_eq!(config.mount_point, Some("transit".to_string()));

        let config2 = VaultTransitSignerConfig {
            key_name: "transit-key".to_string(),
            address: "https://vault.example.com".to_string(),
            namespace: None,
            role_id: "role-123".to_string(),
            secret_id: "secret-456".to_string(),
            pubkey: "mypubkey123".to_string(),
            mount_point: None,
        };

        assert_eq!(config2.namespace, None);
        assert_eq!(config2.mount_point, None);
    }

    #[test]
    fn test_signer_config_variants() {
        let test_config = SignerConfig::Test(LocalSignerConfig {
            raw_key: vec![1, 2, 3],
        });

        let local_config = SignerConfig::Local(LocalSignerConfig {
            raw_key: vec![4, 5, 6],
        });

        let vault_config = SignerConfig::Vault(LocalSignerConfig {
            raw_key: vec![7, 8, 9],
        });

        let vault_cloud_config = SignerConfig::VaultCloud(LocalSignerConfig {
            raw_key: vec![10, 11, 12],
        });

        let vault_transit_config = SignerConfig::VaultTransit(VaultTransitSignerConfig {
            key_name: "transit-key".to_string(),
            address: "https://vault.example.com".to_string(),
            namespace: None,
            role_id: "role-123".to_string(),
            secret_id: "secret-456".to_string(),
            pubkey: "mypubkey123".to_string(),
            mount_point: None,
        });

        let aws_kms_config = SignerConfig::AwsKms(AwsKmsSignerConfig {});

        assert!(matches!(test_config, SignerConfig::Test(_)));
        assert!(matches!(local_config, SignerConfig::Local(_)));
        assert!(matches!(vault_config, SignerConfig::Vault(_)));
        assert!(matches!(vault_cloud_config, SignerConfig::VaultCloud(_)));
        assert!(matches!(
            vault_transit_config,
            SignerConfig::VaultTransit(_)
        ));
        assert!(matches!(aws_kms_config, SignerConfig::AwsKms(_)));
    }

    #[test]
    fn test_signer_config_get_local() {
        let local_config = SignerConfig::Local(LocalSignerConfig {
            raw_key: vec![1, 2, 3],
        });
        let retrieved = local_config.get_local().unwrap();
        assert_eq!(retrieved.raw_key, vec![1, 2, 3]);

        let test_config = SignerConfig::Test(LocalSignerConfig {
            raw_key: vec![4, 5, 6],
        });
        let retrieved = test_config.get_local().unwrap();
        assert_eq!(retrieved.raw_key, vec![4, 5, 6]);

        let vault_config = SignerConfig::Vault(LocalSignerConfig {
            raw_key: vec![7, 8, 9],
        });
        let retrieved = vault_config.get_local().unwrap();
        assert_eq!(retrieved.raw_key, vec![7, 8, 9]);

        let vault_cloud_config = SignerConfig::VaultCloud(LocalSignerConfig {
            raw_key: vec![10, 11, 12],
        });
        let retrieved = vault_cloud_config.get_local().unwrap();
        assert_eq!(retrieved.raw_key, vec![10, 11, 12]);

        let vault_transit_config = SignerConfig::VaultTransit(VaultTransitSignerConfig {
            key_name: "transit-key".to_string(),
            address: "https://vault.example.com".to_string(),
            namespace: None,
            role_id: "role-123".to_string(),
            secret_id: "secret-456".to_string(),
            pubkey: "mypubkey123".to_string(),
            mount_point: None,
        });
        assert!(vault_transit_config.get_local().is_none());
    }

    #[test]
    fn test_signer_config_get_aws_kms() {
        let aws_kms_config = SignerConfig::AwsKms(AwsKmsSignerConfig {});
        assert!(aws_kms_config.get_aws_kms().is_some());

        // Test with configs that should return None
        let local_config = SignerConfig::Local(LocalSignerConfig {
            raw_key: vec![1, 2, 3],
        });
        assert!(local_config.get_aws_kms().is_none());

        let test_config = SignerConfig::Test(LocalSignerConfig {
            raw_key: vec![4, 5, 6],
        });
        assert!(test_config.get_aws_kms().is_none());
    }

    #[test]
    fn test_signer_config_get_vault_transit() {
        let vault_transit_config = SignerConfig::VaultTransit(VaultTransitSignerConfig {
            key_name: "transit-key".to_string(),
            address: "https://vault.example.com".to_string(),
            namespace: None,
            role_id: "role-123".to_string(),
            secret_id: "secret-456".to_string(),
            pubkey: "mypubkey123".to_string(),
            mount_point: None,
        });
        let retrieved = vault_transit_config.get_vault_transit().unwrap();
        assert_eq!(retrieved.key_name, "transit-key");
        assert_eq!(retrieved.address, "https://vault.example.com");

        let local_config = SignerConfig::Local(LocalSignerConfig {
            raw_key: vec![1, 2, 3],
        });
        assert!(local_config.get_vault_transit().is_none());

        let vault_config = SignerConfig::Vault(LocalSignerConfig {
            raw_key: vec![7, 8, 9],
        });
        assert!(vault_config.get_vault_transit().is_none());
    }
}
