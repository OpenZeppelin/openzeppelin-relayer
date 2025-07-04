pub mod transaction_counter_in_memory;
pub mod transaction_counter_redis;

pub use transaction_counter_in_memory::InMemoryTransactionCounter;
pub use transaction_counter_redis::RedisTransactionCounter;

use async_trait::async_trait;
use serde::Serialize;
use std::sync::Arc;
use thiserror::Error;

#[cfg(test)]
use mockall::automock;

use crate::config::ServerConfig;

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
    async fn get(
        &self,
        relayer_id: &str,
        address: &str,
    ) -> Result<Option<u64>, TransactionCounterError>;

    async fn get_and_increment(
        &self,
        relayer_id: &str,
        address: &str,
    ) -> Result<u64, TransactionCounterError>;

    async fn decrement(
        &self,
        relayer_id: &str,
        address: &str,
    ) -> Result<u64, TransactionCounterError>;

    async fn set(
        &self,
        relayer_id: &str,
        address: &str,
        value: u64,
    ) -> Result<(), TransactionCounterError>;
}

// Enum representing the type of transaction counter repository to use
pub enum TransactionCounterRepositoryType {
    InMemory,
    Redis,
}

/// Enum wrapper for different transaction counter repository implementations
#[derive(Debug, Clone)]
pub enum TransactionCounterRepositoryImpl {
    InMemory(InMemoryTransactionCounter),
    Redis(RedisTransactionCounter),
}

impl TransactionCounterRepositoryType {
    /// Creates a transaction counter repository based on the enum variant
    pub async fn create_repository(
        self,
        config: &ServerConfig,
    ) -> Result<TransactionCounterRepositoryImpl, TransactionCounterError> {
        match self {
            TransactionCounterRepositoryType::InMemory => Ok(
                TransactionCounterRepositoryImpl::InMemory(InMemoryTransactionCounter::new()),
            ),
            TransactionCounterRepositoryType::Redis => {
                let client = redis::Client::open(config.redis_url.clone()).map_err(|e| {
                    TransactionCounterError::NotFound(format!(
                        "Failed to create Redis client: {}",
                        e
                    ))
                })?;
                let connection_manager =
                    redis::aio::ConnectionManager::new(client)
                        .await
                        .map_err(|e| {
                            TransactionCounterError::NotFound(format!(
                                "Failed to create Redis connection manager: {}",
                                e
                            ))
                        })?;
                let redis_counter = RedisTransactionCounter::new(
                    Arc::new(connection_manager),
                    config.redis_key_prefix.clone(),
                )?;
                Ok(TransactionCounterRepositoryImpl::Redis(redis_counter))
            }
        }
    }
}

#[async_trait]
impl TransactionCounterTrait for TransactionCounterRepositoryImpl {
    async fn get(
        &self,
        relayer_id: &str,
        address: &str,
    ) -> Result<Option<u64>, TransactionCounterError> {
        match self {
            TransactionCounterRepositoryImpl::InMemory(counter) => {
                counter.get(relayer_id, address).await
            }
            TransactionCounterRepositoryImpl::Redis(counter) => {
                counter.get(relayer_id, address).await
            }
        }
    }

    async fn get_and_increment(
        &self,
        relayer_id: &str,
        address: &str,
    ) -> Result<u64, TransactionCounterError> {
        match self {
            TransactionCounterRepositoryImpl::InMemory(counter) => {
                counter.get_and_increment(relayer_id, address).await
            }
            TransactionCounterRepositoryImpl::Redis(counter) => {
                counter.get_and_increment(relayer_id, address).await
            }
        }
    }

    async fn decrement(
        &self,
        relayer_id: &str,
        address: &str,
    ) -> Result<u64, TransactionCounterError> {
        match self {
            TransactionCounterRepositoryImpl::InMemory(counter) => {
                counter.decrement(relayer_id, address).await
            }
            TransactionCounterRepositoryImpl::Redis(counter) => {
                counter.decrement(relayer_id, address).await
            }
        }
    }

    async fn set(
        &self,
        relayer_id: &str,
        address: &str,
        value: u64,
    ) -> Result<(), TransactionCounterError> {
        match self {
            TransactionCounterRepositoryImpl::InMemory(counter) => {
                counter.set(relayer_id, address, value).await
            }
            TransactionCounterRepositoryImpl::Redis(counter) => {
                counter.set(relayer_id, address, value).await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::ServerConfig, models::SecretString};

    fn create_test_config() -> ServerConfig {
        ServerConfig {
            redis_url: "redis://127.0.0.1:6379".to_string(),
            redis_key_prefix: "test_counter".to_string(),
            port: 3000,
            host: "127.0.0.1".to_string(),
            metrics_port: 9090,
            api_key: SecretString::new(""),
            config_file_path: "".to_string(),
            enable_swagger: false,
            rate_limit_requests_per_second: 100,
            rate_limit_burst_size: 100,
            redis_connection_timeout_ms: 1000,
            rpc_timeout_ms: 1000,
            provider_max_retries: 3,
            provider_retry_base_delay_ms: 100,
            provider_retry_max_delay_ms: 1000,
            provider_max_failovers: 3,
        }
    }

    #[tokio::test]
    async fn test_in_memory_repository_creation() {
        let config = create_test_config();
        let repo = TransactionCounterRepositoryType::InMemory
            .create_repository(&config)
            .await
            .unwrap();

        matches!(repo, TransactionCounterRepositoryImpl::InMemory(_));
    }

    #[tokio::test]
    async fn test_enum_wrapper_delegation() {
        let config = create_test_config();
        let repo = TransactionCounterRepositoryType::InMemory
            .create_repository(&config)
            .await
            .unwrap();

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
