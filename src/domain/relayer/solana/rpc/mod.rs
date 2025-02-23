//! This module implements the Solana RPC functionality.

mod methods;
pub use methods::*;

mod handler;
pub use handler::*;

use log::error;
use thiserror::Error;

mod validator;
pub use validator::*;

use crate::{
    models::{SignerError, SolanaEncodingError},
    services::SolanaProviderError,
};

#[derive(Debug, Error)]
#[allow(dead_code)]
pub enum SolanaRpcError {
    #[error("Unsupported method: {0}")]
    UnsupportedMethod(String),
    #[error("BadRequest: {0}")]
    BadRequest(String),
    #[error("Feature fetch error: {0}")]
    FeatureFetch(String),
    #[error("Invalid params: {0}")]
    InvalidParams(String),
    #[error("Unsupported Fee token error: {0}")]
    UnsupportedFeeToken(String),
    #[error("Estimation Error: {0}")]
    Estimation(String),
    #[error("Insufficient funds: {0}")]
    InsufficientFunds(String),
    #[error("Transaction preparation error: {0}")]
    TransactionPreparation(String),
    #[error("Preparation error: {0}")]
    Preparation(String),
    #[error("Signature error: {0}")]
    Signature(String),
    #[error("Token fetch error: {0}")]
    TokenFetch(String),
    #[error("Send error: {0}")]
    Send(String),
    #[error("Transaction validation error: {0}")]
    SolanaTransactionValidation(#[from] SolanaTransactionValidationError),
    #[error("Signing error: {0}")]
    SigningError(#[from] SignerError),
    #[error("Encoding error: {0}")]
    EncodingError(#[from] SolanaEncodingError),
    #[error("Provider error: {0}")]
    ProviderError(#[from] SolanaProviderError),
}
