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
//! backend.produce_transaction_request(job, None).await?;
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

pub mod errors;
pub mod queue_type;
pub mod redis;
pub mod retry_config;
pub mod sqs;
pub mod swap_filter;
pub mod worker_types;

pub use errors::QueueBackendError;
pub use queue_type::QueueType;
pub use redis::queue::Queue;
pub use retry_config::status_check_retry_delay_secs;
pub use swap_filter::filter_relayers_for_swap;
pub use worker_types::{HandlerError, QueueHealth, WorkerContext, WorkerHandle};

/// Supported queue backend implementations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueBackendType {
    Redis,
    Sqs,
}

impl QueueBackendType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Redis => "redis",
            Self::Sqs => "sqs",
        }
    }
}

impl std::fmt::Display for QueueBackendType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

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

    /// Returns the backend type identifier.
    fn backend_type(&self) -> QueueBackendType;

    /// Signals all workers to shut down gracefully.
    ///
    /// The default implementation is a no-op (e.g. Redis/Apalis workers handle
    /// shutdown via Monitor's signal handling). SQS backend overrides this to
    /// broadcast a shutdown signal to all polling loops and cron tasks.
    fn shutdown(&self) {}
}

/// Enum-based queue backend storage, following the codebase convention
/// used by `SignerRepositoryStorage`, `NetworkRepositoryStorage`, etc.
///
/// Provides static dispatch over the concrete backend implementations
/// instead of `dyn QueueBackend` trait objects.
#[derive(Clone)]
pub enum QueueBackendStorage {
    Redis(Box<redis::backend::RedisBackend>),
    Sqs(sqs::backend::SqsBackend),
}

impl std::fmt::Debug for QueueBackendStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Redis(b) => std::fmt::Debug::fmt(b, f),
            Self::Sqs(b) => std::fmt::Debug::fmt(b, f),
        }
    }
}

impl QueueBackendStorage {
    /// Returns a reference to the underlying `Queue` when the backend is Redis.
    ///
    /// Returns `None` for non-Redis backends (e.g. SQS) that do not use a `Queue`.
    pub fn queue(&self) -> Option<&Queue> {
        match self {
            Self::Redis(b) => Some(b.queue()),
            Self::Sqs(_) => None,
        }
    }

    /// Returns the Redis connection pools when the backend is Redis.
    ///
    /// Delegates to `Queue::redis_connections()`. Returns `None` for non-Redis backends.
    pub fn redis_connections(&self) -> Option<Arc<RedisConnections>> {
        self.queue().map(|q| q.redis_connections())
    }
}

#[async_trait]
impl QueueBackend for QueueBackendStorage {
    async fn produce_transaction_request(
        &self,
        job: Job<TransactionRequest>,
        scheduled_on: Option<i64>,
    ) -> Result<String, QueueBackendError> {
        match self {
            Self::Redis(b) => b.produce_transaction_request(job, scheduled_on).await,
            Self::Sqs(b) => b.produce_transaction_request(job, scheduled_on).await,
        }
    }

    async fn produce_transaction_submission(
        &self,
        job: Job<TransactionSend>,
        scheduled_on: Option<i64>,
    ) -> Result<String, QueueBackendError> {
        match self {
            Self::Redis(b) => b.produce_transaction_submission(job, scheduled_on).await,
            Self::Sqs(b) => b.produce_transaction_submission(job, scheduled_on).await,
        }
    }

    async fn produce_transaction_status_check(
        &self,
        job: Job<TransactionStatusCheck>,
        scheduled_on: Option<i64>,
    ) -> Result<String, QueueBackendError> {
        match self {
            Self::Redis(b) => b.produce_transaction_status_check(job, scheduled_on).await,
            Self::Sqs(b) => b.produce_transaction_status_check(job, scheduled_on).await,
        }
    }

    async fn produce_notification(
        &self,
        job: Job<NotificationSend>,
        scheduled_on: Option<i64>,
    ) -> Result<String, QueueBackendError> {
        match self {
            Self::Redis(b) => b.produce_notification(job, scheduled_on).await,
            Self::Sqs(b) => b.produce_notification(job, scheduled_on).await,
        }
    }

