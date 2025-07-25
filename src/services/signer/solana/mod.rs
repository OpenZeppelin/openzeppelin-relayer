//! Solana signer implementation for managing Solana-compatible private keys and signing operations.
//!
//! Provides:
//! - Local keystore support (encrypted JSON files)
//!
//! # Architecture
//!
//! ```text
//! SolanaSigner
//!   ├── Local (Raw Key Signer)
//!   ├── Vault (HashiCorp Vault backend)
//!   ├── VaultTransit (HashiCorp Vault Transit signer)
//!   |── GoogleCloudKms (Google Cloud KMS backend)
//!   └── Turnkey (Turnkey backend)

//! ```
use async_trait::async_trait;
mod local_signer;
use local_signer::*;

mod vault_signer;
use vault_signer::*;

mod vault_transit_signer;
use vault_transit_signer::*;

mod turnkey_signer;
use turnkey_signer::*;

mod google_cloud_kms_signer;
use google_cloud_kms_signer::*;

use solana_sdk::signature::Signature;

use crate::{
    domain::{
        SignDataRequest, SignDataResponse, SignDataResponseEvm, SignTransactionResponse,
        SignTypedDataRequest,
    },
    models::{
        Address, NetworkTransactionData, Signer as SignerDomainModel, SignerConfig,
        SignerRepoModel, SignerType, TransactionRepoModel, VaultSignerConfig,
    },
    services::{GoogleCloudKmsService, TurnkeyService, VaultConfig, VaultService},
};
use eyre::Result;

use super::{Signer, SignerError, SignerFactoryError};
#[cfg(test)]
use mockall::automock;

pub enum SolanaSigner {
    Local(LocalSigner),
    Vault(VaultSigner<VaultService>),
    VaultTransit(VaultTransitSigner),
    Turnkey(TurnkeySigner),
    GoogleCloudKms(GoogleCloudKmsSigner),
}

#[async_trait]
impl Signer for SolanaSigner {
    async fn address(&self) -> Result<Address, SignerError> {
        match self {
            Self::Local(signer) => signer.address().await,
            Self::Vault(signer) => signer.address().await,
            Self::VaultTransit(signer) => signer.address().await,
            Self::Turnkey(signer) => signer.address().await,
            Self::GoogleCloudKms(signer) => signer.address().await,
        }
    }

    async fn sign_transaction(
        &self,
        transaction: NetworkTransactionData,
    ) -> Result<SignTransactionResponse, SignerError> {
        match self {
            Self::Local(signer) => signer.sign_transaction(transaction).await,
            Self::Vault(signer) => signer.sign_transaction(transaction).await,
            Self::VaultTransit(signer) => signer.sign_transaction(transaction).await,
            Self::Turnkey(signer) => signer.sign_transaction(transaction).await,
            Self::GoogleCloudKms(signer) => signer.sign_transaction(transaction).await,
        }
    }
}

#[async_trait]
#[cfg_attr(test, automock)]
/// Trait defining Solana-specific signing operations
///
/// This trait extends the basic signing functionality with methods specific
/// to the Solana blockchain, including public key retrieval and message signing.
pub trait SolanaSignTrait: Sync + Send {
    /// Returns the public key of the Solana signer as an Address
    async fn pubkey(&self) -> Result<Address, SignerError>;

    /// Signs a message using the Solana signing scheme
    ///
    /// # Arguments
    ///
    /// * `message` - The message bytes to sign
    ///
    /// # Returns
    ///
    /// A Result containing either the Solana Signature or a SignerError
    async fn sign(&self, message: &[u8]) -> Result<Signature, SignerError>;
}

#[async_trait]
impl SolanaSignTrait for SolanaSigner {
    async fn pubkey(&self) -> Result<Address, SignerError> {
        match self {
            Self::Local(signer) => signer.pubkey().await,
            Self::Vault(signer) => signer.pubkey().await,
            Self::VaultTransit(signer) => signer.pubkey().await,
            Self::Turnkey(signer) => signer.pubkey().await,
            Self::GoogleCloudKms(signer) => signer.pubkey().await,
        }
    }

    async fn sign(&self, message: &[u8]) -> Result<Signature, SignerError> {
        match self {
            Self::Local(signer) => Ok(signer.sign(message).await?),
            Self::Vault(signer) => Ok(signer.sign(message).await?),
            Self::VaultTransit(signer) => Ok(signer.sign(message).await?),
            Self::Turnkey(signer) => Ok(signer.sign(message).await?),
            Self::GoogleCloudKms(signer) => Ok(signer.sign(message).await?),
        }
    }
}

