//! This module provides an in-memory implementation of a transaction counter.
//!
//! The `InMemoryTransactionCounter` struct is used to track and manage transaction nonces
//! for different relayers and addresses. It supports operations to get, increment, decrement,
//! and set nonce values. This implementation uses a `DashMap` for concurrent access and
//! modification of the nonce values.
use async_trait::async_trait;
use dashmap::DashMap;

use crate::repositories::{RepositoryError, TransactionCounterTrait};

#[derive(Debug, Default, Clone)]
pub struct InMemoryTransactionCounter {
    store: DashMap<(String, String), u64>, // (relayer_id, address) -> nonce/sequence
}

impl InMemoryTransactionCounter {
    pub fn new() -> Self {
        Self {
            store: DashMap::new(),
        }
    }
}

#[async_trait]
impl TransactionCounterTrait for InMemoryTransactionCounter {
    async fn get(&self, relayer_id: &str, address: &str) -> Result<Option<u64>, RepositoryError> {
        Ok(self
            .store
            .get(&(relayer_id.to_string(), address.to_string()))
            .map(|n| *n))
    }

    async fn get_and_increment(
        &self,
        relayer_id: &str,
        address: &str,
    ) -> Result<u64, RepositoryError> {
        let mut entry = self
            .store
            .entry((relayer_id.to_string(), address.to_string()))
            .or_insert(0);
        let current = *entry;
        *entry += 1;
        Ok(current)
    }

    async fn decrement(&self, relayer_id: &str, address: &str) -> Result<u64, RepositoryError> {
        let mut entry = self
            .store
            .get_mut(&(relayer_id.to_string(), address.to_string()))
            .ok_or_else(|| {
                RepositoryError::NotFound(format!("Counter not found for {}", address))
            })?;
        if *entry > 0 {
            *entry -= 1;
        }
        Ok(*entry)
    }

    async fn set(
        &self,
        relayer_id: &str,
        address: &str,
        value: u64,
    ) -> Result<(), RepositoryError> {
        self.store
            .insert((relayer_id.to_string(), address.to_string()), value);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_decrement_not_found() {
        let store = InMemoryTransactionCounter::new();
        let result = store.decrement("nonexistent", "0x1234").await;
        assert!(matches!(result, Err(RepositoryError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_nonce_store() {
        let store = InMemoryTransactionCounter::new();
        let relayer_id = "relayer_1";
        let address = "0x1234";

        // Initially should be None
        assert_eq!(store.get(relayer_id, address).await.unwrap(), None);

        // Set a value explicitly
        store.set(relayer_id, address, 100).await.unwrap();
        assert_eq!(store.get(relayer_id, address).await.unwrap(), Some(100));

        // Increment
        assert_eq!(
            store.get_and_increment(relayer_id, address).await.unwrap(),
            100
        );
        assert_eq!(store.get(relayer_id, address).await.unwrap(), Some(101));

        // Decrement
        assert_eq!(store.decrement(relayer_id, address).await.unwrap(), 100);
        assert_eq!(store.get(relayer_id, address).await.unwrap(), Some(100));
    }

    #[tokio::test]
    async fn test_multiple_relayers() {
        let store = InMemoryTransactionCounter::new();

        // Setup different relayer/address combinations
        store.set("relayer_1", "0x1234", 100).await.unwrap();
        store.set("relayer_1", "0x5678", 200).await.unwrap();
        store.set("relayer_2", "0x1234", 300).await.unwrap();

        // Verify independent counters
        assert_eq!(store.get("relayer_1", "0x1234").await.unwrap(), Some(100));
        assert_eq!(store.get("relayer_1", "0x5678").await.unwrap(), Some(200));
        assert_eq!(store.get("relayer_2", "0x1234").await.unwrap(), Some(300));

        // Verify independent increments
        assert_eq!(
            store
                .get_and_increment("relayer_1", "0x1234")
                .await
                .unwrap(),
            100
        );
        assert_eq!(
            store
                .get_and_increment("relayer_1", "0x1234")
                .await
                .unwrap(),
            101
        );
        assert_eq!(
            store
                .get_and_increment("relayer_1", "0x5678")
                .await
                .unwrap(),
            200
        );
        assert_eq!(
            store
                .get_and_increment("relayer_1", "0x5678")
                .await
                .unwrap(),
            201
        );
        assert_eq!(store.get("relayer_2", "0x1234").await.unwrap(), Some(300));
    }
}
