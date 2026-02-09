//! Queue backend type definitions.
//!
//! This module defines types for the queue backend abstraction layer:
//! - QueueType: Enum representing different queue types
//! - QueueBackendError: Error types for queue operations
//! - WorkerHandle: Handle to running worker tasks
//! - QueueHealth: Queue health status information

use actix_web::web::ThinData;
use apalis::prelude::{Attempt, Data, Error, TaskId};
use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;

/// Queue types for relayer operations.
///
/// Each variant represents a specific job processing queue with its own
/// configuration, concurrency limits, and retry behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum QueueType {
    /// Transaction request queue - initial validation and preparation
    TransactionRequest,
    /// Transaction submission queue - network submission (Submit/Resubmit/Cancel/Resend)
    TransactionSubmission,
    /// Status check queue - high-frequency status monitoring
    StatusCheck,
    /// Notification queue - webhook delivery
    Notification,
    /// Token swap request queue
    TokenSwapRequest,
    /// Relayer health check queue
    RelayerHealthCheck,
}

impl QueueType {
    /// Returns the queue name for logging and identification purposes.
    pub fn queue_name(&self) -> &'static str {
        match self {
            Self::TransactionRequest => "transaction-request",
            Self::TransactionSubmission => "transaction-submission",
            Self::StatusCheck => "status-check",
            Self::Notification => "notification",
            Self::TokenSwapRequest => "token-swap-request",
            Self::RelayerHealthCheck => "relayer-health-check",
        }
    }

    /// Returns the Redis namespace for this queue type (Apalis format).
    pub fn redis_namespace(&self) -> &'static str {
        match self {
            Self::TransactionRequest => "relayer:transaction_request",
            Self::TransactionSubmission => "relayer:transaction_submission",
            Self::StatusCheck => "relayer:transaction_status_stellar",
            Self::Notification => "relayer:notification",
            Self::TokenSwapRequest => "relayer:token_swap_request",
            Self::RelayerHealthCheck => "relayer:relayer_health_check",
        }
    }

    /// Returns the maximum number of retries for this queue type.
    ///
    /// - Transaction request: 5 retries (validation can be retried)
    /// - Transaction submission: 1 retry (avoid duplicate submissions)
    /// - Status check: usize::MAX (circuit breaker handles termination)
    /// - Notification: 5 retries (webhook delivery can be retried)
    pub fn max_retries(&self) -> usize {
        match self {
            Self::TransactionRequest => 5,
            Self::TransactionSubmission => 1,
            Self::StatusCheck => usize::MAX, // Circuit breaker terminates
            Self::Notification => 5,
            Self::TokenSwapRequest => 5,
            Self::RelayerHealthCheck => 5,
        }
    }

    /// Returns the visibility timeout in seconds for SQS (how long worker has to process).
    pub fn visibility_timeout_secs(&self) -> u32 {
        match self {
            Self::TransactionRequest => 300,    // 5 minutes
            Self::TransactionSubmission => 120, // 2 minutes
            Self::StatusCheck => 300,           // 5 minutes
            Self::Notification => 180,          // 3 minutes
            Self::TokenSwapRequest => 300,      // 5 minutes
            Self::RelayerHealthCheck => 300,    // 5 minutes
        }
    }

    /// Returns the polling interval in seconds (how often to check for new messages).
    pub fn polling_interval_secs(&self) -> u64 {
        match self {
            Self::TransactionRequest => 20,
            Self::TransactionSubmission => 20,
            Self::StatusCheck => 2, // High frequency for status checks
            Self::Notification => 20,
            Self::TokenSwapRequest => 20,
            Self::RelayerHealthCheck => 20,
        }
    }
}

impl fmt::Display for QueueType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.queue_name())
    }
}

/// Errors that can occur during queue backend operations.
#[derive(Debug, Error, Serialize, Clone)]
pub enum QueueBackendError {
    /// Redis-specific error (from Apalis or redis crate)
    #[error("Redis error: {0}")]
    RedisError(String),

    /// SQS-specific error (from AWS SDK)
    #[error("SQS error: {0}")]
    SqsError(String),

    /// Job serialization error
    #[error("Serialization error: {0}")]
    SerializationError(String),

    /// Configuration error (missing env vars, invalid config)
    #[error("Configuration error: {0}")]
    ConfigError(String),

    /// Queue not found or not initialized
    #[error("Queue not found: {0}")]
    QueueNotFound(String),

    /// Worker initialization error
    #[error("Worker initialization error: {0}")]
    WorkerInitError(String),

    /// Generic queue error
    #[error("Queue error: {0}")]
    QueueError(String),
}