    async fn produce_token_swap_request(
        &self,
        job: Job<TokenSwapRequest>,
        scheduled_on: Option<i64>,
    ) -> Result<String, QueueBackendError> {
        match self {
            Self::Redis(b) => b.produce_token_swap_request(job, scheduled_on).await,
            Self::Sqs(b) => b.produce_token_swap_request(job, scheduled_on).await,
        }
    }

    async fn produce_relayer_health_check(
        &self,
        job: Job<RelayerHealthCheck>,
        scheduled_on: Option<i64>,
    ) -> Result<String, QueueBackendError> {
        match self {
            Self::Redis(b) => b.produce_relayer_health_check(job, scheduled_on).await,
            Self::Sqs(b) => b.produce_relayer_health_check(job, scheduled_on).await,
        }
    }

    async fn initialize_workers(
        &self,
        app_state: Arc<ThinData<DefaultAppState>>,
    ) -> Result<Vec<WorkerHandle>, QueueBackendError> {
        match self {
            Self::Redis(b) => b.initialize_workers(app_state).await,
            Self::Sqs(b) => b.initialize_workers(app_state).await,
        }
    }

    async fn health_check(&self) -> Result<Vec<QueueHealth>, QueueBackendError> {
        match self {
            Self::Redis(b) => b.health_check().await,
            Self::Sqs(b) => b.health_check().await,
        }
    }

    fn backend_type(&self) -> QueueBackendType {
        match self {
            Self::Redis(b) => b.backend_type(),
            Self::Sqs(b) => b.backend_type(),
        }
    }

    fn shutdown(&self) {
        match self {
            Self::Redis(b) => b.shutdown(),
            Self::Sqs(b) => b.shutdown(),
        }
    }
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
/// Arc-wrapped `QueueBackendStorage` implementing QueueBackend
///
/// # Errors
/// Returns QueueBackendError::ConfigError if:
/// - QUEUE_BACKEND contains an unsupported value
/// - Required backend-specific configuration is missing
pub async fn create_queue_backend(
    redis_connections: Arc<RedisConnections>,
) -> Result<Arc<QueueBackendStorage>, QueueBackendError> {
    let backend_type = ServerConfig::get_queue_backend();

    let storage = match backend_type.to_lowercase().as_str() {
        "redis" => {
            let backend = redis::backend::RedisBackend::new(redis_connections).await?;
            QueueBackendStorage::Redis(Box::new(backend))
        }
        "sqs" => {
            let backend = sqs::backend::SqsBackend::new().await?;
            QueueBackendStorage::Sqs(backend)
        }
        other => {
            return Err(QueueBackendError::ConfigError(format!(
                "Unsupported QUEUE_BACKEND value: {other}. Must be 'redis' or 'sqs'"
            )));
        }
    };

    Ok(Arc::new(storage))
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
            QueueType::StatusCheckEvm,
            QueueType::StatusCheckStellar,
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
        // All visibility timeouts should be within SQS limits (0-43200).
        let all_types = [
            QueueType::TransactionRequest,
            QueueType::TransactionSubmission,
            QueueType::StatusCheck,
            QueueType::StatusCheckEvm,
            QueueType::StatusCheckStellar,
            QueueType::Notification,
            QueueType::TokenSwapRequest,
            QueueType::RelayerHealthCheck,
        ];
        for qt in all_types {
            let vt = qt.visibility_timeout_secs();
            assert!(vt > 0, "{qt}: visibility timeout must be > 0");
            assert!(
                vt <= 43200,
                "{qt}: visibility timeout {vt}s exceeds SQS max (43200s)"
            );
        }
    }

    #[test]
    fn test_queue_type_polling_intervals_appropriate() {
        // Status check should poll most frequently
        assert_eq!(QueueType::StatusCheck.polling_interval_secs(), 5);

        // Others should be slower
        assert!(QueueType::TransactionRequest.polling_interval_secs() >= 5);
        assert!(QueueType::TransactionSubmission.polling_interval_secs() >= 5);
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

    #[test]
    fn test_queue_backend_type_string_representations() {
        assert_eq!(QueueBackendType::Redis.as_str(), "redis");
        assert_eq!(QueueBackendType::Sqs.as_str(), "sqs");
        assert_eq!(QueueBackendType::Redis.to_string(), "redis");
        assert_eq!(QueueBackendType::Sqs.to_string(), "sqs");
    }
}
