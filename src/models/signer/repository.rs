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
    pub token: String,
    pub pubkey: String,
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
            SignerConfig::Local(config) => Some(config),
            SignerConfig::Test(config) => Some(config),
            SignerConfig::Vault(config) => Some(config),
            SignerConfig::VaultCloud(config) => Some(config),
            _ => None,
        }
    }

    pub fn get_aws_kms(&self) -> Option<&AwsKmsSignerConfig> {
        match self {
            SignerConfig::AwsKms(config) => Some(config),
            _ => None,
        }
    }

    pub fn get_vault_transit(&self) -> Option<&VaultTransitSignerConfig> {
        match self {
            SignerConfig::VaultTransit(config) => Some(config),
            _ => None,
        }
    }
}
