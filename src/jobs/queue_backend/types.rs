//! Queue backend type definitions.
//!
//! This module defines types for the queue backend abstraction layer:
//! - QueueType: Enum representing different queue types (Stellar-specific)
//! - QueueBackendError: Error types for queue operations
//! - WorkerHandle: Handle to running worker tasks
//! - QueueHealth: Queue health status information

use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;

/// Queue types for Stellar relayer operations.
///
/// Each variant represents a specific job processing queue with its own
/// configuration, concurrency limits, and retry behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum QueueType {
    /// Transaction request queue - initial validation and preparation
    StellarTransactionRequest,
    /// Transaction submission queue - network submission (Submit/Resubmit/Cancel/Resend)
    StellarTransactionSubmission,
    /// Stellar status check queue - high-frequency status monitoring
    StellarStatusCheck,
    /// Notification queue - webhook delivery
    StellarNotification,
}

impl QueueType {
    /// Returns the queue name for logging and identification purposes.
    pub fn queue_name(&self) -> &'static str {
        match self {
            Self::StellarTransactionRequest => "transaction-request",
            Self::StellarTransactionSubmission => "transaction-submission",
            Self::StellarStatusCheck => "status-check",
            Self::StellarNotification => "notification",
        }
    }

    /// Returns the Redis namespace for this queue type (Apalis format).
    pub fn redis_namespace(&self) -> &'static str {
        match self {
            Self::StellarTransactionRequest => "relayer:transaction_request",
            Self::StellarTransactionSubmission => "relayer:transaction_submission",
            Self::StellarStatusCheck => "relayer:transaction_status_stellar",
            Self::StellarNotification => "relayer:notification",
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
            Self::StellarTransactionRequest => 5,
            Self::StellarTransactionSubmission => 1,
            Self::StellarStatusCheck => usize::MAX, // Circuit breaker terminates
            Self::StellarNotification => 5,
        }
    }

    /// Returns the visibility timeout in seconds for SQS (how long worker has to process).
    pub fn visibility_timeout_secs(&self) -> u32 {
        match self {
            Self::StellarTransactionRequest => 300,    // 5 minutes
            Self::StellarTransactionSubmission => 120, // 2 minutes
            Self::StellarStatusCheck => 300,           // 5 minutes
            Self::StellarNotification => 180,          // 3 minutes
        }
    }

    /// Returns the polling interval in seconds (how often to check for new messages).
    pub fn polling_interval_secs(&self) -> u64 {
        match self {
            Self::StellarTransactionRequest => 20,
            Self::StellarTransactionSubmission => 20,
            Self::StellarStatusCheck => 2, // High frequency for status checks
            Self::StellarNotification => 20,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_queue_type_names() {
        assert_eq!(
            QueueType::StellarTransactionRequest.queue_name(),
            "transaction-request"
        );
        assert_eq!(
            QueueType::StellarTransactionSubmission.queue_name(),
            "transaction-submission"
        );
        assert_eq!(QueueType::StellarStatusCheck.queue_name(), "status-check");
        assert_eq!(QueueType::StellarNotification.queue_name(), "notification");
    }

    #[test]
    fn test_queue_type_redis_namespaces() {
        assert_eq!(
            QueueType::StellarTransactionRequest.redis_namespace(),
            "relayer:transaction_request"
        );
        assert_eq!(
            QueueType::StellarTransactionSubmission.redis_namespace(),
            "relayer:transaction_submission"
        );
        assert_eq!(
            QueueType::StellarStatusCheck.redis_namespace(),
            "relayer:transaction_status_stellar"
        );
        assert_eq!(
            QueueType::StellarNotification.redis_namespace(),
            "relayer:notification"
        );
    }

    #[test]
    fn test_queue_type_max_retries() {
        assert_eq!(QueueType::StellarTransactionRequest.max_retries(), 5);
        assert_eq!(QueueType::StellarTransactionSubmission.max_retries(), 1);
        assert_eq!(QueueType::StellarStatusCheck.max_retries(), usize::MAX);
        assert_eq!(QueueType::StellarNotification.max_retries(), 5);
    }

    #[test]
    fn test_queue_type_visibility_timeout() {
        assert_eq!(
            QueueType::StellarTransactionRequest.visibility_timeout_secs(),
            300
        );
        assert_eq!(
            QueueType::StellarTransactionSubmission.visibility_timeout_secs(),
            120
        );
        assert_eq!(QueueType::StellarStatusCheck.visibility_timeout_secs(), 300);
        assert_eq!(
            QueueType::StellarNotification.visibility_timeout_secs(),
            180
        );
    }

    #[test]
    fn test_queue_type_polling_interval() {
        assert_eq!(
            QueueType::StellarTransactionRequest.polling_interval_secs(),
            20
        );
        assert_eq!(
            QueueType::StellarTransactionSubmission.polling_interval_secs(),
            20
        );
        assert_eq!(QueueType::StellarStatusCheck.polling_interval_secs(), 2);
        assert_eq!(QueueType::StellarNotification.polling_interval_secs(), 20);
    }
}
