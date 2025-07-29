//! Repository layer models and data persistence for signers.
//!
//! This module provides the data layer representation of signers, including:
//!
//! - **Repository Models**: Data structures optimized for storage and retrieval
//! - **Data Conversions**: Mapping between domain objects and repository representations
//! - **Persistence Logic**: Storage-specific validation and constraints
//!
//! Acts as the bridge between the domain layer and actual data storage implementations
//! (in-memory, Redis, etc.), ensuring consistent data representation across repositories.
//!

use crate::{
    models::{
        signer::{
            AwsKmsSignerConfig, GoogleCloudKmsSignerConfig, GoogleCloudKmsSignerKeyConfig,
            GoogleCloudKmsSignerServiceAccountConfig, LocalSignerConfig, Signer, SignerConfig,
            SignerValidationError, TurnkeySignerConfig, VaultSignerConfig,
            VaultTransitSignerConfig,
        },
        SecretString,
    },
    utils::{
        deserialize_secret_string, deserialize_secret_vec, serialize_secret_string,
        serialize_secret_vec,
    },
};
use secrets::SecretVec;
use serde::{Deserialize, Serialize};
/// Repository model for signer storage and retrieval
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignerRepoModel {
    pub id: String,
    pub config: SignerConfigStorage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SignerConfigStorage {
    Local(LocalSignerConfigStorage),
    Vault(VaultSignerConfigStorage),
    VaultTransit(VaultTransitSignerConfigStorage),
    AwsKms(AwsKmsSignerConfigStorage),
    Turnkey(TurnkeySignerConfigStorage),
    GoogleCloudKms(GoogleCloudKmsSignerConfigStorage),
}

/// Local signer configuration for storage (with base64 encoding)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalSignerConfigStorage {
    #[serde(
        serialize_with = "serialize_secret_vec",
        deserialize_with = "deserialize_secret_vec"
    )]
    pub raw_key: SecretVec<u8>,
}

impl From<LocalSignerConfig> for LocalSignerConfigStorage {
    fn from(config: LocalSignerConfig) -> Self {
        Self {
            raw_key: config.raw_key,
        }
    }
}

impl From<LocalSignerConfigStorage> for LocalSignerConfig {
    fn from(storage: LocalSignerConfigStorage) -> Self {
        Self {
            raw_key: storage.raw_key,
        }
    }
}

