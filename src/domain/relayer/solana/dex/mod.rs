//! DEX integration module for Solana token swaps

use crate::domain::relayer::RelayerError;
use crate::models::SolanaSwapStrategy;
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
}

// Re-export the specific implementations
pub mod jupiter_swap;

pub enum NetworkDex {
    JupiterSwap(jupiter_swap::JupiterSwapDex),
}

#[async_trait]
impl DexStrategy for NetworkDex {
    async fn execute_swap(
        &self,
        provider: &SolanaProvider,
        signer: &SolanaSigner,
        params: SwapParams,
    ) -> Result<SwapResult, RelayerError> {
        match self {
            NetworkDex::JupiterSwap(dex) => dex.execute_swap(provider, signer, params).await,
        }
    }
}

/// Factory function to create the appropriate DEX strategy
pub fn create_dex_strategy(strategy: &SolanaSwapStrategy) -> Result<NetworkDex, RelayerError> {
    match strategy {
        SolanaSwapStrategy::JupiterSwap => {
            Ok(NetworkDex::JupiterSwap(jupiter_swap::JupiterSwapDex::new()))
        }
        _ => Err(RelayerError::InvalidDexName(
            "Unsupported DEX strategy".to_string(),
        )),
    }
}
