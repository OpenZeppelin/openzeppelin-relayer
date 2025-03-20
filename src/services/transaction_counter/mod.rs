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

#[derive(Clone)]
pub struct TransactionCounterService<T: TransactionCounterTrait + Send + Sync> {
    relayer_id: String,
    address: String,
    store: Arc<T>,
}

impl<T: TransactionCounterTrait + Send + Sync> TransactionCounterService<T> {
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
    async fn set(&self, value: u64) -> Result<(), TransactionCounterError>;
}

#[async_trait]
#[allow(dead_code)]
impl<T: TransactionCounterTrait + Send + Sync> TransactionCounterServiceTrait
    for TransactionCounterService<T>
{
    async fn get(&self) -> Result<Option<u64>, TransactionCounterError> {
        self.store.get(&self.relayer_id, &self.address)
    }

    async fn get_and_increment(&self) -> Result<u64, TransactionCounterError> {
        self.store
            .get_and_increment(&self.relayer_id, &self.address)
    }

    async fn decrement(&self) -> Result<u64, TransactionCounterError> {
        self.store.decrement(&self.relayer_id, &self.address)
    }

    async fn set(&self, value: u64) -> Result<(), TransactionCounterError> {
        self.store.set(&self.relayer_id, &self.address, value)
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
}
