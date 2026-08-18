//! This module provides a service for managing transaction counters.
//!
//! The `TransactionCounterService` struct offers methods to get, increment,
//! decrement, and set transaction counts associated with a specific relayer
//! and address. It uses an in-memory store to keep track of these counts.
use std::sync::Arc;

use crate::repositories::{TransactionCounterError, TransactionCounterTrait};
use async_trait::async_trait;

#[cfg(test)]
use mockall::automock;

#[derive(Clone, Debug)]
pub struct TransactionCounterService<T> {
    relayer_id: String,
    address: String,
    store: Arc<T>,
}

impl<T> TransactionCounterService<T> {
    pub fn new(relayer_id: String, address: String, store: Arc<T>) -> Self {
        Self {
            relayer_id,
            address,
            store,
        }
    }
}

#[async_trait]
#[cfg_attr(test, automock)]
pub trait TransactionCounterServiceTrait: Send + Sync {
    async fn get(&self) -> Result<Option<u64>, TransactionCounterError>;
    async fn get_and_increment(&self) -> Result<u64, TransactionCounterError>;
    async fn decrement(&self) -> Result<u64, TransactionCounterError>;

    /// Blind unconditional write of the counter to `value`.
    ///
    /// This overwrites the counter regardless of its current value. Production code should
    /// prefer `sync_floor` (to atomically raise the counter) or `set_if_equals` (to atomically
    /// lower it under a compare-and-set guard); `set` is intended for tests and bootstrap.
    async fn set(&self, value: u64) -> Result<(), TransactionCounterError>;

    /// Monotonically raise the counter to `floor` only if the current value is lower.
    ///
    /// Used for race-safe sync with the live chain sequence: the chain value is treated as a
    /// floor, never as the authoritative next assignment, so concurrent allocations that have
    /// already advanced the counter beyond `floor` are never rewound. Returns the effective
    /// value after the operation (always `>= floor`).
    async fn sync_floor(&self, floor: u64) -> Result<u64, TransactionCounterError>;

    /// Atomic compare-and-set used for rewind/rollback.
    ///
    /// Sets the counter to `value` only if the current value equals `expected` AND
    /// `value < expected`, guarding against clobbering a counter that has moved on
    /// since it was last observed. Returns whether the write applied. A missing key
    /// is a no-op and returns `false`.
    async fn set_if_equals(
        &self,
        expected: u64,
        value: u64,
    ) -> Result<bool, TransactionCounterError>;
}

#[async_trait]
#[allow(dead_code)]
impl<T> TransactionCounterServiceTrait for TransactionCounterService<T>
where
    T: TransactionCounterTrait + Send + Sync,
{
    async fn get(&self) -> Result<Option<u64>, TransactionCounterError> {
        self.store
            .get(&self.relayer_id, &self.address)
            .await
            .map_err(|e| TransactionCounterError::NotFound(e.to_string()))
    }

    async fn get_and_increment(&self) -> Result<u64, TransactionCounterError> {
        self.store
            .get_and_increment(&self.relayer_id, &self.address)
            .await
            .map_err(|e| TransactionCounterError::NotFound(e.to_string()))
    }

    async fn decrement(&self) -> Result<u64, TransactionCounterError> {
        self.store
            .decrement(&self.relayer_id, &self.address)
            .await
            .map_err(|e| TransactionCounterError::NotFound(e.to_string()))
    }

    async fn set(&self, value: u64) -> Result<(), TransactionCounterError> {
        self.store
            .set(&self.relayer_id, &self.address, value)
            .await
            .map_err(|e| TransactionCounterError::NotFound(e.to_string()))
    }

    async fn sync_floor(&self, floor: u64) -> Result<u64, TransactionCounterError> {
        self.store
            .sync_floor(&self.relayer_id, &self.address, floor)
            .await
            .map_err(|e| TransactionCounterError::NotFound(e.to_string()))
    }

    async fn set_if_equals(
        &self,
        expected: u64,
        value: u64,
    ) -> Result<bool, TransactionCounterError> {
        self.store
            .set_if_equals(&self.relayer_id, &self.address, expected, value)
            .await
            .map_err(|e| TransactionCounterError::NotFound(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repositories::InMemoryTransactionCounter;

    #[tokio::test]
    async fn test_transaction_counter() {
        let store = Arc::new(InMemoryTransactionCounter::default());
        let service =
            TransactionCounterService::new("relayer_id".to_string(), "address".to_string(), store);

        assert_eq!(service.get().await.unwrap(), None);
        assert_eq!(service.get_and_increment().await.unwrap(), 0);
        assert_eq!(service.get_and_increment().await.unwrap(), 1);
        assert_eq!(service.decrement().await.unwrap(), 1);
        assert!(service.set(10).await.is_ok());
        assert_eq!(service.get().await.unwrap(), Some(10));
    }

    #[tokio::test]
    async fn test_sync_floor_does_not_rewind() {
        let store = Arc::new(InMemoryTransactionCounter::default());
        let service =
            TransactionCounterService::new("relayer_id".to_string(), "address".to_string(), store);

        // Counter already advanced past the chain floor by concurrent allocations.
        service.set(15).await.unwrap();

        // A stale/lower chain floor must not rewind the counter.
        let effective = service.sync_floor(6).await.unwrap();
        assert_eq!(effective, 15);
        assert_eq!(service.get().await.unwrap(), Some(15));

        // A genuinely-ahead floor does advance the counter.
        let effective = service.sync_floor(20).await.unwrap();
        assert_eq!(effective, 20);
        assert_eq!(service.get().await.unwrap(), Some(20));
    }

    #[tokio::test]
    async fn test_set_if_equals_delegates_to_store() {
        let store = Arc::new(InMemoryTransactionCounter::default());
        let service =
            TransactionCounterService::new("relayer_id".to_string(), "address".to_string(), store);

        service.set(15).await.unwrap();

        // A stale `expected` must not apply.
        let applied = service.set_if_equals(10, 5).await.unwrap();
        assert!(!applied);
        assert_eq!(service.get().await.unwrap(), Some(15));

        // A matching `expected` with a genuinely lower `value` applies.
        let applied = service.set_if_equals(15, 5).await.unwrap();
        assert!(applied);
        assert_eq!(service.get().await.unwrap(), Some(5));
    }
}
