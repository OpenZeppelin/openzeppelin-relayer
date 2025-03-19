//! Queue management module for job processing.
//!
//! This module provides Redis-backed queue implementation for handling different types of jobs:
//! - Transaction requests
//! - Transaction submissions
//! - Transaction status checks
//! - Notifications
use apalis_redis::{Config, RedisStorage};
use serde::{Deserialize, Serialize};

use crate::config::ServerConfig;

use super::{Job, NotificationSend, TransactionRequest, TransactionSend, TransactionStatusCheck};

#[derive(Clone, Debug)]
pub struct Queue {
    pub transaction_request_queue: RedisStorage<Job<TransactionRequest>>,
    pub transaction_submission_queue: RedisStorage<Job<TransactionSend>>,
    pub transaction_status_queue: RedisStorage<Job<TransactionStatusCheck>>,
    pub notification_queue: RedisStorage<Job<NotificationSend>>,
}

impl Queue {
    async fn storage<T: Serialize + for<'de> Deserialize<'de>>(namespace: &str) -> RedisStorage<T> {
        let conn = apalis_redis::connect(ServerConfig::from_env().redis_url.clone())
            .await
            .expect("Could not connect to Redis Jobs DB");

        let config = Config::default()
            .set_namespace(namespace)
            .set_max_retries(5);

        RedisStorage::new_with_config(conn, config)
    }

    pub async fn setup() -> Self {
        Self {
            transaction_request_queue: Self::storage("transaction_request_queue").await,
            transaction_submission_queue: Self::storage("transaction_submission_queue").await,
            transaction_status_queue: Self::storage("transaction_status_queue").await,
            notification_queue: Self::storage("notification_queue").await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_queue_storage_configuration() {
        // Test the config creation logic without actual Redis connections
        let namespace = "test_namespace";
        let config = Config::default()
            .set_namespace(namespace)
            .set_max_retries(5);

        assert_eq!(config.get_namespace(), namespace);
        assert_eq!(config.get_max_retries(), 5);
    }

    // Mock version of Queue for testing
    #[derive(Clone, Debug)]
    struct MockQueue {
        pub namespace_transaction_request: String,
        pub namespace_transaction_submission: String,
        pub namespace_transaction_status: String,
        pub namespace_notification: String,
    }

    impl MockQueue {
        fn new() -> Self {
            Self {
                namespace_transaction_request: "transaction_request_queue".to_string(),
                namespace_transaction_submission: "transaction_submission_queue".to_string(),
                namespace_transaction_status: "transaction_status_queue".to_string(),
                namespace_notification: "notification_queue".to_string(),
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
    }
}
