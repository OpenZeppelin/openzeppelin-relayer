//! Status check metadata store for circuit breaker failure counters.
//!
//! Transaction status checks track consecutive and total failure counts to implement
//! a circuit breaker pattern. These counters are stored independently from job metadata
//! so they persist across job retries.
//!
//! This module provides a trait abstraction and a Redis implementation, allowing
//! handlers to be decoupled from infrastructure.

use async_trait::async_trait;
use deadpool_redis::Pool;
use eyre::Result;
use redis::AsyncCommands;
use std::sync::Arc;
use tracing::{debug, warn};

use crate::config::ServerConfig;

/// Redis key prefix for transaction status check metadata (failure counters).
const TX_STATUS_CHECK_METADATA_PREFIX: &str = "queue:tx_status_check_metadata";

/// Abstraction for reading/writing status check failure counters.
///
/// Implementations store per-transaction counters used by the circuit breaker
/// in the status check handler. The two counters are:
/// - `consecutive`: resets to 0 on each successful (non-final) status check
/// - `total`: monotonically increasing across the lifetime of the status check
#[async_trait]
pub trait StatusCheckMetadataStore: Send + Sync + std::fmt::Debug {
    /// Reads failure counters for a transaction.
    /// Returns `(consecutive_failures, total_failures)`, defaulting to `(0, 0)` if not found.
    async fn read_counters(&self, tx_id: &str) -> (u32, u32);

    /// Updates failure counters for a transaction.
    async fn update_counters(&self, tx_id: &str, consecutive: u32, total: u32) -> Result<()>;

    /// Deletes failure counters when a transaction reaches final state.
    async fn delete_counters(&self, tx_id: &str) -> Result<()>;
}

/// Redis-backed implementation of `StatusCheckMetadataStore`.
///
/// Stores counters as a Redis hash with fields `consecutive` and `total`,
/// with a 12-hour TTL refreshed on every update (inactivity timeout).
#[derive(Debug)]
pub struct RedisStatusCheckMetadataStore {
    pool: Arc<Pool>,
}

impl RedisStatusCheckMetadataStore {
    pub fn new(pool: Arc<Pool>) -> Self {
        Self { pool }
    }

    fn get_metadata_key(tx_id: &str) -> String {
        let redis_key_prefix = ServerConfig::get_redis_key_prefix();
        format!("{redis_key_prefix}:{TX_STATUS_CHECK_METADATA_PREFIX}:{tx_id}")
    }
}

#[async_trait]
impl StatusCheckMetadataStore for RedisStatusCheckMetadataStore {
    async fn read_counters(&self, tx_id: &str) -> (u32, u32) {
        let key = Self::get_metadata_key(tx_id);

        let result: Result<(u32, u32)> = async {
            let mut conn = self
                .pool
                .get()
                .await
                .map_err(|e| eyre::eyre!("Failed to get Redis connection: {e}"))?;

            let values: Vec<Option<String>> = conn
                .hget(&key, &["consecutive", "total"])
                .await
                .map_err(|e| eyre::eyre!("Failed to read counters from Redis: {e}"))?;

            let consecutive = values
                .first()
                .and_then(|v| v.as_ref())
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            let total = values
                .get(1)
                .and_then(|v| v.as_ref())
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);

            Ok((consecutive, total))
        }
        .await;

        match result {
            Ok(counters) => counters,
            Err(e) => {
                warn!(error = %e, tx_id = %tx_id, "failed to read counters from Redis, using defaults");
                (0, 0)
            }
        }
    }

    async fn update_counters(&self, tx_id: &str, consecutive: u32, total: u32) -> Result<()> {
        let key = Self::get_metadata_key(tx_id);

        let mut conn = self
            .pool
            .get()
            .await
            .map_err(|e| eyre::eyre!("Failed to get Redis connection: {e}"))?;

        let (ttl_result,): (i64,) = redis::pipe()
            .hset_multiple(
                &key,
                &[
                    ("consecutive", consecutive.to_string()),
                    ("total", total.to_string()),
                ],
            )
            .ignore()
            .expire(&key, 43200) // 12 hours TTL
            .query_async(&mut *conn)
            .await
            .map_err(|e| eyre::eyre!("Failed to update counters in Redis: {e}"))?;

        let ttl_set = ttl_result == 1;

        debug!(
            tx_id = %tx_id,
            consecutive,
            total,
            key,
            ttl_set,
            "updated status check counters in Redis"
        );

        Ok(())
    }

    async fn delete_counters(&self, tx_id: &str) -> Result<()> {
        let key = Self::get_metadata_key(tx_id);

        let mut conn = self
            .pool
            .get()
            .await
            .map_err(|e| eyre::eyre!("Failed to get Redis connection: {e}"))?;

        conn.del::<_, ()>(&key)
            .await
            .map_err(|e| eyre::eyre!("Failed to delete counters from Redis: {e}"))?;

        debug!(tx_id = %tx_id, key, "deleted status check counters from Redis");

        Ok(())
    }
}

/// In-memory no-op implementation for testing and non-Redis deployments.
#[derive(Debug)]
pub struct NoOpStatusCheckMetadataStore;

#[async_trait]
impl StatusCheckMetadataStore for NoOpStatusCheckMetadataStore {
    async fn read_counters(&self, _tx_id: &str) -> (u32, u32) {
        (0, 0)
    }

    async fn update_counters(&self, _tx_id: &str, _consecutive: u32, _total: u32) -> Result<()> {
        Ok(())
    }

    async fn delete_counters(&self, _tx_id: &str) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metadata_key_format() {
        let key = RedisStatusCheckMetadataStore::get_metadata_key("tx123");
        assert!(key.contains(TX_STATUS_CHECK_METADATA_PREFIX));
        assert!(key.contains("tx123"));
    }

    #[tokio::test]
    async fn test_noop_store_defaults() {
        let store = NoOpStatusCheckMetadataStore;
        assert_eq!(store.read_counters("tx1").await, (0, 0));
        assert!(store.update_counters("tx1", 5, 10).await.is_ok());
        assert!(store.delete_counters("tx1").await.is_ok());
    }
}