pub struct SolanaSignerFactory;

impl SolanaSignerFactory {
    pub fn create_solana_signer(
        signer_model: &SignerDomainModel,
    ) -> Result<SolanaSigner, SignerFactoryError> {
        let signer = match &signer_model.config {
            SignerConfig::Local(_) => SolanaSigner::Local(LocalSigner::new(signer_model)?),
            SignerConfig::Vault(config) => {
                let vault_config = VaultConfig::new(
                    config.address.clone(),
                    config.role_id.clone(),
                    config.secret_id.clone(),
                    config.namespace.clone(),
                    config
                        .mount_point
                        .clone()
                        .unwrap_or_else(|| "secret".to_string()),
                    None,
                );
                let vault_service = VaultService::new(vault_config);

                return Ok(SolanaSigner::Vault(VaultSigner::new(
                    signer_model.id.clone(),
                    config.clone(),
                    vault_service,
                )));
            }
            SignerConfig::VaultTransit(vault_transit_signer_config) => {
                let vault_service = VaultService::new(VaultConfig {
                    address: vault_transit_signer_config.address.clone(),
                    namespace: vault_transit_signer_config.namespace.clone(),
                    role_id: vault_transit_signer_config.role_id.clone(),
                    secret_id: vault_transit_signer_config.secret_id.clone(),
                    mount_path: "transit".to_string(),
                    token_ttl: None,
                });

                return Ok(SolanaSigner::VaultTransit(VaultTransitSigner::new(
                    signer_model,
                    vault_service,
                )));
            }
            SignerConfig::AwsKms(_) => {
                return Err(SignerFactoryError::UnsupportedType("AWS KMS".into()));
            }
            SignerConfig::Turnkey(turnkey_signer_config) => {
                let turnkey_service =
                    TurnkeyService::new(turnkey_signer_config.clone()).map_err(|e| {
                        SignerFactoryError::InvalidConfig(format!(
                            "Failed to create Turnkey service: {}",
                            e
                        ))
                    })?;

                return Ok(SolanaSigner::Turnkey(TurnkeySigner::new(turnkey_service)));
            }
            SignerConfig::GoogleCloudKms(google_cloud_kms_signer_config) => {
                let google_cloud_kms_service =
                    GoogleCloudKmsService::new(google_cloud_kms_signer_config).map_err(|e| {
                        SignerFactoryError::InvalidConfig(format!(
                            "Failed to create Google Cloud KMS service: {}",
                            e
                        ))
                    })?;
                return Ok(SolanaSigner::GoogleCloudKms(GoogleCloudKmsSigner::new(
                    google_cloud_kms_service,
                )));
            }
        };

        Ok(signer)
    }
}

#[cfg(test)]
mod solana_signer_factory_tests {
    use super::*;
    use crate::models::{
        AwsKmsSignerConfig, GoogleCloudKmsSignerConfig, GoogleCloudKmsSignerKeyConfig,
        GoogleCloudKmsSignerServiceAccountConfig, LocalSignerConfig, SecretString, SignerConfig,
        SignerRepoModel, SolanaTransactionData, TurnkeySignerConfig, VaultSignerConfig,
        VaultTransitSignerConfig,
    };
    use mockall::predicate::*;
    use secrets::SecretVec;
    use std::sync::Arc;

    fn test_key_bytes() -> SecretVec<u8> {
        let key_bytes = vec![
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24,
            25, 26, 27, 28, 29, 30, 31, 32,
        ];
        SecretVec::new(key_bytes.len(), |v| v.copy_from_slice(&key_bytes))
    }

    fn test_key_bytes_pubkey() -> Address {
        Address::Solana("9C6hybhQ6Aycep9jaUnP6uL9ZYvDjUp1aSkFWPUFJtpj".to_string())
    }

    #[test]
    fn test_create_solana_signer_local() {
        let signer_model = SignerDomainModel {
            id: "test".to_string(),
            config: SignerConfig::Local(LocalSignerConfig {
                raw_key: test_key_bytes(),
            }),
        };

        let signer = SolanaSignerFactory::create_solana_signer(&signer_model).unwrap();

        match signer {
            SolanaSigner::Local(_) => {}
            _ => panic!("Expected Local signer"),
        }
    }

