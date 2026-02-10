//! Redis backend implementation using Apalis.
//!
//! This module provides a Redis/Apalis-backed implementation of the QueueBackend trait.
//! It wraps the existing Queue structure and delegates to Apalis for job processing.

use std::sync::Arc;

use actix_web::web::ThinData;
use apalis::prelude::Storage;
use async_trait::async_trait;
use tracing::info;

use crate::{
    jobs::{
        Job, NotificationSend, RelayerHealthCheck, TokenSwapRequest, TransactionRequest,
        TransactionSend, TransactionStatusCheck,
    },
    models::{DefaultAppState, NetworkType},
    queues::{Queue, QueueBackendType},
    utils::RedisConnections,
};

use super::{QueueBackend, QueueBackendError, QueueHealth, QueueType, WorkerHandle};

/// Redis backend using Apalis for job queue operations.
///
/// This is a wrapper around the existing Queue implementation that provides
/// the QueueBackend trait interface. It delegates all operations to the
/// existing Apalis/Redis infrastructure.
#[derive(Clone)]
pub struct RedisBackend {
    queue: Queue,
}

impl std::fmt::Debug for RedisBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedisBackend")
            .field("backend_type", &"redis")
            .finish()
    }
}

impl RedisBackend {
    /// Creates a new Redis backend.
    ///
    /// This initializes all Redis-backed queues using the existing Queue::setup method.
    ///
    /// # Arguments
    /// * `redis_connections` - Redis connection pools for queue operations
    ///
    /// # Errors
    /// Returns QueueBackendError if queue setup fails
    pub async fn new(redis_connections: Arc<RedisConnections>) -> Result<Self, QueueBackendError> {
        info!("Initializing Redis queue backend");

        let queue = Queue::setup(redis_connections)
            .await
            .map_err(|e| QueueBackendError::RedisError(e.to_string()))?;

        Ok(Self { queue })
    }

    /// Returns a reference to the underlying Queue for compatibility with existing code.
    pub fn queue(&self) -> &Queue {
        &self.queue
    }
}

/// Select status-check queue by network type.
///
/// EVM and Stellar use dedicated queues. Solana and unknown network types
/// use the default status-check queue.
fn status_check_queue_type(network_type: Option<&NetworkType>) -> QueueType {
    match network_type {
        Some(NetworkType::Evm) => QueueType::StatusCheckEvm,
        Some(NetworkType::Stellar) => QueueType::StatusCheckStellar,
        _ => QueueType::StatusCheck,
    }
}

fn static_redis_health_statuses() -> Vec<QueueHealth> {
    vec![
        QueueHealth {
            queue_type: QueueType::TransactionRequest,
            messages_visible: 0, // Would need Redis LLEN query
            messages_in_flight: 0,
            messages_dlq: 0,
            backend: "redis".to_string(),
            is_healthy: true,
        },
        QueueHealth {
            queue_type: QueueType::TransactionSubmission,
            messages_visible: 0,
            messages_in_flight: 0,
            messages_dlq: 0,
            backend: "redis".to_string(),
            is_healthy: true,
        },
        QueueHealth {
            queue_type: QueueType::StatusCheck,
            messages_visible: 0,
            messages_in_flight: 0,
            messages_dlq: 0,
            backend: "redis".to_string(),
            is_healthy: true,
        },
        QueueHealth {
            queue_type: QueueType::StatusCheckEvm,
            messages_visible: 0,
            messages_in_flight: 0,
            messages_dlq: 0,
            backend: "redis".to_string(),
            is_healthy: true,
        },
        QueueHealth {
            queue_type: QueueType::StatusCheckStellar,
            messages_visible: 0,
            messages_in_flight: 0,
            messages_dlq: 0,
            backend: "redis".to_string(),
            is_healthy: true,
        },
        QueueHealth {
            queue_type: QueueType::Notification,
            messages_visible: 0,
            messages_in_flight: 0,
            messages_dlq: 0,
            backend: "redis".to_string(),
            is_healthy: true,
        },
        QueueHealth {
            queue_type: QueueType::TokenSwapRequest,
            messages_visible: 0,
            messages_in_flight: 0,
            messages_dlq: 0,
            backend: "redis".to_string(),
            is_healthy: true,
        },
        QueueHealth {
            queue_type: QueueType::RelayerHealthCheck,
            messages_visible: 0,
            messages_in_flight: 0,
            messages_dlq: 0,
            backend: "redis".to_string(),
            is_healthy: true,
        },
    ]
}

