//! Worker types for the queue abstraction.
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;

use crate::queues::QueueType;

/// Handle to a running worker task.
#[derive(Debug)]
pub enum WorkerHandle {
    Apalis(Box<dyn std::any::Any + Send>),
    Tokio(tokio::task::JoinHandle<()>),
}

/// Queue health status information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueHealth {
    pub queue_type: QueueType,
    pub messages_visible: u64,
    pub messages_in_flight: u64,
    pub messages_dlq: u64,
    pub backend: String,
    pub is_healthy: bool,
}

/// Backend-neutral context passed to all job handlers.
#[derive(Debug, Clone)]
pub struct WorkerContext {
    pub attempt: usize,
    pub task_id: String,
}

impl WorkerContext {
    pub fn new(attempt: usize, task_id: String) -> Self {
        Self { attempt, task_id }
    }
}

/// Backend-neutral handler error for retry control.
#[derive(Debug)]
pub enum HandlerError {
    Retry(String),
    Abort(String),
}

impl fmt::Display for HandlerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Retry(msg) => write!(f, "Retry: {msg}"),
            Self::Abort(msg) => write!(f, "Abort: {msg}"),
        }
    }
}

impl std::error::Error for HandlerError {}

impl From<HandlerError> for apalis::prelude::Error {
    fn from(err: HandlerError) -> Self {
        match err {
            HandlerError::Retry(msg) => apalis::prelude::Error::Failed(Arc::new(msg.into())),
            HandlerError::Abort(msg) => apalis::prelude::Error::Abort(Arc::new(msg.into())),
        }
    }
}