/// Storage representations for other signer types (these are simpler as they don't contain secrets that need encoding)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AwsKmsSignerConfigStorage {
    pub region: Option<String>,
    pub key_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultSignerConfigStorage {
    pub address: String,
    pub namespace: Option<String>,
    #[serde(
        serialize_with = "serialize_secret_string",
        deserialize_with = "deserialize_secret_string"
    )]
    pub role_id: SecretString,
    #[serde(
        serialize_with = "serialize_secret_string",
        deserialize_with = "deserialize_secret_string"
    )]
    pub secret_id: SecretString,
    pub key_name: String,
    pub mount_point: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultTransitSignerConfigStorage {
    pub key_name: String,
    pub address: String,
    pub namespace: Option<String>,
    #[serde(
        serialize_with = "serialize_secret_string",
        deserialize_with = "deserialize_secret_string"
    )]
    pub role_id: SecretString,
    #[serde(
        serialize_with = "serialize_secret_string",
        deserialize_with = "deserialize_secret_string"
    )]
    pub secret_id: SecretString,
    pub pubkey: String,
    pub mount_point: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnkeySignerConfigStorage {
    pub api_public_key: String,
    #[serde(
        serialize_with = "serialize_secret_string",
        deserialize_with = "deserialize_secret_string"
    )]
    pub api_private_key: SecretString,
    pub organization_id: String,
    pub private_key_id: String,
    pub public_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleCloudKmsSignerServiceAccountConfigStorage {
    #[serde(
        serialize_with = "serialize_secret_string",
        deserialize_with = "deserialize_secret_string"
    )]
    pub private_key: SecretString,
    #[serde(
        serialize_with = "serialize_secret_string",
        deserialize_with = "deserialize_secret_string"
    )]
    pub private_key_id: SecretString,
    pub project_id: String,
    #[serde(
        serialize_with = "serialize_secret_string",
        deserialize_with = "deserialize_secret_string"
    )]
    pub client_email: SecretString,
    pub client_id: String,
    pub auth_uri: String,
    pub token_uri: String,
    pub auth_provider_x509_cert_url: String,
    pub client_x509_cert_url: String,
    pub universe_domain: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleCloudKmsSignerKeyConfigStorage {
    pub location: String,
    pub key_ring_id: String,
    pub key_id: String,
    pub key_version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleCloudKmsSignerConfigStorage {
    pub service_account: GoogleCloudKmsSignerServiceAccountConfigStorage,
    pub key: GoogleCloudKmsSignerKeyConfigStorage,
}

/// Convert from domain model to repository model
impl From<Signer> for SignerRepoModel {
    fn from(signer: Signer) -> Self {
        Self {
            id: signer.id,
            config: signer.config.into(),
        }
    }
}

/// Convert from repository model to domain model
impl From<SignerRepoModel> for Signer {
    fn from(repo_model: SignerRepoModel) -> Self {
        Self {
            id: repo_model.id,
            config: repo_model.config.into(),
        }
    }
}

impl From<AwsKmsSignerConfig> for AwsKmsSignerConfigStorage {
    fn from(config: AwsKmsSignerConfig) -> Self {
        Self {
            region: config.region,
            key_id: config.key_id,
        }
    }
}

impl From<AwsKmsSignerConfigStorage> for AwsKmsSignerConfig {
    fn from(storage: AwsKmsSignerConfigStorage) -> Self {
        Self {
            region: storage.region,
            key_id: storage.key_id,
        }
    }
}

impl From<VaultSignerConfig> for VaultSignerConfigStorage {
    fn from(config: VaultSignerConfig) -> Self {
        Self {
            address: config.address,
            namespace: config.namespace,
            role_id: config.role_id,
            secret_id: config.secret_id,
            key_name: config.key_name,
            mount_point: config.mount_point,
        }
    }
}

impl From<VaultSignerConfigStorage> for VaultSignerConfig {
    fn from(storage: VaultSignerConfigStorage) -> Self {
        Self {
            address: storage.address,
            namespace: storage.namespace,
            role_id: storage.role_id,
            secret_id: storage.secret_id,
            key_name: storage.key_name,
            mount_point: storage.mount_point,
        }
    }
}

impl From<VaultTransitSignerConfig> for VaultTransitSignerConfigStorage {
    fn from(config: VaultTransitSignerConfig) -> Self {
        Self {
            key_name: config.key_name,
            address: config.address,
            namespace: config.namespace,
            role_id: config.role_id,
            secret_id: config.secret_id,
            pubkey: config.pubkey,
            mount_point: config.mount_point,
        }
    }
}

impl From<VaultTransitSignerConfigStorage> for VaultTransitSignerConfig {
    fn from(storage: VaultTransitSignerConfigStorage) -> Self {
        Self {
            key_name: storage.key_name,
            address: storage.address,
            namespace: storage.namespace,
            role_id: storage.role_id,
            secret_id: storage.secret_id,
            pubkey: storage.pubkey,
            mount_point: storage.mount_point,
        }
    }
}

impl From<TurnkeySignerConfig> for TurnkeySignerConfigStorage {
    fn from(config: TurnkeySignerConfig) -> Self {
        Self {
            api_public_key: config.api_public_key,
            api_private_key: config.api_private_key,
            organization_id: config.organization_id,
            private_key_id: config.private_key_id,
            public_key: config.public_key,
        }
    }
}

impl From<TurnkeySignerConfigStorage> for TurnkeySignerConfig {
    fn from(storage: TurnkeySignerConfigStorage) -> Self {
        Self {
            api_public_key: storage.api_public_key,
            api_private_key: storage.api_private_key,
            organization_id: storage.organization_id,
            private_key_id: storage.private_key_id,
            public_key: storage.public_key,
        }
    }
}

impl From<GoogleCloudKmsSignerConfig> for GoogleCloudKmsSignerConfigStorage {
    fn from(config: GoogleCloudKmsSignerConfig) -> Self {
        Self {
            service_account: config.service_account.into(),
            key: config.key.into(),
        }
    }
}

impl From<GoogleCloudKmsSignerConfigStorage> for GoogleCloudKmsSignerConfig {
    fn from(storage: GoogleCloudKmsSignerConfigStorage) -> Self {
        Self {
            service_account: storage.service_account.into(),
            key: storage.key.into(),
        }
    }
}

impl From<GoogleCloudKmsSignerServiceAccountConfig>
    for GoogleCloudKmsSignerServiceAccountConfigStorage
{
    fn from(config: GoogleCloudKmsSignerServiceAccountConfig) -> Self {
        Self {
            private_key: config.private_key,
            private_key_id: config.private_key_id,
            project_id: config.project_id,
            client_email: config.client_email,
            client_id: config.client_id,
            auth_uri: config.auth_uri,
            token_uri: config.token_uri,
            auth_provider_x509_cert_url: config.auth_provider_x509_cert_url,
            client_x509_cert_url: config.client_x509_cert_url,
            universe_domain: config.universe_domain,
        }
    }
}

impl From<GoogleCloudKmsSignerServiceAccountConfigStorage>
    for GoogleCloudKmsSignerServiceAccountConfig
{
    fn from(storage: GoogleCloudKmsSignerServiceAccountConfigStorage) -> Self {
        Self {
            private_key: storage.private_key,
            private_key_id: storage.private_key_id,
            project_id: storage.project_id,
            client_email: storage.client_email,
            client_id: storage.client_id,
            auth_uri: storage.auth_uri,
            token_uri: storage.token_uri,
            auth_provider_x509_cert_url: storage.auth_provider_x509_cert_url,
            client_x509_cert_url: storage.client_x509_cert_url,
            universe_domain: storage.universe_domain,
        }
    }
}

impl From<GoogleCloudKmsSignerKeyConfig> for GoogleCloudKmsSignerKeyConfigStorage {
    fn from(config: GoogleCloudKmsSignerKeyConfig) -> Self {
        Self {
            location: config.location,
            key_ring_id: config.key_ring_id,
            key_id: config.key_id,
            key_version: config.key_version,
        }
    }
}

impl From<GoogleCloudKmsSignerKeyConfigStorage> for GoogleCloudKmsSignerKeyConfig {
    fn from(storage: GoogleCloudKmsSignerKeyConfigStorage) -> Self {
        Self {
            location: storage.location,
            key_ring_id: storage.key_ring_id,
            key_id: storage.key_id,
            key_version: storage.key_version,
        }
    }
}

impl SignerRepoModel {
    /// Validates the repository model using core validation logic
    pub fn validate(&self) -> Result<(), SignerValidationError> {
        let core_signer = Signer::from(self.clone());
        core_signer.validate()
    }
}

impl From<SignerConfig> for SignerConfigStorage {
    fn from(config: SignerConfig) -> Self {
        match config {
            SignerConfig::Local(local) => SignerConfigStorage::Local(local.into()),
            SignerConfig::Vault(vault) => SignerConfigStorage::Vault(vault.into()),
            SignerConfig::VaultTransit(vault_transit) => {
                SignerConfigStorage::VaultTransit(vault_transit.into())
            }
            SignerConfig::AwsKms(aws_kms) => SignerConfigStorage::AwsKms(aws_kms.into()),
            SignerConfig::Turnkey(turnkey) => SignerConfigStorage::Turnkey(turnkey.into()),
            SignerConfig::GoogleCloudKms(gcp) => SignerConfigStorage::GoogleCloudKms(gcp.into()),
        }
    }
}

impl From<SignerConfigStorage> for SignerConfig {
    fn from(storage: SignerConfigStorage) -> Self {
        match storage {
            SignerConfigStorage::Local(local) => SignerConfig::Local(local.into()),
            SignerConfigStorage::Vault(vault) => SignerConfig::Vault(vault.into()),
            SignerConfigStorage::VaultTransit(vault_transit) => {
                SignerConfig::VaultTransit(vault_transit.into())
            }
            SignerConfigStorage::AwsKms(aws_kms) => SignerConfig::AwsKms(aws_kms.into()),
            SignerConfigStorage::Turnkey(turnkey) => SignerConfig::Turnkey(turnkey.into()),
            SignerConfigStorage::GoogleCloudKms(gcp) => SignerConfig::GoogleCloudKms(gcp.into()),
        }
    }
}

impl SignerConfigStorage {
    /// Get local signer config, returns error if not a local signer
    pub fn get_local(&self) -> Option<&LocalSignerConfigStorage> {
        match self {
            Self::Local(config) => Some(config),
            _ => None,
        }
    }

    /// Get vault transit signer config, returns error if not a vault transit signer
    pub fn get_vault_transit(&self) -> Option<&VaultTransitSignerConfigStorage> {
        match self {
            Self::VaultTransit(config) => Some(config),
            _ => None,
        }
    }

    /// Get vault signer config, returns error if not a vault signer
    pub fn get_vault(&self) -> Option<&VaultSignerConfigStorage> {
        match self {
            Self::Vault(config) => Some(config),
            _ => None,
        }
    }

    /// Get turnkey signer config, returns error if not a turnkey signer
    pub fn get_turnkey(&self) -> Option<&TurnkeySignerConfigStorage> {
        match self {
            Self::Turnkey(config) => Some(config),
            _ => None,
        }
    }

    /// Get google cloud kms signer config, returns error if not a google cloud kms signer
    pub fn get_google_cloud_kms(&self) -> Option<&GoogleCloudKmsSignerConfigStorage> {
        match self {
            Self::GoogleCloudKms(config) => Some(config),
            _ => None,
        }
    }

    /// Get aws kms signer config, returns error if not an aws kms signer
    pub fn get_aws_kms(&self) -> Option<&AwsKmsSignerConfigStorage> {
        match self {
            Self::AwsKms(config) => Some(config),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::signer::{LocalSignerConfig, SignerConfig};
    use secrets::SecretVec;

    #[test]
    fn test_from_core_signer() {
        let config = LocalSignerConfig {
            raw_key: SecretVec::new(32, |v| v.fill(1)),
        };

        let core =
            crate::models::signer::Signer::new("test-id".to_string(), SignerConfig::Local(config));

        let repo_model = SignerRepoModel::from(core);
        assert_eq!(repo_model.id, "test-id");
        assert!(matches!(repo_model.config, SignerConfigStorage::Local(_)));
    }

    #[test]
    fn test_to_core_signer() {
        use crate::models::signer::AwsKmsSignerConfigStorage;

        let domain_config = AwsKmsSignerConfigStorage {
            region: Some("us-east-1".to_string()),
            key_id: "test-key".to_string(),
        };

        let repo_model = SignerRepoModel {
            id: "test-id".to_string(),
            config: SignerConfigStorage::AwsKms(domain_config),
        };

        let core = Signer::from(repo_model);
        assert_eq!(core.id, "test-id");
        assert_eq!(
            core.signer_type(),
            crate::models::signer::SignerType::AwsKms
        );
    }

    #[test]
    fn test_validation() {
        use secrets::SecretVec;

        let domain_config = LocalSignerConfig {
            raw_key: SecretVec::new(32, |v| v.fill(1)),
        };
        // Convert to storage config properly
        let storage_config = LocalSignerConfigStorage::from(domain_config);

        let repo_model = SignerRepoModel {
            id: "test-id".to_string(),
            config: SignerConfigStorage::Local(storage_config),
        };

        assert!(repo_model.validate().is_ok());
    }

    #[test]
    fn test_local_config_storage_conversion() {
        let domain_config = LocalSignerConfig {
            raw_key: SecretVec::new(4, |v| v.copy_from_slice(&[1, 2, 3, 4])),
        };

        let storage_config = LocalSignerConfigStorage::from(domain_config.clone());
        let converted_back = LocalSignerConfig::from(storage_config);

        // Compare the actual secret data
        let original_data = domain_config.raw_key.borrow();
        let converted_data = converted_back.raw_key.borrow();
        assert_eq!(*original_data, *converted_data);
    }
}
