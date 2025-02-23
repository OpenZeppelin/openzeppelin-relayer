use serde::Serialize;
use thiserror::Error;

use super::TransactionError;

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

    #[error("Invalid transaction: {0}")]
    InvalidTransaction(#[from] TransactionError),
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
