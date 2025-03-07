use serde::Serialize;
use thiserror::Error;

pub mod evm;
pub use evm::*;

mod solana;
pub use solana::*;

use crate::models::SolanaNetwork;

#[derive(Error, Debug, Serialize)]
pub enum ProviderError {
    #[error("RPC client error: {0}")]
    SolanaRpcError(#[from] SolanaProviderError),
    #[error("Invalid address: {0}")]
    InvalidAddress(String),
    #[error("Network configuration error: {0}")]
    NetworkConfiguration(String),
}

pub fn get_solana_network_provider_from_str(
    network: &str,
) -> Result<SolanaProvider, ProviderError> {
    let network = match SolanaNetwork::from_network_str(network) {
        Ok(network) => network,
        Err(e) => return Err(ProviderError::NetworkConfiguration(e.to_string())),
    };
    let rpc_url =
        network
            .public_rpc_urls()
            .first()
            .copied()
            .ok_or(ProviderError::NetworkConfiguration(
                "No RPC URLs configured".to_string(),
            ))?;

    SolanaProvider::new(rpc_url)
}
