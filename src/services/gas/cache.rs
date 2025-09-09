//! Gas Price Cache Module
//!
//! This module provides caching functionality for EVM gas prices to reduce RPC calls
//! and improve performance. It implements a stale-while-revalidate pattern for optimal
//! response times.

use crate::{
    config::GasPriceCacheConfig,
    constants::{GAS_PRICE_CACHE_REFRESH_TIMEOUT_SECS, HISTORICAL_BLOCKS},
    models::{EvmNetwork, TransactionError},
    services::EvmProviderTrait,
};
use alloy::rpc::types::{BlockNumberOrTag, FeeHistory};
use dashmap::DashMap;
use log::info;
use std::{
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};
use tokio::sync::RwLock;

#[derive(Debug, Clone)]
pub struct GasPriceSnapshot {
    pub gas_price: u128,
    pub base_fee_per_gas: u128,
    pub fee_history: FeeHistory,
    pub is_stale: bool,
}

/// Represents an entry in the gas price cache.
#[derive(Clone, Debug)]
pub struct GasPriceCacheEntry {
    pub gas_price: u128,
    pub base_fee_per_gas: u128,
    pub fee_history: FeeHistory,

    pub fetched_at: Instant,
    pub stale_after: Duration,
    pub expire_after: Duration,
}

impl GasPriceCacheEntry {
    /// Creates a new cache entry.
    pub fn new(
        gas_price: u128,
        base_fee_per_gas: u128,
        fee_history: FeeHistory,
        stale_after: Duration,
        expire_after: Duration,
    ) -> Self {
        Self {
            gas_price,
            base_fee_per_gas,
            fee_history,
            fetched_at: Instant::now(),
            stale_after,
            expire_after,
        }
    }

    /// Checks if the cache entry is still fresh
    pub fn is_fresh(&self) -> bool {
        self.fetched_at.elapsed() < self.stale_after
    }

    /// Checks if the cache entry is stale but not expired
    pub fn is_stale(&self) -> bool {
        let elapsed = self.fetched_at.elapsed();
        elapsed >= self.stale_after && elapsed < self.expire_after
    }

    /// Checks if the cache entry has expired
    pub fn is_expired(&self) -> bool {
        self.fetched_at.elapsed() >= self.expire_after
    }

    /// Returns the age of the cache entry
    pub fn age(&self) -> Duration {
        self.fetched_at.elapsed()
    }
}

/// Thread-safe gas price cache supporting multiple networks
#[derive(Debug)]
pub struct GasPriceCache {
    /// Cache storage mapping chain_id to cached entries
    entries: Arc<DashMap<u64, Arc<RwLock<GasPriceCacheEntry>>>>,
    /// Network-specific cache configurations
    network_configs: Arc<DashMap<u64, GasPriceCacheConfig>>,
    /// Track ongoing refresh operations to prevent duplicates
    refreshing_networks: Arc<DashMap<u64, Instant>>,
}

