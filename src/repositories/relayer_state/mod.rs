//! Relayer State Repository Module
//!
//! This module provides the relayer state repository layer for the OpenZeppelin Relayer service.
//! It implements the Repository pattern to track sync state for different relayers,
//! including the last synced blockchain index and serialized ledger context.
//!
//! ## Features
//!
//! - **Sync State Tracking**: Maintain last synced index and ledger context per relayer
//! - **Atomic Updates**: Update-if-greater operations for safe concurrent access
//! - **Context Management**: Store and retrieve serialized ledger contexts
//! - **Bulk Operations**: Get all sync states across relayers
//!
//! ## Repository Implementations
//!
//! - [`InMemoryRelayerStateRepository`]: Fast in-memory storage for testing/development
//! - [`RedisRelayerStateRepository`]: Redis-backed storage for production environments

use async_trait::async_trait;
use redis::aio::ConnectionManager;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use thiserror::Error;

#[cfg(test)]
use mockall::automock;

mod relayer_state_in_memory;
mod relayer_state_redis;

pub use relayer_state_in_memory::InMemoryRelayerStateRepository;
pub use relayer_state_redis::RedisRelayerStateRepository;

use crate::models::RepositoryError;

/// Represents the sync state for a relayer, including blockchain index and ledger context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayerSyncState {
    /// The last synced blockchain index
    pub last_synced_index: u64,
    /// The serialized ledger context (optional, as it may not be available initially)
    pub ledger_context: Option<Vec<u8>>,
}

#[derive(Error, Debug, Serialize)]
pub enum SyncStateError {
    #[error("Sync state not found for relayer {relayer_id}")]
    NotFound { relayer_id: String },
    #[error("Invalid blockchain index {index} for relayer {relayer_id}")]
    InvalidIndex { relayer_id: String, index: u64 },
    #[error("Failed to serialize/deserialize: {0}")]
    SerializationError(String),
}

#[allow(dead_code)]
#[cfg_attr(test, automock)]
#[async_trait]
pub trait SyncStateTrait: Send + Sync {
    /// Get the last synced blockchain index for a relayer
    async fn get_last_synced_index(&self, relayer_id: &str) -> Result<Option<u64>, SyncStateError>;

    /// Get the serialized ledger context for a relayer
    async fn get_ledger_context(&self, relayer_id: &str)
    -> Result<Option<Vec<u8>>, SyncStateError>;

    /// Set the last synced blockchain index for a relayer
    async fn set_last_synced_index(
        &self,
        relayer_id: &str,
        index: u64,
    ) -> Result<(), SyncStateError>;

    /// Set the ledger context for a relayer
    async fn set_ledger_context(
        &self,
        relayer_id: &str,
        context: Vec<u8>,
    ) -> Result<(), SyncStateError>;

    /// Set both the last synced index and ledger context for a relayer
    async fn set_sync_state(
        &self,
        relayer_id: &str,
        index: u64,
        context: Option<Vec<u8>>,
    ) -> Result<(), SyncStateError>;

    /// Update the last synced blockchain index only if the new index is greater
    async fn update_if_greater(&self, relayer_id: &str, index: u64)
    -> Result<bool, SyncStateError>;

    /// Reset the sync state for a relayer
    async fn reset(&self, relayer_id: &str) -> Result<(), SyncStateError>;

    /// Get all sync states
    async fn get_all(&self) -> Vec<(String, RelayerSyncState)>;
}

#[derive(Debug, Clone)]
pub enum RelayerStateRepositoryStorage {
    InMemory(InMemoryRelayerStateRepository),
    Redis(RedisRelayerStateRepository),
}

impl RelayerStateRepositoryStorage {
    pub fn new_in_memory() -> Self {
        Self::InMemory(InMemoryRelayerStateRepository::new())
    }

    pub fn new_redis(
        connection_manager: Arc<ConnectionManager>,
        key_prefix: String,
    ) -> Result<Self, RepositoryError> {
        let redis_repo = RedisRelayerStateRepository::new(connection_manager, key_prefix)?;
        Ok(Self::Redis(redis_repo))
    }
}

