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

/// Configuration for queue storage tuning.
#[derive(Clone, Debug)]
struct QueueConfig {
    /// How often to move scheduled jobs to active queue (default: 30s)
    enqueue_scheduled: Duration,
}

impl Default for QueueConfig {
    fn default() -> Self {
        Self {
            enqueue_scheduled: Duration::from_secs(30),
        }
    }
}

impl QueueConfig {
    /// - Faster poll interval for lower latency
    fn high_frequency() -> Self {
        Self {
            enqueue_scheduled: Duration::from_secs(2),
        }
    }

    /// Configuration for lower-frequency queues.
    /// - Smaller buffer (less memory pressure)
    /// - Slower scheduled job polling (reduces Redis load)
    fn low_frequency() -> Self {
        Self {
            enqueue_scheduled: Duration::from_secs(20),
        }
    }
}

impl Queue {
    /// Creates a RedisStorage for a specific job type using a ConnectionManager.
    ///
    /// # Arguments
    /// * `namespace` - Redis key namespace for this queue
    /// * `conn` - ConnectionManager with auto-reconnect
    /// * `queue_config` - Tuning parameters for this queue
    ///
    /// ConnectionManager provides automatic reconnection on connection failures,
    /// ensuring queue processing continues even if the Redis connection drops temporarily.
    fn storage<T: Serialize + for<'de> Deserialize<'de>>(
        namespace: &str,
        conn: ConnectionManager,
        queue_config: QueueConfig,
    ) -> RedisStorage<T> {
        let config = Config::default()
            .set_namespace(namespace)
            .set_enqueue_scheduled(queue_config.enqueue_scheduled);

        RedisStorage::new_with_config(conn, config)
    }

    /// Creates a ConnectionManager with the standard queue configuration.
    ///
    /// Each ConnectionManager represents a single Redis connection with auto-reconnect.
    /// Creating separate managers for different queue types enables parallel Redis operations.
    async fn create_connection_manager(
        client: &redis::Client,
        queue_timeout: Duration,
    ) -> Result<ConnectionManager> {
        let conn_config = ConnectionManagerConfig::new()
            .set_connection_timeout(queue_timeout)
            .set_response_timeout(queue_timeout)
            .set_number_of_retries(2)
            .set_max_delay(1000);

        ConnectionManager::new_with_config(client.clone(), conn_config)
            .await
            .map_err(|e| eyre::eyre!("Failed to create Redis connection manager: {}", e))
    }

    /// Sets up all job queues with properly configured Redis connections.
    ///
    /// # Architecture
    /// - **Queue storages**: Each queue gets its own `ConnectionManager` for maximum parallelism
    /// - **Handler operations**: Use `redis_connections` pool for metadata, locking, counters
    ///
    /// # Connection Strategy
    /// Each queue has a dedicated Redis connection to prevent contention under high throughput.
    /// This allows 8 parallel Redis operations (one per queue type).
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

        // Create Redis client
        let client = redis::Client::open(redis_url.as_str())
            .map_err(|e| eyre::eyre!("Failed to create Redis client for queue: {}", e))?;

        // Configure timeout for all ConnectionManagers
        // Worst case calculation: 3 attempts × 5s timeout + ~0.3s backoff = ~15.3s
        let queue_timeout = Duration::from_secs(5);

        // Create one ConnectionManager per queue to prevent connection contention.
        // Each ConnectionManager is a single Redis connection with auto-reconnect.
        let conn_tx_request = Self::create_connection_manager(&client, queue_timeout).await?;
        let conn_tx_submit = Self::create_connection_manager(&client, queue_timeout).await?;
        let conn_status = Self::create_connection_manager(&client, queue_timeout).await?;
        let conn_status_evm = Self::create_connection_manager(&client, queue_timeout).await?;
        let conn_status_stellar = Self::create_connection_manager(&client, queue_timeout).await?;
        let conn_notification = Self::create_connection_manager(&client, queue_timeout).await?;
        let conn_swap = Self::create_connection_manager(&client, queue_timeout).await?;
        let conn_health = Self::create_connection_manager(&client, queue_timeout).await?;

        info!(
            redis_url = %redis_url,
            connection_timeout_ms = 5000,
            response_timeout_ms = 5000,
            retries = 2,
            max_backoff_ms = 1000,
            connection_count = 8,
            "Queue setup: created dedicated ConnectionManager per queue"
        );

        // use REDIS_KEY_PREFIX only if set, otherwise do not use it
        let redis_key_prefix = env::var("REDIS_KEY_PREFIX")
            .ok()
            .filter(|v| !v.is_empty())
            .map(|value| format!("{value}:queue:"))
            .unwrap_or_default();

        // Queue configurations:
        // - High-frequency: transaction_status (critical path)
        // - Low-frequency: request, submission, notifications, health checks, swaps
        let high_frequency = QueueConfig::high_frequency();
        let low_frequency = QueueConfig::low_frequency();

        Ok(Self {
            transaction_request_queue: Self::storage(
                &format!("{redis_key_prefix}transaction_request_queue"),
                conn_tx_request,
                low_frequency.clone(), // scheduling not used
            ),
            transaction_submission_queue: Self::storage(
                &format!("{redis_key_prefix}transaction_submission_queue"),
                conn_tx_submit,
                low_frequency.clone(), // scheduling not used
            ),
            transaction_status_queue: Self::storage(
                &format!("{redis_key_prefix}transaction_status_queue"),
                conn_status,
                high_frequency.clone(),
            ),
            transaction_status_queue_evm: Self::storage(
                &format!("{redis_key_prefix}transaction_status_queue_evm"),
                conn_status_evm,
                high_frequency.clone(),
            ),
            transaction_status_queue_stellar: Self::storage(
                &format!("{redis_key_prefix}transaction_status_queue_stellar"),
                conn_status_stellar,
                high_frequency.clone(),
            ),
            // Lower-frequency queues
            notification_queue: Self::storage(
                &format!("{redis_key_prefix}notification_queue"),
                conn_notification,
                low_frequency.clone(), // scheduling not used
            ),
            token_swap_request_queue: Self::storage(
                &format!("{redis_key_prefix}token_swap_request_queue"),
                conn_swap,
                low_frequency.clone(), // scheduling not used
            ),
            relayer_health_check_queue: Self::storage(
                &format!("{redis_key_prefix}relayer_health_check_queue"),
                conn_health,
                low_frequency.clone(), // scheduling not used
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
        let full_namespace = format!("{prefix}{queue_name}");

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

    #[test]
    fn test_queue_config_default() {
        let config = QueueConfig::default();

        assert_eq!(config.enqueue_scheduled, Duration::from_secs(30));
    }

    #[test]
    fn test_queue_config_high_throughput() {
        let config = QueueConfig::high_frequency();

        // High frequency should have faster enqueue_scheduled
        assert_eq!(config.enqueue_scheduled, Duration::from_secs(2));

        // Verify it's faster than default
        let default = QueueConfig::default();
        assert!(config.enqueue_scheduled < default.enqueue_scheduled);
    }

    #[test]
    fn test_queue_config_low_frequency() {
        let config = QueueConfig::low_frequency();

        assert_eq!(config.enqueue_scheduled, Duration::from_secs(20));

        // Low frequency should have longer enqueue_scheduled than high frequency
        let high = QueueConfig::high_frequency();
        assert!(config.enqueue_scheduled > high.enqueue_scheduled);
    }
}
