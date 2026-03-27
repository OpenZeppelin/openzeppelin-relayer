// openzeppelin-relayer/src/services/signer/stellar/mod.rs
//! Stellar signer implementation (local keystore, Google Cloud KMS, AWS KMS, and Turnkey)

mod aws_kms_signer;
mod google_cloud_kms_signer;
mod local_signer;
mod turnkey_signer;
mod vault_signer;

use async_trait::async_trait;
use aws_kms_signer::*;
use google_cloud_kms_signer::*;
use local_signer::*;
use turnkey_signer::*;
use vault_signer::*;

use soroban_rs::xdr::SignatureHint;

use crate::{
    domain::{SignDataRequest, SignDataResponse, SignTransactionResponse, SignTypedDataRequest},
    models::{
        Address, NetworkTransactionData, Signer as SignerDomainModel, SignerConfig,
        SignerRepoModel, SignerType, TransactionRepoModel, VaultSignerConfig,
    },
    services::{
        signer::{SignXdrTransactionResponseStellar, Signer, SignerError, SignerFactoryError},
        AwsKmsService, GoogleCloudKmsService, TurnkeyService, VaultConfig, VaultService,
    },
};

use super::DataSignerTrait;

/// Derive a `SignatureHint` (last 4 bytes of the Ed25519 public key) from a Stellar address.
fn derive_signature_hint(address: &Address) -> Result<SignatureHint, SignerError> {
    match address {
        Address::Stellar(addr) => {
            let pk = stellar_strkey::ed25519::PublicKey::from_string(addr).map_err(|e| {
                SignerError::SigningError(format!("Failed to parse Stellar address '{addr}': {e}"))
            })?;
            // pk.0 is [u8; 32], last 4 bytes are the hint
            let hint_bytes: [u8; 4] = pk.0[28..].try_into().map_err(|_| {
                SignerError::SigningError(
                    "Failed to create signature hint from public key".to_string(),
                )
            })?;
            Ok(SignatureHint(hint_bytes))
        }
        _ => Err(SignerError::SigningError(format!(
            "Expected Stellar address, got: {address:?}"
        ))),
    }
}

#[cfg(test)]
use mockall::automock;

#[cfg_attr(test, automock)]
/// Trait defining Stellar-specific signing operations
///
/// This trait extends the basic signing functionality with methods specific
/// to the Stellar blockchain, following the same pattern as SolanaSignTrait.
#[async_trait]
pub trait StellarSignTrait: Sync + Send {
    /// Signs a Stellar transaction in XDR format
    ///
    /// # Arguments
    ///
    /// * `unsigned_xdr` - The unsigned transaction in XDR format
    /// * `network_passphrase` - The network passphrase for the Stellar network
    ///
    /// # Returns
    ///
    /// A signed transaction response containing the signed XDR and signature
    async fn sign_xdr_transaction(
        &self,
        unsigned_xdr: &str,
        network_passphrase: &str,
    ) -> Result<SignXdrTransactionResponseStellar, SignerError>;
}

pub enum StellarSigner {
    Local(Box<LocalSigner>),
    Vault(VaultSigner<VaultService>),
    GoogleCloudKms(Box<GoogleCloudKmsSigner>),
    AwsKms(AwsKmsSigner),
    Turnkey(TurnkeySigner),
}

#[async_trait]
impl Signer for StellarSigner {
    async fn address(&self) -> Result<Address, SignerError> {
        match self {
            Self::Local(s) => s.address().await,
            Self::Vault(s) => s.address().await,
            Self::GoogleCloudKms(s) => s.address().await,
            Self::AwsKms(s) => s.address().await,
            Self::Turnkey(s) => s.address().await,
        }
    }

    async fn sign_transaction(
        &self,
        tx: NetworkTransactionData,
    ) -> Result<SignTransactionResponse, SignerError> {
        match self {
            Self::Local(s) => s.sign_transaction(tx).await,
            Self::Vault(s) => s.sign_transaction(tx).await,
            Self::GoogleCloudKms(s) => s.sign_transaction(tx).await,
            Self::AwsKms(s) => s.sign_transaction(tx).await,
            Self::Turnkey(s) => s.sign_transaction(tx).await,
        }
    }
}

