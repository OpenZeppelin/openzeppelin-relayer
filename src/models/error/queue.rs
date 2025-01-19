use thiserror::Error;

#[derive(Debug, Error)]
pub enum QueueError {
    #[error("Failed to push job: {0}")]
    PushError(String),
    #[error("Invalid job data: {0}")]
    ValidationError(String),
}
