//! Worker types for the queue abstraction.
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;

use crate::queues::QueueType;

/// Marks a successful status check whose transaction has not reached a final state.
#[derive(Debug, thiserror::Error)]
#[error("transaction not in final state")]
pub struct NotYetFinal;

/// Retry classification used by queue backends when selecting a delay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RetryKind {
    NotYetFinal,
    Other,
}

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
    /// Backlog depth (undelivered messages). `None` means the depth is
    /// **unavailable** (e.g. the backlog read failed or the backend cannot
    /// report it), which must NOT be conflated with an empty queue (`Some(0)`).
    pub messages_visible: Option<u64>,
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
    NotYetFinal,
    Abort(String),
}

impl fmt::Display for HandlerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Retry(msg) => write!(f, "Retry: {msg}"),
            Self::NotYetFinal => NotYetFinal.fmt(f),
            Self::Abort(msg) => write!(f, "Abort: {msg}"),
        }
    }
}

impl std::error::Error for HandlerError {}

impl From<HandlerError> for apalis::prelude::Error {
    fn from(err: HandlerError) -> Self {
        match err {
            HandlerError::Retry(msg) => apalis::prelude::Error::Failed(Arc::new(msg.into())),
            HandlerError::NotYetFinal => {
                apalis::prelude::Error::Failed(Arc::new(Box::new(NotYetFinal)))
            }
            HandlerError::Abort(msg) => apalis::prelude::Error::Abort(Arc::new(msg.into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_worker_context_new() {
        let ctx = WorkerContext::new(3, "task-abc".to_string());
        assert_eq!(ctx.attempt, 3);
        assert_eq!(ctx.task_id, "task-abc");
    }

    #[test]
    fn test_handler_error_retry_display() {
        let err = HandlerError::Retry("connection timeout".to_string());
        assert_eq!(err.to_string(), "Retry: connection timeout");
    }

    #[test]
    fn test_handler_error_abort_display() {
        let err = HandlerError::Abort("invalid payload".to_string());
        assert_eq!(err.to_string(), "Abort: invalid payload");
    }

    #[test]
    fn test_handler_error_not_yet_final_display() {
        assert_eq!(
            HandlerError::NotYetFinal.to_string(),
            "transaction not in final state"
        );
    }

    #[test]
    fn test_handler_error_retry_into_apalis_failed() {
        let err = HandlerError::Retry("temp failure".to_string());
        let apalis_err: apalis::prelude::Error = err.into();
        assert!(
            matches!(apalis_err, apalis::prelude::Error::Failed(_)),
            "Retry should map to Failed"
        );
    }

    #[test]
    fn test_handler_error_abort_into_apalis_abort() {
        let err = HandlerError::Abort("permanent failure".to_string());
        let apalis_err: apalis::prelude::Error = err.into();
        assert!(
            matches!(apalis_err, apalis::prelude::Error::Abort(_)),
            "Abort should map to Abort"
        );
    }

    #[test]
    fn test_not_yet_final_marker_survives_apalis_conversion() {
        let apalis_err: apalis::prelude::Error = HandlerError::NotYetFinal.into();
        match apalis_err {
            apalis::prelude::Error::Failed(inner) => {
                assert!(inner
                    .as_ref()
                    .as_ref()
                    .downcast_ref::<NotYetFinal>()
                    .is_some());
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }
}
