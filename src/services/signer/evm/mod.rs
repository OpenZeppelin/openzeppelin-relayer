//! EVM signer implementation for managing Ethereum-compatible private keys and signing operations.
//!
//! Provides:
//! - Local keystore support (encrypted JSON files)
//! - Turnkey (Turnkey backend)
//! - AWS KMS support
//!
//! # Architecture
//!
//! ```text
//! EvmSigner
//!   ├── TestSigner (Temporary testing private key)
//!   ├── LocalSigner (encrypted JSON keystore)
//!   ├── AwsKmsSigner (AWS KMS backend)
//!   ├── Vault (HashiCorp Vault backend)
//!   ├── VaultCould (HashiCorp Vault backend)
//!   └── Turnkey (Turnkey backend)
//! ```
mod aws_kms_signer;
mod google_cloud_kms_signer;
mod local_signer;
mod turnkey_signer;
use aws_kms_signer::*;
use google_cloud_kms_signer::*;
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
    services::{
        turnkey::TurnkeyService, AwsKmsService, GoogleCloudKmsService, TurnkeyServiceTrait,
    },
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
    AwsKms(AwsKmsSigner),
    GoogleCloudKms(GoogleCloudKmsSigner),
}

#[async_trait]
impl Signer for EvmSigner {
    async fn address(&self) -> Result<Address, SignerError> {
        match self {
            Self::Local(signer) => signer.address().await,
            Self::Vault(signer) => signer.address().await,
            Self::VaultCloud(signer) => signer.address().await,
            Self::Turnkey(signer) => signer.address().await,
            Self::AwsKms(signer) => signer.address().await,
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
            Self::VaultCloud(signer) => signer.sign_transaction(transaction).await,
            Self::Turnkey(signer) => signer.sign_transaction(transaction).await,
            Self::AwsKms(signer) => signer.sign_transaction(transaction).await,
            Self::GoogleCloudKms(signer) => signer.sign_transaction(transaction).await,
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
            Self::AwsKms(signer) => signer.sign_data(request).await,
            Self::GoogleCloudKms(signer) => signer.sign_data(request).await,
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
            Self::AwsKms(signer) => signer.sign_typed_data(request).await,
            Self::GoogleCloudKms(signer) => signer.sign_typed_data(request).await,
        }
    }
}

pub struct EvmSignerFactory;

