use serde::{Deserialize, Serialize};

use crate::{
    config::{SignerConfig as ConfigFileSignerConfig, SignerConfigKeystore, SignerFileConfig},
    repositories::ConversionError,
    utils::unsafe_generate_random_private_key,
};

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
pub struct VaultSignerConfig {}

#[derive(Debug, Clone, Serialize)]
pub enum SignerConfig {
    Test(TestSignerConfig),
    Local(LocalSignerConfig),
    AwsKms(AwsKmsSignerConfig),
    Vault(VaultSignerConfig),
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

    pub fn get_test(&self) -> Option<&TestSignerConfig> {
        match self {
            SignerConfig::Test(config) => Some(config),
            _ => None,
        }
    }
}

impl TryFrom<SignerFileConfig> for SignerRepoModel {
    type Error = ConversionError;

    fn try_from(config: SignerFileConfig) -> Result<Self, Self::Error> {
        Ok(Self {
            id: config.id,
            config: match config.config {
                ConfigFileSignerConfig::Test(config) => SignerConfig::Test(TestSignerConfig {
                    raw_key: unsafe_generate_random_private_key(),
                }),
                ConfigFileSignerConfig::Local(config) => SignerConfig::Local(LocalSignerConfig {
                    raw_key: config
                        .load_keystore()
                        .map_err(|e| ConversionError::InvalidConfig(e.to_string()))?,
                }),
                ConfigFileSignerConfig::AwsKms(_) => SignerConfig::AwsKms(AwsKmsSignerConfig {}),
                ConfigFileSignerConfig::Vault(_) => SignerConfig::Vault(VaultSignerConfig {}),
            },
        })
    }
}