    #[test]
    fn test_create_solana_signer_test() {
        let signer_model = SignerDomainModel {
            id: "test".to_string(),
            config: SignerConfig::Local(LocalSignerConfig {
                raw_key: test_key_bytes(),
            }),
        };

        let signer = SolanaSignerFactory::create_solana_signer(&signer_model).unwrap();

        match signer {
            SolanaSigner::Local(_) => {}
            _ => panic!("Expected Local signer"),
        }
    }

    #[test]
    fn test_create_solana_signer_vault() {
        let signer_model = SignerDomainModel {
            id: "test".to_string(),
            config: SignerConfig::Vault(VaultSignerConfig {
                address: "https://vault.test.com".to_string(),
                namespace: Some("test-namespace".to_string()),
                role_id: crate::models::SecretString::new("test-role-id"),
                secret_id: crate::models::SecretString::new("test-secret-id"),
                key_name: "test-key".to_string(),
                mount_point: Some("secret".to_string()),
            }),
        };

        let signer = SolanaSignerFactory::create_solana_signer(&signer_model).unwrap();

        match signer {
            SolanaSigner::Vault(_) => {}
            _ => panic!("Expected Vault signer"),
        }
    }

    #[test]
    fn test_create_solana_signer_vault_transit() {
        let signer_model = SignerDomainModel {
            id: "test".to_string(),
            config: SignerConfig::VaultTransit(VaultTransitSignerConfig {
                key_name: "test".to_string(),
                address: "address".to_string(),
                namespace: None,
                role_id: SecretString::new("role_id"),
                secret_id: SecretString::new("secret_id"),
                pubkey: "pubkey".to_string(),
                mount_point: None,
            }),
        };

        let signer = SolanaSignerFactory::create_solana_signer(&signer_model).unwrap();

        match signer {
            SolanaSigner::VaultTransit(_) => {}
            _ => panic!("Expected Transit signer"),
        }
    }

    #[test]
    fn test_create_solana_signer_turnkey() {
        let signer_model = SignerDomainModel {
            id: "test".to_string(),
            config: SignerConfig::Turnkey(TurnkeySignerConfig {
                api_private_key: SecretString::new("api_private_key"),
                api_public_key: "api_public_key".to_string(),
                organization_id: "organization_id".to_string(),
                private_key_id: "private_key_id".to_string(),
                public_key: "public_key".to_string(),
            }),
        };

        let signer = SolanaSignerFactory::create_solana_signer(&signer_model).unwrap();

        match signer {
            SolanaSigner::Turnkey(_) => {}
            _ => panic!("Expected Turnkey signer"),
        }
    }

    #[tokio::test]
    async fn test_create_solana_signer_google_cloud_kms() {
        let signer_model = SignerDomainModel {
            id: "test".to_string(),
            config: SignerConfig::GoogleCloudKms(GoogleCloudKmsSignerConfig {
                service_account: GoogleCloudKmsSignerServiceAccountConfig {
                    project_id: "project_id".to_string(),
                    private_key_id: SecretString::new("private_key_id"),
                    private_key: SecretString::new("private_key"),
                    client_email: SecretString::new("client_email"),
                    client_id: "client_id".to_string(),
                    auth_uri: "auth_uri".to_string(),
                    token_uri: "token_uri".to_string(),
                    auth_provider_x509_cert_url: "auth_provider_x509_cert_url".to_string(),
                    client_x509_cert_url: "client_x509_cert_url".to_string(),
                    universe_domain: "universe_domain".to_string(),
                },
                key: GoogleCloudKmsSignerKeyConfig {
                    location: "global".to_string(),
                    key_id: "id".to_string(),
                    key_ring_id: "key_ring".to_string(),
                    key_version: 1,
                },
            }),
        };

        let signer = SolanaSignerFactory::create_solana_signer(&signer_model).unwrap();

        match signer {
            SolanaSigner::GoogleCloudKms(_) => {}
            _ => panic!("Expected Google Cloud KMS signer"),
        }
    }

    #[tokio::test]
    async fn test_address_solana_signer_local() {
        let signer_model = SignerDomainModel {
            id: "test".to_string(),
            config: SignerConfig::Local(LocalSignerConfig {
                raw_key: test_key_bytes(),
            }),
        };

        let signer = SolanaSignerFactory::create_solana_signer(&signer_model).unwrap();
        let signer_address = signer.address().await.unwrap();
        let signer_pubkey = signer.pubkey().await.unwrap();

        assert_eq!(test_key_bytes_pubkey(), signer_address);
        assert_eq!(test_key_bytes_pubkey(), signer_pubkey);
    }

