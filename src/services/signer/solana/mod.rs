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

mod cdp_signer;
use cdp_signer::*;

mod google_cloud_kms_signer;
use google_cloud_kms_signer::*;

use solana_program::message::compiled_instruction::CompiledInstruction;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature;
use solana_sdk::transaction::Transaction as SolanaTransaction;
use std::str::FromStr;

use solana_system_interface::instruction as system_instruction;

use crate::{
    domain::{
        SignDataRequest, SignDataResponse, SignDataResponseEvm, SignTransactionResponse,
        SignTransactionResponseSolana, SignTypedDataRequest,
    },
    models::{
        Address, EncodedSerializedTransaction, NetworkTransactionData, Signer as SignerDomainModel,
        SignerConfig, SignerRepoModel, SignerType, TransactionRepoModel, VaultSignerConfig,
    },
    services::{CdpService, GoogleCloudKmsService, TurnkeyService, VaultConfig, VaultService},
};
use eyre::Result;

use super::{Signer, SignerError, SignerFactoryError};
#[cfg(test)]
use mockall::automock;

#[derive(Debug)]
pub enum SolanaSigner {
    Local(LocalSigner),
    Vault(VaultSigner<VaultService>),
    VaultTransit(VaultTransitSigner),
    Turnkey(TurnkeySigner),
    Cdp(CdpSigner),
    GoogleCloudKms(GoogleCloudKmsSigner),
}

#[async_trait]
impl Signer for SolanaSigner {
    async fn address(&self) -> Result<Address, SignerError> {
        // Delegate to SolanaSignTrait::pubkey() which all inner types implement
        self.pubkey().await
    }