#[async_trait]
impl SyncStateTrait for RelayerStateRepositoryStorage {
    async fn get_last_synced_index(&self, relayer_id: &str) -> Result<Option<u64>, SyncStateError> {
        match self {
            RelayerStateRepositoryStorage::InMemory(repo) => {
                repo.get_last_synced_index(relayer_id).await
            }
            RelayerStateRepositoryStorage::Redis(repo) => {
                repo.get_last_synced_index(relayer_id).await
            }
        }
    }

    async fn get_ledger_context(
        &self,
        relayer_id: &str,
    ) -> Result<Option<Vec<u8>>, SyncStateError> {
        match self {
            RelayerStateRepositoryStorage::InMemory(repo) => {
                repo.get_ledger_context(relayer_id).await
            }
            RelayerStateRepositoryStorage::Redis(repo) => repo.get_ledger_context(relayer_id).await,
        }
    }

    async fn set_last_synced_index(
        &self,
        relayer_id: &str,
        index: u64,
    ) -> Result<(), SyncStateError> {
        match self {
            RelayerStateRepositoryStorage::InMemory(repo) => {
                repo.set_last_synced_index(relayer_id, index).await
            }
            RelayerStateRepositoryStorage::Redis(repo) => {
                repo.set_last_synced_index(relayer_id, index).await
            }
        }
    }

    async fn set_ledger_context(
        &self,
        relayer_id: &str,
        context: Vec<u8>,
    ) -> Result<(), SyncStateError> {
        match self {
            RelayerStateRepositoryStorage::InMemory(repo) => {
                repo.set_ledger_context(relayer_id, context).await
            }
            RelayerStateRepositoryStorage::Redis(repo) => {
                repo.set_ledger_context(relayer_id, context).await
            }
        }
    }

    async fn set_sync_state(
        &self,
        relayer_id: &str,
        index: u64,
        context: Option<Vec<u8>>,
    ) -> Result<(), SyncStateError> {
        match self {
            RelayerStateRepositoryStorage::InMemory(repo) => {
                repo.set_sync_state(relayer_id, index, context).await
            }
            RelayerStateRepositoryStorage::Redis(repo) => {
                repo.set_sync_state(relayer_id, index, context).await
            }
        }
    }

    async fn update_if_greater(
        &self,
        relayer_id: &str,
        index: u64,
    ) -> Result<bool, SyncStateError> {
        match self {
            RelayerStateRepositoryStorage::InMemory(repo) => {
                repo.update_if_greater(relayer_id, index).await
            }
            RelayerStateRepositoryStorage::Redis(repo) => {
                repo.update_if_greater(relayer_id, index).await
            }
        }
    }

    async fn reset(&self, relayer_id: &str) -> Result<(), SyncStateError> {
        match self {
            RelayerStateRepositoryStorage::InMemory(repo) => repo.reset(relayer_id).await,
            RelayerStateRepositoryStorage::Redis(repo) => repo.reset(relayer_id).await,
        }
    }

    async fn get_all(&self) -> Vec<(String, RelayerSyncState)> {
        match self {
            RelayerStateRepositoryStorage::InMemory(repo) => repo.get_all().await,
            RelayerStateRepositoryStorage::Redis(repo) => repo.get_all().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_trait_methods_accessibility() {
        // Create in-memory repository through the storage enum
        let repo = RelayerStateRepositoryStorage::new_in_memory();

        // Test basic operations
        let relayer_id = "test_relayer";

        // Initially should be None
        assert_eq!(repo.get_last_synced_index(relayer_id).await.unwrap(), None);

        // Set a value
        repo.set_last_synced_index(relayer_id, 100).await.unwrap();
        assert_eq!(
            repo.get_last_synced_index(relayer_id).await.unwrap(),
            Some(100)
        );

        // Reset
        repo.reset(relayer_id).await.unwrap();
        assert_eq!(repo.get_last_synced_index(relayer_id).await.unwrap(), None);
    }
}
