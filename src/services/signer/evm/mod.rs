//! EVM signer implementation for managing Ethereum-compatible private keys and signing operations.
//!
//! Provides:
//! - Local keystore support (encrypted JSON files)
//! - Turnkey (Turnkey backend)
//!
//! # Architecture
//!
//! ```text
//! EvmSigner
//!   ├── TestSigner (Temporary testing private key)
//!   ├── LocalSigner (encrypted JSON keystore)
//!   ├── AwsKmsSigner (AWS KMS backend) [NOT IMPLEMENTED]
//!   ├── Vault (HashiCorp Vault backend)
//!   ├── VaultCould (HashiCorp Vault backend)
//!   └── Turnkey (Turnkey backend)
//! ```
mod local_signer;
mod turnkey_signer;
use local_signer::*;
use turnkey_signer::*;

use async_trait::async_trait;
use color_eyre::config;

use crate::{
    domain::{
        SignDataRequest, SignDataResponse, SignDataResponseEvm, SignTransactionResponse,
        SignTypedDataRequest,
    },
    models::{
        Address, NetworkTransactionData, SignerConfig, SignerRepoModel, SignerType,
        TransactionRepoModel,
    },
    services::{turnkey::TurnkeyService, GoogleCloudKmsService, TurnkeyServiceTrait},
};
use eyre::Result;

use super::{Signer, SignerError, SignerFactoryError};

#[async_trait]
pub trait DataSignerTrait: Send + Sync {
    /// Signs arbitrary message data
    async fn sign_data(&self, request: SignDataRequest) -> Result<SignDataResponse, SignerError>;

    /// Signs EIP-712 typed data
    async fn sign_typed_data(
        &self,
        request: SignTypedDataRequest,
    ) -> Result<SignDataResponse, SignerError>;
}

pub enum EvmSigner {
    Local(LocalSigner),
    Vault(LocalSigner),
    VaultCloud(LocalSigner),
    Turnkey(TurnkeySigner),
}

#[async_trait]
impl Signer for EvmSigner {
    async fn address(&self) -> Result<Address, SignerError> {
        match self {
            Self::Local(signer) => signer.address().await,
            Self::Vault(signer) => signer.address().await,
            Self::VaultCloud(signer) => signer.address().await,
            Self::Turnkey(signer) => signer.address().await,
        }
    }

    async fn sign_transaction(
        &self,
        transaction: NetworkTransactionData,
    ) -> Result<SignTransactionResponse, SignerError> {
        match self {
            Self::Local(signer) => signer.sign_transaction(transaction).await,
            Self::Vault(signer) => signer.sign_transaction(transaction).await,
            Self::VaultCloud(signer) => signer.sign_transaction(transaction).await,
            Self::Turnkey(signer) => signer.sign_transaction(transaction).await,
        }
    }
}

#[async_trait]
impl DataSignerTrait for EvmSigner {
    async fn sign_data(&self, request: SignDataRequest) -> Result<SignDataResponse, SignerError> {
        match self {
            Self::Local(signer) => signer.sign_data(request).await,
            Self::Vault(signer) => signer.sign_data(request).await,
            Self::VaultCloud(signer) => signer.sign_data(request).await,
            Self::Turnkey(signer) => signer.sign_data(request).await,
        }
    }

    async fn sign_typed_data(
        &self,
        request: SignTypedDataRequest,
    ) -> Result<SignDataResponse, SignerError> {
        match self {
            Self::Local(signer) => signer.sign_typed_data(request).await,
            Self::Vault(signer) => signer.sign_typed_data(request).await,
            Self::VaultCloud(signer) => signer.sign_typed_data(request).await,
            Self::Turnkey(signer) => signer.sign_typed_data(request).await,
        }
    }
}

pub struct EvmSignerFactory;

