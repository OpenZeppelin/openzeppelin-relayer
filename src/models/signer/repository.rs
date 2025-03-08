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

// TODO use single file for all signer config with raw_key
#[derive(Debug, Clone, Serialize)]
pub struct TestSignerConfig {
    pub raw_key: Vec<u8>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalSignerConfig {
    pub raw_key: Vec<u8>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AwsKmsSignerConfig {}

#[derive(Debug, Clone, Serialize)]
pub struct VaultSignerConfig {
    pub raw_key: Vec<u8>,
}

#[derive(Debug, Clone, Serialize)]
pub struct VaultCloudSignerConfig {
    pub raw_key: Vec<u8>,
}

#[derive(Debug, Clone, Serialize)]
pub enum SignerConfig {
    Test(TestSignerConfig),
    Local(LocalSignerConfig),
    AwsKms(AwsKmsSignerConfig),
    Vault(VaultSignerConfig),
    VaultCloud(VaultCloudSignerConfig),
}

impl SignerConfig {
    pub fn get_local(&self) -> Option<&LocalSignerConfig> {
        match self {
            SignerConfig::Local(config) => Some(config),
            _ => None,
        }
    }

    pub fn get_aws_kms(&self) -> Option<&AwsKmsSignerConfig> {
        match self {
            SignerConfig::AwsKms(config) => Some(config),
            _ => None,
        }
    }

    pub fn get_vault(&self) -> Option<&VaultSignerConfig> {
        match self {
            SignerConfig::Vault(config) => Some(config),
            _ => None,
        }
    }

    pub fn get_vault_cloud(&self) -> Option<&VaultCloudSignerConfig> {
        match self {
            SignerConfig::VaultCloud(config) => Some(config),
            _ => None,
        }
    }

    pub fn get_test(&self) -> Option<&TestSignerConfig> {
        match self {
            SignerConfig::Test(config) => Some(config),
            _ => None,
        }
    }
}
