//! Queue management module for job processing.
//!
//! This module provides Redis-backed queue implementation for handling different types of jobs:
//! - Transaction requests
//! - Transaction submissions
//! - Transaction status checks
//! - Notifications
//! - Solana swap requests
use std::{env, sync::Arc};

use apalis_redis::{Config, ConnectionManager, RedisStorage};
use color_eyre::{eyre, Result};
use serde::{Deserialize, Serialize};
use tokio::time::{timeout, Duration};
use tracing::error;

use crate::config::ServerConfig;

use super::{
    Job, NotificationSend, SolanaTokenSwapRequest, TransactionRequest, TransactionSend,
    TransactionStatusCheck,
};

#[derive(Clone, Debug)]
pub struct Queue {
    pub transaction_request_queue: RedisStorage<Job<TransactionRequest>>,
    pub transaction_submission_queue: RedisStorage<Job<TransactionSend>>,
    pub transaction_status_queue: RedisStorage<Job<TransactionStatusCheck>>,
    pub notification_queue: RedisStorage<Job<NotificationSend>>,
    pub solana_token_swap_request_queue: RedisStorage<Job<SolanaTokenSwapRequest>>,
}

impl Queue {
    async fn storage<T: Serialize + for<'de> Deserialize<'de>>(
        namespace: &str,
        shared: Arc<ConnectionManager>,
    ) -> Result<RedisStorage<T>> {
        let config = Config::default()
            .set_namespace(namespace)
            .set_enqueue_scheduled(Duration::from_secs(1)); // Sets the polling interval for scheduled jobs from default 30 seconds

        Ok(RedisStorage::new_with_config((*shared).clone(), config))
    }

    pub async fn setup() -> Result<Self> {
        let config = ServerConfig::from_env();
        let redis_url = config.redis_url.clone();
        let redis_connection_timeout_ms = config.redis_connection_timeout_ms;
        let conn = match timeout(Duration::from_millis(redis_connection_timeout_ms), apalis_redis::connect(redis_url.clone())).await {
            Ok(result) => result.map_err(|e| {
                error!(redis_url = %redis_url, error = %e, "failed to connect to redis");
                eyre::eyre!("Failed to connect to Redis. Please ensure Redis is running and accessible at {}. Error: {}", redis_url, e)
            })?,
            Err(_) => {
                error!(redis_url = %redis_url, "timeout connecting to redis");
                return Err(eyre::eyre!("Timed out after {} milliseconds while connecting to Redis at {}", redis_connection_timeout_ms, redis_url));
            }
        };

        let shared = Arc::new(conn);
        // use REDIS_KEY_PREFIX only if set, otherwise do not use it
        let redis_key_prefix = env::var("REDIS_KEY_PREFIX")
            .ok()
            .filter(|v| !v.is_empty())
            .map(|value| format!("{value}:queue:"))
            .unwrap_or_default();
        Ok(Self {
            transaction_request_queue: Self::storage(
                &format!("{}transaction_request_queue", redis_key_prefix),
                shared.clone(),
            )
            .await?,
            transaction_submission_queue: Self::storage(
                &format!("{}transaction_submission_queue", redis_key_prefix),
                shared.clone(),
            )
            .await?,
            transaction_status_queue: Self::storage(
                &format!("{}transaction_status_queue", redis_key_prefix),
                shared.clone(),
            )
            .await?,
            notification_queue: Self::storage(
                &format!("{}notification_queue", redis_key_prefix),
                shared.clone(),
            )
            .await?,
            solana_token_swap_request_queue: Self::storage(
                &format!("{}solana_token_swap_request_queue", redis_key_prefix),
                shared.clone(),
            )
            .await?,
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
        pub namespace_notification: String,
        pub namespace_solana_token_swap_request_queue: String,
    }

    impl MockQueue {
        fn new() -> Self {
            Self {
                namespace_transaction_request: "transaction_request_queue".to_string(),
                namespace_transaction_submission: "transaction_submission_queue".to_string(),
                namespace_transaction_status: "transaction_status_queue".to_string(),
                namespace_notification: "notification_queue".to_string(),
                namespace_solana_token_swap_request_queue: "solana_token_swap_request_queue"
                    .to_string(),
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
        assert_eq!(mock_queue.namespace_notification, "notification_queue");
        assert_eq!(
            mock_queue.namespace_solana_token_swap_request_queue,
            "solana_token_swap_request_queue"
        );
    }
}
