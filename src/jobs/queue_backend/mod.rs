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
//! backend.produce(job, QueueType::StellarTransactionRequest, None).await?;
//!
//! // Initialize workers
//! let workers = backend.initialize_workers(app_state).await?;
//! ```

use async_trait::async_trait;
use std::sync::Arc;

use crate::{
    jobs::{Job, NotificationSend, TransactionRequest, TransactionSend, TransactionStatusCheck},
    models::DefaultAppState,
    utils::RedisConnections,
};
use actix_web::web::ThinData;

pub mod redis_backend;
pub mod types;

pub use types::{QueueBackendError, QueueHealth, QueueType, WorkerHandle};

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
    let backend_type = std::env::var("QUEUE_BACKEND").unwrap_or_else(|_| "redis".to_string());

    match backend_type.to_lowercase().as_str() {
        "redis" => {
            let backend = redis_backend::RedisBackend::new(redis_connections).await?;
            Ok(Arc::new(backend))
        }
        "sqs" => {
            // SQS backend will be implemented in Phase 2
            Err(QueueBackendError::ConfigError(
                "SQS backend not yet implemented. Use QUEUE_BACKEND=redis".to_string(),
            ))
        }
        other => Err(QueueBackendError::ConfigError(format!(
            "Unsupported QUEUE_BACKEND value: {}. Must be 'redis' or 'sqs'",
            other
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
            QueueType::StellarTransactionRequest,
            QueueType::StellarTransactionSubmission,
            QueueType::StellarStatusCheck,
            QueueType::StellarNotification,
        ];

        for queue_type in types {
            assert!(!queue_type.queue_name().is_empty());
            assert!(!queue_type.redis_namespace().is_empty());
            assert!(queue_type.max_retries() > 0 || queue_type.max_retries() == usize::MAX);
        }
    }
}
