use thiserror::Error;

#[derive(Debug, Error)]
pub enum NetworkError {
    #[error("Invalid network: {0}")]
    InvalidNetwork(String),
}
