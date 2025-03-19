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
//!   ├── VaultCloud (HashiCorp Cloud Vault backend)
//!   └── VaultTransit (HashiCorp Vault Transit signer, most secure)

//! ```
use async_trait::async_trait;
mod local_signer;
use local_signer::*;
mod vault_transit_signer;
use solana_sdk::signature::Signature;
use vault_transit_signer::*;

use crate::{
    domain::{
        SignDataRequest, SignDataResponse, SignDataResponseEvm, SignTransactionResponse,
        SignTypedDataRequest,
    },
    models::{
        Address, NetworkTransactionData, SignerConfig, SignerRepoModel, SignerType,
        TransactionRepoModel,
    },
    services::{VaultConfig, VaultService},
};
use eyre::Result;

use super::{Signer, SignerError, SignerFactoryError};
#[cfg(test)]
use mockall::automock;

pub enum SolanaSigner {
    Local(LocalSigner),
    Vault(LocalSigner),
    VaultCloud(LocalSigner),
    VaultTransit(VaultTransitSigner),
}

#[async_trait]
impl Signer for SolanaSigner {
    async fn address(&self) -> Result<Address, SignerError> {
        match self {
            Self::Local(signer) | Self::Vault(signer) | Self::VaultCloud(signer) => {
                signer.address().await
            }
            Self::VaultTransit(signer) => signer.address().await,
        }
    }

    async fn sign_transaction(
        &self,
        transaction: NetworkTransactionData,
    ) -> Result<SignTransactionResponse, SignerError> {
        match self {
            Self::Local(signer) | Self::Vault(signer) | Self::VaultCloud(signer) => {
                signer.sign_transaction(transaction).await
            }
            Self::VaultTransit(signer) => signer.sign_transaction(transaction).await,
        }
    }
}

#[async_trait]
#[cfg_attr(test, automock)]
pub trait SolanaSignTrait: Send + Sync {
    fn pubkey(&self) -> Result<Address, SignerError>;
    async fn sign(&self, message: &[u8]) -> Result<Signature, SignerError>;
}

#[async_trait]
impl SolanaSignTrait for SolanaSigner {
    fn pubkey(&self) -> Result<Address, SignerError> {
        match self {
            Self::Local(signer) | Self::Vault(signer) | Self::VaultCloud(signer) => signer.pubkey(),
            Self::VaultTransit(signer) => signer.pubkey(),
        }
    }

    async fn sign(&self, message: &[u8]) -> Result<Signature, SignerError> {
        match self {
            Self::Local(signer) | Self::Vault(signer) | Self::VaultCloud(signer) => {
                Ok(signer.sign(message).await?)
            }
            Self::VaultTransit(signer) => Ok(signer.sign(message).await?),
        }
    }
}

pub struct SolanaSignerFactory;

impl SolanaSignerFactory {
    pub fn create_solana_signer(
        signer_model: &SignerRepoModel,
    ) -> Result<SolanaSigner, SignerFactoryError> {
        let signer = match &signer_model.config {
            SignerConfig::Local(_)
            | SignerConfig::Test(_)
            | SignerConfig::Vault(_)
            | SignerConfig::VaultCloud(_) => SolanaSigner::Local(LocalSigner::new(signer_model)?),
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
        };

        Ok(signer)
    }
}