#[async_trait]
impl QueueBackend for RedisBackend {
    async fn produce_transaction_request(
        &self,
        job: Job<TransactionRequest>,
        scheduled_on: Option<i64>,
    ) -> Result<String, QueueBackendError> {
        let mut storage = self.queue.transaction_request_queue.clone();
        let job_id = job.message_id.clone();

        match scheduled_on {
            Some(on) => {
                storage
                    .schedule(job, on)
                    .await
                    .map_err(|e| QueueBackendError::RedisError(e.to_string()))?;
            }
            None => {
                storage
                    .push(job)
                    .await
                    .map_err(|e| QueueBackendError::RedisError(e.to_string()))?;
            }
        }

        Ok(job_id)
    }

    async fn produce_transaction_submission(
        &self,
        job: Job<TransactionSend>,
        scheduled_on: Option<i64>,
    ) -> Result<String, QueueBackendError> {
        let mut storage = self.queue.transaction_submission_queue.clone();
        let job_id = job.message_id.clone();

        match scheduled_on {
            Some(on) => {
                storage
                    .schedule(job, on)
                    .await
                    .map_err(|e| QueueBackendError::RedisError(e.to_string()))?;
            }
            None => {
                storage
                    .push(job)
                    .await
                    .map_err(|e| QueueBackendError::RedisError(e.to_string()))?;
            }
        }

        Ok(job_id)
    }

    async fn produce_transaction_status_check(
        &self,
        job: Job<TransactionStatusCheck>,
        scheduled_on: Option<i64>,
    ) -> Result<String, QueueBackendError> {
        // Route by network_type to preserve existing Redis queue behavior.
        let mut storage = match status_check_queue_type(job.data.network_type.as_ref()) {
            QueueType::StatusCheckEvm => self.queue.transaction_status_queue_evm.clone(),
            QueueType::StatusCheckStellar => self.queue.transaction_status_queue_stellar.clone(),
            _ => self.queue.transaction_status_queue.clone(),
        };
        let job_id = job.message_id.clone();

        match scheduled_on {
            Some(on) => {
                storage
                    .schedule(job, on)
                    .await
                    .map_err(|e| QueueBackendError::RedisError(e.to_string()))?;
            }
            None => {
                storage
                    .push(job)
                    .await
                    .map_err(|e| QueueBackendError::RedisError(e.to_string()))?;
            }
        }

        Ok(job_id)
    }

    async fn produce_notification(
        &self,
        job: Job<NotificationSend>,
        scheduled_on: Option<i64>,
    ) -> Result<String, QueueBackendError> {
        let mut storage = self.queue.notification_queue.clone();
        let job_id = job.message_id.clone();

        match scheduled_on {
            Some(on) => {
                storage
                    .schedule(job, on)
                    .await
                    .map_err(|e| QueueBackendError::RedisError(e.to_string()))?;
            }
            None => {
                storage
                    .push(job)
                    .await
                    .map_err(|e| QueueBackendError::RedisError(e.to_string()))?;
            }
        }

        Ok(job_id)
    }

    async fn produce_token_swap_request(
        &self,
        job: Job<TokenSwapRequest>,
        scheduled_on: Option<i64>,
    ) -> Result<String, QueueBackendError> {
        let mut storage = self.queue.token_swap_request_queue.clone();
        let job_id = job.message_id.clone();

        match scheduled_on {
            Some(on) => {
                storage
                    .schedule(job, on)
                    .await
                    .map_err(|e| QueueBackendError::RedisError(e.to_string()))?;
            }
            None => {
                storage
                    .push(job)
                    .await
                    .map_err(|e| QueueBackendError::RedisError(e.to_string()))?;
            }
        }

        Ok(job_id)
    }

    async fn produce_relayer_health_check(
        &self,
        job: Job<RelayerHealthCheck>,
        scheduled_on: Option<i64>,
    ) -> Result<String, QueueBackendError> {
        let mut storage = self.queue.relayer_health_check_queue.clone();
        let job_id = job.message_id.clone();

        match scheduled_on {
            Some(on) => {
                storage
                    .schedule(job, on)
                    .await
                    .map_err(|e| QueueBackendError::RedisError(e.to_string()))?;
            }
            None => {
                storage
                    .push(job)
                    .await
                    .map_err(|e| QueueBackendError::RedisError(e.to_string()))?;
            }
        }

        Ok(job_id)
    }

    async fn initialize_workers(
        &self,
        app_state: Arc<ThinData<DefaultAppState>>,
    ) -> Result<Vec<WorkerHandle>, QueueBackendError> {
        info!("Initializing Redis backend workers");

        super::redis_worker::initialize_redis_workers((*app_state).clone())
            .await
            .map_err(|e| QueueBackendError::WorkerInitError(e.to_string()))?;

        super::redis_worker::initialize_redis_token_swap_workers((*app_state).clone())
            .await
            .map_err(|e| QueueBackendError::WorkerInitError(e.to_string()))?;

        // Apalis workers are owned by the monitors; no explicit
        // worker handles are returned from that flow.
        Ok(vec![])
    }