/// Handle to a running worker task.
///
/// Workers process jobs from queues. The handle can be used to:
/// - Monitor worker health
/// - Gracefully shutdown workers
/// - Await worker completion
#[derive(Debug)]
pub enum WorkerHandle {
    /// Apalis worker handle (from Redis backend)
    Apalis(Box<dyn std::any::Any + Send>),
    /// Tokio task handle (from SQS backend)
    Tokio(tokio::task::JoinHandle<()>),
}

/// Queue health status information.
///
/// Used for health checks and monitoring queue depths.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueHealth {
    /// Queue type being checked
    pub queue_type: QueueType,
    /// Number of messages visible in queue (ready to process)
    pub messages_visible: u64,
    /// Number of messages in flight (being processed)
    pub messages_in_flight: u64,
    /// Number of messages in dead-letter queue (failed after max retries)
    pub messages_dlq: u64,
    /// Backend type (redis or sqs)
    pub backend: String,
    /// Is the queue healthy? (defined by backend-specific criteria)
    pub is_healthy: bool,
}

/// Shared worker argument types used by queue backends.
pub type QueueWorkerData = Data<ThinData<crate::models::DefaultAppState>>;
pub type QueueWorkerAttempt = Attempt;
pub type QueueWorkerTaskId = TaskId;
pub type QueueWorkerError = Error;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_queue_type_names() {
        assert_eq!(
            QueueType::TransactionRequest.queue_name(),
            "transaction-request"
        );
        assert_eq!(
            QueueType::TransactionSubmission.queue_name(),
            "transaction-submission"
        );
        assert_eq!(QueueType::StatusCheck.queue_name(), "status-check");
        assert_eq!(QueueType::Notification.queue_name(), "notification");
        assert_eq!(
            QueueType::TokenSwapRequest.queue_name(),
            "token-swap-request"
        );
        assert_eq!(
            QueueType::RelayerHealthCheck.queue_name(),
            "relayer-health-check"
        );
    }

    #[test]
    fn test_queue_type_redis_namespaces() {
        assert_eq!(
            QueueType::TransactionRequest.redis_namespace(),
            "relayer:transaction_request"
        );
        assert_eq!(
            QueueType::TransactionSubmission.redis_namespace(),
            "relayer:transaction_submission"
        );
        assert_eq!(
            QueueType::StatusCheck.redis_namespace(),
            "relayer:transaction_status_stellar"
        );
        assert_eq!(
            QueueType::Notification.redis_namespace(),
            "relayer:notification"
        );
        assert_eq!(
            QueueType::TokenSwapRequest.redis_namespace(),
            "relayer:token_swap_request"
        );
        assert_eq!(
            QueueType::RelayerHealthCheck.redis_namespace(),
            "relayer:relayer_health_check"
        );
    }

    #[test]
    fn test_queue_type_max_retries() {
        assert_eq!(QueueType::TransactionRequest.max_retries(), 5);
        assert_eq!(QueueType::TransactionSubmission.max_retries(), 1);
        assert_eq!(QueueType::StatusCheck.max_retries(), usize::MAX);
        assert_eq!(QueueType::Notification.max_retries(), 5);
        assert_eq!(QueueType::TokenSwapRequest.max_retries(), 5);
        assert_eq!(QueueType::RelayerHealthCheck.max_retries(), 5);
    }

    #[test]
    fn test_queue_type_visibility_timeout() {
        assert_eq!(QueueType::TransactionRequest.visibility_timeout_secs(), 300);
        assert_eq!(
            QueueType::TransactionSubmission.visibility_timeout_secs(),
            120
        );
        assert_eq!(QueueType::StatusCheck.visibility_timeout_secs(), 300);
        assert_eq!(QueueType::Notification.visibility_timeout_secs(), 180);
        assert_eq!(QueueType::TokenSwapRequest.visibility_timeout_secs(), 300);
        assert_eq!(QueueType::RelayerHealthCheck.visibility_timeout_secs(), 300);
    }

    #[test]
    fn test_queue_type_polling_interval() {
        assert_eq!(QueueType::TransactionRequest.polling_interval_secs(), 20);
        assert_eq!(QueueType::TransactionSubmission.polling_interval_secs(), 20);
        assert_eq!(QueueType::StatusCheck.polling_interval_secs(), 2);
        assert_eq!(QueueType::Notification.polling_interval_secs(), 20);
        assert_eq!(QueueType::TokenSwapRequest.polling_interval_secs(), 20);
        assert_eq!(QueueType::RelayerHealthCheck.polling_interval_secs(), 20);
    }
}
