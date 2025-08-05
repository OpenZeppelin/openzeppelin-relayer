// openzeppelin-relayer/src/services/signer/stellar/mod.rs
//! Stellar signer implementation (local keystore)

mod local_signer;
mod vault_signer;

use async_trait::async_trait;
use local_signer::*;
use vault_signer::*;

use crate::{
    domain::{SignDataRequest, SignDataResponse, SignTransactionResponse, SignTypedDataRequest},
    models::{
        Address, NetworkTransactionData, Signer as SignerDomainModel, SignerConfig,
        SignerRepoModel, SignerType, TransactionRepoModel, VaultSignerConfig,
    },
    services::{
        signer::{SignXdrTransactionResponseStellar, SignerError, SignerFactoryError},
        Signer, VaultConfig, VaultService,
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
}

#[async_trait]
impl Signer for StellarSigner {
    async fn address(&self) -> Result<Address, SignerError> {
        match self {
            Self::Local(s) => s.address().await,
            Self::Vault(s) => s.address().await,
        }
    }

    async fn sign_transaction(
        &self,
        tx: NetworkTransactionData,
    ) -> Result<SignTransactionResponse, SignerError> {
        match self {
            Self::Local(s) => s.sign_transaction(tx).await,
            Self::Vault(s) => s.sign_transaction(tx).await,
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
            SignerConfig::AwsKms(_) => {
                return Err(SignerFactoryError::UnsupportedType("AWS KMS".into()))
            }
            SignerConfig::VaultTransit(_) => {
                return Err(SignerFactoryError::UnsupportedType("Vault Transit".into()))
            }
            SignerConfig::Turnkey(_) => {
                return Err(SignerFactoryError::UnsupportedType("Turnkey".into()))
            }
            SignerConfig::GoogleCloudKms(_) => {
                return Err(SignerFactoryError::UnsupportedType(
                    "Google Cloud KMS".into(),
                ))
            }
        };
        Ok(signer)
    }
}
