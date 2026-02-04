//! Queue management module for job processing.
//!
//! This module provides Redis-backed queue implementation for handling different types of jobs:
//! - Transaction requests
//! - Transaction submissions
//! - Transaction status checks
//! - Notifications
//! - Solana swap requests
//! - Relayer health checks
use std::{env, sync::Arc};

use apalis::prelude::MakeShared;
use apalis_redis::{
    aio::MultiplexedConnection, shared::SharedRedisStorage, RedisConfig, RedisStorage,
};
use color_eyre::{eyre, Result};
use tracing::info;

use crate::{config::ServerConfig, utils::RedisConnections};

use super::{
    Job, NotificationSend, RelayerHealthCheck, TokenSwapRequest, TransactionRequest,
    TransactionSend, TransactionStatusCheck,
};

#[derive(Clone)]
pub struct Queue {
    pub transaction_request_queue: RedisStorage<Job<TransactionRequest>, MultiplexedConnection>,
    pub transaction_submission_queue: RedisStorage<Job<TransactionSend>, MultiplexedConnection>,
    /// Default/fallback status queue for backward compatibility, Solana, and future networks
    pub transaction_status_queue: RedisStorage<Job<TransactionStatusCheck>, MultiplexedConnection>,
    /// EVM-specific status queue with slower retries
    pub transaction_status_queue_evm:
        RedisStorage<Job<TransactionStatusCheck>, MultiplexedConnection>,
    /// Stellar-specific status queue with fast retries
    pub transaction_status_queue_stellar:
        RedisStorage<Job<TransactionStatusCheck>, MultiplexedConnection>,
    pub notification_queue: RedisStorage<Job<NotificationSend>, MultiplexedConnection>,
    pub token_swap_request_queue: RedisStorage<Job<TokenSwapRequest>, MultiplexedConnection>,
    pub relayer_health_check_queue: RedisStorage<Job<RelayerHealthCheck>, MultiplexedConnection>,
    /// Redis connection pools for handlers that need pool-based access.
    /// Provides both primary (write) and reader (read) pools for:
    /// - Distributed locking
    /// - Status check metadata (failure counters)
    /// - Any handler-specific Redis operations
    redis_connections: Arc<RedisConnections>,
}

impl std::fmt::Debug for Queue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Queue")
            .field("transaction_request_queue", &"RedisStorage<...>")
            .field("transaction_submission_queue", &"RedisStorage<...>")
            .field("transaction_status_queue", &"RedisStorage<...>")
            .field("transaction_status_queue_evm", &"RedisStorage<...>")
            .field("transaction_status_queue_stellar", &"RedisStorage<...>")
            .field("notification_queue", &"RedisStorage<...>")
            .field("token_swap_request_queue", &"RedisStorage<...>")
            .field("relayer_health_check_queue", &"RedisStorage<...>")
            .field("redis_connections", &"RedisConnections")
            .finish()
    }
}

impl Queue {
    /// Sets up all job queues with properly configured Redis connections.
    ///
    /// # Architecture
    /// - **Queue storages**: Use `SharedRedisStorage` for efficient connection sharing
    /// - **Handler operations**: Use `redis_connections` pool for metadata, locking, counters
    ///
    /// # Arguments
    /// * `redis_connections` - Redis connection pools for handler operations.
    ///
    /// # Connection Configuration
    /// Uses `SharedRedisStorage` from apalis-redis v1.0 which internally manages
    /// connection pooling and reconnection. Each queue gets its own storage instance
    /// but shares the underlying Redis connection for efficiency.
    pub async fn setup(redis_connections: Arc<RedisConnections>) -> Result<Self> {
        let server_config = ServerConfig::from_env();
        let redis_url = &server_config.redis_url;

        // Create Redis client for SharedRedisStorage with RESP3 protocol
        // apalis-redis v1.0.0-rc.3 requires RESP3 for push notifications
        let client = redis::Client::open(redis_url.as_str())
            .map_err(|e| eyre::eyre!("Failed to create Redis client for queue: {}", e))?;

        // Get connection info and set RESP3 protocol
        let mut connection_info = client.get_connection_info().clone();
        connection_info.redis.protocol = redis::ProtocolVersion::RESP3;

        // Recreate client with RESP3 protocol
        let client = redis::Client::open(connection_info)
            .map_err(|e| eyre::eyre!("Failed to create Redis client with RESP3: {}", e))?;

        // Create SharedRedisStorage - manages connection internally with auto-reconnect
        let mut shared_storage = SharedRedisStorage::new(client)
            .await
            .map_err(|e| eyre::eyre!("Failed to create SharedRedisStorage: {}", e))?;

        info!(
            redis_url = %redis_url,
            "Queue setup: created SharedRedisStorage for efficient connection sharing"
        );

        // use REDIS_KEY_PREFIX only if set, otherwise do not use it
        let redis_key_prefix = env::var("REDIS_KEY_PREFIX")
            .ok()
            .filter(|v| !v.is_empty())
            .map(|value| format!("{value}:queue:"))
            .unwrap_or_default();

        Ok(Self {
            // Transaction queues - need fast polling for responsive processing
            transaction_request_queue: shared_storage
                .make_shared_with_config(
                    RedisConfig::default()
                        .set_namespace(&format!("{redis_key_prefix}transaction_request_queue")),
                )
                .map_err(|e| eyre::eyre!("Failed to create transaction_request_queue: {}", e))?,
            transaction_submission_queue: shared_storage
                .make_shared_with_config(
                    RedisConfig::default()
                        .set_namespace(&format!("{redis_key_prefix}transaction_submission_queue")),
                )
                .map_err(|e| eyre::eyre!("Failed to create transaction_submission_queue: {}", e))?,
            transaction_status_queue: shared_storage
                .make_shared_with_config(
                    RedisConfig::default()
                        .set_namespace(&format!("{redis_key_prefix}transaction_status_queue")),
                )
                .map_err(|e| eyre::eyre!("Failed to create transaction_status_queue: {}", e))?,
            transaction_status_queue_evm: shared_storage
                .make_shared_with_config(
                    RedisConfig::default()
                        .set_namespace(&format!("{redis_key_prefix}transaction_status_queue_evm")),
                )
                .map_err(|e| eyre::eyre!("Failed to create transaction_status_queue_evm: {}", e))?,
            transaction_status_queue_stellar: shared_storage
                .make_shared_with_config(RedisConfig::default().set_namespace(&format!(
                    "{redis_key_prefix}transaction_status_queue_stellar"
                )))
                .map_err(|e| {
                    eyre::eyre!("Failed to create transaction_status_queue_stellar: {}", e)
                })?,
            notification_queue: shared_storage
                .make_shared_with_config(
                    RedisConfig::default()
                        .set_namespace(&format!("{redis_key_prefix}notification_queue")),
                )
                .map_err(|e| eyre::eyre!("Failed to create notification_queue: {}", e))?,
            token_swap_request_queue: shared_storage
                .make_shared_with_config(
                    RedisConfig::default()
                        .set_namespace(&format!("{redis_key_prefix}token_swap_request_queue")),
                )
                .map_err(|e| eyre::eyre!("Failed to create token_swap_request_queue: {}", e))?,
            relayer_health_check_queue: shared_storage
                .make_shared_with_config(
                    RedisConfig::default()
                        .set_namespace(&format!("{redis_key_prefix}relayer_health_check_queue")),
                )
                .map_err(|e| eyre::eyre!("Failed to create relayer_health_check_queue: {}", e))?,
            redis_connections,
        })
    }

