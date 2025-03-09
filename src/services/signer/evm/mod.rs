//! EVM signer implementation for managing Ethereum-compatible private keys and signing operations.
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
//!   ├── AwsKmsSigner (AWS KMS backend) [NOT IMPLEMENTED]
//!   └── VaultSigner (HashiCorp Vault backend) [NOT IMPLEMENTED]
//! ```
mod local_signer;
use async_trait::async_trait;
use local_signer::*;

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
}

#[async_trait]
impl Signer for EvmSigner {
    async fn address(&self) -> Result<Address, SignerError> {
        match self {
            Self::Local(signer) => signer.address().await,
            Self::Vault(signer) => signer.address().await,
            Self::VaultCloud(signer) => signer.address().await,
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
        }
    }
}

pub struct EvmSignerFactory;

impl EvmSignerFactory {
    pub fn create_evm_signer(
        signer_model: &SignerRepoModel,
    ) -> Result<EvmSigner, SignerFactoryError> {
        let signer = match signer_model.config {
            SignerConfig::Test(_) => EvmSigner::Local(LocalSigner::new(signer_model)),
            SignerConfig::Local(_) => EvmSigner::Local(LocalSigner::new(signer_model)),
            SignerConfig::Vault(_) => EvmSigner::Local(LocalSigner::new(signer_model)),
            SignerConfig::VaultCloud(_) => EvmSigner::Local(LocalSigner::new(signer_model)),
            SignerConfig::AwsKms(_) => {
                return Err(SignerFactoryError::UnsupportedType("AWS KMS".into()));
            }
            SignerConfig::VaultTransit(_) => {
                return Err(SignerFactoryError::UnsupportedType("Vault Transit".into()));
            }
        };

        Ok(signer)
    }
}

#[cfg(test)]
mod tests {}
