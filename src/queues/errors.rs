use serde::Serialize;
use thiserror::Error;

/// Errors that can occur during queue backend operations.
#[derive(Debug, Error, Serialize, Clone)]
pub enum QueueBackendError {
    #[error("Redis error: {0}")]
    RedisError(String),
    #[error("SQS error: {0}")]
    SqsError(String),
    #[error("Serialization error: {0}")]
    SerializationError(String),
    #[error("Configuration error: {0}")]
    ConfigError(String),
    #[error("Queue not found: {0}")]
    QueueNotFound(String),
    #[error("Worker initialization error: {0}")]
    WorkerInitError(String),
    #[error("Queue error: {0}")]
    QueueError(String),
}