impl EvmSignerFactory {
    pub fn create_evm_signer(
        signer_model: &SignerRepoModel,
    ) -> Result<EvmSigner, SignerFactoryError> {
        let signer = match signer_model.config {
            SignerConfig::Local(_)
            | SignerConfig::Test(_)
            | SignerConfig::Vault(_)
            | SignerConfig::VaultCloud(_) => EvmSigner::Local(LocalSigner::new(signer_model)?),
            SignerConfig::AwsKms(_) => {
                return Err(SignerFactoryError::UnsupportedType("AWS KMS".into()));
            }
            SignerConfig::VaultTransit(_) => {
                return Err(SignerFactoryError::UnsupportedType("Vault Transit".into()));
            }
            SignerConfig::Turnkey(ref config) => {
                let turnkey_service = TurnkeyService::new(config.clone()).map_err(|e| {
                    SignerFactoryError::CreationFailed(format!("Turnkey service error: {}", e))
                })?;
                EvmSigner::Turnkey(TurnkeySigner::new(turnkey_service))
            }
            SignerConfig::GoogleCloudKms(_) => {
                return Err(SignerFactoryError::UnsupportedType(
                    "Google Cloud KMS".into(),
                ));
            }
        };

        Ok(signer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        AwsKmsSignerConfig, EvmTransactionData, GoogleCloudKmsSignerConfig,
        GoogleCloudKmsSignerKeyConfig, GoogleCloudKmsSignerServiceAccountConfig, LocalSignerConfig,
        SecretString, SignerConfig, SignerRepoModel, TurnkeySignerConfig, VaultTransitSignerConfig,
        U256,
    };
    use mockall::predicate::*;
    use secrets::SecretVec;
    use std::str::FromStr;
    use std::sync::Arc;

    fn test_key_bytes() -> SecretVec<u8> {
        let key_bytes =
            hex::decode("0000000000000000000000000000000000000000000000000000000000000001")
                .unwrap();
        SecretVec::new(key_bytes.len(), |v| v.copy_from_slice(&key_bytes))
    }

    fn test_key_address() -> Address {
        Address::Evm([
            126, 95, 69, 82, 9, 26, 105, 18, 93, 93, 252, 183, 184, 194, 101, 144, 41, 57, 91, 223,
        ])
    }

    #[test]
    fn test_create_evm_signer_local() {
        let signer_model = SignerRepoModel {
            id: "test".to_string(),
            config: SignerConfig::Local(LocalSignerConfig {
                raw_key: test_key_bytes(),
            }),
        };

        let signer = EvmSignerFactory::create_evm_signer(&signer_model).unwrap();

        match signer {
            EvmSigner::Local(_) => {}
            _ => panic!("Expected Local signer"),
        }
    }

    #[test]
    fn test_create_evm_signer_test() {
        let signer_model = SignerRepoModel {
            id: "test".to_string(),
            config: SignerConfig::Test(LocalSignerConfig {
                raw_key: test_key_bytes(),
            }),
        };

        let signer = EvmSignerFactory::create_evm_signer(&signer_model).unwrap();

        match signer {
            EvmSigner::Local(_) => {}
            _ => panic!("Expected Local signer"),
        }
    }

    #[test]
    fn test_create_evm_signer_vault() {
        let signer_model = SignerRepoModel {
            id: "test".to_string(),
            config: SignerConfig::Vault(LocalSignerConfig {
                raw_key: test_key_bytes(),
            }),
        };

        let signer = EvmSignerFactory::create_evm_signer(&signer_model).unwrap();

        match signer {
            EvmSigner::Local(_) => {}
            _ => panic!("Expected Local Vault signer"),
        }
    }

    #[test]
    fn test_create_evm_signer_vault_cloud() {
        let signer_model = SignerRepoModel {
            id: "test".to_string(),
            config: SignerConfig::VaultCloud(LocalSignerConfig {
                raw_key: test_key_bytes(),
            }),
        };

        let signer = EvmSignerFactory::create_evm_signer(&signer_model).unwrap();

        match signer {
            EvmSigner::Local(_) => {}
            _ => panic!("Expected Local VaultCloud signer"),
        }
    }

    #[test]
    fn test_create_evm_signer_aws_kms() {
        let signer_model = SignerRepoModel {
            id: "test".to_string(),
            config: SignerConfig::AwsKms(AwsKmsSignerConfig {}),
        };

        let result = EvmSignerFactory::create_evm_signer(&signer_model);

        match result {
            Err(SignerFactoryError::UnsupportedType(msg)) => {
                assert_eq!(msg, "AWS KMS");
            }
            _ => panic!("Expected UnsupportedType error"),
        }
    }

    #[test]
    fn test_create_evm_signer_vault_transit() {
        let signer_model = SignerRepoModel {
            id: "test".to_string(),
            config: SignerConfig::VaultTransit(VaultTransitSignerConfig {
                key_name: "test".to_string(),
                address: "address".to_string(),
                namespace: None,
                role_id: SecretString::new("test-role"),
                secret_id: SecretString::new("test-secret"),
                pubkey: "pubkey".to_string(),
                mount_point: None,
            }),
        };

        let result = EvmSignerFactory::create_evm_signer(&signer_model);

        match result {
            Err(SignerFactoryError::UnsupportedType(msg)) => {
                assert_eq!(msg, "Vault Transit");
            }
            _ => panic!("Expected UnsupportedType error"),
        }
    }

    #[test]
    fn test_create_evm_signer_turnkey() {
        let signer_model = SignerRepoModel {
            id: "test".to_string(),
            config: SignerConfig::Turnkey(TurnkeySignerConfig {
                api_private_key: SecretString::new("api_private_key"),
                api_public_key: "api_public_key".to_string(),
                organization_id: "organization_id".to_string(),
                private_key_id: "private_key_id".to_string(),
                public_key: "public_key".to_string(),
            }),
        };

        let result = EvmSignerFactory::create_evm_signer(&signer_model).unwrap();

        match result {
            EvmSigner::Turnkey(_) => {}
            _ => panic!("Expected UnsupportedType error"),
        }
    }

    #[test]
    fn test_create_evm_signer_google_cloud_kms() {
        let signer_model = SignerRepoModel {
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
                    key_id: "id".to_string(),
                    key_ring_id: "key_ring".to_string(),
                    key_version: 1,
                },
            }),
        };

        let result = EvmSignerFactory::create_evm_signer(&signer_model);

        match result {
            Err(SignerFactoryError::UnsupportedType(msg)) => {
                assert_eq!(msg, "Google Cloud KMS");
            }
            _ => panic!("Expected UnsupportedType error"),
        }
    }

    #[tokio::test]
    async fn test_address_evm_signer_local() {
        let signer_model = SignerRepoModel {
            id: "test".to_string(),
            config: SignerConfig::Local(LocalSignerConfig {
                raw_key: test_key_bytes(),
            }),
        };

        let signer = EvmSignerFactory::create_evm_signer(&signer_model).unwrap();
        let signer_address = signer.address().await.unwrap();

        assert_eq!(test_key_address(), signer_address);
    }

    #[tokio::test]
    async fn test_address_evm_signer_test() {
        let signer_model = SignerRepoModel {
            id: "test".to_string(),
            config: SignerConfig::Test(LocalSignerConfig {
                raw_key: test_key_bytes(),
            }),
        };

        let signer = EvmSignerFactory::create_evm_signer(&signer_model).unwrap();
        let signer_address = signer.address().await.unwrap();

        assert_eq!(test_key_address(), signer_address);
    }

    #[tokio::test]
    async fn test_address_evm_signer_vault() {
        let signer_model = SignerRepoModel {
            id: "test".to_string(),
            config: SignerConfig::Vault(LocalSignerConfig {
                raw_key: test_key_bytes(),
            }),
        };

        let signer = EvmSignerFactory::create_evm_signer(&signer_model).unwrap();
        let signer_address = signer.address().await.unwrap();

        assert_eq!(test_key_address(), signer_address);
    }

    #[tokio::test]
    async fn test_address_evm_signer_vault_cloud() {
        let signer_model = SignerRepoModel {
            id: "test".to_string(),
            config: SignerConfig::VaultCloud(LocalSignerConfig {
                raw_key: test_key_bytes(),
            }),
        };

        let signer = EvmSignerFactory::create_evm_signer(&signer_model).unwrap();
        let signer_address = signer.address().await.unwrap();

        assert_eq!(test_key_address(), signer_address);
    }

    #[tokio::test]
    async fn test_address_evm_signer_turnkey() {
        let signer_model = SignerRepoModel {
            id: "test".to_string(),
            config: SignerConfig::Turnkey(TurnkeySignerConfig {
                api_private_key: SecretString::new("api_private_key"),
                api_public_key: "api_public_key".to_string(),
                organization_id: "organization_id".to_string(),
                private_key_id: "private_key_id".to_string(),
                public_key: "047d3bb8e0317927700cf19fed34e0627367be1390ec247dddf8c239e4b4321a49aea80090e49b206b6a3e577a4f11d721ab063482001ee10db40d6f2963233eec".to_string(),
            }),
        };

        let signer = EvmSignerFactory::create_evm_signer(&signer_model).unwrap();
        let signer_address = signer.address().await.unwrap();

        assert_eq!(
            "0xb726167dc2ef2ac582f0a3de4c08ac4abb90626a",
            signer_address.to_string()
        );
    }

    #[tokio::test]
    async fn test_sign_data_evm_signer_local() {
        let signer_model = SignerRepoModel {
            id: "test".to_string(),
            config: SignerConfig::Local(LocalSignerConfig {
                raw_key: test_key_bytes(),
            }),
        };

        let signer = EvmSignerFactory::create_evm_signer(&signer_model).unwrap();
        let request = SignDataRequest {
            message: "Test message".to_string(),
        };

        let result = signer.sign_data(request).await;

        assert!(result.is_ok());

        match result.unwrap() {
            SignDataResponse::Evm(sig) => {
                assert_eq!(sig.r.len(), 64); // 32 bytes in hex
                assert_eq!(sig.s.len(), 64); // 32 bytes in hex
                assert!(sig.v == 27 || sig.v == 28); // Valid v values
                assert_eq!(sig.sig.len(), 130); // 65 bytes in hex
            }
            _ => panic!("Expected EVM signature"),
        }
    }

    #[tokio::test]
    async fn test_sign_transaction_evm() {
        let signer_model = SignerRepoModel {
            id: "test".to_string(),
            config: SignerConfig::Local(LocalSignerConfig {
                raw_key: test_key_bytes(),
            }),
        };

        let signer = EvmSignerFactory::create_evm_signer(&signer_model).unwrap();

        let transaction_data = NetworkTransactionData::Evm(EvmTransactionData {
            from: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            to: Some("0x742d35Cc6634C0532925a3b844Bc454e4438f44f".to_string()),
            gas_price: Some(20000000000),
            gas_limit: 21000,
            nonce: Some(0),
            value: U256::from(1000000000000000000u64),
            data: Some("0x".to_string()),
            chain_id: 1,
            hash: None,
            signature: None,
            raw: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            speed: None,
        });

        let result = signer.sign_transaction(transaction_data).await;

        assert!(result.is_ok());

        let signed_tx = result.unwrap();

        match signed_tx {
            SignTransactionResponse::Evm(signed_tx) => {
                assert!(!signed_tx.hash.is_empty());
                assert!(!signed_tx.raw.is_empty());
                assert!(!signed_tx.signature.sig.is_empty());
            }
            _ => panic!("Expected EVM transaction response"),
        }
    }
}