impl GasPriceCache {
    pub fn global() -> &'static Arc<Self> {
        static GLOBAL_CACHE: OnceLock<Arc<GasPriceCache>> = OnceLock::new();
        GLOBAL_CACHE.get_or_init(|| Arc::new(Self::create_instance()))
    }

    #[cfg(test)]
    pub fn new_instance() -> Self {
        Self::create_instance()
    }

    fn create_instance() -> Self {
        Self {
            entries: Arc::new(DashMap::new()),
            network_configs: Arc::new(DashMap::new()),
            refreshing_networks: Arc::new(DashMap::new()),
        }
    }

    pub fn configure_network(&self, chain_id: u64, config: GasPriceCacheConfig) {
        self.network_configs.insert(chain_id, config);
    }

    pub fn has_configuration_for_network(&self, chain_id: u64) -> bool {
        self.network_configs.contains_key(&chain_id)
    }

    /// Removes all data for a specific network (both config and cached entries)
    pub fn remove_network(&self, chain_id: u64) -> bool {
        let config_removed = self.network_configs.remove(&chain_id).is_some();
        let entries_removed = self.entries.remove(&chain_id).is_some();
        config_removed || entries_removed
    }

    /// Returns a snapshot of cached gas pricing components if present and not expired.
    /// Includes stale flag for stale-while-revalidate strategies.
    pub async fn get_snapshot(&self, chain_id: u64) -> Option<GasPriceSnapshot> {
        let config = self.network_configs.get(&chain_id)?;
        if !config.enabled {
            return None;
        }

        if let Some(entry) = self.entries.get(&chain_id) {
            let cached = entry.read().await;
            if cached.is_fresh() || cached.is_stale() {
                return Some(GasPriceSnapshot {
                    gas_price: cached.gas_price,
                    base_fee_per_gas: cached.base_fee_per_gas,
                    fee_history: cached.fee_history.clone(),
                    is_stale: cached.is_stale(),
                });
            }
        }
        None
    }

    pub async fn set_snapshot(
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
            Duration::from_millis(cfg.stale_after_ms),
            Duration::from_millis(cfg.expire_after_ms),
        );

        self.set(chain_id, entry).await;
        info!("Updated gas price snapshot for chain_id {}", chain_id);
    }

    /// Gets a cached entry for the given chain ID
    pub async fn get(&self, chain_id: u64) -> Option<GasPriceCacheEntry> {
        if let Some(entry) = self.entries.get(&chain_id) {
            let cached = entry.read().await;
            Some(cached.clone())
        } else {
            None
        }
    }

    /// Sets a cache entry for the given chain ID
    pub async fn set(&self, chain_id: u64, entry: GasPriceCacheEntry) {
        let entry = Arc::new(RwLock::new(entry));
        self.entries.insert(chain_id, entry);
    }

    /// Updates an existing cache entry
    pub async fn update<F>(&self, chain_id: u64, updater: F) -> Result<(), TransactionError>
    where
        F: FnOnce(&mut GasPriceCacheEntry),
    {
        if let Some(entry) = self.entries.get(&chain_id) {
            let mut cached = entry.write().await;
            updater(&mut cached);
            Ok(())
        } else {
            Err(TransactionError::NetworkConfiguration(
                "Cache entry not found".into(),
            ))
        }
    }

    /// Removes a cache entry
    pub fn remove(&self, chain_id: u64) -> Option<()> {
        self.entries.remove(&chain_id).map(|_| ())
    }

    /// Clears all cache entries
    pub fn clear(&self) {
        self.entries.clear();
    }

    /// Returns the number of cached entries
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Checks if the cache is empty
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Triggers a background refresh for the specified network if not already refreshing.
    pub fn refresh_network_in_background(
        &self,
        network: &EvmNetwork,
        reward_percentiles: Vec<f64>,
    ) -> bool {
        let now = Instant::now();

        // Clean up old refresh entries (probably stuck)
        let cleanup_threshold = Duration::from_secs(GAS_PRICE_CACHE_REFRESH_TIMEOUT_SECS);
        self.refreshing_networks
            .retain(|_, started_at| now.duration_since(*started_at) < cleanup_threshold);

        let already_refreshing = self
            .refreshing_networks
            .insert(network.chain_id, now)
            .is_some();
        if already_refreshing {
            return false;
        }

        // Start the background refresh
        let network = network.clone();

        // Clone the Arc references
        let entries = self.entries.clone();
        let network_configs = self.network_configs.clone();
        let refreshing_networks = self.refreshing_networks.clone();

        tokio::spawn(async move {
            let refresh = async {
                // Get network provider and fetch fresh data
                let provider = crate::services::get_network_provider(&network, None).ok()?;
                let fresh_gas_price = provider.get_gas_price().await.ok()?;
                let block = provider.get_block_by_number().await.ok()?;
                let fresh_base_fee: u128 = block.header.base_fee_per_gas.unwrap_or(0).into();
                let fee_hist = provider
                    .get_fee_history(
                        HISTORICAL_BLOCKS,
                        BlockNumberOrTag::Latest,
                        reward_percentiles,
                    )
                    .await
                    .ok()?;

                // Update the cache using the cloned Arc references
                // Check if caching is enabled for this network
                let cfg = network_configs.get(&network.chain_id)?;
                if !cfg.enabled {
                    return None;
                }

                let entry = GasPriceCacheEntry::new(
                    fresh_gas_price,
                    fresh_base_fee,
                    fee_hist,
                    Duration::from_millis(cfg.stale_after_ms),
                    Duration::from_millis(cfg.expire_after_ms),
                );

                let entry = Arc::new(RwLock::new(entry));
                entries.insert(network.chain_id, entry);
                info!(
                    "Updated gas price snapshot for chain_id {} in background",
                    network.chain_id
                );
                Some(())
            };

            // Execute refresh and clean up tracking
            let _ = refresh.await;
            refreshing_networks.remove(&network.chain_id);
        });

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::rpc::types::FeeHistory;

    fn create_test_components() -> (u128, u128, FeeHistory) {
        (
            20_000_000_000,
            10_000_000_000,
            FeeHistory {
                oldest_block: 100,
                base_fee_per_gas: vec![10_000_000_000],
                gas_used_ratio: vec![0.5],
                reward: Some(vec![vec![
                    1_000_000_000,
                    2_000_000_000,
                    3_000_000_000,
                    4_000_000_000,
                ]]),
                base_fee_per_blob_gas: vec![],
                blob_gas_used_ratio: vec![],
            },
        )
    }

    #[tokio::test]
    async fn test_cache_entry_freshness() {
        let (gas_price, base_fee, fee_history) = create_test_components();
        let entry = GasPriceCacheEntry::new(
            gas_price,
            base_fee,
            fee_history,
            Duration::from_secs(30),
            Duration::from_secs(120),
        );

        assert!(entry.is_fresh());
        assert!(!entry.is_stale());
        assert!(!entry.is_expired());
    }

    #[tokio::test]
    async fn test_cache_basic_operations() {
        let cache = GasPriceCache::new_instance();
        let chain_id = 1u64;

        // Test empty cache
        assert!(cache.get(chain_id).await.is_none());
        assert!(cache.is_empty());

        // Test set and get
        let (gas_price, base_fee, fee_history) = create_test_components();
        let entry = GasPriceCacheEntry::new(
            gas_price,
            base_fee,
            fee_history,
            Duration::from_secs(30),
            Duration::from_secs(120),
        );

        cache.set(chain_id, entry.clone()).await;
        assert_eq!(cache.len(), 1);

        let retrieved = cache.get(chain_id).await.unwrap();
        assert_eq!(retrieved.gas_price, entry.gas_price);
    }

    #[tokio::test]
    async fn test_cache_update() {
        let cache = GasPriceCache::new_instance();
        let chain_id = 1u64;

        let (gas_price, base_fee, fee_history) = create_test_components();
        let entry = GasPriceCacheEntry::new(
            gas_price,
            base_fee,
            fee_history,
            Duration::from_secs(30),
            Duration::from_secs(120),
        );

        cache.set(chain_id, entry).await;

        // Update the entry
        cache
            .update(chain_id, |entry| {
                entry.gas_price = 25_000_000_000;
            })
            .await
            .unwrap();

        let updated = cache.get(chain_id).await.unwrap();
        assert_eq!(updated.gas_price, 25_000_000_000);
    }

    #[tokio::test]
    async fn test_cache_clear() {
        let cache = GasPriceCache::new_instance();

        // Add multiple entries
        for chain_id in 1..=3 {
            let (gas_price, base_fee, fee_history) = create_test_components();
            let entry = GasPriceCacheEntry::new(
                gas_price,
                base_fee,
                fee_history,
                Duration::from_secs(30),
                Duration::from_secs(120),
            );
            cache.set(chain_id, entry).await;
        }

        assert_eq!(cache.len(), 3);

        // Clear cache
        cache.clear();
        assert!(cache.is_empty());
    }

    #[tokio::test]
    async fn test_network_management() {
        use crate::config::GasPriceCacheConfig;

        let cache = GasPriceCache::new_instance();
        let chain_id = 1u64;

        // Initially no entries or config
        assert!(!cache.has_configuration_for_network(chain_id));

        // Add configuration
        let config = GasPriceCacheConfig {
            enabled: true,
            stale_after_ms: 30000,
            expire_after_ms: 120000,
        };
        cache.configure_network(chain_id, config);

        assert!(cache.has_configuration_for_network(chain_id));

        // Add cache entry
        let (gas_price, base_fee, fee_history) = create_test_components();
        let entry = GasPriceCacheEntry::new(
            gas_price,
            base_fee,
            fee_history,
            Duration::from_secs(30),
            Duration::from_secs(120),
        );
        cache.set(chain_id, entry).await;

        // Now we have entries
        assert!(cache.has_configuration_for_network(chain_id));
        assert_eq!(cache.len(), 1);

        // Remove all network data
        assert!(cache.remove_network(chain_id));

        // Verify everything is removed
        assert!(!cache.has_configuration_for_network(chain_id));
        assert!(cache.is_empty());

        // Removing again should return false (nothing to remove)
        assert!(!cache.remove_network(chain_id));
    }
}
