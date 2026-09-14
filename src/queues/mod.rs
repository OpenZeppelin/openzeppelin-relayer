//! Queue backend abstraction layer.
//!
//! This module provides a backend-agnostic interface for job queue operations.
//! Implementations can use Redis/Apalis, AWS SQS, or GCP Pub/Sub as the backend.
//!
//! # Environment Variables
//!
//! - `QUEUE_BACKEND`: Backend to use ("redis", "sqs", or "pubsub" / "gcp-pubsub",
//!   default: "redis")
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
//! // Initialize workers on the pipeline runtime
//! let workers = backend.initialize_workers(app_state, handle).await?;
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

pub mod cron;
pub mod errors;
pub mod pubsub;
pub mod queue_type;
pub mod rabbitmq;
pub mod redis;
pub mod retry_config;
pub mod schedule;
pub mod sqs;
pub mod swap_filter;
pub mod worker_shared;
pub mod worker_types;

pub use errors::QueueBackendError;
pub use queue_type::QueueType;
pub use redis::queue::Queue;
pub use retry_config::{backoff_config_for_queue, retry_delay_secs, status_check_retry_delay_secs};
pub use swap_filter::filter_relayers_for_swap;
pub use worker_types::{HandlerError, QueueHealth, WorkerContext, WorkerHandle};

/// Installs a process-default rustls `CryptoProvider` if none is set yet.
///
/// rustls 0.23 needs a process-default `CryptoProvider` when more than one
/// provider is compiled in. Both are present in this binary (aws-lc-rs via the
/// gcloud / AWS SDK / lapin trees, ring via aws-config / Solana), so rustls
/// cannot select one automatically and any TLS that relies on the process
/// default — gcloud-pubsub's gRPC TLS, lapin's `amqps://` — panics on the first
/// real connection without this. Install once, ignore if a default is already
/// set. (Shared by the Pub/Sub and RabbitMQ backends.)
pub(crate) fn ensure_crypto_provider() {
    use rustls::crypto::{aws_lc_rs, CryptoProvider};
    if CryptoProvider::get_default().is_none() {
        let _ = aws_lc_rs::default_provider().install_default();
    }
}

/// Supported queue backend implementations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueBackendType {
    Redis,
    Sqs,
    PubSub,
    RabbitMq,
}

impl QueueBackendType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Redis => "redis",
            Self::Sqs => "sqs",
            Self::PubSub => "pubsub",
            Self::RabbitMq => "rabbitmq",
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
    /// * `handle` - Handle to the multi-thread pipeline runtime. Workers MUST be
    ///   spawned via `handle.spawn` (not bare `tokio::spawn`) so background work
    ///   is distributed across the pipeline runtime's worker threads instead of
    ///   landing on the actix `System` arbiter's single thread.
    ///
    /// # Returns
    /// Vector of worker handles that can be used to monitor or stop workers
    async fn initialize_workers(
        &self,
        app_state: Arc<ThinData<DefaultAppState>>,
        handle: tokio::runtime::Handle,
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
    /// The default implementation is a no-op. Redis, SQS, and Pub/Sub backends all
    /// override this to broadcast a shutdown signal (via a `watch` channel) into
    /// their respective polling loops / cron tasks / Apalis Monitor signal futures,
    /// so a programmatic shutdown (no OS signal) still stops them promptly.
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
    PubSub(Box<pubsub::backend::PubSubBackend>),
    RabbitMq(Box<rabbitmq::backend::RabbitMqBackend>),
}

impl std::fmt::Debug for QueueBackendStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Redis(b) => std::fmt::Debug::fmt(b, f),
            Self::Sqs(b) => std::fmt::Debug::fmt(b, f),
            Self::PubSub(b) => std::fmt::Debug::fmt(b, f),
            Self::RabbitMq(b) => std::fmt::Debug::fmt(b, f),
        }
    }
}

impl QueueBackendStorage {
    /// Returns a reference to the underlying `Queue` when the backend is Redis.
    ///
    /// Returns `None` for non-Redis backends (e.g. SQS, Pub/Sub) that do not use
    /// a `Queue`.
    pub fn queue(&self) -> Option<&Queue> {
        match self {
            Self::Redis(b) => Some(b.queue()),
            Self::Sqs(_) | Self::PubSub(_) | Self::RabbitMq(_) => None,
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
            Self::PubSub(b) => b.produce_transaction_request(job, scheduled_on).await,
            Self::RabbitMq(b) => b.produce_transaction_request(job, scheduled_on).await,
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
            Self::PubSub(b) => b.produce_transaction_submission(job, scheduled_on).await,
            Self::RabbitMq(b) => b.produce_transaction_submission(job, scheduled_on).await,
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
            Self::PubSub(b) => b.produce_transaction_status_check(job, scheduled_on).await,
            Self::RabbitMq(b) => b.produce_transaction_status_check(job, scheduled_on).await,
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
            Self::PubSub(b) => b.produce_notification(job, scheduled_on).await,
            Self::RabbitMq(b) => b.produce_notification(job, scheduled_on).await,
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
            Self::PubSub(b) => b.produce_token_swap_request(job, scheduled_on).await,
            Self::RabbitMq(b) => b.produce_token_swap_request(job, scheduled_on).await,
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
            Self::PubSub(b) => b.produce_relayer_health_check(job, scheduled_on).await,
            Self::RabbitMq(b) => b.produce_relayer_health_check(job, scheduled_on).await,
        }
    }

