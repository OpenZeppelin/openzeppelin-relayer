use secrets::SecretVec;
use serde::{Deserialize, Serialize, Serializer};

use crate::models::SecretString;

fn serialize_secretvec<S>(_secret: &SecretVec<u8>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str("[REDACTED]")
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SignerType {
    Test,
    Local,
    #[serde(rename = "aws_kms")]
    AwsKms,
    Vault,
    Turnkey,
}

#[derive(Debug, Clone, Serialize)]
pub struct SignerRepoModel {
    pub id: String,
    pub config: SignerConfig,
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalSignerConfig {
    #[serde(serialize_with = "serialize_secretvec")]
    pub raw_key: SecretVec<u8>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AwsKmsSignerConfig {
    pub region: Option<String>,
    pub key_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct VaultTransitSignerConfig {
    pub key_name: String,
    pub address: String,
    pub namespace: Option<String>,
    pub role_id: SecretString,
    pub secret_id: SecretString,
    pub pubkey: String,
    pub mount_point: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TurnkeySignerConfig {
    pub api_public_key: String,
    pub api_private_key: SecretString,
    pub organization_id: String,
    pub private_key_id: String,
    pub public_key: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct GoogleCloudKmsSignerServiceAccountConfig {
    pub private_key: SecretString,
    pub private_key_id: SecretString,
    pub project_id: String,
    pub client_email: SecretString,
    pub client_id: String,
    pub auth_uri: String,
    pub token_uri: String,
    pub auth_provider_x509_cert_url: String,
    pub client_x509_cert_url: String,
    pub universe_domain: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct GoogleCloudKmsSignerKeyConfig {
    pub key_ring_id: String,
    pub key_id: String,
    pub key_version: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct GoogleCloudKmsSignerConfig {
    pub service_account: GoogleCloudKmsSignerServiceAccountConfig,
    pub key: GoogleCloudKmsSignerKeyConfig,
}

#[derive(Debug, Clone, Serialize)]
pub enum SignerConfig {
    Test(LocalSignerConfig),
    Local(LocalSignerConfig),
    Vault(LocalSignerConfig),
    VaultCloud(LocalSignerConfig),
    VaultTransit(VaultTransitSignerConfig),
    AwsKms(AwsKmsSignerConfig),
    Turnkey(TurnkeySignerConfig),
    GoogleCloudKms(GoogleCloudKmsSignerConfig),
}

impl SignerConfig {
    pub fn get_local(&self) -> Option<&LocalSignerConfig> {
        match self {
            Self::Local(config)
            | Self::Test(config)
            | Self::Vault(config)
            | Self::VaultCloud(config) => Some(config),
            Self::VaultTransit(_)
            | Self::AwsKms(_)
            | Self::Turnkey(_)
            | Self::GoogleCloudKms(_) => None,
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

    pub fn get_turnkey(&self) -> Option<&TurnkeySignerConfig> {
        let SignerConfig::Turnkey(config) = self else {
            return None;
        };

        Some(config)
    }

    pub fn get_google_cloud_kms(&self) -> Option<&GoogleCloudKmsSignerConfig> {
        let SignerConfig::GoogleCloudKms(config) = self else {
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
        assert_eq!(to_string(&SignerType::AwsKms).unwrap(), "\"aws_kms\"");
        assert_eq!(to_string(&SignerType::Vault).unwrap(), "\"vault\"");
        assert_eq!(to_string(&SignerType::Turnkey).unwrap(), "\"turnkey\"");
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
            from_str::<SignerType>("\"aws_kms\"").unwrap(),
            SignerType::AwsKms
        );
        assert_eq!(
            from_str::<SignerType>("\"vault\"").unwrap(),
            SignerType::Vault
        );
        assert_eq!(
            from_str::<SignerType>("\"turnkey\"").unwrap(),
            SignerType::Turnkey
        );
    }

    #[test]
    fn test_signer_repo_model_creation() {
        let model = SignerRepoModel {
            id: "test-signer".to_string(),
            config: SignerConfig::Test(LocalSignerConfig {
                raw_key: SecretVec::new(4, |v| v.copy_from_slice(&[1, 2, 3, 4])),
            }),
        };

        assert_eq!(model.id, "test-signer");
        assert!(matches!(model.config, SignerConfig::Test(_)));
    }

    #[test]
    fn test_local_signer_config() {
        let private_key = vec![0, 1, 2, 3, 4, 5];
        let config = LocalSignerConfig {
            raw_key: SecretVec::new(private_key.len(), |v| v.copy_from_slice(&private_key)),
        };

        let test = config.raw_key.borrow();
        assert_eq!(*test, private_key);
    }

    #[test]
    fn test_vault_transit_signer_config() {
        let config = VaultTransitSignerConfig {
            key_name: "transit-key".to_string(),
            address: "https://vault.example.com".to_string(),
            namespace: Some("ns1".to_string()),
            role_id: SecretString::new("role-123"),
            secret_id: SecretString::new("secret-456"),
            pubkey: "mypubkey123".to_string(),
            mount_point: Some("transit".to_string()),
        };

        assert_eq!(config.key_name, "transit-key");
        assert_eq!(config.address, "https://vault.example.com");
        assert_eq!(config.namespace, Some("ns1".to_string()));
        assert_eq!(config.role_id.to_str().as_str(), "role-123");
        assert_eq!(config.secret_id.to_str().as_str(), "secret-456");
        assert_eq!(config.pubkey, "mypubkey123");
        assert_eq!(config.mount_point, Some("transit".to_string()));

        let config2 = VaultTransitSignerConfig {
            key_name: "transit-key".to_string(),
            address: "https://vault.example.com".to_string(),
            namespace: None,
            role_id: SecretString::new("role-123"),
            secret_id: SecretString::new("secret-456"),
            pubkey: "mypubkey123".to_string(),
            mount_point: None,
        };

        assert_eq!(config2.namespace, None);
        assert_eq!(config2.mount_point, None);
    }

    #[test]
    fn test_turnkey_signer_config() {
        let config = TurnkeySignerConfig {
            api_private_key: SecretString::new("123"),
            api_public_key: "api_public_key".to_string(),
            organization_id: "organization_id".to_string(),
            private_key_id: "private_key_id".to_string(),
            public_key: "public_key".to_string(),
        };

        assert_eq!(config.api_public_key, "api_public_key");
        assert_eq!(config.organization_id, "organization_id");
        assert_eq!(config.api_private_key.to_str().as_str(), "123");
        assert_eq!(config.private_key_id, "private_key_id");
        assert_eq!(config.public_key, "public_key");
    }

    #[test]
    fn test_google_cloud_kms_config() {
        let config = GoogleCloudKmsSignerConfig {
            service_account: GoogleCloudKmsSignerServiceAccountConfig {
                private_key: SecretString::new("private_key"),
                private_key_id: SecretString::new("private_key_id"),
                project_id: "project_id".to_string(),
                client_email: SecretString::new("client_email"),
                client_id: "client_id".to_string(),
                auth_uri: "auth_uri".to_string(),
                token_uri: "token_uri".to_string(),
                auth_provider_x509_cert_url: "auth_provider_x509_cert_url".to_string(),
                client_x509_cert_url: "client_x509_cert_url".to_string(),
                universe_domain: "universe_domain".to_string(),
            },
            key: GoogleCloudKmsSignerKeyConfig {
                key_ring_id: "key_ring_id".to_string(),
                key_id: "key_id".to_string(),
                key_version: 1,
            },
        };

        assert_eq!(config.service_account.project_id, "project_id");
        assert_eq!(config.key.key_ring_id, "key_ring_id");
        assert_eq!(config.key.key_id, "key_id");
        assert_eq!(config.key.key_version, 1);
        assert_eq!(
            config.service_account.private_key.to_str().as_str(),
            "private_key"
        );
        assert_eq!(
            config.service_account.private_key_id.to_str().as_str(),
            "private_key_id"
        );
        assert_eq!(
            config.service_account.client_email.to_str().as_str(),
            "client_email"
        );
        assert_eq!(config.service_account.client_id, "client_id");
        assert_eq!(config.service_account.auth_uri, "auth_uri");
        assert_eq!(config.service_account.token_uri, "token_uri");
        assert_eq!(
            config.service_account.auth_provider_x509_cert_url,
            "auth_provider_x509_cert_url"
        );
        assert_eq!(
            config.service_account.client_x509_cert_url,
            "client_x509_cert_url"
        );
        assert_eq!(config.service_account.universe_domain, "universe_domain");
    }

    #[test]
    fn test_signer_config_variants() {
        let test_config = SignerConfig::Test(LocalSignerConfig {
            raw_key: SecretVec::new(3, |v| v.copy_from_slice(&[1, 2, 3])),
        });

        let local_config = SignerConfig::Local(LocalSignerConfig {
            raw_key: SecretVec::new(3, |v| v.copy_from_slice(&[4, 5, 6])),
        });

        let vault_config = SignerConfig::Vault(LocalSignerConfig {
            raw_key: SecretVec::new(3, |v| v.copy_from_slice(&[7, 8, 9])),
        });

        let vault_cloud_config = SignerConfig::VaultCloud(LocalSignerConfig {
            raw_key: SecretVec::new(3, |v| v.copy_from_slice(&[10, 11, 12])),
        });

        let vault_transit_config = SignerConfig::VaultTransit(VaultTransitSignerConfig {
            key_name: "transit-key".to_string(),
            address: "https://vault.example.com".to_string(),
            namespace: None,
            role_id: SecretString::new("role-123"),
            secret_id: SecretString::new("secret-456"),
            pubkey: "mypubkey123".to_string(),
            mount_point: None,
        });

        let aws_kms_config = SignerConfig::AwsKms(AwsKmsSignerConfig {
            region: Some("us-east-1".to_string()),
            key_id: "test-key-id".to_string(),
        });

        let turnkey_config = SignerConfig::Turnkey(TurnkeySignerConfig {
            api_private_key: SecretString::new("123"),
            api_public_key: "api_public_key".to_string(),
            organization_id: "organization_id".to_string(),
            private_key_id: "private_key_id".to_string(),
            public_key: "public_key".to_string(),
        });

        let google_cloud_kms_config = SignerConfig::GoogleCloudKms(GoogleCloudKmsSignerConfig {
            service_account: GoogleCloudKmsSignerServiceAccountConfig {
                private_key: SecretString::new("private_key"),
                private_key_id: SecretString::new("private_key_id"),
                project_id: "project_id".to_string(),
                client_email: SecretString::new("client_email"),
                client_id: "client_id".to_string(),
                auth_uri: "auth_uri".to_string(),
                token_uri: "token_uri".to_string(),
                auth_provider_x509_cert_url: "auth_provider_x509_cert_url".to_string(),
                client_x509_cert_url: "client_x509_cert_url".to_string(),
                universe_domain: "universe_domain".to_string(),
            },
            key: GoogleCloudKmsSignerKeyConfig {
                key_ring_id: "key_ring_id".to_string(),
                key_id: "key_id".to_string(),
                key_version: 1,
            },
        });

        assert!(matches!(test_config, SignerConfig::Test(_)));
        assert!(matches!(local_config, SignerConfig::Local(_)));
        assert!(matches!(vault_config, SignerConfig::Vault(_)));
        assert!(matches!(vault_cloud_config, SignerConfig::VaultCloud(_)));
        assert!(matches!(
            vault_transit_config,
            SignerConfig::VaultTransit(_)
        ));
        assert!(matches!(aws_kms_config, SignerConfig::AwsKms(_)));
        assert!(matches!(turnkey_config, SignerConfig::Turnkey(_)));
        assert!(matches!(
            google_cloud_kms_config,
            SignerConfig::GoogleCloudKms(_)
        ));
    }

    #[test]
    fn test_signer_config_get_local() {
        let local_config = SignerConfig::Local(LocalSignerConfig {
            raw_key: SecretVec::new(3, |v| v.copy_from_slice(&[1, 2, 3])),
        });
        let retrieved = local_config.get_local().unwrap();
        assert_eq!(*retrieved.raw_key.borrow(), vec![1, 2, 3]);

        let test_config = SignerConfig::Test(LocalSignerConfig {
            raw_key: SecretVec::new(3, |v| v.copy_from_slice(&[4, 5, 6])),
        });
        let retrieved = test_config.get_local().unwrap();
        assert_eq!(*retrieved.raw_key.borrow(), vec![4, 5, 6]);

        let vault_config = SignerConfig::Vault(LocalSignerConfig {
            raw_key: SecretVec::new(3, |v| v.copy_from_slice(&[7, 8, 9])),
        });
        let retrieved = vault_config.get_local().unwrap();
        assert_eq!(*retrieved.raw_key.borrow(), vec![7, 8, 9]);

        let vault_cloud_config = SignerConfig::VaultCloud(LocalSignerConfig {
            raw_key: SecretVec::new(3, |v| v.copy_from_slice(&[10, 11, 12])),
        });
        let retrieved = vault_cloud_config.get_local().unwrap();
        assert_eq!(*retrieved.raw_key.borrow(), vec![10, 11, 12]);

        let vault_transit_config = SignerConfig::VaultTransit(VaultTransitSignerConfig {
            key_name: "transit-key".to_string(),
            address: "https://vault.example.com".to_string(),
            namespace: None,
            role_id: SecretString::new("role-123"),
            secret_id: SecretString::new("secret-456"),
            pubkey: "mypubkey123".to_string(),
            mount_point: None,
        });
        assert!(vault_transit_config.get_local().is_none());

        let google_cloud_kms_config = SignerConfig::GoogleCloudKms(GoogleCloudKmsSignerConfig {
            service_account: GoogleCloudKmsSignerServiceAccountConfig {
                private_key: SecretString::new("private_key"),
                private_key_id: SecretString::new("private_key_id"),
                project_id: "project_id".to_string(),
                client_email: SecretString::new("client_email"),
                client_id: "client_id".to_string(),
                auth_uri: "auth_uri".to_string(),
                token_uri: "token_uri".to_string(),
                auth_provider_x509_cert_url: "auth_provider_x509_cert_url".to_string(),
                client_x509_cert_url: "client_x509_cert_url".to_string(),
                universe_domain: "universe_domain".to_string(),
            },
            key: GoogleCloudKmsSignerKeyConfig {
                key_ring_id: "key_ring_id".to_string(),
                key_id: "key_id".to_string(),
                key_version: 1,
            },
        });
        assert!(google_cloud_kms_config.get_local().is_none());

        let aws_kms_config = SignerConfig::AwsKms(AwsKmsSignerConfig {
            region: Some("us-east-1".to_string()),
            key_id: "test-key-id".to_string(),
        });
        assert!(aws_kms_config.get_local().is_none());

        let turnkey_config = SignerConfig::Turnkey(TurnkeySignerConfig {
            api_private_key: SecretString::new("123"),
            api_public_key: "api_public_key".to_string(),
            organization_id: "organization_id".to_string(),
            private_key_id: "private_key_id".to_string(),
            public_key: "public_key".to_string(),
        });
        assert!(turnkey_config.get_local().is_none());
    }

    #[test]
    fn test_signer_config_get_aws_kms() {
        let aws_kms_config = SignerConfig::AwsKms(AwsKmsSignerConfig {
            region: Some("us-east-1".to_string()),
            key_id: "test-key-id".to_string(),
        });
        assert!(aws_kms_config.get_aws_kms().is_some());

        // Test with configs that should return None
        let local_config = SignerConfig::Local(LocalSignerConfig {
            raw_key: SecretVec::new(3, |v| v.copy_from_slice(&[1, 2, 3])),
        });
        assert!(local_config.get_aws_kms().is_none());

        let test_config = SignerConfig::Test(LocalSignerConfig {
            raw_key: SecretVec::new(3, |v| v.copy_from_slice(&[4, 5, 6])),
        });
        assert!(test_config.get_aws_kms().is_none());
    }

    #[test]
    fn test_signer_config_get_vault_transit() {
        let vault_transit_config = SignerConfig::VaultTransit(VaultTransitSignerConfig {
            key_name: "transit-key".to_string(),
            address: "https://vault.example.com".to_string(),
            namespace: None,
            role_id: SecretString::new("role-123"),
            secret_id: SecretString::new("secret-456"),
            pubkey: "mypubkey123".to_string(),
            mount_point: None,
        });
        let retrieved = vault_transit_config.get_vault_transit().unwrap();
        assert_eq!(retrieved.key_name, "transit-key");
        assert_eq!(retrieved.address, "https://vault.example.com");

        let local_config = SignerConfig::Local(LocalSignerConfig {
            raw_key: SecretVec::new(3, |v| v.copy_from_slice(&[1, 2, 3])),
        });
        assert!(local_config.get_vault_transit().is_none());

        let vault_config = SignerConfig::Vault(LocalSignerConfig {
            raw_key: SecretVec::new(3, |v| v.copy_from_slice(&[7, 8, 9])),
        });
        assert!(vault_config.get_vault_transit().is_none());
    }

    #[test]
    fn test_signer_config_get_turnkey() {
        let turnkey_config = SignerConfig::Turnkey(TurnkeySignerConfig {
            api_private_key: SecretString::new("123"),
            api_public_key: "api_public_key".to_string(),
            organization_id: "organization_id".to_string(),
            private_key_id: "private_key_id".to_string(),
            public_key: "public_key".to_string(),
        });

        let retrieved = turnkey_config.get_turnkey().unwrap();

        assert_eq!(retrieved.api_public_key, "api_public_key");
        assert_eq!(retrieved.organization_id, "organization_id");
        assert_eq!(retrieved.api_private_key.to_str().as_str(), "123");
        assert_eq!(retrieved.private_key_id, "private_key_id");
        assert_eq!(retrieved.public_key, "public_key");
        assert!(turnkey_config.get_aws_kms().is_none());
        assert!(turnkey_config.get_local().is_none());
        assert!(turnkey_config.get_vault_transit().is_none());
    }

    #[test]
    fn test_signer_config_get_google_cloud_kms() {
        let google_config = SignerConfig::GoogleCloudKms(GoogleCloudKmsSignerConfig {
            service_account: GoogleCloudKmsSignerServiceAccountConfig {
                private_key: SecretString::new("private_key"),
                private_key_id: SecretString::new("private_key_id"),
                project_id: "project_id".to_string(),
                client_email: SecretString::new("client_email"),
                client_id: "client_id".to_string(),
                auth_uri: "auth_uri".to_string(),
                token_uri: "token_uri".to_string(),
                auth_provider_x509_cert_url: "auth_provider_x509_cert_url".to_string(),
                client_x509_cert_url: "client_x509_cert_url".to_string(),
                universe_domain: "universe_domain".to_string(),
            },
            key: GoogleCloudKmsSignerKeyConfig {
                key_ring_id: "key_ring_id".to_string(),
                key_id: "key_id".to_string(),
                key_version: 1,
            },
        });
        let retrieved = google_config.get_google_cloud_kms().unwrap();
        assert_eq!(retrieved.service_account.project_id, "project_id");
        assert!(google_config.get_aws_kms().is_none());
        assert!(google_config.get_local().is_none());
        assert!(google_config.get_vault_transit().is_none());
    }
}
