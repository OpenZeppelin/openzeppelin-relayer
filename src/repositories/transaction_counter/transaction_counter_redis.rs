//! Redis implementation of the transaction counter.
//!
//! This module provides a Redis-based implementation of the `TransactionCounterTrait`,
//! allowing transaction counters to be stored and retrieved from a Redis database.
//! The implementation includes comprehensive error handling, logging, and atomic operations
//! to ensure consistency when incrementing and decrementing counters.

use super::TransactionCounterTrait;
use crate::models::RepositoryError;
use crate::repositories::redis_base::RedisRepository;
use async_trait::async_trait;
use redis::aio::ConnectionManager;
use redis::AsyncCommands;
use std::fmt;
use std::sync::Arc;
use tracing::debug;

const COUNTER_PREFIX: &str = "transaction_counter";

#[derive(Clone)]
pub struct RedisTransactionCounter {
    pub client: Arc<ConnectionManager>,
    pub key_prefix: String,
}

impl RedisRepository for RedisTransactionCounter {}

impl fmt::Debug for RedisTransactionCounter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RedisTransactionCounter")
            .field("key_prefix", &self.key_prefix)
            .finish()
    }
}

impl RedisTransactionCounter {
    pub fn new(
        connection_manager: Arc<ConnectionManager>,
        key_prefix: String,
    ) -> Result<Self, RepositoryError> {
        if key_prefix.is_empty() {
            return Err(RepositoryError::InvalidData(
                "Redis key prefix cannot be empty".to_string(),
            ));
        }

        Ok(Self {
            client: connection_manager,
            key_prefix,
        })
    }

    /// Generate key for transaction counter: {prefix}:transaction_counter:{relayer_id}:{address}
    fn counter_key(&self, relayer_id: &str, address: &str) -> String {
        format!(
            "{}:{}:{}:{}",
            self.key_prefix, COUNTER_PREFIX, relayer_id, address
        )
    }
}

#[async_trait]
impl TransactionCounterTrait for RedisTransactionCounter {
    async fn get(&self, relayer_id: &str, address: &str) -> Result<Option<u64>, RepositoryError> {
        if relayer_id.is_empty() {
            return Err(RepositoryError::InvalidData(
                "Relayer ID cannot be empty".to_string(),
            ));
        }

        if address.is_empty() {
            return Err(RepositoryError::InvalidData(
                "Address cannot be empty".to_string(),
            ));
        }

        let key = self.counter_key(relayer_id, address);
        debug!(relayer_id = %relayer_id, address = %address, "getting counter for relayer and address");

        let mut conn = self.client.as_ref().clone();

        let value: Option<u64> = conn
            .get(&key)
            .await
            .map_err(|e| self.map_redis_error(e, "get_counter"))?;

        debug!(value = ?value, "retrieved counter value");
        Ok(value)
    }

    async fn get_and_increment(
        &self,
        relayer_id: &str,
        address: &str,
    ) -> Result<u64, RepositoryError> {
        if relayer_id.is_empty() {
            return Err(RepositoryError::InvalidData(
                "Relayer ID cannot be empty".to_string(),
            ));
        }

        if address.is_empty() {
            return Err(RepositoryError::InvalidData(
                "Address cannot be empty".to_string(),
            ));
        }

        let key = self.counter_key(relayer_id, address);
        debug!(relayer_id = %relayer_id, address = %address, "getting and incrementing counter for relayer and address");

        let mut conn = self.client.as_ref().clone();

        // Use Redis INCR for atomic increment
        let new_value: u64 = conn
            .incr(&key, 1)
            .await
            .map_err(|e| self.map_redis_error(e, "get_and_increment"))?;

        let current = new_value.saturating_sub(1);

        debug!(from = %current, to = %(current + 1), "counter incremented");
        Ok(current)
    }

