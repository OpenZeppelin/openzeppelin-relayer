//! Queue backend abstraction layer.
//!
//! This module provides a backend-agnostic interface for job queue operations.
//! Implementations can use Redis/Apalis (current) or AWS SQS (new) as the backend.
//!
//! # Environment Variables
//!
//! - `QUEUE_BACKEND`: Backend to use ("redis" or "sqs", default: "redis")
//!
//! # Example
//!
//! ```ignore
//! // Create backend from environment
//! let backend = create_queue_backend(redis_connections).await?;
//!
//! // Produce a job
//! backend.produce(job, QueueType::TransactionRequest, None).await?;
//!
//! // Initialize workers
//! let workers = backend.initialize_workers(app_state).await?;
//! ```

use async_trait::async_trait;
use std::sync::Arc;

use crate::{
    config::ServerConfig,
    jobs::{
        Job, NotificationSend, RelayerHealthCheck, TokenSwapRequest, TransactionRequest,
        TransactionSend, TransactionStatusCheck,
    },
    models::DefaultAppState,
    utils::RedisConnections,
};
use actix_web::web::ThinData;

pub mod redis_backend;
pub mod sqs_backend;
pub mod sqs_worker;
pub mod types;

pub use types::{
    QueueBackendError, QueueHealth, QueueType, QueueWorkerAttempt, QueueWorkerData,
    QueueWorkerError, QueueWorkerTaskId, WorkerHandle,
};

/// Queue backend abstraction trait.
///
/// This trait defines the interface for job queue operations that can be
/// implemented by different backends (Redis/Apalis, AWS SQS, etc.).
///
/// The trait is designed to be backend-agnostic, with no Redis or SQS-specific
/// types in the interface.
#[async_trait]
pub trait QueueBackend: Send + Sync {
    /// Produces a transaction request job to the queue.
    ///
    /// # Arguments
    /// * `job` - The job to enqueue
    /// * `scheduled_on` - Optional Unix timestamp for delayed execution
    ///
    /// # Returns
    /// Result with job ID on success, or QueueBackendError on failure
    async fn produce_transaction_request(
        &self,
        job: Job<TransactionRequest>,
        scheduled_on: Option<i64>,
    ) -> Result<String, QueueBackendError>;

    /// Produces a transaction submission job to the queue.
    async fn produce_transaction_submission(
        &self,
        job: Job<TransactionSend>,
        scheduled_on: Option<i64>,
    ) -> Result<String, QueueBackendError>;

    /// Produces a transaction status check job to the queue.
    async fn produce_transaction_status_check(
        &self,
        job: Job<TransactionStatusCheck>,
        scheduled_on: Option<i64>,
    ) -> Result<String, QueueBackendError>;

    /// Produces a notification send job to the queue.
    async fn produce_notification(
        &self,
        job: Job<NotificationSend>,
        scheduled_on: Option<i64>,
    ) -> Result<String, QueueBackendError>;

    /// Produces a token swap request job to the queue.
    async fn produce_token_swap_request(
        &self,
        job: Job<TokenSwapRequest>,
        scheduled_on: Option<i64>,
    ) -> Result<String, QueueBackendError>;

    /// Produces a relayer health check job to the queue.
    async fn produce_relayer_health_check(
        &self,
        job: Job<RelayerHealthCheck>,
        scheduled_on: Option<i64>,
    ) -> Result<String, QueueBackendError>;

    /// Initializes and starts all worker tasks for this backend.
    ///
    /// Workers will poll their respective queues and process jobs using
    /// the provided application state.
    ///
    /// # Arguments
    /// * `app_state` - Application state containing handlers and configuration
    ///
    /// # Returns
    /// Vector of worker handles that can be used to monitor or stop workers
    async fn initialize_workers(
        &self,
        app_state: Arc<ThinData<DefaultAppState>>,
    ) -> Result<Vec<WorkerHandle>, QueueBackendError>;

