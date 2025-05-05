//! DEX integration module for Solana token swaps

use crate::domain::relayer::RelayerError;
use crate::services::{SolanaProvider, SolanaSigner};
use async_trait::async_trait;

/// Result of a swap operation
#[derive(Debug)]
pub struct SwapResult {
    pub source_amount: u64,
    pub destination_amount: u64,
    pub transaction_signature: String,
}

/// Parameters for a swap operation
#[derive(Debug)]
pub struct SwapParams {
    pub owner_address: String,
    pub source_mint: String,
    pub destination_mint: String,
    pub amount: u64,
    pub slippage_percent: f64,
}

/// Trait defining DEX swap functionality
#[async_trait]
pub trait DexStrategy: Send + Sync {
    /// Execute a token swap operation
    async fn execute_swap(
        &self,
        provider: &SolanaProvider,
        signer: &SolanaSigner,
        params: SwapParams,
    ) -> Result<SwapResult, RelayerError>;

    /// Get the name of the DEX implementation
    fn name(&self) -> &str;
}

// Re-export the specific implementations
pub mod jupiter;

/// Create a DEX strategy based on the given name
pub fn create_dex_strategy(name: &str) -> Result<Box<dyn DexStrategy>, RelayerError> {
    match name.to_lowercase().as_str() {
        "jupiter" => Ok(Box::new(jupiter::JupiterDex::new())),
        _ => Err(RelayerError::PolicyConfigurationError(format!(
            "Unsupported DEX: {}",
            name
        ))),
    }
}
