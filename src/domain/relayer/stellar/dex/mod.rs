//! DEX integration module for Stellar token swaps

use std::sync::Arc;

use crate::domain::relayer::RelayerError;
use crate::models::{RelayerRepoModel, StellarSwapStrategy};
use crate::services::stellar_dex::{OrderBookService, StellarDexServiceTrait};
use async_trait::async_trait;
#[cfg(test)]
use mockall::automock;
use serde::{Deserialize, Serialize};

/// Result of a swap operation
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct StellarSwapResult {
    pub asset_id: String,
    pub source_amount: u64,
    pub destination_amount: u64,
    pub transaction_hash: String,
    pub error: Option<String>,
}

impl Default for StellarSwapResult {
    fn default() -> Self {
        Self {
            asset_id: "".into(),
            source_amount: 0,
            destination_amount: 0,
            transaction_hash: "".into(),
            error: None,
        }
    }
}

/// Parameters for a swap operation
#[derive(Debug)]
pub struct StellarSwapParams {
    pub relayer_address: String,
    pub source_asset: String,
    pub destination_asset: String,
    pub amount: u64,
    pub slippage_percent: f32,
}

/// Trait defining DEX swap functionality
#[async_trait]
#[cfg_attr(test, automock)]
pub trait DexStrategy: Send + Sync {
    /// Execute a token swap operation
    async fn execute_swap(
        &self,
        params: StellarSwapParams,
    ) -> Result<StellarSwapResult, RelayerError>;

    /// Get a quote for converting a token to XLM
    async fn get_token_to_xlm_quote(
        &self,
        asset_id: &str,
        amount: u64,
        slippage: f32,
    ) -> Result<
        crate::services::stellar_dex::StellarQuoteResponse,
        crate::services::stellar_dex::StellarDexServiceError,
    >;

    /// Get a quote for converting XLM to a token
    async fn get_xlm_to_token_quote(
        &self,
        asset_id: &str,
        amount: u64,
        slippage: f32,
    ) -> Result<
        crate::services::stellar_dex::StellarQuoteResponse,
        crate::services::stellar_dex::StellarDexServiceError,
    >;
}

// Re-export the specific implementations
pub mod paths_swap;

/// Enum representing different DEX strategies for Stellar
pub enum StellarNetworkDex<D>
where
    D: StellarDexServiceTrait + Send + Sync + 'static,
{
    Paths { dex: paths_swap::PathsSwapDex<D> },
    Noop { dex: StellarNoopDex },
}

pub type DefaultStellarNetworkDex = StellarNetworkDex<OrderBookService>;

#[async_trait]
impl<D> DexStrategy for StellarNetworkDex<D>
where
    D: StellarDexServiceTrait + Send + Sync + 'static,
{
    async fn execute_swap(
        &self,
        params: StellarSwapParams,
    ) -> Result<StellarSwapResult, RelayerError> {
        match self {
            StellarNetworkDex::Paths { dex } => dex.execute_swap(params).await,
            StellarNetworkDex::Noop { dex } => dex.execute_swap(params).await,
        }
    }

    async fn get_token_to_xlm_quote(
        &self,
        asset_id: &str,
        amount: u64,
        slippage: f32,
    ) -> Result<
        crate::services::stellar_dex::StellarQuoteResponse,
        crate::services::stellar_dex::StellarDexServiceError,
    > {
        match self {
            StellarNetworkDex::Paths { dex } => {
                DexStrategy::get_token_to_xlm_quote(dex, asset_id, amount, slippage).await
            }
            StellarNetworkDex::Noop { dex } => {
                DexStrategy::get_token_to_xlm_quote(dex, asset_id, amount, slippage).await
            }
        }
    }

    async fn get_xlm_to_token_quote(
        &self,
        asset_id: &str,
        amount: u64,
        slippage: f32,
    ) -> Result<
        crate::services::stellar_dex::StellarQuoteResponse,
        crate::services::stellar_dex::StellarDexServiceError,
    > {
        match self {
            StellarNetworkDex::Paths { dex } => {
                DexStrategy::get_xlm_to_token_quote(dex, asset_id, amount, slippage).await
            }
            StellarNetworkDex::Noop { dex } => {
                DexStrategy::get_xlm_to_token_quote(dex, asset_id, amount, slippage).await
            }
        }
    }
}

