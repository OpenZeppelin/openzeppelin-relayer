//! DEX integration module for Solana token swaps

use std::sync::Arc;

use crate::domain::relayer::RelayerError;
use crate::models::{RelayerRepoModel, SolanaSwapStrategy};
use crate::services::{
    JupiterService, JupiterServiceTrait, SolanaProvider, SolanaProviderTrait, SolanaSignTrait,
    SolanaSigner,
};
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
pub mod jupiter_ultra;

pub enum NetworkDex<P, S, J>
where
    P: SolanaProviderTrait + 'static,
    S: SolanaSignTrait + Send + Sync + 'static,
    J: JupiterServiceTrait + Send + Sync + 'static,
{
    JupiterSwap {
        dex: jupiter_swap::JupiterSwapDex<P, S, J>,
    },
    JupiterUltra {
        dex: jupiter_ultra::JupiterUltraDex<S, J>,
    },
}

pub type DefaultNetworkDex = NetworkDex<SolanaProvider, SolanaSigner, JupiterService>;

#[async_trait]
impl<P, S, J> DexStrategy for NetworkDex<P, S, J>
where
    P: SolanaProviderTrait + Send + Sync + 'static,
    S: SolanaSignTrait + Send + Sync + 'static,
    J: JupiterServiceTrait + Send + Sync + 'static,
{
    async fn execute_swap(&self, params: SwapParams) -> Result<SwapResult, RelayerError> {
        match self {
            NetworkDex::JupiterSwap { dex } => dex.execute_swap(params).await,
            NetworkDex::JupiterUltra { dex } => dex.execute_swap(params).await,
        }
    }
}

// Helper function to create the appropriate DEX implementation
pub fn create_network_dex(
    relayer: &RelayerRepoModel,
    provider: Arc<SolanaProvider>,
    signer_service: Arc<SolanaSigner>,
    jupiter_service: Arc<JupiterService>,
) -> Result<DefaultNetworkDex, RelayerError> {
    // Get the DEX strategy from the relayer's policies
    let policy = relayer.policies.get_solana_policy();
    let strategy = match policy.get_swap_config() {
        Some(config) => config.strategy.unwrap_or(SolanaSwapStrategy::JupiterUltra),
        None => SolanaSwapStrategy::JupiterUltra, // Default to Jupiter Ultra if not specified
    };

    match strategy {
        SolanaSwapStrategy::JupiterSwap => Ok(DefaultNetworkDex::JupiterSwap {
            dex: jupiter_swap::DefaultJupiterSwapDex::new(
                provider,
                signer_service,
                jupiter_service,
            ),
        }),
        SolanaSwapStrategy::JupiterUltra => Ok(DefaultNetworkDex::JupiterUltra {
            dex: jupiter_ultra::DefaultJupiterUltraDex::new(signer_service, jupiter_service),
        }),
    }
}

#[cfg(test)]
mod tests {
    use secrets::SecretVec;

    use crate::{
        models::{
            LocalSignerConfig, RelayerSolanaPolicy, RelayerSolanaSwapConfig, SignerConfig,
            SignerRepoModel,
        },
        services::SolanaSignerFactory,
    };

    use super::*;

    fn create_test_signer_model() -> SignerRepoModel {
        let seed = vec![1u8; 32];
        let raw_key = SecretVec::new(32, |v| v.copy_from_slice(&seed));
        SignerRepoModel {
            id: "test".to_string(),
            config: SignerConfig::Local(LocalSignerConfig { raw_key }),
        }
    }

    #[test]
    fn test_create_network_dex_jupiter_swap_explicit() {
        let mut relayer = RelayerRepoModel::default();
        let policy = crate::models::RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            swap_config: Some(RelayerSolanaSwapConfig {
                strategy: Some(SolanaSwapStrategy::JupiterSwap),
                cron_schedule: None,
                min_balance_threshold: None,
            }),
            ..Default::default()
        });

        relayer.policies = policy;

        let provider =
            Arc::new(SolanaProvider::new("https://api.mainnet-beta.solana.com").unwrap());
        let signer_service = Arc::new(
            SolanaSignerFactory::create_solana_signer(&create_test_signer_model()).unwrap(),
        );
        let jupiter_service = Arc::new(JupiterService::new_from_network(relayer.network.as_str()));

        let result = create_network_dex(&relayer, provider, signer_service, jupiter_service);

        assert!(result.is_ok());
        match result.unwrap() {
            DefaultNetworkDex::JupiterSwap { .. } => {}
            _ => panic!("Expected JupiterSwap strategy"),
        }
    }

    #[test]
    fn test_create_network_dex_jupiter_ultra_explicit() {
        let mut relayer = RelayerRepoModel::default();
        let policy = crate::models::RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            swap_config: Some(RelayerSolanaSwapConfig {
                strategy: Some(SolanaSwapStrategy::JupiterUltra),
                cron_schedule: None,
                min_balance_threshold: None,
            }),
            ..Default::default()
        });

        relayer.policies = policy;

        let provider =
            Arc::new(SolanaProvider::new("https://api.mainnet-beta.solana.com").unwrap());
        let signer_service = Arc::new(
            SolanaSignerFactory::create_solana_signer(&create_test_signer_model()).unwrap(),
        );
        let jupiter_service = Arc::new(JupiterService::new_from_network(relayer.network.as_str()));

        let result = create_network_dex(&relayer, provider, signer_service, jupiter_service);

        assert!(result.is_ok());
        match result.unwrap() {
            DefaultNetworkDex::JupiterUltra { .. } => { /* Success case */ }
            _ => panic!("Expected JupiterUltra strategy"),
        }
    }

    #[test]
    fn test_create_network_dex_default_when_no_strategy() {
        let mut relayer = RelayerRepoModel::default();
        let policy = crate::models::RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            swap_config: Some(RelayerSolanaSwapConfig {
                strategy: None,
                cron_schedule: None,
                min_balance_threshold: None,
            }),
            ..Default::default()
        });

        relayer.policies = policy;

        let provider =
            Arc::new(SolanaProvider::new("https://api.mainnet-beta.solana.com").unwrap());
        let signer_service = Arc::new(
            SolanaSignerFactory::create_solana_signer(&create_test_signer_model()).unwrap(),
        );
        let jupiter_service = Arc::new(JupiterService::new_from_network(relayer.network.as_str()));

        let result = create_network_dex(&relayer, provider, signer_service, jupiter_service);

        assert!(result.is_ok());
        match result.unwrap() {
            DefaultNetworkDex::JupiterUltra { .. } => { /* Success case - should default to Jupiter */
            }
            _ => panic!("Expected default JupiterUltra strategy"),
        }
    }

    #[test]
    fn test_create_network_dex_default_when_no_swap_config() {
        let mut relayer = RelayerRepoModel::default();
        let policy = crate::models::RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            swap_config: None,
            ..Default::default()
        });

        relayer.policies = policy;

        let provider =
            Arc::new(SolanaProvider::new("https://api.mainnet-beta.solana.com").unwrap());
        let signer_service = Arc::new(
            SolanaSignerFactory::create_solana_signer(&create_test_signer_model()).unwrap(),
        );
        let jupiter_service = Arc::new(JupiterService::new_from_network(relayer.network.as_str()));

        let result = create_network_dex(&relayer, provider, signer_service, jupiter_service);

        assert!(result.is_ok());
        match result.unwrap() {
            DefaultNetworkDex::JupiterUltra { .. } => {}
            _ => panic!("Expected default JupiterUltra strategy"),
        }
    }
}