    async fn sign_transaction(
        &self,
        transaction: NetworkTransactionData,
    ) -> Result<SignTransactionResponse, SignerError> {
        // Extract Solana transaction data
        let solana_data = transaction.get_solana_transaction_data().map_err(|e| {
            SignerError::SigningError(format!("Invalid transaction type for Solana signer: {e}"))
        })?;

        // Get the pre-built transaction string
        let transaction_str = solana_data.transaction.ok_or_else(|| {
            SignerError::SigningError(
                "Transaction not yet built - only available after preparation".to_string(),
            )
        })?;

        // Decode transaction from base64
        let encoded_tx = EncodedSerializedTransaction::new(transaction_str);
        let sdk_transaction = SolanaTransaction::try_from(encoded_tx)
            .map_err(|e| SignerError::SigningError(format!("Failed to decode transaction: {e}")))?;

        // Sign using the SDK transaction signing helper function
        let (signed_tx, signature) = sign_sdk_transaction(self, sdk_transaction).await?;

        // Encode back to base64
        let encoded_signed_tx =
            EncodedSerializedTransaction::try_from(&signed_tx).map_err(|e| {
                SignerError::SigningError(format!("Failed to encode signed transaction: {e}"))
            })?;

        // Return Solana-specific response
        Ok(SignTransactionResponse::Solana(
            SignTransactionResponseSolana {
                transaction: encoded_signed_tx,
                signature: signature.to_string(),
            },
        ))
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

/// Signs a raw Solana SDK transaction by finding the signer's position and adding the signature
///
/// This helper function:
/// 1. Retrieves the signer's public key
/// 2. Finds its position in the transaction's account_keys
/// 3. Validates it's marked as a required signer
/// 4. Signs the transaction message
/// 5. Inserts the signature at the correct position
///
/// # Arguments
///
/// * `signer` - A type implementing SolanaSignTrait
/// * `transaction` - The Solana SDK transaction to sign
///
/// # Returns
///
/// A Result containing either a tuple of (signed Transaction, Signature) or a SignerError
///
/// # Note
///
/// This is distinct from the `Signer::sign_transaction` method which operates on domain models.
/// This function works directly with `solana_sdk::transaction::Transaction`.
pub async fn sign_sdk_transaction<T: SolanaSignTrait + ?Sized>(
    signer: &T,
    mut transaction: solana_sdk::transaction::Transaction,
) -> Result<(solana_sdk::transaction::Transaction, Signature), SignerError> {
    // Get signer's public key
    let signer_address = signer.pubkey().await?;
    let signer_pubkey = Pubkey::from_str(&signer_address.to_string())
        .map_err(|e| SignerError::KeyError(format!("Invalid signer address: {e}")))?;

    // Find the position of the signer's public key in account_keys
    let signer_index = transaction
        .message
        .account_keys
        .iter()
        .position(|key| *key == signer_pubkey)
        .ok_or_else(|| {
            SignerError::SigningError(
                "Signer public key not found in transaction signers".to_string(),
            )
        })?;

    // Check if this is a signer position (within num_required_signatures)
    if signer_index >= transaction.message.header.num_required_signatures as usize {
        return Err(SignerError::SigningError(format!(
            "Signer is not marked as a required signer in the transaction (position {} >= {})",
            signer_index, transaction.message.header.num_required_signatures
        )));
    }

    // Generate signature
    let signature = signer.sign(&transaction.message_data()).await?;

    // Ensure signatures array has exactly num_required_signatures slots
    // This preserves any existing signatures and doesn't shrink the array
    let num_required = transaction.message.header.num_required_signatures as usize;
    transaction
        .signatures
        .resize(num_required, Signature::default());

    // Set our signature at the correct index
    transaction.signatures[signer_index] = signature;

    Ok((transaction, signature))
}

#[async_trait]
impl SolanaSignTrait for SolanaSigner {
    async fn pubkey(&self) -> Result<Address, SignerError> {
        match self {
            Self::Local(signer) => signer.pubkey().await,
            Self::Vault(signer) => signer.pubkey().await,
            Self::VaultTransit(signer) => signer.pubkey().await,
            Self::Turnkey(signer) => signer.pubkey().await,
            Self::Cdp(signer) => signer.pubkey().await,
            Self::GoogleCloudKms(signer) => signer.pubkey().await,
        }
    }

    async fn sign(&self, message: &[u8]) -> Result<Signature, SignerError> {
        match self {
            Self::Local(signer) => Ok(signer.sign(message).await?),
            Self::Vault(signer) => Ok(signer.sign(message).await?),
            Self::VaultTransit(signer) => Ok(signer.sign(message).await?),
            Self::Turnkey(signer) => Ok(signer.sign(message).await?),
            Self::Cdp(signer) => Ok(signer.sign(message).await?),
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
            SignerConfig::Cdp(config) => {
                let cdp_signer = CdpSigner::new(config.clone()).map_err(|e| {
                    SignerFactoryError::CreationFailed(format!("CDP service error: {e}"))
                })?;
                return Ok(SolanaSigner::Cdp(cdp_signer));
            }
            SignerConfig::Turnkey(turnkey_signer_config) => {
                let turnkey_service =
                    TurnkeyService::new(turnkey_signer_config.clone()).map_err(|e| {
                        SignerFactoryError::InvalidConfig(format!(
                            "Failed to create Turnkey service: {e}"
                        ))
                    })?;

                return Ok(SolanaSigner::Turnkey(TurnkeySigner::new(turnkey_service)));
            }
            SignerConfig::GoogleCloudKms(google_cloud_kms_signer_config) => {
                let google_cloud_kms_service =
                    GoogleCloudKmsService::new(google_cloud_kms_signer_config).map_err(|e| {
                        SignerFactoryError::InvalidConfig(format!(
                            "Failed to create Google Cloud KMS service: {e}"
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
        AwsKmsSignerConfig, CdpSignerConfig, GoogleCloudKmsSignerConfig,
        GoogleCloudKmsSignerKeyConfig, GoogleCloudKmsSignerServiceAccountConfig, LocalSignerConfig,
        SecretString, SignerConfig, SignerRepoModel, SolanaTransactionData, TurnkeySignerConfig,
        VaultSignerConfig, VaultTransitSignerConfig,
    };
    use mockall::predicate::*;
    use secrets::SecretVec;
    use std::str::FromStr;
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

    #[test]
    fn test_create_solana_signer_cdp() {
        let signer_model = SignerDomainModel {
            id: "test".to_string(),
            config: SignerConfig::Cdp(CdpSignerConfig {
                api_key_id: "test-api-key-id".to_string(),
                api_key_secret: SecretString::new("test-api-key-secret"),
                wallet_secret: SecretString::new("test-wallet-secret"),
                account_address: "6s7RsvzcdXFJi1tXeDoGfSKZFzN3juVt9fTar6WEhEm2".to_string(),
            }),
        };

        let signer = SolanaSignerFactory::create_solana_signer(&signer_model).unwrap();

        match signer {
            SolanaSigner::Cdp(_) => {}
            _ => panic!("Expected CDP signer"),
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
    async fn test_address_solana_signer_cdp() {
        let signer_model = SignerDomainModel {
            id: "test".to_string(),
            config: SignerConfig::Cdp(CdpSignerConfig {
                api_key_id: "test-api-key-id".to_string(),
                api_key_secret: SecretString::new("test-api-key-secret"),
                wallet_secret: SecretString::new("test-wallet-secret"),
                account_address: "6s7RsvzcdXFJi1tXeDoGfSKZFzN3juVt9fTar6WEhEm2".to_string(),
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

    #[tokio::test]
    async fn test_sign_sdk_transaction_success() {
        use solana_sdk::message::Message;
        use solana_sdk::pubkey::Pubkey;
        use solana_sdk::signature::Signature;
        use solana_sdk::transaction::Transaction;

        // Create a mock signer
        let signer_model = SignerDomainModel {
            id: "test".to_string(),
            config: SignerConfig::Local(LocalSignerConfig {
                raw_key: test_key_bytes(),
            }),
        };
        let signer = SolanaSignerFactory::create_solana_signer(&signer_model).unwrap();

        // Create a simple transaction with our signer as the first account
        let signer_pubkey = Pubkey::from_str(&test_key_bytes_pubkey().to_string()).unwrap();
        let recipient = Pubkey::new_unique();

        let message = Message::new(
            &[solana_system_interface::instruction::transfer(
                &signer_pubkey,
                &recipient,
                1000,
            )],
            Some(&signer_pubkey),
        );
        let transaction = Transaction::new_unsigned(message);

        // Sign the transaction
        let result = sign_sdk_transaction(&signer, transaction).await;
        assert!(result.is_ok());

        let (signed_tx, signature) = result.unwrap();
        assert!(!signature.to_string().is_empty());
        assert_eq!(signed_tx.signatures.len(), 1);
        assert_eq!(signed_tx.signatures[0], signature);
    }

    #[tokio::test]
    async fn test_sign_sdk_transaction_signer_not_in_accounts() {
        use solana_sdk::message::Message;
        use solana_sdk::pubkey::Pubkey;
        use solana_sdk::transaction::Transaction;

        // Create a mock signer
        let signer_model = SignerDomainModel {
            id: "test".to_string(),
            config: SignerConfig::Local(LocalSignerConfig {
                raw_key: test_key_bytes(),
            }),
        };
        let signer = SolanaSignerFactory::create_solana_signer(&signer_model).unwrap();

        // Create a transaction where our signer is NOT in the account keys
        let other_pubkey = Pubkey::new_unique();
        let recipient = Pubkey::new_unique();

        let message = Message::new(
            &[solana_system_interface::instruction::transfer(
                &other_pubkey,
                &recipient,
                1000,
            )],
            Some(&other_pubkey),
        );
        let transaction = Transaction::new_unsigned(message);

        // Try to sign - should fail because signer is not in account_keys
        let result = sign_sdk_transaction(&signer, transaction).await;
        assert!(result.is_err());
        let error = result.unwrap_err();
        match error {
            SignerError::SigningError(msg) => {
                assert!(msg.contains("Signer public key not found in transaction signers"));
            }
            _ => panic!("Expected SigningError, got {:?}", error),
        }
    }

    #[tokio::test]
    async fn test_sign_sdk_transaction_signer_not_required() {
        use solana_sdk::message::Message;
        use solana_sdk::pubkey::Pubkey;
        use solana_sdk::transaction::Transaction;

        // Create a mock signer
        let signer_model = SignerDomainModel {
            id: "test".to_string(),
            config: SignerConfig::Local(LocalSignerConfig {
                raw_key: test_key_bytes(),
            }),
        };
        let signer = SolanaSignerFactory::create_solana_signer(&signer_model).unwrap();

        // Create a transaction where our signer is in account_keys but NOT marked as required
        let signer_pubkey = Pubkey::from_str(&test_key_bytes_pubkey().to_string()).unwrap();
        let fee_payer = Pubkey::new_unique();
        let recipient = Pubkey::new_unique();

        // Create message with signer as a readonly account (not required signer)
        // Use a different approach - create a message where signer is not the fee payer
        let message = Message::new(
            &[solana_system_interface::instruction::transfer(
                &fee_payer, &recipient, 1000,
            )],
            Some(&fee_payer),
        );
        let transaction = Transaction::new_unsigned(message);

        // Manually modify the message to include our signer as a readonly account
        // This simulates a transaction where our signer is present but not required
        let mut modified_message = transaction.message.clone();
        modified_message.account_keys.push(signer_pubkey); // Add signer as additional account
        modified_message.header.num_readonly_unsigned_accounts += 1; // Make it readonly unsigned

        let modified_transaction = Transaction::new_unsigned(modified_message);

        // Try to sign - should fail because signer is not a required signer
        let result = sign_sdk_transaction(&signer, modified_transaction).await;
        assert!(result.is_err());
        let error = result.unwrap_err();
        match error {
            SignerError::SigningError(msg) => {
                assert!(msg.contains("Signer is not marked as a required signer"));
            }
            _ => panic!("Expected SigningError, got {:?}", error),
        }
    }

    #[tokio::test]
    async fn test_sign_transaction_with_domain_model() {
        use crate::models::{NetworkTransactionData, SolanaTransactionData};
        use solana_sdk::message::Message;
        use solana_sdk::pubkey::Pubkey;

        // Create a mock signer
        let signer_model = SignerDomainModel {
            id: "test".to_string(),
            config: SignerConfig::Local(LocalSignerConfig {
                raw_key: test_key_bytes(),
            }),
        };
        let signer = SolanaSignerFactory::create_solana_signer(&signer_model).unwrap();

        // Create a domain transaction data
        let signer_pubkey = Pubkey::from_str(&test_key_bytes_pubkey().to_string()).unwrap();
        let recipient = Pubkey::new_unique();

        let message = Message::new(
            &[solana_system_interface::instruction::transfer(
                &signer_pubkey,
                &recipient,
                1000,
            )],
            Some(&signer_pubkey),
        );
        let transaction = solana_sdk::transaction::Transaction::new_unsigned(message);
        let encoded_tx =
            crate::models::EncodedSerializedTransaction::try_from(&transaction).unwrap();

        let solana_data = SolanaTransactionData {
            transaction: Some(encoded_tx.into_inner()),
            ..Default::default()
        };

        let network_data = NetworkTransactionData::Solana(solana_data);

        // Sign using the domain model method
        let result = signer.sign_transaction(network_data).await;
        assert!(result.is_ok());

        let response = result.unwrap();
        match response {
            crate::domain::SignTransactionResponse::Solana(solana_response) => {
                assert!(!solana_response.transaction.into_inner().is_empty());
                assert!(!solana_response.signature.is_empty());
            }
            _ => panic!("Expected Solana response"),
        }
    }

    #[test]
    fn test_create_solana_signer_aws_kms_unsupported() {
        let signer_model = SignerDomainModel {
            id: "test".to_string(),
            config: SignerConfig::AwsKms(AwsKmsSignerConfig {
                region: Some("us-east-1".to_string()),
                key_id: "test-key-id".to_string(),
            }),
        };

        let result = SolanaSignerFactory::create_solana_signer(&signer_model);
        assert!(result.is_err());
        let error = result.unwrap_err();
        match error {
            SignerFactoryError::UnsupportedType(msg) => {
                assert_eq!(msg, "AWS KMS");
            }
            _ => panic!("Expected UnsupportedType error, got {:?}", error),
        }
    }

    #[cfg(test)]
    #[async_trait]
    impl Signer for MockSolanaSignTrait {
        async fn address(&self) -> Result<Address, SignerError> {
            self.pubkey().await
        }

        async fn sign_transaction(
            &self,
            _transaction: NetworkTransactionData,
        ) -> Result<SignTransactionResponse, SignerError> {
            // For testing, return a mock response
            Ok(SignTransactionResponse::Solana(
                crate::domain::SignTransactionResponseSolana {
                    transaction: crate::models::EncodedSerializedTransaction::new(
                        "signed_transaction_data".to_string(),
                    ),
                    signature: "signature_data".to_string(),
                },
            ))
        }
    }
}
