mod local_signer;
use async_trait::async_trait;
pub use local_signer::*;
use serde_json::Value;

use crate::models::{Address, SignerRepoModel, SignerType, TransactionRepoModel};
use bytes::Bytes;
use eyre::Result;

use super::{Signer, SignerError, SignerFactoryError};

#[async_trait]
pub trait EvmSignerTrait: Send + Sync {
    /// Signs arbitrary message data
    async fn sign_data(&self, data: Bytes) -> Result<Vec<u8>, SignerError>;

    /// Signs EIP-712 typed data
    async fn sign_typed_data(&self, typed_data: Value) -> Result<Vec<u8>, SignerError>;
}

pub enum EvmSigner {
    Local(LocalSigner),
}

#[async_trait]
impl Signer for EvmSigner {
    async fn address(&self) -> Result<Address, SignerError> {
        match self {
            Self::Local(signer) => signer.address().await,
        }
    }

    async fn sign_transaction(
        &self,
        transaction: TransactionRepoModel,
    ) -> Result<Vec<u8>, SignerError> {
        match self {
            Self::Local(signer) => signer.sign_transaction(transaction).await,
        }
    }
}

#[async_trait]
impl EvmSignerTrait for EvmSigner {
    async fn sign_data(&self, data: Bytes) -> Result<Vec<u8>, SignerError> {
        match self {
            Self::Local(signer) => signer.sign_data(data).await,
        }
    }

    async fn sign_typed_data(&self, typed_data: Value) -> Result<Vec<u8>, SignerError> {
        match self {
            Self::Local(signer) => signer.sign_typed_data(typed_data).await,
        }
    }
}

pub struct EvmSignerFactory;

impl EvmSignerFactory {
    pub fn create_evm_signer(
        signer_model: SignerRepoModel,
    ) -> Result<EvmSigner, SignerFactoryError> {
        let signer = match signer_model.signer_type {
            SignerType::Local => EvmSigner::Local(LocalSigner::new(signer_model)),
            SignerType::AwsKms => {
                return Err(SignerFactoryError::UnsupportedType("AWS KMS".into()))
            }
            SignerType::Vault => return Err(SignerFactoryError::UnsupportedType("Vault".into())),
        };

        Ok(signer)
    }
}

#[cfg(test)]
mod tests {}