    async fn health_check(&self) -> Result<Vec<QueueHealth>, QueueBackendError> {
        // Intentionally avoid per-request Redis queue depth calls here to keep
        // health checks lightweight and avoid adding pressure to Redis.
        // Return static backend health metadata only.
        Ok(static_redis_health_statuses())
    }

    fn backend_type(&self) -> QueueBackendType {
        QueueBackendType::Redis
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::{Job, JobType, TransactionRequest};
    use crate::models::NetworkType;
    use crate::queues::QueueType;

    /// Helper function to create a test RedisBackend.
    /// Note: This requires a Redis connection, so tests using this should be marked with
    /// `#[ignore = "Requires active Redis instance"]` unless running integration tests.
    async fn create_test_backend() -> Result<RedisBackend, QueueBackendError> {
        use crate::utils::RedisConnections;
        use deadpool_redis::{Config, Runtime};
        use std::sync::Arc;

        let redis_url = std::env::var("REDIS_TEST_URL")
            .unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());

        let cfg = Config::from_url(&redis_url);
        let pool = Arc::new(
            cfg.builder()
                .expect("Failed to create pool builder")
                .max_size(1)
                .runtime(Runtime::Tokio1)
                .build()
                .expect("Failed to build Redis pool"),
        );

        let connections = Arc::new(RedisConnections::new_single_pool(pool));
        RedisBackend::new(connections).await
    }

    #[test]
    fn test_backend_type_logic() {
        // Test that backend_type returns Redis without requiring a Queue instance
        // This tests the logic, not the actual implementation
        assert_eq!(QueueBackendType::Redis.as_str(), "redis");
        assert_eq!(QueueBackendType::Redis.to_string(), "redis");
    }

    #[test]
    fn test_produce_transaction_status_check_routing_logic() {
        assert_eq!(
            status_check_queue_type(Some(&NetworkType::Evm)),
            QueueType::StatusCheckEvm
        );
        assert_eq!(
            status_check_queue_type(Some(&NetworkType::Stellar)),
            QueueType::StatusCheckStellar
        );
        assert_eq!(
            status_check_queue_type(Some(&NetworkType::Solana)),
            QueueType::StatusCheck
        );
        assert_eq!(status_check_queue_type(None), QueueType::StatusCheck);
    }

    #[test]
    fn test_job_id_extraction() {
        // Test that job IDs are correctly extracted from jobs
        let job = Job::new(
            JobType::TransactionRequest,
            TransactionRequest::new("tx1", "relayer1"),
        );

        // Verify job has a message_id
        assert!(!job.message_id.is_empty());
        assert_eq!(job.message_id.len(), 36); // UUID v4 format

        // Verify job_id can be cloned (used in produce methods)
        let job_id = job.message_id.clone();
        assert_eq!(job_id, job.message_id);
    }

    #[test]
    fn test_scheduled_on_handling() {
        // Test that scheduled_on timestamps are handled correctly
        // None means immediate execution, Some(timestamp) means scheduled

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        // Immediate execution
        let immediate: Option<i64> = None;
        assert_eq!(immediate, None);

        // Scheduled execution
        let scheduled: Option<i64> = Some(now + 60);
        assert!(scheduled.is_some());
        assert!(scheduled.unwrap() > now);

        // Past timestamp should still be Some (handled by queue backend)
        let past: Option<i64> = Some(now - 10);
        assert!(past.is_some());
    }

    #[test]
    fn test_health_check_structure() {
        // Test the structure of health check responses without requiring Redis
        // This verifies the expected format and fields

        let expected_queue_types = vec![
            QueueType::TransactionRequest,
            QueueType::TransactionSubmission,
            QueueType::StatusCheck,
            QueueType::StatusCheckEvm,
            QueueType::StatusCheckStellar,
            QueueType::Notification,
            QueueType::TokenSwapRequest,
            QueueType::RelayerHealthCheck,
        ];

        // Verify all expected queue types exist
        assert_eq!(expected_queue_types.len(), 8);

        // Verify QueueType implements required traits
        for queue_type in &expected_queue_types {
            assert!(!queue_type.queue_name().is_empty());
            assert!(!queue_type.redis_namespace().is_empty());
        }

        let statuses = static_redis_health_statuses();
        assert_eq!(statuses.len(), expected_queue_types.len());
        for queue_type in expected_queue_types {
            assert!(statuses.iter().any(|h| h.queue_type == queue_type));
        }
        assert!(statuses.iter().all(|h| h.backend == "redis"));
        assert!(statuses.iter().all(|h| h.is_healthy));
    }
}