    async fn decrement(&self, relayer_id: &str, address: &str) -> Result<u64, RepositoryError> {
        if relayer_id.is_empty() {
            return Err(RepositoryError::InvalidData(
                "Relayer ID cannot be empty".to_string(),
            ));
        }

        if address.is_empty() {
            return Err(RepositoryError::InvalidData(
                "Address cannot be empty".to_string(),
            ));
        }

        let key = self.counter_key(relayer_id, address);
        debug!(relayer_id = %relayer_id, address = %address, "decrementing counter for relayer and address");

        let mut conn = self.client.as_ref().clone();

        // Check if counter exists first
        let exists: bool = conn
            .exists(&key)
            .await
            .map_err(|e| self.map_redis_error(e, "check_counter_exists"))?;

        if !exists {
            return Err(RepositoryError::NotFound(format!(
                "Counter not found for relayer {relayer_id} and address {address}"
            )));
        }

        // Use Redis DECR and correct if it goes below 0
        let new_value: i64 = conn
            .decr(&key, 1)
            .await
            .map_err(|e| self.map_redis_error(e, "decrement_counter"))?;

        let new_value = if new_value < 0 {
            // Correct negative values back to 0
            let _: () = conn
                .set(&key, 0)
                .await
                .map_err(|e| self.map_redis_error(e, "correct_negative_counter"))?;
            0u64
        } else {
            new_value as u64
        };

        debug!(new_value = %new_value, "counter decremented");
        Ok(new_value)
    }