#[async_trait]
impl<D> StellarDexServiceTrait for StellarNetworkDex<D>
where
    D: StellarDexServiceTrait + Send + Sync + 'static,
{
    async fn get_token_to_xlm_quote(
        &self,
        asset_id: &str,
        amount: u64,
        slippage: f32,
    ) -> Result<
        crate::services::stellar_dex::StellarQuoteResponse,
        crate::services::stellar_dex::StellarDexServiceError,
    > {
        match self {
            StellarNetworkDex::Paths { dex } => {
                StellarDexServiceTrait::get_token_to_xlm_quote(dex, asset_id, amount, slippage)
                    .await
            }
            StellarNetworkDex::Noop { dex } => {
                StellarDexServiceTrait::get_token_to_xlm_quote(dex, asset_id, amount, slippage)
                    .await
            }
        }
    }

    async fn get_xlm_to_token_quote(
        &self,
        asset_id: &str,
        amount: u64,
        slippage: f32,
    ) -> Result<
        crate::services::stellar_dex::StellarQuoteResponse,
        crate::services::stellar_dex::StellarDexServiceError,
    > {
        match self {
            StellarNetworkDex::Paths { dex } => {
                StellarDexServiceTrait::get_xlm_to_token_quote(dex, asset_id, amount, slippage)
                    .await
            }
            StellarNetworkDex::Noop { dex } => {
                StellarDexServiceTrait::get_xlm_to_token_quote(dex, asset_id, amount, slippage)
                    .await
            }
        }
    }
}

fn resolve_strategy(relayer: &RelayerRepoModel) -> StellarSwapStrategy {
    relayer
        .policies
        .get_stellar_policy()
        .get_swap_config()
        .and_then(|cfg| cfg.strategy)
        .unwrap_or_else(|| {
            // Default to OrderBook strategy if not specified
            StellarSwapStrategy::OrderBook
        })
}

pub struct StellarNoopDex;

#[async_trait]
impl DexStrategy for StellarNoopDex {
    async fn execute_swap(
        &self,
        _params: StellarSwapParams,
    ) -> Result<StellarSwapResult, RelayerError> {
        Ok(StellarSwapResult::default())
    }

    async fn get_token_to_xlm_quote(
        &self,
        _asset_id: &str,
        _amount: u64,
        _slippage: f32,
    ) -> Result<
        crate::services::stellar_dex::StellarQuoteResponse,
        crate::services::stellar_dex::StellarDexServiceError,
    > {
        // Noop implementation returns a default quote
        Err(
            crate::services::stellar_dex::StellarDexServiceError::UnknownError(
                "No-op DEX strategy does not provide quotes".to_string(),
            ),
        )
    }

    async fn get_xlm_to_token_quote(
        &self,
        _asset_id: &str,
        _amount: u64,
        _slippage: f32,
    ) -> Result<
        crate::services::stellar_dex::StellarQuoteResponse,
        crate::services::stellar_dex::StellarDexServiceError,
    > {
        // Noop implementation returns a default quote
        Err(
            crate::services::stellar_dex::StellarDexServiceError::UnknownError(
                "No-op DEX strategy does not provide quotes".to_string(),
            ),
        )
    }
}

#[async_trait]
impl StellarDexServiceTrait for StellarNoopDex {
    async fn get_token_to_xlm_quote(
        &self,
        _asset_id: &str,
        _amount: u64,
        _slippage: f32,
    ) -> Result<
        crate::services::stellar_dex::StellarQuoteResponse,
        crate::services::stellar_dex::StellarDexServiceError,
    > {
        Err(
            crate::services::stellar_dex::StellarDexServiceError::UnknownError(
                "No-op DEX strategy does not provide quotes".to_string(),
            ),
        )
    }

    async fn get_xlm_to_token_quote(
        &self,
        _asset_id: &str,
        _amount: u64,
        _slippage: f32,
    ) -> Result<
        crate::services::stellar_dex::StellarQuoteResponse,
        crate::services::stellar_dex::StellarDexServiceError,
    > {
        Err(
            crate::services::stellar_dex::StellarDexServiceError::UnknownError(
                "No-op DEX strategy does not provide quotes".to_string(),
            ),
        )
    }
}

