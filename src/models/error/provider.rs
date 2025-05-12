use serde::Serialize;
use thiserror::Error;

#[derive(Error, Debug, Serialize)]
pub enum StellarProviderError {
    #[error("RPC client error: {0}")]
    RpcError(String),
    #[error("Simulation failed: {0}")]
    SimulationFailed(String),
    #[error("Insufficient balance: {0}")]
    InsufficientBalance(String),
    #[error("Bad sequence number: {0}")]
    BadSeq(String),
    #[error("Unknown error: {0}")]
    Unknown(String),
}

impl From<eyre::Report> for StellarProviderError {
    fn from(err: eyre::Report) -> Self {
        StellarProviderError::RpcError(err.to_string())
    }
}
