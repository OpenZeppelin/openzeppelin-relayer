//! DEX integration module for Solana token swaps

use std::sync::Arc;

use crate::domain::relayer::RelayerError;
use crate::models::{RelayerRepoModel, SolanaSwapStrategy};
use crate::services::{
    JupiterService, JupiterServiceTrait, SolanaProvider, SolanaProviderTrait, SolanaSignTrait,
    SolanaSigner,
};
use async_trait::async_trait;
#[cfg(test)]
use mockall::automock;
use serde::{Deserialize, Serialize};
/// Result of a swap operation
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct SwapResult {
    pub mint: String,
    pub source_amount: u64,
    pub destination_amount: u64,
    pub transaction_signature: String,
    pub error: Option<String>,
}

impl Default for SwapResult {
    fn default() -> Self {
        Self {
            mint: "".into(),
            source_amount: 0,
            destination_amount: 0,
            transaction_signature: "".into(),
            error: None,
        }
    }
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
#[cfg_attr(test, automock)]
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
    Noop {
        dex: NoopDex,
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
            NetworkDex::Noop { dex } => dex.execute_swap(params).await,
        }
    }
}

fn resolve_strategy(relayer: &RelayerRepoModel) -> SolanaSwapStrategy {
    relayer
        .policies
        .get_solana_policy()
        .get_swap_config()
        .and_then(|cfg| cfg.strategy)
        .unwrap_or(SolanaSwapStrategy::Noop) // Provide a default strategy
}

pub struct NoopDex;
#[async_trait]
impl DexStrategy for NoopDex {
    async fn execute_swap(&self, _params: SwapParams) -> Result<SwapResult, RelayerError> {
        Ok(SwapResult::default())
    }
}

// Helper function to create the appropriate DEX implementation
pub fn create_network_dex_generic<P, S, J>(
    relayer: &RelayerRepoModel,
    provider: Arc<P>,
    signer_service: Arc<S>,
    jupiter_service: Arc<J>,
) -> Result<NetworkDex<P, S, J>, RelayerError>
where
    P: SolanaProviderTrait + Send + Sync + 'static,
    S: SolanaSignTrait + Send + Sync + 'static,
    J: JupiterServiceTrait + Send + Sync + 'static,
{
    let jupiter_swap_options = relayer
        .policies
        .get_solana_policy()
        .get_swap_config()
        .and_then(|cfg| cfg.jupiter_swap_options.clone());

    match resolve_strategy(relayer) {
        SolanaSwapStrategy::JupiterSwap => Ok(NetworkDex::JupiterSwap {
            dex: jupiter_swap::JupiterSwapDex::<P, S, J>::new(
                provider,
                signer_service,
                jupiter_service,
                jupiter_swap_options,
            ),
        }),
        SolanaSwapStrategy::JupiterUltra => Ok(NetworkDex::JupiterUltra {
            dex: jupiter_ultra::JupiterUltraDex::<S, J>::new(signer_service, jupiter_service),
        }),
        _ => Ok(NetworkDex::Noop { dex: NoopDex }),
    }
}

#[cfg(test)]
mod tests {
    use secrets::SecretVec;

    use crate::{
        models::{
            LocalSignerConfigStorage, RelayerSolanaPolicy, RelayerSolanaSwapConfig,
            SignerConfigStorage, SignerRepoModel,
        },
        services::{MockSolanaProviderTrait, SolanaSignerFactory},
    };

    use super::*;

    fn create_test_signer_model() -> SignerRepoModel {
        let seed = vec![1u8; 32];
        let raw_key = SecretVec::new(32, |v| v.copy_from_slice(&seed));
        SignerRepoModel {
            id: "test".to_string(),
            config: SignerConfigStorage::Local(LocalSignerConfigStorage { raw_key }),
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
                jupiter_swap_options: None,
            }),
            ..Default::default()
        });

        relayer.policies = policy;

        let provider = Arc::new(MockSolanaProviderTrait::new());

        let signer_service = Arc::new(
            SolanaSignerFactory::create_solana_signer(&create_test_signer_model().into()).unwrap(),
        );
        let jupiter_service = Arc::new(JupiterService::new_from_network(relayer.network.as_str()));

        let result =
            create_network_dex_generic(&relayer, provider, signer_service, jupiter_service);

        match result {
            Ok(NetworkDex::JupiterSwap { .. }) => {}
            Ok(_) => panic!("Expected JupiterSwap strategy"),
            Err(e) => panic!("Expected Ok with JupiterSwap, but got error: {:?}", e),
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
                jupiter_swap_options: None,
            }),
            ..Default::default()
        });

        relayer.policies = policy;

        let provider = Arc::new(MockSolanaProviderTrait::new());

        let signer_service = Arc::new(
            SolanaSignerFactory::create_solana_signer(&create_test_signer_model().into()).unwrap(),
        );
        let jupiter_service = Arc::new(JupiterService::new_from_network(relayer.network.as_str()));

        let result =
            create_network_dex_generic(&relayer, provider, signer_service, jupiter_service);

        match result {
            Ok(NetworkDex::JupiterUltra { .. }) => {}
            Ok(_) => panic!("Expected JupiterUltra strategy"),
            Err(e) => panic!("Expected Ok with JupiterUltra, but got error: {:?}", e),
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
                jupiter_swap_options: None,
            }),
            ..Default::default()
        });

        relayer.policies = policy;

        let provider = Arc::new(MockSolanaProviderTrait::new());

        let signer_service = Arc::new(
            SolanaSignerFactory::create_solana_signer(&create_test_signer_model().into()).unwrap(),
        );
        let jupiter_service = Arc::new(JupiterService::new_from_network(relayer.network.as_str()));

        let result =
            create_network_dex_generic(&relayer, provider, signer_service, jupiter_service);

        match result {
            Ok(NetworkDex::Noop { .. }) => {}
            Ok(_) => panic!("Expected Noop strategy"),
            Err(e) => panic!("Expected Ok with Noop, but got error: {:?}", e),
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

        let provider = Arc::new(MockSolanaProviderTrait::new());

        let signer_service = Arc::new(
            SolanaSignerFactory::create_solana_signer(&create_test_signer_model().into()).unwrap(),
        );
        let jupiter_service = Arc::new(JupiterService::new_from_network(relayer.network.as_str()));

        let result =
            create_network_dex_generic(&relayer, provider, signer_service, jupiter_service);

        match result {
            Ok(NetworkDex::Noop { .. }) => {}
            Ok(_) => panic!("Expected Noop strategy"),
            Err(e) => panic!("Expected Ok with Noop, but got error: {:?}", e),
        }
    }
}
