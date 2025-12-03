//! Multi-strategy Stellar DEX service implementation
//!
//! This module provides a DEX service that automatically selects the appropriate strategy
//! based on asset type and configured strategies. It implements `StellarDexServiceTrait`
//! and internally routes calls to the first strategy that can handle the requested asset.

use super::{
    AssetType, OrderBookService, StellarDexServiceError, StellarDexServiceTrait,
    StellarQuoteResponse, SwapExecutionResult, SwapTransactionParams,
};
use crate::services::{provider::StellarProviderTrait, signer::Signer, signer::StellarSignTrait};
use async_trait::async_trait;
use std::collections::HashSet;
use std::sync::Arc;
use tracing::debug;

/// Enum wrapper for different DEX service implementations
///
/// This enum allows storing different concrete DEX service types in a collection
/// without using dynamic dispatch (`dyn`).
#[derive(Clone)]
pub enum DexServiceWrapper<P, S>
where
    P: StellarProviderTrait + Send + Sync + 'static,
    S: StellarSignTrait + Signer + Send + Sync + 'static,
{
    /// Order Book DEX service
    OrderBook(Arc<OrderBookService<P, S>>),
    // TODO: Add Soroswap variant when implemented
    // Soroswap(Arc<SoroswapService<P, S>>),
}

impl<P, S> DexServiceWrapper<P, S>
where
    P: StellarProviderTrait + Send + Sync + 'static,
    S: StellarSignTrait + Signer + Send + Sync + 'static,
{
    fn can_handle_asset(&self, asset_id: &str) -> bool {
        match self {
            DexServiceWrapper::OrderBook(service) => service.can_handle_asset(asset_id),
            // DexServiceWrapper::Soroswap(service) => service.can_handle_asset(asset_id),
        }
    }

    fn supported_asset_types(&self) -> HashSet<AssetType> {
        match self {
            DexServiceWrapper::OrderBook(service) => service.supported_asset_types(),
            // DexServiceWrapper::Soroswap(service) => service.supported_asset_types(),
        }
    }
}

/// Multi-strategy Stellar DEX service
///
/// This service maintains a list of DEX strategy implementations (one per configured strategy)
/// and automatically selects the first one that can handle a given asset when methods are called.
/// The routing logic is integrated directly into the service implementation.
///
/// Uses static dispatch via generics and an enum wrapper instead of dynamic dispatch.
pub struct StellarDexService<P, S>
where
    P: StellarProviderTrait + Send + Sync + 'static,
    S: StellarSignTrait + Signer + Send + Sync + 'static,
{
    /// List of DEX strategy implementations in priority order (matching the configured strategies)
    strategies: Vec<DexServiceWrapper<P, S>>,
}

impl<P, S> StellarDexService<P, S>
where
    P: StellarProviderTrait + Send + Sync + 'static,
    S: StellarSignTrait + Signer + Send + Sync + 'static,
{
    /// Create a new multi-strategy DEX service with the given strategy implementations
    ///
    /// # Arguments
    /// * `strategies` - Vector of DEX strategy implementations in priority order
    pub fn new(strategies: Vec<DexServiceWrapper<P, S>>) -> Self {
        Self { strategies }
    }

    /// Find the first strategy that can handle the given asset
    ///
    /// # Arguments
    /// * `asset_id` - Asset identifier to check
    ///
    /// # Returns
    /// `Some(strategy)` if a strategy can handle the asset, `None` otherwise
    fn find_strategy_for_asset(&self, asset_id: &str) -> Option<&DexServiceWrapper<P, S>> {
        for strategy in &self.strategies {
            if strategy.can_handle_asset(asset_id) {
                debug!(
                    asset_id = %asset_id,
                    "Selected DEX strategy that can handle asset"
                );
                return Some(strategy);
            }
        }
        None
    }
}

#[async_trait]
impl<P, S> StellarDexServiceTrait for StellarDexService<P, S>
where
    P: StellarProviderTrait + Send + Sync + 'static,
    S: StellarSignTrait + Signer + Send + Sync + 'static,
{
    fn supported_asset_types(&self) -> HashSet<AssetType> {
        // Return the union of all supported asset types from all strategies
        let mut types = HashSet::new();
        for strategy in &self.strategies {
            types.extend(strategy.supported_asset_types());
        }
        types
    }

    fn can_handle_asset(&self, asset_id: &str) -> bool {
        // Check if any strategy can handle this asset
        self.find_strategy_for_asset(asset_id).is_some()
    }

    async fn get_token_to_xlm_quote(
        &self,
        asset_id: &str,
        amount: u64,
        slippage: f32,
        asset_decimals: Option<u8>,
    ) -> Result<StellarQuoteResponse, StellarDexServiceError> {
        let strategy = self.find_strategy_for_asset(asset_id).ok_or_else(|| {
            StellarDexServiceError::InvalidAssetIdentifier(format!(
                "No configured strategy can handle asset: {asset_id}"
            ))
        })?;

        match strategy {
            DexServiceWrapper::OrderBook(svc) => {
                svc.get_token_to_xlm_quote(asset_id, amount, slippage, asset_decimals)
                    .await
            } // DexServiceWrapper::Soroswap(svc) => {
              //     svc.get_token_to_xlm_quote(asset_id, amount, slippage, asset_decimals)
              //         .await
              // }
        }
    }

    async fn get_xlm_to_token_quote(
        &self,
        asset_id: &str,
        amount: u64,
        slippage: f32,
        asset_decimals: Option<u8>,
    ) -> Result<StellarQuoteResponse, StellarDexServiceError> {
        let strategy = self.find_strategy_for_asset(asset_id).ok_or_else(|| {
            StellarDexServiceError::InvalidAssetIdentifier(format!(
                "No configured strategy can handle asset: {asset_id}"
            ))
        })?;

        match strategy {
            DexServiceWrapper::OrderBook(svc) => {
                svc.get_xlm_to_token_quote(asset_id, amount, slippage, asset_decimals)
                    .await
            } // DexServiceWrapper::Soroswap(svc) => {
              //     svc.get_xlm_to_token_quote(asset_id, amount, slippage, asset_decimals)
              //         .await
              // }
        }
    }

    async fn prepare_swap_transaction(
        &self,
        params: SwapTransactionParams,
    ) -> Result<(String, StellarQuoteResponse), StellarDexServiceError> {
        let strategy = self
            .find_strategy_for_asset(&params.source_asset)
            .ok_or_else(|| {
                StellarDexServiceError::InvalidAssetIdentifier(format!(
                    "No configured strategy can handle asset: {}",
                    params.source_asset
                ))
            })?;

        match strategy {
            DexServiceWrapper::OrderBook(svc) => svc.prepare_swap_transaction(params).await,
            // DexServiceWrapper::Soroswap(svc) => svc.prepare_swap_transaction(params).await,
        }
    }

    async fn execute_swap(
        &self,
        params: SwapTransactionParams,
    ) -> Result<SwapExecutionResult, StellarDexServiceError> {
        let strategy = self
            .find_strategy_for_asset(&params.source_asset)
            .ok_or_else(|| {
                StellarDexServiceError::InvalidAssetIdentifier(format!(
                    "No configured strategy can handle asset: {}",
                    params.source_asset
                ))
            })?;

        match strategy {
            DexServiceWrapper::OrderBook(svc) => svc.execute_swap(params).await,
            // DexServiceWrapper::Soroswap(svc) => svc.execute_swap(params).await,
        }
    }
}
