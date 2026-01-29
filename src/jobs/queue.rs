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

use apalis_redis::{Config, RedisStorage};
use color_eyre::{eyre, Result};
use redis::aio::{ConnectionManager, ConnectionManagerConfig};
use serde::{Deserialize, Serialize};
use tokio::time::Duration;
use tracing::info;

use crate::{config::ServerConfig, utils::RedisConnections};

use super::{
    Job, NotificationSend, RelayerHealthCheck, TokenSwapRequest, TransactionRequest,
    TransactionSend, TransactionStatusCheck,
};

#[derive(Clone)]
pub struct Queue {
    pub transaction_request_queue: RedisStorage<Job<TransactionRequest>>,
    pub transaction_submission_queue: RedisStorage<Job<TransactionSend>>,
    /// Default/fallback status queue for backward compatibility, Solana, and future networks
    pub transaction_status_queue: RedisStorage<Job<TransactionStatusCheck>>,
    /// EVM-specific status queue with slower retries
    pub transaction_status_queue_evm: RedisStorage<Job<TransactionStatusCheck>>,
    /// Stellar-specific status queue with fast retries
    pub transaction_status_queue_stellar: RedisStorage<Job<TransactionStatusCheck>>,
    pub notification_queue: RedisStorage<Job<NotificationSend>>,
    pub token_swap_request_queue: RedisStorage<Job<TokenSwapRequest>>,
    pub relayer_health_check_queue: RedisStorage<Job<RelayerHealthCheck>>,
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
    /// Creates a RedisStorage for a specific job type using a ConnectionManager.
    ///
    /// # Arguments
    /// * `namespace` - Redis key namespace for this queue
    /// * `conn` - ConnectionManager with auto-reconnect
    /// * `poll_interval` - How often to poll for scheduled jobs (None = default 2s for high-frequency queues)
    ///
    /// ConnectionManager provides automatic reconnection on connection failures,
    /// ensuring queue processing continues even if the Redis connection drops temporarily.
    fn storage<T: Serialize + for<'de> Deserialize<'de>>(
        namespace: &str,
        conn: ConnectionManager,
        poll_interval: Option<Duration>,
    ) -> RedisStorage<T> {
        // Default to 2 seconds for high-frequency queues (transaction processing)
        // Use longer intervals for lower-frequency queues to reduce Redis load
        let interval = poll_interval.unwrap_or(Duration::from_secs(2));

        let config = Config::default()
            .set_namespace(namespace)
            .set_enqueue_scheduled(interval);

        RedisStorage::new_with_config(conn, config)
    }

    /// Sets up all job queues with properly configured Redis connections.
    ///
    /// # Architecture
    /// - **Queue storages**: Use `ConnectionManager` with configured timeouts for auto-reconnect
    /// - **Handler operations**: Use `redis_connections` pool for metadata, locking, counters
    ///
    /// # Arguments
    /// * `redis_connections` - Redis connection pools for handler operations.
    ///
    /// # Connection Configuration
    /// - `connection_timeout`: Max time to establish TCP connection to Redis
    /// - `response_timeout`: Max time to wait for Redis command responses
    /// - Auto-reconnect: ConnectionManager automatically reconnects on failures
    pub async fn setup(redis_connections: Arc<RedisConnections>) -> Result<Self> {
        let server_config = ServerConfig::from_env();
        let redis_url = &server_config.redis_url;
        let timeout = Duration::from_millis(server_config.redis_connection_timeout_ms);

        // Create Redis client
        let client = redis::Client::open(redis_url.as_str())
            .map_err(|e| eyre::eyre!("Failed to create Redis client for queue: {}", e))?;

        // Configure ConnectionManager with proper timeouts and retry settings
        // These settings ensure the connection recovers gracefully from failures
        let conn_config = ConnectionManagerConfig::new()
            .set_connection_timeout(timeout)   // TCP connection timeout
            .set_response_timeout(timeout)     // Redis command response timeout
            .set_number_of_retries(8)          // Reconnection attempts (default: 6)
            .set_max_delay(5000); // Cap backoff at 5s (default: unbounded, would grow to 25.6s at retry 8)

        // Create ConnectionManager with config - provides auto-reconnect for queue operations
        let conn = ConnectionManager::new_with_config(client, conn_config)
            .await
            .map_err(|e| {
                eyre::eyre!("Failed to create Redis connection manager for queue: {}", e)
            })?;

        info!(
            redis_url = %redis_url,
            connection_timeout_ms = %server_config.redis_connection_timeout_ms,
            response_timeout_ms = %server_config.redis_connection_timeout_ms,
            "Queue setup: created ConnectionManager with configured timeouts"
        );

        // use REDIS_KEY_PREFIX only if set, otherwise do not use it
        let redis_key_prefix = env::var("REDIS_KEY_PREFIX")
            .ok()
            .filter(|v| !v.is_empty())
            .map(|value| format!("{value}:queue:"))
            .unwrap_or_default();

        // Poll intervals for scheduled jobs:
        // - High-frequency queues (transactions): 2s (default) for responsive processing
        // - Lower-frequency queues: 20s to reduce Redis polling load
        let poll_fast = Some(Duration::from_secs(2)); // Uses default 2s
        let poll_slow = Some(Duration::from_secs(20));

        Ok(Self {
            // Transaction queues - need fast polling for responsive processing
            transaction_request_queue: Self::storage(
                &format!("{redis_key_prefix}transaction_request_queue"),
                conn.clone(),
                poll_slow, // scheduling not used
            ),
            transaction_submission_queue: Self::storage(
                &format!("{redis_key_prefix}transaction_submission_queue"),
                conn.clone(),
                poll_slow, // scheduling not used
            ),
            transaction_status_queue: Self::storage(
                &format!("{redis_key_prefix}transaction_status_queue"),
                conn.clone(),
                poll_fast,
            ),
            transaction_status_queue_evm: Self::storage(
                &format!("{redis_key_prefix}transaction_status_queue_evm"),
                conn.clone(),
                poll_fast,
            ),
            transaction_status_queue_stellar: Self::storage(
                &format!("{redis_key_prefix}transaction_status_queue_stellar"),
                conn.clone(),
                poll_fast,
            ),
            // Lower-frequency queues - can use longer poll interval
            notification_queue: Self::storage(
                &format!("{redis_key_prefix}notification_queue"),
                conn.clone(),
                poll_slow, // scheduling not used
            ),
            token_swap_request_queue: Self::storage(
                &format!("{redis_key_prefix}token_swap_request_queue"),
                conn.clone(),
                poll_slow, // scheduling not used
            ),
            relayer_health_check_queue: Self::storage(
                &format!("{redis_key_prefix}relayer_health_check_queue"),
                conn,
                poll_slow, // scheduling not used
            ),
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
    use super::*;

    #[test]
    fn test_queue_storage_configuration() {
        // Test the config creation logic without actual Redis connections
        let namespace = "test_namespace";
        let config = Config::default().set_namespace(namespace);

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

        let config = Config::default().set_namespace(&full_namespace);
        assert_eq!(
            config.get_namespace(),
            "myprefix:queue:transaction_request_queue"
        );
    }

    #[test]
    fn test_queue_config_without_prefix() {
        // Test that namespace works without prefix
        let queue_name = "transaction_request_queue";

        let config = Config::default().set_namespace(queue_name);
        assert_eq!(config.get_namespace(), "transaction_request_queue");
    }
}
