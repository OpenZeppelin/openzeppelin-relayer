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
use deadpool_redis::Pool;
use redis::aio::MultiplexedConnection;
use serde::{Deserialize, Serialize};
use tokio::time::Duration;

use super::{
    Job, NotificationSend, RelayerHealthCheck, TokenSwapRequest, TransactionRequest,
    TransactionSend, TransactionStatusCheck,
};

#[derive(Clone, Debug)]
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
}

impl Queue {
    /// Creates a RedisStorage for a specific job type using a MultiplexedConnection.
    ///
    /// MultiplexedConnection is Clone-able (uses internal Arc), which allows the Queue
    /// to be cloned for sharing between JobProducer and workers.
    fn storage<T: Serialize + for<'de> Deserialize<'de>>(
        namespace: &str,
        conn: MultiplexedConnection,
    ) -> RedisStorage<T, MultiplexedConnection> {
        let config = Config::default()
            .set_namespace(namespace)
            .set_enqueue_scheduled(Duration::from_secs(1)); // Sets the polling interval for scheduled jobs from default 30 seconds

        RedisStorage::new_with_config(conn, config)
    }

    /// Sets up all job queues using a MultiplexedConnection from the provided pool.
    ///
    /// The MultiplexedConnection is obtained from the pool's underlying client and is
    /// Clone-able, allowing all 8 queues to share the same underlying connection.
    /// This is more efficient than 8 separate connections for long-lived queue operations.
    ///
    /// Benefits of this approach:
    /// - Unified connection configuration via deadpool
    /// - Clone-able connection type for Queue sharing
    /// - Multiplexed connection handles concurrent operations efficiently
    pub async fn setup(pool: Arc<Pool>) -> Result<Self> {
        // Get a connection from the pool to extract the underlying MultiplexedConnection
        // The pool's connection wraps a MultiplexedConnection internally
        let conn = pool
            .get()
            .await
            .map_err(|e| eyre::eyre!("Failed to get Redis connection from pool: {}", e))?;

        // Get the underlying MultiplexedConnection by dereferencing
        // MultiplexedConnection is Clone, so we can clone it for each queue
        let multiplexed_conn: MultiplexedConnection = (*conn).clone();

        // use REDIS_KEY_PREFIX only if set, otherwise do not use it
        let redis_key_prefix = env::var("REDIS_KEY_PREFIX")
            .ok()
            .filter(|v| !v.is_empty())
            .map(|value| format!("{value}:queue:"))
            .unwrap_or_default();

        Ok(Self {
            transaction_request_queue: Self::storage(
                &format!("{redis_key_prefix}transaction_request_queue"),
                multiplexed_conn.clone(),
            ),
            transaction_submission_queue: Self::storage(
                &format!("{redis_key_prefix}transaction_submission_queue"),
                multiplexed_conn.clone(),
            ),
            transaction_status_queue: Self::storage(
                &format!("{redis_key_prefix}transaction_status_queue"),
                multiplexed_conn.clone(),
            ),
            transaction_status_queue_evm: Self::storage(
                &format!("{redis_key_prefix}transaction_status_queue_evm"),
                multiplexed_conn.clone(),
            ),
            transaction_status_queue_stellar: Self::storage(
                &format!("{redis_key_prefix}transaction_status_queue_stellar"),
                multiplexed_conn.clone(),
            ),
            notification_queue: Self::storage(
                &format!("{redis_key_prefix}notification_queue"),
                multiplexed_conn.clone(),
            ),
            token_swap_request_queue: Self::storage(
                &format!("{redis_key_prefix}token_swap_request_queue"),
                multiplexed_conn.clone(),
            ),
            relayer_health_check_queue: Self::storage(
                &format!("{redis_key_prefix}relayer_health_check_queue"),
                multiplexed_conn,
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_queue_storage_configuration() {
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
}