/// Helper function to create the appropriate DEX implementation
pub fn create_stellar_network_dex<D>(
    relayer: &RelayerRepoModel,
    dex_service: Arc<D>,
) -> Result<StellarNetworkDex<D>, RelayerError>
where
    D: StellarDexServiceTrait + Send + Sync + 'static,
{
    // Check if swap config exists
    let swap_config = match relayer.policies.get_stellar_policy().get_swap_config() {
        Some(config) => config,
        None => {
            // No swap config, return Noop
            return Ok(StellarNetworkDex::Noop {
                dex: StellarNoopDex,
            });
        }
    };

    // Check if strategy is specified
    match swap_config.strategy {
        Some(strategy) => match strategy {
            StellarSwapStrategy::OrderBook => Ok(StellarNetworkDex::Paths {
                dex: paths_swap::PathsSwapDex::<D>::new(dex_service),
            }),
            StellarSwapStrategy::Soroswap => {
                // TODO: Implement Soroswap strategy when available
                Err(RelayerError::NotSupported(
                    "Soroswap strategy not yet implemented".to_string(),
                ))
            }
        },
        None => {
            // No strategy specified, return Noop
            Ok(StellarNetworkDex::Noop {
                dex: StellarNoopDex,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        NetworkType, RelayerNetworkPolicy, RelayerRepoModel, RelayerStellarPolicy,
        RelayerStellarSwapConfig,
    };
    use crate::services::stellar_dex::MockStellarDexServiceTrait;

    fn create_test_relayer(swap_config: Option<RelayerStellarSwapConfig>) -> RelayerRepoModel {
        RelayerRepoModel {
            id: "test-relayer".to_string(),
            name: "Test Relayer".to_string(),
            network: "testnet".to_string(),
            network_type: NetworkType::Stellar,
            policies: RelayerNetworkPolicy::Stellar(RelayerStellarPolicy {
                swap_config,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn test_create_network_dex_paths_explicit() {
        let relayer = create_test_relayer(Some(RelayerStellarSwapConfig {
            strategy: Some(StellarSwapStrategy::OrderBook),
            cron_schedule: None,
            min_balance_threshold: None,
        }));

        let dex_service = Arc::new(MockStellarDexServiceTrait::new());
        let result = create_stellar_network_dex(&relayer, dex_service);

        match result {
            Ok(StellarNetworkDex::Paths { .. }) => {}
            Ok(_) => panic!("Expected OrderBook strategy"),
            Err(e) => panic!("Expected Ok with OrderBook, but got error: {:?}", e),
        }
    }

    #[test]
    fn test_create_network_dex_noop_when_no_strategy() {
        let relayer = create_test_relayer(Some(RelayerStellarSwapConfig {
            strategy: None,
            cron_schedule: None,
            min_balance_threshold: None,
        }));

        let dex_service = Arc::new(MockStellarDexServiceTrait::new());
        let result = create_stellar_network_dex(&relayer, dex_service);

        match result {
            Ok(StellarNetworkDex::Noop { .. }) => {}
            Ok(_) => panic!("Expected Noop strategy"),
            Err(e) => panic!("Expected Ok with Noop, but got error: {:?}", e),
        }
    }

    #[test]
    fn test_create_network_dex_noop_when_no_swap_config() {
        let relayer = create_test_relayer(None);

        let dex_service = Arc::new(MockStellarDexServiceTrait::new());
        let result = create_stellar_network_dex(&relayer, dex_service);

        match result {
            Ok(StellarNetworkDex::Noop { .. }) => {}
            Ok(_) => panic!("Expected Noop strategy"),
            Err(e) => panic!("Expected Ok with Noop, but got error: {:?}", e),
        }
    }

    #[test]
    fn test_create_network_dex_soroswap_not_implemented() {
        let relayer = create_test_relayer(Some(RelayerStellarSwapConfig {
            strategy: Some(StellarSwapStrategy::Soroswap),
            cron_schedule: None,
            min_balance_threshold: None,
        }));

        let dex_service = Arc::new(MockStellarDexServiceTrait::new());
        let result = create_stellar_network_dex(&relayer, dex_service);

        match result {
            Err(RelayerError::NotSupported(msg)) => {
                assert!(msg.contains("Soroswap"));
            }
            _ => panic!("Expected NotSupported error for Soroswap"),
        }
    }

    #[test]
    fn test_resolve_strategy_default() {
        let relayer = create_test_relayer(None);
        let strategy = resolve_strategy(&relayer);
        assert_eq!(strategy, StellarSwapStrategy::OrderBook);
    }

    #[test]
    fn test_resolve_strategy_explicit() {
        let relayer = create_test_relayer(Some(RelayerStellarSwapConfig {
            strategy: Some(StellarSwapStrategy::Soroswap),
            cron_schedule: None,
            min_balance_threshold: None,
        }));
        let strategy = resolve_strategy(&relayer);
        assert_eq!(strategy, StellarSwapStrategy::Soroswap);
    }
}