    /// Returns the Redis connection pools.
    ///
    /// This provides access to both primary and reader pools for handlers
    /// that need Redis pool-based access (e.g., for metadata storage, distributed locking).
    pub fn redis_connections(&self) -> Arc<RedisConnections> {
        self.redis_connections.clone()
    }
}

#[cfg(test)]
mod tests {
    use apalis_redis::RedisConfig;

    #[test]
    fn test_queue_storage_configuration() {
        // Test the config creation logic without actual Redis connections
        let namespace = "test_namespace";
        let config = RedisConfig::default().set_namespace(namespace);

        assert_eq!(config.get_namespace(), namespace);
    }

    // Mock version of Queue for testing
    #[derive(Clone, Debug)]
    struct MockQueue {
        pub namespace_transaction_request: String,
        pub namespace_transaction_submission: String,
        pub namespace_transaction_status: String,
        pub namespace_transaction_status_evm: String,
        pub namespace_transaction_status_stellar: String,
        pub namespace_notification: String,
        pub namespace_token_swap_request_queue: String,
        pub namespace_relayer_health_check_queue: String,
    }

    impl MockQueue {
        fn new() -> Self {
            Self {
                namespace_transaction_request: "transaction_request_queue".to_string(),
                namespace_transaction_submission: "transaction_submission_queue".to_string(),
                namespace_transaction_status: "transaction_status_queue".to_string(),
                namespace_transaction_status_evm: "transaction_status_queue_evm".to_string(),
                namespace_transaction_status_stellar: "transaction_status_queue_stellar"
                    .to_string(),
                namespace_notification: "notification_queue".to_string(),
                namespace_token_swap_request_queue: "token_swap_request_queue".to_string(),
                namespace_relayer_health_check_queue: "relayer_health_check_queue".to_string(),
            }
        }
    }

    #[test]
    fn test_queue_namespaces() {
        let mock_queue = MockQueue::new();

        assert_eq!(
            mock_queue.namespace_transaction_request,
            "transaction_request_queue"
        );
        assert_eq!(
            mock_queue.namespace_transaction_submission,
            "transaction_submission_queue"
        );
        assert_eq!(
            mock_queue.namespace_transaction_status,
            "transaction_status_queue"
        );
        assert_eq!(
            mock_queue.namespace_transaction_status_evm,
            "transaction_status_queue_evm"
        );
        assert_eq!(
            mock_queue.namespace_transaction_status_stellar,
            "transaction_status_queue_stellar"
        );
        assert_eq!(mock_queue.namespace_notification, "notification_queue");
        assert_eq!(
            mock_queue.namespace_token_swap_request_queue,
            "token_swap_request_queue"
        );
        assert_eq!(
            mock_queue.namespace_relayer_health_check_queue,
            "relayer_health_check_queue"
        );
    }

    #[test]
    fn test_queue_config_with_prefix() {
        // Test that namespace includes prefix when set
        let prefix = "myprefix:queue:";
        let queue_name = "transaction_request_queue";
        let full_namespace = format!("{}{}", prefix, queue_name);

        let config = RedisConfig::default().set_namespace(&full_namespace);
        assert_eq!(
            config.get_namespace(),
            "myprefix:queue:transaction_request_queue"
        );
    }

    #[test]
    fn test_queue_config_without_prefix() {
        // Test that namespace works without prefix
        let queue_name = "transaction_request_queue";

        let config = RedisConfig::default().set_namespace(queue_name);
        assert_eq!(config.get_namespace(), "transaction_request_queue");
    }
}
