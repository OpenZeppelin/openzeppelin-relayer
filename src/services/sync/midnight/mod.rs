pub mod handler;
pub mod indexer;
pub mod utils;

#[cfg(test)]
pub mod test_utils;

use crate::services::midnight::indexer::IndexerError;

///
/// Error types for wallet synchronization and session management.
///
/// Defines errors for indexer integration, session handling, viewing key issues, transaction parsing,
/// I/O, Merkle tree updates, and general sync errors.
#[allow(clippy::enum_variant_names)]
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("Indexer error: {0}")]
    IndexerError(#[from] IndexerError),

    #[error("Session error: {0}")]
    SessionError(String),

    #[error("Viewing key error: {0}")]
    ViewingKeyError(String),

    #[error("Transaction parse error: {0}")]
    ParseError(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Merkle tree update error: {0}")]
    MerkleTreeUpdateError(String),

    #[error("Sync error: {0}")]
    SyncError(String),
}
