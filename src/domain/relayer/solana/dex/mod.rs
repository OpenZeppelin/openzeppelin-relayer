//! DEX integration module for Solana token swaps

use std::sync::Arc;

use crate::domain::relayer::RelayerError;
use crate::models::{RelayerRepoModel, SolanaSwapStrategy};
use crate::services::{JupiterService, SolanaProvider, SolanaSigner};
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
    async fn execute_swap(&self, params: SwapParams) -> Result<SwapResult, RelayerError>;
}

// Re-export the specific implementations
pub mod jupiter_swap;

pub enum NetworkDex {
    JupiterSwap { dex: jupiter_swap::JupiterSwapDex },
}

#[async_trait]
impl DexStrategy for NetworkDex {
    async fn execute_swap(&self, params: SwapParams) -> Result<SwapResult, RelayerError> {
        match self {
            NetworkDex::JupiterSwap { dex } => dex.execute_swap(params).await,
        }
    }
}

// Helper function to create the appropriate DEX implementation
pub fn create_network_dex(
    relayer: &RelayerRepoModel,
    provider: Arc<SolanaProvider>,
    signer_service: Arc<SolanaSigner>,
    jupiter_service: Arc<JupiterService>,
) -> Result<NetworkDex, RelayerError> {
    // Get the DEX strategy from the relayer's policies
    let policy = relayer.policies.get_solana_policy();
    let strategy = match policy.get_swap_config() {
        Some(config) => config.strategy.unwrap_or(SolanaSwapStrategy::JupiterSwap),
        None => SolanaSwapStrategy::JupiterSwap, // Default to Jupiter if not specified
    };

    match strategy {
        SolanaSwapStrategy::JupiterSwap => Ok(NetworkDex::JupiterSwap {
            dex: jupiter_swap::JupiterSwapDex::new(provider, signer_service, jupiter_service),
        }),
    }
}
