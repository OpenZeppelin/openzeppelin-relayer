#![allow(unused_imports)]
use async_trait::async_trait;
use eyre::Result;
use serde::Serialize;
use thiserror::Error;

mod evm;
pub use evm::*;

mod solana;
pub use solana::*;

mod stellar;
pub use stellar::*;

use crate::{
    domain::{SignDataRequest, SignDataResponse},
    models::{Address, SignerRepoModel, SignerType, TransactionRepoModel},
};

#[derive(Error, Debug, Serialize)]
pub enum SignerError {
    #[error("Failed to sign transaction: {0}")]
    Signing(String),

    #[error("Invalid key format: {0}")]
    Key(String),

    #[error("Provider error: {0}")]
    Provider(String),

    #[error("Unsupported signer type: {0}")]
    UnsupportedType(String),
}

#[async_trait]
pub trait Signer: Send + Sync {
    /// Returns the signer's ethereum address
    async fn address(&self) -> Result<Address, SignerError>;

    async fn sign_transaction(
        &self,
        transaction: TransactionRepoModel, /* TODO introduce Transactions models for specific
                                            * operations */
    ) -> Result<Vec<u8>, SignerError>;
}

#[derive(Error, Debug, Serialize)]
pub enum SignerFactoryError {
    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),
    #[error("Signer creation failed: {0}")]
    CreationFailed(String),
    #[error("Unsupported signer type: {0}")]
    UnsupportedType(String),
}

#[allow(dead_code)]
pub enum NetworkSigner {
    Evm(EvmSigner),
    Solana(EvmSigner),  // TODO replace with SolanaSigner
    Stellar(EvmSigner), // TODO replace with StellarSigner
}

#[async_trait]
impl Signer for NetworkSigner {
    async fn address(&self) -> Result<Address, SignerError> {
        match self {
            Self::Evm(signer) => signer.address().await,
            Self::Solana(signer) => signer.address().await,
            Self::Stellar(signer) => signer.address().await,
        }
    }

    async fn sign_transaction(
        &self,
        transaction: TransactionRepoModel,
    ) -> Result<Vec<u8>, SignerError> {
        match self {
            Self::Evm(signer) => signer.sign_transaction(transaction).await,
            Self::Solana(signer) => signer.sign_transaction(transaction).await,
            Self::Stellar(signer) => signer.sign_transaction(transaction).await,
        }
    }
}

#[async_trait]
impl EvmSignerTrait for NetworkSigner {
    async fn sign_data(&self, request: SignDataRequest) -> Result<SignDataResponse, SignerError> {
        match self {
            Self::Evm(signer) => signer
                .sign_data(request)
                .await
                .map_err(|e| SignerError::UnsupportedType(e.to_string())),
            Self::Solana(_) => Err(SignerError::UnsupportedType(
                "Solana: sign data not supported".into(),
            )),
            Self::Stellar(_) => Err(SignerError::UnsupportedType(
                "Stellar: sign data not supported".into(),
            )),
        }
    }

    async fn sign_typed_data(
        &self,
        request: SignDataRequest,
    ) -> Result<SignDataResponse, SignerError> {
        match self {
            Self::Evm(signer) => signer
                .sign_typed_data(request)
                .await
                .map_err(|e| SignerError::UnsupportedType(e.to_string())),
            Self::Solana(_) => Err(SignerError::UnsupportedType(
                "Solana: Signing typed data not supported".into(),
            )),
            Self::Stellar(_) => Err(SignerError::UnsupportedType(
                "Stellar: Signing typed data not supported".into(),
            )),
        }
    }
}

pub struct SignerFactory;

impl SignerFactory {
    pub fn create_signer(
        signer_model: SignerRepoModel,
    ) -> Result<NetworkSigner, SignerFactoryError> {
        let signer = match signer_model.signer_type {
            SignerType::Local => {
                let evm_signer = EvmSignerFactory::create_evm_signer(signer_model)?;
                NetworkSigner::Evm(evm_signer)
            }
            SignerType::AwsKms => {
                return Err(SignerFactoryError::UnsupportedType("AWS KMS".into()))
            }
            SignerType::Vault => return Err(SignerFactoryError::UnsupportedType("Vault".into())),
        };

        Ok(signer)
    }
}