    #[tokio::test]
    async fn test_address_solana_signer_vault_transit() {
        let signer_model = SignerDomainModel {
            id: "test".to_string(),
            config: SignerConfig::VaultTransit(VaultTransitSignerConfig {
                key_name: "test".to_string(),
                address: "address".to_string(),
                namespace: None,
                role_id: SecretString::new("role_id"),
                secret_id: SecretString::new("secret_id"),
                pubkey: "fV060x5X3Eo4uK/kTqQbSVL/qmMNaYKF2oaTa15hNfU=".to_string(),
                mount_point: None,
            }),
        };
        let expected_pubkey =
            Address::Solana("9SNR5Sf993aphA7hzWSQsGv63x93trfuN8WjaToXcqKA".to_string());

        let signer = SolanaSignerFactory::create_solana_signer(&signer_model).unwrap();
        let signer_address = signer.address().await.unwrap();
        let signer_pubkey = signer.pubkey().await.unwrap();

        assert_eq!(expected_pubkey, signer_address);
        assert_eq!(expected_pubkey, signer_pubkey);
    }

    #[tokio::test]
    async fn test_address_solana_signer_turnkey() {
        let signer_model = SignerDomainModel {
            id: "test".to_string(),
            config: SignerConfig::Turnkey(TurnkeySignerConfig {
                api_private_key: SecretString::new("api_private_key"),
                api_public_key: "api_public_key".to_string(),
                organization_id: "organization_id".to_string(),
                private_key_id: "private_key_id".to_string(),
                public_key: "5720be8aa9d2bb4be8e91f31d2c44c8629e42da16981c2cebabd55cafa0b76bd"
                    .to_string(),
            }),
        };
        let expected_pubkey =
            Address::Solana("6s7RsvzcdXFJi1tXeDoGfSKZFzN3juVt9fTar6WEhEm2".to_string());

        let signer = SolanaSignerFactory::create_solana_signer(&signer_model).unwrap();
        let signer_address = signer.address().await.unwrap();
        let signer_pubkey = signer.pubkey().await.unwrap();

        assert_eq!(expected_pubkey, signer_address);
        assert_eq!(expected_pubkey, signer_pubkey);
    }

    #[tokio::test]
    async fn test_address_solana_signer_google_cloud_kms() {
        let signer_model = SignerDomainModel {
            id: "test".to_string(),
            config: SignerConfig::GoogleCloudKms(GoogleCloudKmsSignerConfig {
                service_account: GoogleCloudKmsSignerServiceAccountConfig {
                    project_id: "project_id".to_string(),
                    private_key_id: SecretString::new("private_key_id"),
                    private_key: SecretString::new("private_key"),
                    client_email: SecretString::new("client_email"),
                    client_id: "client_id".to_string(),
                    auth_uri: "auth_uri".to_string(),
                    token_uri: "token_uri".to_string(),
                    auth_provider_x509_cert_url: "auth_provider_x509_cert_url".to_string(),
                    client_x509_cert_url: "client_x509_cert_url".to_string(),
                    universe_domain: "universe_domain".to_string(),
                },
                key: GoogleCloudKmsSignerKeyConfig {
                    location: "global".to_string(),
                    key_id: "id".to_string(),
                    key_ring_id: "key_ring".to_string(),
                    key_version: 1,
                },
            }),
        };

        let signer = SolanaSignerFactory::create_solana_signer(&signer_model).unwrap();
        let signer_address = signer.address().await;
        let signer_pubkey = signer.pubkey().await;

        // should fail due to call to google cloud
        assert!(signer_address.is_err());
        assert!(signer_pubkey.is_err());
    }

    #[tokio::test]
    async fn test_sign_solana_signer_local() {
        let signer_model = SignerDomainModel {
            id: "test".to_string(),
            config: SignerConfig::Local(LocalSignerConfig {
                raw_key: test_key_bytes(),
            }),
        };

        let signer = SolanaSignerFactory::create_solana_signer(&signer_model).unwrap();
        let message = b"test message";
        let signature = signer.sign(message).await;

        assert!(signature.is_ok());
    }

    #[tokio::test]
    async fn test_sign_solana_signer_test() {
        let signer_model = SignerDomainModel {
            id: "test".to_string(),
            config: SignerConfig::Local(LocalSignerConfig {
                raw_key: test_key_bytes(),
            }),
        };

        let signer = SolanaSignerFactory::create_solana_signer(&signer_model).unwrap();
        let message = b"test message";
        let signature = signer.sign(message).await;

        assert!(signature.is_ok());
    }
}
