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
    GoogleCloudKms(GoogleCloudKmsSigner),
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
                StellarSigner::GoogleCloudKms(GoogleCloudKmsSigner::new(service))
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
