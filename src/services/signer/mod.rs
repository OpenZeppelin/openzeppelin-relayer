use async_trait::async_trait;
use eyre::Result;
use thiserror::Error;

mod evm;
pub use evm::*;

mod solana;
pub use solana::*;

mod stellar;
pub use stellar::*;

use crate::models::{Address, TransactionRepoModel};

#[derive(Error, Debug)]
pub enum SignerError {
    #[error("Failed to sign transaction: {0}")]
    SigningError(String),

    #[error("Invalid key format: {0}")]
    KeyError(String),

    #[error("Provider error: {0}")]
    ProviderError(String),
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
