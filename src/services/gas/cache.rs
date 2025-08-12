//! Gas Price Cache Module
//!
//! This module provides caching functionality for EVM gas prices to reduce RPC calls
//! and improve performance. It implements a stale-while-revalidate pattern for optimal
//! response times.

use crate::{models::TransactionError, services::gas::l2_fee::L2FeeData};
use alloy::rpc::types::FeeHistory;
use dashmap::DashMap;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::RwLock;

/// Represents an entry in the gas price cache.
#[derive(Clone, Debug)]
pub struct GasPriceCacheEntry {
    pub gas_price: u128,
    pub base_fee_per_gas: u128,
    pub fee_history: FeeHistory,
    pub l2_fee_data: Option<L2FeeData>,
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
        l2_fee_data: Option<L2FeeData>,
        stale_after: Duration,
        expire_after: Duration,
    ) -> Self {
        Self {
            gas_price,
            base_fee_per_gas,
            fee_history,
            l2_fee_data,
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
}

impl GasPriceCache {
    /// Creates a new empty gas price cache
    pub fn new() -> Self {
        Self {
            entries: Arc::new(DashMap::new()),
        }
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
}

impl Default for GasPriceCache {
    fn default() -> Self {
        Self::new()
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
            None,
            Duration::from_secs(30),
            Duration::from_secs(120),
        );

        assert!(entry.is_fresh());
        assert!(!entry.is_stale());
        assert!(!entry.is_expired());
    }

    #[tokio::test]
    async fn test_cache_basic_operations() {
        let cache = GasPriceCache::new();
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
            None,
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
        let cache = GasPriceCache::new();
        let chain_id = 1u64;

        let (gas_price, base_fee, fee_history) = create_test_components();
        let entry = GasPriceCacheEntry::new(
            gas_price,
            base_fee,
            fee_history,
            None,
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
        let cache = GasPriceCache::new();

        // Add multiple entries
        for chain_id in 1..=3 {
            let (gas_price, base_fee, fee_history) = create_test_components();
            let entry = GasPriceCacheEntry::new(
                gas_price,
                base_fee,
                fee_history,
                None,
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
}
