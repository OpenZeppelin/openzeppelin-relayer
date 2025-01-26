//! Signer service module for handling cryptographic operations across different blockchain
//! networks.
//!
//! This module provides:
//! - Common signer traits for different blockchain networks
//! - Network-specific signer implementations (EVM, Solana, Stellar)
//! - Factory methods for creating signers
//! - Error handling for signing operations
//!
//! # Architecture
//!
//! ```text
//! Signer Trait (Common Interface)
//!   ├── EvmSigner
//!   │   └── LocalSigner
//!   ├── SolanaSigner
//!   └── StellarSigner

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
    domain::{SignDataRequest, SignDataResponse, SignTypedDataRequest},
    models::{Address, SignerRepoModel, SignerType, TransactionRepoModel},
};

#[derive(Error, Debug, Serialize)]
#[allow(clippy::enum_variant_names)]
pub enum SignerError {
    #[error("Failed to sign transaction: {0}")]
    SigningError(String),

    #[error("Invalid key format: {0}")]
    KeyError(String),

    #[error("Provider error: {0}")]
    ProviderError(String),

    #[error("Unsupported signer type: {0}")]
    UnsupportedTypeError(String),
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
impl DataSignerTrait for NetworkSigner {
    async fn sign_data(&self, request: SignDataRequest) -> Result<SignDataResponse, SignerError> {
        match self {
            Self::Evm(signer) => {
                let signature = signer
                    .sign_data(request)
                    .await
                    .map_err(|e| SignerError::SigningError(e.to_string()))?;

                Ok(signature)
            }
            Self::Solana(_) => Err(SignerError::UnsupportedTypeError(
                "Solana: sign data not supported".into(),
            )),
            Self::Stellar(_) => Err(SignerError::UnsupportedTypeError(
                "Stellar: sign data not supported".into(),
            )),
        }
    }

    async fn sign_typed_data(
        &self,
        request: SignTypedDataRequest,
    ) -> Result<SignDataResponse, SignerError> {
        match self {
            Self::Evm(signer) => signer
                .sign_typed_data(request)
                .await
                .map_err(|e| SignerError::SigningError(e.to_string())),
            Self::Solana(_) => Err(SignerError::UnsupportedTypeError(
                "Solana: Signing typed data not supported".into(),
            )),
            Self::Stellar(_) => Err(SignerError::UnsupportedTypeError(
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