    async fn initialize_workers(
        &self,
        app_state: Arc<ThinData<DefaultAppState>>,
        handle: tokio::runtime::Handle,
    ) -> Result<Vec<WorkerHandle>, QueueBackendError> {
        match self {
            Self::Redis(b) => b.initialize_workers(app_state, handle).await,
            Self::Sqs(b) => b.initialize_workers(app_state, handle).await,
            Self::PubSub(b) => b.initialize_workers(app_state, handle).await,
            Self::RabbitMq(b) => b.initialize_workers(app_state, handle).await,
        }
    }

    async fn health_check(&self) -> Result<Vec<QueueHealth>, QueueBackendError> {
        match self {
            Self::Redis(b) => b.health_check().await,
            Self::Sqs(b) => b.health_check().await,
            Self::PubSub(b) => b.health_check().await,
            Self::RabbitMq(b) => b.health_check().await,
        }
    }

    fn backend_type(&self) -> QueueBackendType {
        match self {
            Self::Redis(b) => b.backend_type(),
            Self::Sqs(b) => b.backend_type(),
            Self::PubSub(b) => b.backend_type(),
            Self::RabbitMq(b) => b.backend_type(),
        }
    }

    fn shutdown(&self) {
        match self {
            Self::Redis(b) => b.shutdown(),
            Self::Sqs(b) => b.shutdown(),
            Self::PubSub(b) => b.shutdown(),
            Self::RabbitMq(b) => b.shutdown(),
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
        "pubsub" | "gcp-pubsub" => {
            let backend = pubsub::backend::PubSubBackend::new(redis_connections).await?;
            QueueBackendStorage::PubSub(Box::new(backend))
        }
        "rabbitmq" => {
            let backend = rabbitmq::backend::RabbitMqBackend::new(redis_connections).await?;
            QueueBackendStorage::RabbitMq(Box::new(backend))
        }
        other => {
            return Err(QueueBackendError::ConfigError(format!(
                "Unsupported QUEUE_BACKEND value: {other}. Must be 'redis', 'sqs', 'pubsub' (alias 'gcp-pubsub'), or 'rabbitmq'"
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
        assert_eq!(QueueType::StatusCheck.default_wait_time_secs(), 5);

        // Others should be slower
        assert!(QueueType::TransactionRequest.default_wait_time_secs() >= 5);
        assert!(QueueType::TransactionSubmission.default_wait_time_secs() >= 5);
        assert!(QueueType::Notification.default_wait_time_secs() >= 10);
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
        assert_eq!(QueueBackendType::PubSub.as_str(), "pubsub");
        assert_eq!(QueueBackendType::RabbitMq.as_str(), "rabbitmq");
        assert_eq!(QueueBackendType::Redis.to_string(), "redis");
        assert_eq!(QueueBackendType::Sqs.to_string(), "sqs");
        assert_eq!(QueueBackendType::PubSub.to_string(), "pubsub");
        assert_eq!(QueueBackendType::RabbitMq.to_string(), "rabbitmq");
    }

    // ── create_queue_backend dispatch ──────────────────────────────
    //
    // Serializes the env mutation these tests need. The Pub/Sub path fast-fails
    // on a missing PUBSUB_PROJECT_ID *before* any network/auth, so both the
    // alias-routing and unsupported-value checks stay deterministic and offline.
    use std::sync::Mutex as StdMutex;
    static BACKEND_ENV_LOCK: StdMutex<()> = StdMutex::new(());

    fn dummy_redis_connections() -> Arc<RedisConnections> {
        // deadpool builds lazily, so this never opens a connection.
        let pool = deadpool_redis::Config::from_url("redis://127.0.0.1:6379")
            .builder()
            .expect("pool builder")
            .max_size(1)
            .runtime(deadpool_redis::Runtime::Tokio1)
            .build()
            .expect("pool build");
        Arc::new(RedisConnections::new_single_pool(Arc::new(pool)))
    }

    #[tokio::test]
    async fn test_create_queue_backend_rejects_unsupported_value() {
        let _lock = BACKEND_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("QUEUE_BACKEND").ok();

        std::env::set_var("QUEUE_BACKEND", "kafka-not-real");
        let result = create_queue_backend(dummy_redis_connections()).await;

        match prev {
            Some(v) => std::env::set_var("QUEUE_BACKEND", v),
            None => std::env::remove_var("QUEUE_BACKEND"),
        }

        let err = result
            .expect_err("unsupported backend must error")
            .to_string();
        assert!(
            err.contains("Unsupported QUEUE_BACKEND"),
            "expected unsupported error, got: {err}"
        );
        // The error must advertise pubsub and rabbitmq as valid options.
        assert!(err.contains("pubsub"), "error should list pubsub: {err}");
        assert!(
            err.contains("rabbitmq"),
            "error should list rabbitmq: {err}"
        );
    }

    // No regression: adding the Pub/Sub backend must not remove redis/sqs
    // from the dispatch — both are still recognized as valid backend selectors.
    #[tokio::test]
    async fn test_redis_and_sqs_backends_still_recognized() {
        let _lock = BACKEND_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("QUEUE_BACKEND").ok();

        std::env::set_var("QUEUE_BACKEND", "definitely-not-a-backend");
        let err = create_queue_backend(dummy_redis_connections())
            .await
            .expect_err("unsupported value must error")
            .to_string();

        match prev {
            Some(v) => std::env::set_var("QUEUE_BACKEND", v),
            None => std::env::remove_var("QUEUE_BACKEND"),
        }

        // The unsupported-value error still lists redis and sqs alongside the
        // newer backends, proving the pre-existing backends were not dropped.
        assert!(err.contains("redis"), "redis must remain selectable: {err}");
        assert!(err.contains("sqs"), "sqs must remain selectable: {err}");
        assert!(err.contains("pubsub"), "pubsub must be selectable: {err}");
        assert!(
            err.contains("rabbitmq"),
            "rabbitmq must be selectable: {err}"
        );
    }

    #[tokio::test]
    async fn test_create_queue_backend_routes_pubsub_aliases() {
        let _lock = BACKEND_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev_backend = std::env::var("QUEUE_BACKEND").ok();
        let prev_project = std::env::var("PUBSUB_PROJECT_ID").ok();

        // Force the Pub/Sub path to fast-fail before any network call.
        std::env::remove_var("PUBSUB_PROJECT_ID");

        for alias in ["pubsub", "gcp-pubsub"] {
            std::env::set_var("QUEUE_BACKEND", alias);
            let err = create_queue_backend(dummy_redis_connections())
                .await
                .expect_err("missing PUBSUB_PROJECT_ID must error")
                .to_string();
            // Routed to the Pub/Sub branch (NOT the catch-all "Unsupported").
            assert!(
                !err.contains("Unsupported QUEUE_BACKEND"),
                "alias '{alias}' should route to the Pub/Sub backend, got: {err}"
            );
            assert!(
                err.contains("PUBSUB_PROJECT_ID"),
                "alias '{alias}' should fail fast on missing project id, got: {err}"
            );
        }

        match prev_backend {
            Some(v) => std::env::set_var("QUEUE_BACKEND", v),
            None => std::env::remove_var("QUEUE_BACKEND"),
        }
        if let Some(v) = prev_project {
            std::env::set_var("PUBSUB_PROJECT_ID", v);
        }
    }

    #[tokio::test]
    async fn test_create_queue_backend_routes_rabbitmq() {
        let _lock = BACKEND_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev_backend = std::env::var("QUEUE_BACKEND").ok();
        let prev_url = std::env::var("RABBITMQ_URL").ok();

        // Force the RabbitMQ path to fast-fail before any network/connection.
        std::env::remove_var("RABBITMQ_URL");

        std::env::set_var("QUEUE_BACKEND", "rabbitmq");
        let err = create_queue_backend(dummy_redis_connections())
            .await
            .expect_err("missing RABBITMQ_URL must error")
            .to_string();
        // Routed to the RabbitMQ branch (NOT the catch-all "Unsupported").
        assert!(
            !err.contains("Unsupported QUEUE_BACKEND"),
            "'rabbitmq' should route to the RabbitMQ backend, got: {err}"
        );
        assert!(
            err.contains("RABBITMQ_URL"),
            "'rabbitmq' should fail fast on missing RABBITMQ_URL, got: {err}"
        );

        match prev_backend {
            Some(v) => std::env::set_var("QUEUE_BACKEND", v),
            None => std::env::remove_var("QUEUE_BACKEND"),
        }
        if let Some(v) = prev_url {
            std::env::set_var("RABBITMQ_URL", v);
        }
    }
}
