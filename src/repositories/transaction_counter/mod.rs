//! Transaction Counter Repository Module
//!
//! This module provides the transaction counter repository layer for the OpenZeppelin Relayer service.
//! It implements specialized counters for tracking transaction nonces and sequence numbers
//! across different blockchain networks, supporting both in-memory and Redis-backed storage.
//!
//! ## Repository Implementations
//!
//! - [`InMemoryTransactionCounter`]: Fast in-memory storage using DashMap for concurrency
//! - [`RedisTransactionCounter`]: Redis-backed storage for production environments
//!
//! ## Counter Operations
//!
//! The transaction counter supports several key operations:
//!
//! - **Get**: Retrieve current counter value
//! - **Get and Increment**: Atomically get current value and increment
//! - **Decrement**: Decrement counter (for rollbacks)
//! - **Set**: Set counter to specific value
//!
pub mod transaction_counter_in_memory;
pub mod transaction_counter_redis;

use redis::aio::ConnectionManager;
pub use transaction_counter_in_memory::InMemoryTransactionCounter;
pub use transaction_counter_redis::RedisTransactionCounter;

use async_trait::async_trait;
use serde::Serialize;
use std::sync::Arc;
use thiserror::Error;

#[cfg(test)]
use mockall::automock;

use crate::models::RepositoryError;

#[derive(Error, Debug, Serialize)]
pub enum TransactionCounterError {
    #[error("No sequence found for relayer {relayer_id} and address {address}")]
    SequenceNotFound { relayer_id: String, address: String },
    #[error("Counter not found for {0}")]
    NotFound(String),
}

#[allow(dead_code)]
#[async_trait]
#[cfg_attr(test, automock)]
pub trait TransactionCounterTrait {
    async fn get(&self, relayer_id: &str, address: &str) -> Result<Option<u64>, RepositoryError>;

    async fn get_and_increment(
        &self,
        relayer_id: &str,
        address: &str,
    ) -> Result<u64, RepositoryError>;

    async fn decrement(&self, relayer_id: &str, address: &str) -> Result<u64, RepositoryError>;

    async fn set(&self, relayer_id: &str, address: &str, value: u64)
        -> Result<(), RepositoryError>;
}

/// Enum wrapper for different transaction counter repository implementations
#[derive(Debug, Clone)]
pub enum TransactionCounterRepositoryStorage {
    InMemory(InMemoryTransactionCounter),
    Redis(RedisTransactionCounter),
}

impl TransactionCounterRepositoryStorage {
    pub fn new_in_memory() -> Self {
        Self::InMemory(InMemoryTransactionCounter::new())
    }
    pub fn new_redis(
        connection_manager: Arc<ConnectionManager>,
        key_prefix: String,
    ) -> Result<Self, RepositoryError> {
        Ok(Self::Redis(RedisTransactionCounter::new(
            connection_manager,
            key_prefix,
        )?))
    }
}

#[async_trait]
impl TransactionCounterTrait for TransactionCounterRepositoryStorage {
    async fn get(&self, relayer_id: &str, address: &str) -> Result<Option<u64>, RepositoryError> {
        match self {
            TransactionCounterRepositoryStorage::InMemory(counter) => {
                counter.get(relayer_id, address).await
            }
            TransactionCounterRepositoryStorage::Redis(counter) => {
                counter.get(relayer_id, address).await
            }
        }
    }

    async fn get_and_increment(
        &self,
        relayer_id: &str,
        address: &str,
    ) -> Result<u64, RepositoryError> {
        match self {
            TransactionCounterRepositoryStorage::InMemory(counter) => {
                counter.get_and_increment(relayer_id, address).await
            }
            TransactionCounterRepositoryStorage::Redis(counter) => {
                counter.get_and_increment(relayer_id, address).await
            }
        }
    }

    async fn decrement(&self, relayer_id: &str, address: &str) -> Result<u64, RepositoryError> {
        match self {
            TransactionCounterRepositoryStorage::InMemory(counter) => {
                counter.decrement(relayer_id, address).await
            }
            TransactionCounterRepositoryStorage::Redis(counter) => {
                counter.decrement(relayer_id, address).await
            }
        }
    }

    async fn set(
        &self,
        relayer_id: &str,
        address: &str,
        value: u64,
    ) -> Result<(), RepositoryError> {
        match self {
            TransactionCounterRepositoryStorage::InMemory(counter) => {
                counter.set(relayer_id, address, value).await
            }
            TransactionCounterRepositoryStorage::Redis(counter) => {
                counter.set(relayer_id, address, value).await
            }
        }
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    #[tokio::test]
    async fn test_in_memory_repository_creation() {
        let repo = TransactionCounterRepositoryStorage::new_in_memory();

        matches!(repo, TransactionCounterRepositoryStorage::InMemory(_));
    }

    #[tokio::test]
    async fn test_enum_wrapper_delegation() {
        let repo = TransactionCounterRepositoryStorage::new_in_memory();

        // Test that the enum wrapper properly delegates to the underlying implementation
        let result = repo.get("test_relayer", "0x1234").await.unwrap();
        assert_eq!(result, None);

        repo.set("test_relayer", "0x1234", 100).await.unwrap();
        let result = repo.get("test_relayer", "0x1234").await.unwrap();
        assert_eq!(result, Some(100));

        let current = repo
            .get_and_increment("test_relayer", "0x1234")
            .await
            .unwrap();
        assert_eq!(current, 100);

        let result = repo.get("test_relayer", "0x1234").await.unwrap();
        assert_eq!(result, Some(101));

        let new_value = repo.decrement("test_relayer", "0x1234").await.unwrap();
        assert_eq!(new_value, 100);
    }
}
