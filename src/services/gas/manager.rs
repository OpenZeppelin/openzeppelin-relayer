//! Gas Price Manager Module
//!
//! This module provides a centralized manager for gas price caching across multiple
//! EVM networks. It handles background updates and a stale-while-revalidate logic.

use crate::{
    config::GasPriceCacheConfig,
    services::gas::cache::{GasPriceCache, GasPriceCacheEntry},
};
use alloy::rpc::types::FeeHistory;
use dashmap::DashMap;
use log::info;
use std::{sync::Arc, time::Duration};

#[cfg(test)]
use mockall::automock;

#[cfg_attr(test, automock)]
#[async_trait::async_trait]
pub trait GasPriceManagerTrait: Send + Sync {
    /// Configures caching for a specific network
    fn configure_network(&self, chain_id: u64, config: GasPriceCacheConfig);

    /// Returns cached components (jsonrpc gas price, base fee, fee history) if present and not expired.
    /// Stale entries are also returned.
    async fn get_cached_components(&self, chain_id: u64) -> Option<(u128, u128, FeeHistory)>;

    /// Upserts a cache entry from externally-fetched components
    async fn update_from_components(
        &self,
        chain_id: u64,
        gas_price: u128,
        base_fee_per_gas: u128,
        fee_history: FeeHistory,
    );

    /// Clears the cache
    fn clear_cache(&self);

    /// Sets a cache entry directly (primarily for testing)
    #[cfg(test)]
    async fn set_cache_entry(&self, chain_id: u64, entry: GasPriceCacheEntry);
}

#[async_trait::async_trait]
impl GasPriceManagerTrait for GasPriceManager {
    fn configure_network(&self, chain_id: u64, config: GasPriceCacheConfig) {
        self.network_configs.insert(chain_id, config);
    }

    async fn get_cached_components(&self, chain_id: u64) -> Option<(u128, u128, FeeHistory)> {
        // Check if caching is enabled for this network
        let config = self.network_configs.get(&chain_id)?;
        if !config.enabled {
            return None;
        }

        // Try to get from cache
        if let Some(entry) = self.cache.get(chain_id).await {
            if entry.is_fresh() || entry.is_stale() {
                return Some((
                    entry.gas_price,
                    entry.base_fee_per_gas,
                    entry.fee_history.clone(),
                ));
            }
        }
        None
    }

    async fn update_from_components(
        &self,
        chain_id: u64,
        gas_price: u128,
        base_fee_per_gas: u128,
        fee_history: FeeHistory,
    ) {
        // If caching is disabled or missing config, ignore the update
        let Some(cfg) = self.network_configs.get(&chain_id) else {
            return;
        };
        if !cfg.enabled {
            return;
        }

        let entry = GasPriceCacheEntry::new(
            gas_price,
            base_fee_per_gas,
            fee_history,
            None,
            Duration::from_millis(cfg.stale_after_ms),
            Duration::from_millis(cfg.expire_after_ms),
        );

        self.cache.set(chain_id, entry).await;
        info!("Updated gas price cache for chain_id {}", chain_id);
    }

    fn clear_cache(&self) {
        self.clear_cache();
    }

    #[cfg(test)]
    async fn set_cache_entry(&self, chain_id: u64, entry: GasPriceCacheEntry) {
        self.set_cache_entry(chain_id, entry).await;
    }
}

/// Manages gas price caching for multiple EVM networks
#[derive(Debug, Clone)]
pub struct GasPriceManager {
    /// The underlying cache storage
    cache: Arc<GasPriceCache>,
    /// Network-specific cache configurations
    network_configs: Arc<DashMap<u64, GasPriceCacheConfig>>,
}

impl GasPriceManager {
    /// Creates a new gas price manager
    pub fn new() -> Self {
        Self {
            cache: Arc::new(GasPriceCache::new()),
            network_configs: Arc::new(DashMap::new()),
        }
    }

    /// Clears the cache
    pub fn clear_cache(&self) {
        self.cache.clear();
    }

    #[cfg(test)]
    pub async fn set_cache_entry(&self, chain_id: u64, entry: GasPriceCacheEntry) {
        self.cache.set(chain_id, entry).await;
    }
}

impl Default for GasPriceManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_gas_price_manager_cache_disabled() {
        let manager = GasPriceManager::new();
        assert!(manager.get_cached_components(1).await.is_none());

        let gas_price = 20_000_000_000u128;
        let base_fee = 10_000_000_000u128;
        let fee_history = FeeHistory {
            oldest_block: 0,
            base_fee_per_gas: vec![],
            gas_used_ratio: vec![],
            reward: Some(vec![vec![
                1_000_000_000,
                2_000_000_000,
                3_000_000_000,
                4_000_000_000,
            ]]),
            base_fee_per_blob_gas: vec![],
            blob_gas_used_ratio: vec![],
        };

        manager.configure_network(1, GasPriceCacheConfig::default());
        manager
            .update_from_components(1, gas_price, base_fee, fee_history)
            .await;
        assert!(manager.get_cached_components(1).await.is_some());
    }
}