    /// Performs a health check on all queues.
    ///
    /// Returns health status for each queue, including message counts
    /// and backend-specific health indicators.
    async fn health_check(&self) -> Result<Vec<QueueHealth>, QueueBackendError>;

    /// Returns the backend type identifier ("redis" or "sqs").
    fn backend_type(&self) -> &'static str;
}

/// Creates a queue backend based on the QUEUE_BACKEND environment variable.
///
/// # Arguments
/// * `redis_connections` - Redis connection pools (used by Redis backend, ignored by SQS)
///
/// # Environment Variables
/// - `QUEUE_BACKEND`: Backend to use ("redis" or "sqs", default: "redis")
///
/// # Returns
/// Arc-wrapped trait object implementing QueueBackend
///
/// # Errors
/// Returns QueueBackendError::ConfigError if:
/// - QUEUE_BACKEND contains an unsupported value
/// - Required backend-specific configuration is missing
pub async fn create_queue_backend(
    redis_connections: Arc<RedisConnections>,
) -> Result<Arc<dyn QueueBackend>, QueueBackendError> {
    let backend_type = ServerConfig::get_queue_backend();

    match backend_type.to_lowercase().as_str() {
        "redis" => {
            let backend = redis_backend::RedisBackend::new(redis_connections).await?;
            Ok(Arc::new(backend))
        }
        "sqs" => {
            let backend = sqs_backend::SqsBackend::new().await?;
            Ok(Arc::new(backend))
        }
        other => Err(QueueBackendError::ConfigError(format!(
            "Unsupported QUEUE_BACKEND value: {other}. Must be 'redis' or 'sqs'"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_queue_type_enum_values() {
        // Ensure all QueueType variants are covered
        let types = vec![
            QueueType::TransactionRequest,
            QueueType::TransactionSubmission,
            QueueType::StatusCheck,
            QueueType::Notification,
            QueueType::TokenSwapRequest,
            QueueType::RelayerHealthCheck,
        ];

        for queue_type in types {
            assert!(!queue_type.queue_name().is_empty());
            assert!(!queue_type.redis_namespace().is_empty());
            assert!(queue_type.max_retries() > 0 || queue_type.max_retries() == usize::MAX);
        }
    }

    #[test]
    fn test_queue_type_visibility_timeouts_in_range() {
        // All visibility timeouts should be reasonable (2-15 minutes)
        assert!(QueueType::TransactionRequest.visibility_timeout_secs() >= 120);
        assert!(QueueType::TransactionRequest.visibility_timeout_secs() <= 900);

        assert!(QueueType::TransactionSubmission.visibility_timeout_secs() >= 120);
        assert!(QueueType::TransactionSubmission.visibility_timeout_secs() <= 900);

        assert!(QueueType::StatusCheck.visibility_timeout_secs() >= 120);
        assert!(QueueType::StatusCheck.visibility_timeout_secs() <= 900);

        assert!(QueueType::Notification.visibility_timeout_secs() >= 120);
        assert!(QueueType::Notification.visibility_timeout_secs() <= 900);
    }

    #[test]
    fn test_queue_type_polling_intervals_appropriate() {
        // Status check should be fastest
        assert_eq!(QueueType::StatusCheck.polling_interval_secs(), 2);

        // Others should be slower
        assert!(QueueType::TransactionRequest.polling_interval_secs() >= 10);
        assert!(QueueType::TransactionSubmission.polling_interval_secs() >= 10);
        assert!(QueueType::Notification.polling_interval_secs() >= 10);
    }

    #[test]
    fn test_queue_backend_error_variants() {
        let errors = vec![
            QueueBackendError::RedisError("test".to_string()),
            QueueBackendError::SqsError("test".to_string()),
            QueueBackendError::SerializationError("test".to_string()),
            QueueBackendError::ConfigError("test".to_string()),
            QueueBackendError::QueueNotFound("test".to_string()),
            QueueBackendError::WorkerInitError("test".to_string()),
            QueueBackendError::QueueError("test".to_string()),
        ];

        for error in errors {
            let error_str = error.to_string();
            assert!(!error_str.is_empty());
        }
    }
}