#[async_trait]
impl StellarSignTrait for StellarSigner {
    async fn sign_xdr_transaction(
        &self,
        unsigned_xdr: &str,
        network_passphrase: &str,
    ) -> Result<SignXdrTransactionResponseStellar, SignerError> {
        match self {
            Self::Local(s) => {
                s.sign_xdr_transaction(unsigned_xdr, network_passphrase)
                    .await
            }
            Self::Vault(s) => {
                s.sign_xdr_transaction(unsigned_xdr, network_passphrase)
                    .await
            }
            Self::GoogleCloudKms(s) => {
                s.sign_xdr_transaction(unsigned_xdr, network_passphrase)
                    .await
            }
            Self::AwsKms(s) => {
                s.sign_xdr_transaction(unsigned_xdr, network_passphrase)
                    .await
            }
            Self::Turnkey(s) => {
                s.sign_xdr_transaction(unsigned_xdr, network_passphrase)
                    .await
            }
        }
    }
}

pub struct StellarSignerFactory;

impl StellarSignerFactory {
    pub fn create_stellar_signer(
        m: &SignerDomainModel,
    ) -> Result<StellarSigner, SignerFactoryError> {
        let signer = match &m.config {
            SignerConfig::Local(_) => {
                let local_signer = LocalSigner::new(m)?;
                StellarSigner::Local(Box::new(local_signer))
            }
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

                StellarSigner::Vault(VaultSigner::new(
                    m.id.clone(),
                    config.clone(),
                    vault_service,
                ))
            }
            SignerConfig::GoogleCloudKms(config) => {
                let service = GoogleCloudKmsService::new(config)
                    .map_err(|e| SignerFactoryError::CreationFailed(e.to_string()))?;
                StellarSigner::GoogleCloudKms(Box::new(GoogleCloudKmsSigner::new(service)))
            }
            SignerConfig::Turnkey(config) => {
                let service = TurnkeyService::new(config.clone())
                    .map_err(|e| SignerFactoryError::CreationFailed(e.to_string()))?;
                StellarSigner::Turnkey(TurnkeySigner::new(service))
            }
            SignerConfig::AwsKms(config) => {
                let aws_kms_service = futures::executor::block_on(AwsKmsService::new(
                    config.clone(),
                ))
                .map_err(|e| {
                    SignerFactoryError::InvalidConfig(format!(
                        "Failed to create AWS KMS service: {e}"
                    ))
                })?;
                StellarSigner::AwsKms(AwsKmsSigner::new(aws_kms_service))
            }
            SignerConfig::VaultTransit(_) => {
                return Err(SignerFactoryError::UnsupportedType("Vault Transit".into()))
            }
            SignerConfig::Cdp(_) => return Err(SignerFactoryError::UnsupportedType("CDP".into())),
        };
        Ok(signer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derive_signature_hint_valid_stellar_address() {
        let pk = stellar_strkey::ed25519::PublicKey([0u8; 32]);
        let address = Address::Stellar(pk.to_string());

        let hint = derive_signature_hint(&address).unwrap();
        // Last 4 bytes of all-zero key
        assert_eq!(hint.0, [0u8; 4]);
    }

    #[test]
    fn test_derive_signature_hint_extracts_last_four_bytes() {
        let mut key_bytes = [0u8; 32];
        key_bytes[28] = 0xAA;
        key_bytes[29] = 0xBB;
        key_bytes[30] = 0xCC;
        key_bytes[31] = 0xDD;
        let pk = stellar_strkey::ed25519::PublicKey(key_bytes);
        let address = Address::Stellar(pk.to_string());

        let hint = derive_signature_hint(&address).unwrap();
        assert_eq!(hint.0, [0xAA, 0xBB, 0xCC, 0xDD]);
    }

    #[test]
    fn test_derive_signature_hint_invalid_stellar_address() {
        let address = Address::Stellar("INVALID_ADDRESS".to_string());
        let result = derive_signature_hint(&address);
        assert!(result.is_err());
        match result.unwrap_err() {
            SignerError::SigningError(msg) => {
                assert!(msg.contains("Failed to parse Stellar address"));
            }
            e => panic!("Expected SigningError, got: {e:?}"),
        }
    }

    #[test]
    fn test_derive_signature_hint_non_stellar_address() {
        let address = Address::Evm([0u8; 20]);
        let result = derive_signature_hint(&address);
        assert!(result.is_err());
        match result.unwrap_err() {
            SignerError::SigningError(msg) => {
                assert!(msg.contains("Expected Stellar address"));
            }
            e => panic!("Expected SigningError, got: {e:?}"),
        }
    }

    #[test]
    fn test_derive_signature_hint_solana_address_rejected() {
        let address = Address::Solana("SomeBase58Address".to_string());
        let result = derive_signature_hint(&address);
        assert!(result.is_err());
    }
}