impl EvmSignerFactory {
    pub async fn create_evm_signer(
        signer_model: SignerRepoModel,
    ) -> Result<EvmSigner, SignerFactoryError> {
        let signer = match signer_model.config {
            SignerConfig::Local(_)
            | SignerConfig::Test(_)
            | SignerConfig::Vault(_)
            | SignerConfig::VaultCloud(_) => EvmSigner::Local(LocalSigner::new(&signer_model)?),
            SignerConfig::AwsKms(ref config) => {
                let aws_service = AwsKmsService::new(config.clone()).await.map_err(|e| {
                    SignerFactoryError::CreationFailed(format!("AWS KMS service error: {}", e))
                })?;
                EvmSigner::AwsKms(AwsKmsSigner::new(aws_service))
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
            SignerConfig::GoogleCloudKms(ref config) => {
                let gcp_service = GoogleCloudKmsService::new(config).map_err(|e| {
                    SignerFactoryError::CreationFailed(format!(
                        "Google Cloud KMS service error: {}",
                        e
                    ))
                })?;
                EvmSigner::GoogleCloudKms(GoogleCloudKmsSigner::new(gcp_service))
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
    use futures;
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

    #[tokio::test]
    async fn test_create_evm_signer_local() {
        let signer_model = SignerRepoModel {
            id: "test".to_string(),
            config: SignerConfig::Local(LocalSignerConfig {
                raw_key: test_key_bytes(),
            }),
        };

        let signer = EvmSignerFactory::create_evm_signer(signer_model)
            .await
            .unwrap();

        assert!(matches!(signer, EvmSigner::Local(_)));
    }

    #[tokio::test]
    async fn test_create_evm_signer_test() {
        let signer_model = SignerRepoModel {
            id: "test".to_string(),
            config: SignerConfig::Test(LocalSignerConfig {
                raw_key: test_key_bytes(),
            }),
        };

        let signer = EvmSignerFactory::create_evm_signer(signer_model)
            .await
            .unwrap();

        assert!(matches!(signer, EvmSigner::Local(_)));
    }

    #[tokio::test]
    async fn test_create_evm_signer_vault() {
        let signer_model = SignerRepoModel {
            id: "test".to_string(),
            config: SignerConfig::Vault(LocalSignerConfig {
                raw_key: test_key_bytes(),
            }),
        };

        let signer = EvmSignerFactory::create_evm_signer(signer_model)
            .await
            .unwrap();

        assert!(matches!(signer, EvmSigner::Local(_)));
    }

    #[tokio::test]
    async fn test_create_evm_signer_vault_cloud() {
        let signer_model = SignerRepoModel {
            id: "test".to_string(),
            config: SignerConfig::VaultCloud(LocalSignerConfig {
                raw_key: test_key_bytes(),
            }),
        };

        let signer = EvmSignerFactory::create_evm_signer(signer_model)
            .await
            .unwrap();

        assert!(matches!(signer, EvmSigner::Local(_)));
    }

    #[tokio::test]
    async fn test_create_evm_signer_aws_kms() {
        let signer_model = SignerRepoModel {
            id: "test".to_string(),
            config: SignerConfig::AwsKms(AwsKmsSignerConfig {
                region: Some("us-east-1".to_string()),
                key_id: "test-key-id".to_string(),
            }),
        };

        let signer = EvmSignerFactory::create_evm_signer(signer_model)
            .await
            .unwrap();

        assert!(matches!(signer, EvmSigner::AwsKms(_)));
    }

    #[tokio::test]
    async fn test_create_evm_signer_vault_transit() {
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

        let result = EvmSignerFactory::create_evm_signer(signer_model).await;

        assert!(matches!(
            result,
            Err(SignerFactoryError::UnsupportedType(_))
        ));
    }

    #[tokio::test]
    async fn test_create_evm_signer_turnkey() {
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

        let signer = EvmSignerFactory::create_evm_signer(signer_model)
            .await
            .unwrap();
        let signer_address = signer.address().await.unwrap();

        assert_eq!(
            "0xb726167dc2ef2ac582f0a3de4c08ac4abb90626a",
            signer_address.to_string()
        );
    }

    #[tokio::test]
    async fn test_address_evm_signer_local() {
        let signer_model = SignerRepoModel {
            id: "test".to_string(),
            config: SignerConfig::Local(LocalSignerConfig {
                raw_key: test_key_bytes(),
            }),
        };

        let signer = EvmSignerFactory::create_evm_signer(signer_model)
            .await
            .unwrap();
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

        let signer = EvmSignerFactory::create_evm_signer(signer_model)
            .await
            .unwrap();
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

        let signer = EvmSignerFactory::create_evm_signer(signer_model)
            .await
            .unwrap();
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

        let signer = EvmSignerFactory::create_evm_signer(signer_model)
            .await
            .unwrap();
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

        let signer = EvmSignerFactory::create_evm_signer(signer_model)
            .await
            .unwrap();
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

        let signer = EvmSignerFactory::create_evm_signer(signer_model)
            .await
            .unwrap();
        let request = SignDataRequest {
            message: "Test message".to_string(),
        };

        let result = signer.sign_data(request).await;

        assert!(result.is_ok());

        let response = result.unwrap();
        assert!(matches!(response, SignDataResponse::Evm(_)));

        if let SignDataResponse::Evm(sig) = response {
            assert_eq!(sig.r.len(), 64); // 32 bytes in hex
            assert_eq!(sig.s.len(), 64); // 32 bytes in hex
            assert!(sig.v == 27 || sig.v == 28); // Valid v values
            assert_eq!(sig.sig.len(), 130); // 65 bytes in hex
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

        let signer = EvmSignerFactory::create_evm_signer(signer_model)
            .await
            .unwrap();

        let transaction_data = NetworkTransactionData::Evm(EvmTransactionData {
            from: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            to: Some("0x742d35Cc6634C0532925a3b844Bc454e4438f44f".to_string()),
            gas_price: Some(20000000000),
            gas_limit: Some(21000),
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

        assert!(matches!(signed_tx, SignTransactionResponse::Evm(_)));

        if let SignTransactionResponse::Evm(evm_tx) = signed_tx {
            assert!(!evm_tx.hash.is_empty());
            assert!(!evm_tx.raw.is_empty());
            assert!(!evm_tx.signature.sig.is_empty());
        }
    }

    #[tokio::test]
    async fn test_create_evm_signer_google_cloud_kms() {
        let signer_model = SignerRepoModel {
            id: "test".to_string(),
            config: SignerConfig::GoogleCloudKms(GoogleCloudKmsSignerConfig {
                service_account: GoogleCloudKmsSignerServiceAccountConfig {
                    project_id: "project_id".to_string(),
                    private_key_id: SecretString::new("private_key_id"),
                    private_key: SecretString::new("-----BEGIN EXAMPLE PRIVATE KEY-----\nFAKEKEYDATA\n-----END EXAMPLE PRIVATE KEY-----\n"),
                    client_email: SecretString::new("client_email@example.com"),
                    client_id: "client_id".to_string(),
                    auth_uri: "https://accounts.google.com/o/oauth2/auth".to_string(),
                    token_uri: "https://oauth2.googleapis.com/token".to_string(),
                    auth_provider_x509_cert_url: "https://www.googleapis.com/oauth2/v1/certs".to_string(),
                    client_x509_cert_url: "https://www.googleapis.com/robot/v1/metadata/x509/client_email%40example.com".to_string(),
                    universe_domain: "googleapis.com".to_string(),
                },
                key: GoogleCloudKmsSignerKeyConfig {
                    location: "global".to_string(),
                    key_id: "id".to_string(),
                    key_ring_id: "key_ring".to_string(),
                    key_version: 1,
                },
            }),
        };

        let result = EvmSignerFactory::create_evm_signer(signer_model).await;

        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), EvmSigner::GoogleCloudKms(_)));
    }

    #[tokio::test]
    async fn test_sign_data_with_different_message_types() {
        let signer_model = SignerRepoModel {
            id: "test".to_string(),
            config: SignerConfig::Local(LocalSignerConfig {
                raw_key: test_key_bytes(),
            }),
        };

        let signer = EvmSignerFactory::create_evm_signer(signer_model)
            .await
            .unwrap();

        // Test with various message types
        let long_message = "a".repeat(1000);
        let test_cases = vec![
            ("Simple message", "Test message"),
            ("Empty message", ""),
            ("Unicode message", "🚀 Test message with émojis"),
            ("Long message", long_message.as_str()),
            ("JSON message", r#"{"test": "value", "number": 123}"#),
        ];

        for (name, message) in test_cases {
            let request = SignDataRequest {
                message: message.to_string(),
            };

            let result = signer.sign_data(request).await;
            assert!(result.is_ok(), "Failed to sign {}", name);

            if let Ok(SignDataResponse::Evm(sig)) = result {
                assert_eq!(sig.r.len(), 64, "Invalid r length for {}", name);
                assert_eq!(sig.s.len(), 64, "Invalid s length for {}", name);
                assert!(sig.v == 27 || sig.v == 28, "Invalid v value for {}", name);
                assert_eq!(sig.sig.len(), 130, "Invalid signature length for {}", name);
            } else {
                panic!("Expected EVM signature for {}", name);
            }
        }
    }

    #[tokio::test]
    async fn test_sign_data_with_vault_signer() {
        let signer_model = SignerRepoModel {
            id: "test".to_string(),
            config: SignerConfig::Vault(LocalSignerConfig {
                raw_key: test_key_bytes(),
            }),
        };

        let signer = EvmSignerFactory::create_evm_signer(signer_model)
            .await
            .unwrap();

        let request = SignDataRequest {
            message: "Test vault message".to_string(),
        };

        let result = signer.sign_data(request).await;
        assert!(result.is_ok());

        if let Ok(SignDataResponse::Evm(sig)) = result {
            assert_eq!(sig.r.len(), 64);
            assert_eq!(sig.s.len(), 64);
            assert!(sig.v == 27 || sig.v == 28);
            assert_eq!(sig.sig.len(), 130);
        } else {
            panic!("Expected successful EVM signature");
        }
    }

    #[tokio::test]
    async fn test_sign_transaction_with_vault_cloud_signer() {
        let signer_model = SignerRepoModel {
            id: "test".to_string(),
            config: SignerConfig::VaultCloud(LocalSignerConfig {
                raw_key: test_key_bytes(),
            }),
        };

        let signer = EvmSignerFactory::create_evm_signer(signer_model)
            .await
            .unwrap();

        let transaction_data = NetworkTransactionData::Evm(EvmTransactionData {
            from: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            to: Some("0x742d35Cc6634C0532925a3b844Bc454e4438f44f".to_string()),
            gas_price: Some(20000000000),
            gas_limit: Some(21000),
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

        if let Ok(SignTransactionResponse::Evm(evm_tx)) = result {
            assert!(!evm_tx.hash.is_empty());
            assert!(!evm_tx.raw.is_empty());
            assert!(!evm_tx.signature.sig.is_empty());
        } else {
            panic!("Expected successful EVM transaction signing");
        }
    }

    #[tokio::test]
    async fn test_address_consistency_across_vault_types() {
        // Test that different vault types using the same key produce the same address
        let key_bytes = test_key_bytes();

        let signers = vec![
            EvmSignerFactory::create_evm_signer(SignerRepoModel {
                id: "local".to_string(),
                config: SignerConfig::Local(LocalSignerConfig {
                    raw_key: key_bytes.clone(),
                }),
            })
            .await
            .unwrap(),
            EvmSignerFactory::create_evm_signer(SignerRepoModel {
                id: "vault".to_string(),
                config: SignerConfig::Vault(LocalSignerConfig {
                    raw_key: key_bytes.clone(),
                }),
            })
            .await
            .unwrap(),
            EvmSignerFactory::create_evm_signer(SignerRepoModel {
                id: "vault_cloud".to_string(),
                config: SignerConfig::VaultCloud(LocalSignerConfig { raw_key: key_bytes }),
            })
            .await
            .unwrap(),
        ];

        let addresses: Vec<Address> =
            futures::future::try_join_all(signers.iter().map(|s| s.address()))
                .await
                .unwrap();

        // All addresses should be identical since they use the same key
        assert_eq!(addresses[0], addresses[1]);
        assert_eq!(addresses[1], addresses[2]);
        assert_eq!(addresses[0], test_key_address());
    }

    #[tokio::test]
    async fn test_transaction_signing_with_different_vault_types() {
        // Test that different vault configurations can sign transactions correctly
        let transaction_data = NetworkTransactionData::Evm(EvmTransactionData {
            from: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            to: Some("0x742d35Cc6634C0532925a3b844Bc454e4438f44f".to_string()),
            gas_price: Some(20000000000),
            gas_limit: Some(21000),
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

        let vault_configs = vec![
            (
                "vault",
                SignerConfig::Vault(LocalSignerConfig {
                    raw_key: test_key_bytes(),
                }),
            ),
            (
                "vault_cloud",
                SignerConfig::VaultCloud(LocalSignerConfig {
                    raw_key: test_key_bytes(),
                }),
            ),
        ];

        for (name, config) in vault_configs {
            let signer_model = SignerRepoModel {
                id: name.to_string(),
                config,
            };

            let signer = EvmSignerFactory::create_evm_signer(signer_model)
                .await
                .unwrap();

            let result = signer.sign_transaction(transaction_data.clone()).await;
            assert!(result.is_ok(), "Failed to sign transaction with {}", name);

            if let Ok(SignTransactionResponse::Evm(evm_tx)) = result {
                assert!(!evm_tx.hash.is_empty(), "Empty hash for {}", name);
                assert!(!evm_tx.raw.is_empty(), "Empty raw for {}", name);
                assert!(
                    !evm_tx.signature.sig.is_empty(),
                    "Empty signature for {}",
                    name
                );
            } else {
                panic!("Expected EVM transaction response for {}", name);
            }
        }
    }
}