    async fn set(
        &self,
        relayer_id: &str,
        address: &str,
        value: u64,
    ) -> Result<(), RepositoryError> {
        if relayer_id.is_empty() {
            return Err(RepositoryError::InvalidData(
                "Relayer ID cannot be empty".to_string(),
            ));
        }

        if address.is_empty() {
            return Err(RepositoryError::InvalidData(
                "Address cannot be empty".to_string(),
            ));
        }

        let key = self.counter_key(relayer_id, address);
        debug!(relayer_id = %relayer_id, address = %address, value = %value, "setting counter for relayer and address");

        let mut conn = self.client.as_ref().clone();

        let _: () = conn
            .set(&key, value)
            .await
            .map_err(|e| self.map_redis_error(e, "set_counter"))?;

        debug!(value = %value, "counter set");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use redis::aio::ConnectionManager;
    use std::sync::Arc;
    use tokio;
    use uuid::Uuid;

    async fn setup_test_repo() -> RedisTransactionCounter {
        let redis_url =
            std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
        let client = redis::Client::open(redis_url).expect("Failed to create Redis client");
        let connection_manager = ConnectionManager::new(client)
            .await
            .expect("Failed to create Redis connection manager");

        RedisTransactionCounter::new(Arc::new(connection_manager), "test_counter".to_string())
            .expect("Failed to create Redis transaction counter")
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_get_nonexistent_counter() {
        let repo = setup_test_repo().await;
        let random_id = Uuid::new_v4().to_string();
        let result = repo.get(&random_id, "0x1234").await.unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_set_and_get_counter() {
        let repo = setup_test_repo().await;
        let relayer_id = uuid::Uuid::new_v4().to_string();
        let address = uuid::Uuid::new_v4().to_string();

        repo.set(&relayer_id, &address, 100).await.unwrap();
        let result = repo.get(&relayer_id, &address).await.unwrap();
        assert_eq!(result, Some(100));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_get_and_increment() {
        let repo = setup_test_repo().await;
        let relayer_id = uuid::Uuid::new_v4().to_string();
        let address = uuid::Uuid::new_v4().to_string();

        // First increment should return 0 and set to 1
        let result = repo.get_and_increment(&relayer_id, &address).await.unwrap();
        assert_eq!(result, 0);

        let current = repo.get(&relayer_id, &address).await.unwrap();
        assert_eq!(current, Some(1));

        // Second increment should return 1 and set to 2
        let result = repo.get_and_increment(&relayer_id, &address).await.unwrap();
        assert_eq!(result, 1);

        let current = repo.get(&relayer_id, &address).await.unwrap();
        assert_eq!(current, Some(2));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_decrement() {
        let repo = setup_test_repo().await;
        let relayer_id = uuid::Uuid::new_v4().to_string();
        let address = uuid::Uuid::new_v4().to_string();

        // Set initial value
        repo.set(&relayer_id, &address, 5).await.unwrap();

        // Decrement should return 4
        let result = repo.decrement(&relayer_id, &address).await.unwrap();
        assert_eq!(result, 4);

        let current = repo.get(&relayer_id, &address).await.unwrap();
        assert_eq!(current, Some(4));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_decrement_not_found() {
        let repo = setup_test_repo().await;
        let result = repo.decrement("nonexistent", "0x1234").await;
        assert!(matches!(result, Err(RepositoryError::NotFound(_))));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_empty_validation() {
        let repo = setup_test_repo().await;

        // Test empty relayer_id
        let result = repo.get("", "0x1234").await;
        assert!(matches!(result, Err(RepositoryError::InvalidData(_))));

        // Test empty address
        let result = repo.get("relayer", "").await;
        assert!(matches!(result, Err(RepositoryError::InvalidData(_))));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_multiple_relayers() {
        let repo = setup_test_repo().await;
        let relayer_1 = uuid::Uuid::new_v4().to_string();
        let relayer_2 = uuid::Uuid::new_v4().to_string();
        let address_1 = uuid::Uuid::new_v4().to_string();
        let address_2 = uuid::Uuid::new_v4().to_string();

        // Set different values for different relayer/address combinations
        repo.set(&relayer_1, &address_1, 100).await.unwrap();
        repo.set(&relayer_1, &address_2, 200).await.unwrap();
        repo.set(&relayer_2, &address_1, 300).await.unwrap();

        // Verify independent counters
        assert_eq!(repo.get(&relayer_1, &address_1).await.unwrap(), Some(100));
        assert_eq!(repo.get(&relayer_1, &address_2).await.unwrap(), Some(200));
        assert_eq!(repo.get(&relayer_2, &address_1).await.unwrap(), Some(300));

        // Verify independent increments
        assert_eq!(
            repo.get_and_increment(&relayer_1, &address_1)
                .await
                .unwrap(),
            100
        );
        assert_eq!(
            repo.get_and_increment(&relayer_1, &address_1)
                .await
                .unwrap(),
            101
        );
        assert_eq!(
            repo.get_and_increment(&relayer_1, &address_2)
                .await
                .unwrap(),
            200
        );
        assert_eq!(
            repo.get_and_increment(&relayer_1, &address_2)
                .await
                .unwrap(),
            201
        );
        assert_eq!(repo.get(&relayer_2, &address_1).await.unwrap(), Some(300));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_concurrent_get_and_increment() {
        let repo = setup_test_repo().await;
        let relayer_id = uuid::Uuid::new_v4().to_string();
        let address = uuid::Uuid::new_v4().to_string();

        // Set initial value
        repo.set(&relayer_id, &address, 100).await.unwrap();

        // Create multiple concurrent tasks that increment the counter
        let handles: Vec<_> = (0..10)
            .map(|_| {
                let repo = repo.clone();
                let relayer_id = relayer_id.clone();
                let address = address.clone();
                tokio::spawn(
                    async move { repo.get_and_increment(&relayer_id, &address).await.unwrap() },
                )
            })
            .collect();

        // Wait for all tasks to complete and collect results
        let mut results = Vec::new();
        for handle in handles {
            results.push(handle.await.unwrap());
        }

        // Sort results to check they are sequential
        results.sort();

        // Verify we get exactly the values 100-109 (no duplicates, no gaps)
        let expected: Vec<u64> = (100..110).collect();
        assert_eq!(results, expected);

        // Verify final value is 110
        let final_value = repo.get(&relayer_id, &address).await.unwrap();
        assert_eq!(final_value, Some(110));
    }
}
