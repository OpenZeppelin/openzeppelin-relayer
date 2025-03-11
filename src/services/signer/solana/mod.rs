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
            Self::Local(signer) => signer.address().await,
            Self::Vault(signer) => signer.address().await,
            Self::VaultCloud(signer) => signer.address().await,
            Self::VaultTransit(signer) => signer.address().await,
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
            Self::Local(signer) => signer.pubkey(),
            Self::Vault(signer) => signer.pubkey(),
            Self::VaultCloud(signer) => signer.pubkey(),
            Self::VaultTransit(signer) => signer.pubkey(),
        }
    }

    async fn sign(&self, message: &[u8]) -> Result<Signature, SignerError> {
        match self {
            Self::Local(signer) => Ok(signer.sign(message).await?),
            Self::Vault(signer) => Ok(signer.sign(message).await?),
            Self::VaultCloud(signer) => Ok(signer.sign(message).await?),
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
            SignerConfig::Test(_) => SolanaSigner::Local(LocalSigner::new(signer_model)),
            SignerConfig::Local(_) => SolanaSigner::Local(LocalSigner::new(signer_model)),
            SignerConfig::Vault(_) => SolanaSigner::Local(LocalSigner::new(signer_model)),
            SignerConfig::VaultCloud(_) => SolanaSigner::Local(LocalSigner::new(signer_model)),
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
