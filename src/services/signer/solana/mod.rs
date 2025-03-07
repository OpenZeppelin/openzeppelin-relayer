//! Solana signer implementation for managing Solana-compatible private keys and signing operations.
//!
//! Provides:
//! - Local keystore support (encrypted JSON files)
//!
//! # Architecture
//!
//! ```text
//! EvmSigner
//!   ├── TestSigner (Temporary testing private key)
//!   ├── LocalSigner (encrypted JSON keystore)
//!   ├── AwsKmsSigner (AWS KMS backend) [NOT SUPPORTED]
//!   └── VaultSigner (HashiCorp Vault backend) [NOT SUPPORTED]
//! ```
mod local_signer;
use async_trait::async_trait;
use local_signer::*;
use solana_sdk::signature::Signature;

use crate::{
    domain::{
        SignDataRequest, SignDataResponse, SignDataResponseEvm, SignTransactionResponse,
        SignTypedDataRequest,
    },
    models::{
        Address, NetworkTransactionData, SignerConfig, SignerRepoModel, SignerType,
        TransactionRepoModel,
    },
};
use eyre::Result;

use super::{Signer, SignerError, SignerFactoryError};
#[cfg(test)]
use mockall::automock;

pub enum SolanaSigner {
    Local(LocalSigner),
}

#[async_trait]
impl Signer for SolanaSigner {
    async fn address(&self) -> Result<Address, SignerError> {
        match self {
            Self::Local(signer) => signer.address().await,
        }
    }

    async fn sign_transaction(
        &self,
        transaction: NetworkTransactionData,
    ) -> Result<SignTransactionResponse, SignerError> {
        match self {
            Self::Local(signer) => signer.sign_transaction(transaction).await,
        }
    }
}

#[cfg_attr(test, automock)]
pub trait SolanaSignTrait: Send + Sync {
    fn pubkey(&self) -> Result<Address, SignerError>;
    fn sign(&self, message: &[u8]) -> Result<Signature, SignerError>;
}

impl SolanaSignTrait for SolanaSigner {
    fn pubkey(&self) -> Result<Address, SignerError> {
        match self {
            Self::Local(signer) => signer.pubkey(),
        }
    }

    fn sign(&self, message: &[u8]) -> Result<Signature, SignerError> {
        match self {
            Self::Local(signer) => Ok(signer.sign(message)?),
        }
    }
}

pub struct SolanaSignerFactory;

impl SolanaSignerFactory {
    pub fn create_solana_signer(
        signer_model: &SignerRepoModel,
    ) -> Result<SolanaSigner, SignerFactoryError> {
        let signer = match signer_model.config {
            SignerConfig::Test(_) => SolanaSigner::Local(LocalSigner::new(signer_model)),
            SignerConfig::Local(_) => SolanaSigner::Local(LocalSigner::new(signer_model)),
            SignerConfig::AwsKms(_) => {
                return Err(SignerFactoryError::UnsupportedType("AWS KMS".into()));
            }
            SignerConfig::Vault(_) => {
                return Err(SignerFactoryError::UnsupportedType("Vault".into()))
            }
        };

        Ok(signer)
    }
}