#[cfg(test)]
mod solana_signer_factory_tests {
    use super::*;
    use crate::models::{
        AwsKmsSignerConfig, LocalSignerConfig, SecretString, SignerConfig, SignerRepoModel,
        SolanaTransactionData, VaultTransitSignerConfig,
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
        let signer_model = SignerRepoModel {
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
        let signer_model = SignerRepoModel {
            id: "test".to_string(),
            config: SignerConfig::Test(LocalSignerConfig {
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
        let signer_model = SignerRepoModel {
            id: "test".to_string(),
            config: SignerConfig::Vault(LocalSignerConfig {
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
    fn test_create_solana_signer_vault_cloud() {
        let signer_model = SignerRepoModel {
            id: "test".to_string(),
            config: SignerConfig::VaultCloud(LocalSignerConfig {
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
    fn test_create_solana_signer_vault_transit() {
        let signer_model = SignerRepoModel {
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

    #[tokio::test]
    async fn test_address_solana_signer_local() {
        let signer_model = SignerRepoModel {
            id: "test".to_string(),
            config: SignerConfig::Local(LocalSignerConfig {
                raw_key: test_key_bytes(),
            }),
        };

        let signer = SolanaSignerFactory::create_solana_signer(&signer_model).unwrap();
        let signer_address = signer.address().await.unwrap();
        let signer_pubkey = signer.pubkey().unwrap();

        assert_eq!(test_key_bytes_pubkey(), signer_address);
        assert_eq!(test_key_bytes_pubkey(), signer_pubkey);
    }

    #[tokio::test]
    async fn test_address_solana_signer_test() {
        let signer_model = SignerRepoModel {
            id: "test".to_string(),
            config: SignerConfig::Test(LocalSignerConfig {
                raw_key: test_key_bytes(),
            }),
        };

        let signer = SolanaSignerFactory::create_solana_signer(&signer_model).unwrap();
        let signer_address = signer.address().await.unwrap();
        let signer_pubkey = signer.pubkey().unwrap();

        assert_eq!(test_key_bytes_pubkey(), signer_address);
        assert_eq!(test_key_bytes_pubkey(), signer_pubkey);
    }

    #[tokio::test]
    async fn test_address_solana_signer_vault() {
        let signer_model = SignerRepoModel {
            id: "test".to_string(),
            config: SignerConfig::Vault(LocalSignerConfig {
                raw_key: test_key_bytes(),
            }),
        };

        let signer = SolanaSignerFactory::create_solana_signer(&signer_model).unwrap();
        let signer_address = signer.address().await.unwrap();
        let signer_pubkey = signer.pubkey().unwrap();

        assert_eq!(test_key_bytes_pubkey(), signer_address);
        assert_eq!(test_key_bytes_pubkey(), signer_pubkey);
    }

    #[tokio::test]
    async fn test_address_solana_signer_vault_cloud() {
        let signer_model = SignerRepoModel {
            id: "test".to_string(),
            config: SignerConfig::VaultCloud(LocalSignerConfig {
                raw_key: test_key_bytes(),
            }),
        };

        let signer = SolanaSignerFactory::create_solana_signer(&signer_model).unwrap();
        let signer_address = signer.address().await.unwrap();
        let signer_pubkey = signer.pubkey().unwrap();

        assert_eq!(test_key_bytes_pubkey(), signer_address);
        assert_eq!(test_key_bytes_pubkey(), signer_pubkey);
    }

    #[tokio::test]
    async fn test_address_solana_signer_vault_transit() {
        let signer_model = SignerRepoModel {
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
        let signer_pubkey = signer.pubkey().unwrap();

        assert_eq!(expected_pubkey, signer_address);
        assert_eq!(expected_pubkey, signer_pubkey);
    }

    #[tokio::test]
    async fn test_sign_solana_signer_local() {
        let signer_model = SignerRepoModel {
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
        let signer_model = SignerRepoModel {
            id: "test".to_string(),
            config: SignerConfig::Test(LocalSignerConfig {
                raw_key: test_key_bytes(),
            }),
        };

        let signer = SolanaSignerFactory::create_solana_signer(&signer_model).unwrap();
        let message = b"test message";
        let signature = signer.sign(message).await;

        assert!(signature.is_ok());
    }

    #[tokio::test]
    async fn test_sign_solana_signer_vault() {
        let signer_model = SignerRepoModel {
            id: "test".to_string(),
            config: SignerConfig::Vault(LocalSignerConfig {
                raw_key: test_key_bytes(),
            }),
        };

        let signer = SolanaSignerFactory::create_solana_signer(&signer_model).unwrap();
        let message = b"test message";
        let signature = signer.sign(message).await;

        assert!(signature.is_ok());
    }

    #[tokio::test]
    async fn test_sign_solana_signer_vault_cloud() {
        let signer_model = SignerRepoModel {
            id: "test".to_string(),
            config: SignerConfig::VaultCloud(LocalSignerConfig {
                raw_key: test_key_bytes(),
            }),
        };

        let signer: SolanaSigner =
            SolanaSignerFactory::create_solana_signer(&signer_model).unwrap();
        let message = b"test message";
        let signature = signer.sign(message).await;

        assert!(signature.is_ok());
    }

    #[tokio::test]
    async fn test_sign_transaction_not_implemented() {
        let signer_model = SignerRepoModel {
            id: "test".to_string(),
            config: SignerConfig::VaultCloud(LocalSignerConfig {
                raw_key: test_key_bytes(),
            }),
        };

        let signer: SolanaSigner =
            SolanaSignerFactory::create_solana_signer(&signer_model).unwrap();
        let transaction_data = NetworkTransactionData::Solana(SolanaTransactionData {
            fee_payer: "test".to_string(),
            hash: None,
            recent_blockhash: None,
            instructions: vec![],
        });

        let result = signer.sign_transaction(transaction_data).await;

        match result {
            Err(SignerError::NotImplemented(msg)) => {
                assert_eq!(msg, "sign_transaction is not implemented".to_string());
            }
            _ => panic!("Expected SignerError::NotImplemented"),
        }
    }
}
