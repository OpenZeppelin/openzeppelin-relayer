//! Midnight relayer sync-state persistence.
//!
//! This module is feature-gated because the state is only needed for Midnight's
//! indexer-led sync flow. It intentionally stays decoupled from the main
//! application state for now so the rest of the system can continue to use the
//! existing repository surface unchanged.

mod relayer_state_in_memory;
mod relayer_state_redis;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[cfg(test)]
use mockall::automock;

use crate::models::RepositoryError;
use crate::utils::RedisConnections;

pub use relayer_state_in_memory::InMemoryRelayerStateRepository;
pub use relayer_state_redis::RedisRelayerStateRepository;

use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RelayerSyncState {
    pub last_synced_index: u64,
    pub ledger_context: Option<Vec<u8>>,
    #[serde(default)]
    pub unshielded_balance: u128,
}

#[derive(Error, Debug, Serialize)]
pub enum SyncStateError {
    #[error("Sync state not found for relayer {relayer_id}")]
    NotFound { relayer_id: String },
    #[error("Invalid blockchain index {index} for relayer {relayer_id}")]
    InvalidIndex { relayer_id: String, index: u64 },
    #[error("Sync state serialization error: {0}")]
    SerializationError(String),
}

#[allow(dead_code)]
#[async_trait]
#[cfg_attr(test, automock)]
pub trait SyncStateTrait: Send + Sync {
    async fn get_last_synced_index(&self, relayer_id: &str) -> Result<Option<u64>, SyncStateError>;

    async fn get_ledger_context(&self, relayer_id: &str)
        -> Result<Option<Vec<u8>>, SyncStateError>;

    async fn set_last_synced_index(
        &self,
        relayer_id: &str,
        index: u64,
    ) -> Result<(), SyncStateError>;

    async fn set_ledger_context(
        &self,
        relayer_id: &str,
        context: Vec<u8>,
    ) -> Result<(), SyncStateError>;

    async fn set_sync_state(
        &self,
        relayer_id: &str,
        index: u64,
        context: Option<Vec<u8>>,
    ) -> Result<(), SyncStateError>;

    async fn update_if_greater(&self, relayer_id: &str, index: u64)
        -> Result<bool, SyncStateError>;

    async fn get_unshielded_balance(&self, relayer_id: &str) -> Result<u128, SyncStateError>;

    async fn set_unshielded_balance(
        &self,
        relayer_id: &str,
        balance: u128,
    ) -> Result<(), SyncStateError>;

    async fn reset(&self, relayer_id: &str) -> Result<(), SyncStateError>;

    async fn get_all(&self) -> Vec<(String, RelayerSyncState)>;
}

/// Process-wide shared store, set during app initialization.
/// Ensures the relayer path and transaction factory path use the same instance.
static SHARED_STORE: std::sync::OnceLock<Arc<RelayerStateRepositoryStorage>> =
    std::sync::OnceLock::new();

/// Set the shared store (called once from initialize_app_state).
pub fn set_shared_store(store: Arc<RelayerStateRepositoryStorage>) {
    let _ = SHARED_STORE.set(store);
}

/// Get the shared store. Falls back to a fresh in-memory store if not initialized.
pub fn get_shared_store() -> Arc<RelayerStateRepositoryStorage> {
    SHARED_STORE
        .get()
        .cloned()
        .unwrap_or_else(|| Arc::new(RelayerStateRepositoryStorage::new_in_memory()))
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
        connections: Arc<RedisConnections>,
        key_prefix: String,
    ) -> Result<Self, RepositoryError> {
        Ok(Self::Redis(RedisRelayerStateRepository::new(
            connections,
            key_prefix,
        )?))
    }
}

#[async_trait]
impl SyncStateTrait for RelayerStateRepositoryStorage {
    async fn get_last_synced_index(&self, relayer_id: &str) -> Result<Option<u64>, SyncStateError> {
        match self {
            Self::InMemory(repo) => repo.get_last_synced_index(relayer_id).await,
            Self::Redis(repo) => repo.get_last_synced_index(relayer_id).await,
        }
    }

    async fn get_ledger_context(
        &self,
        relayer_id: &str,
    ) -> Result<Option<Vec<u8>>, SyncStateError> {
        match self {
            Self::InMemory(repo) => repo.get_ledger_context(relayer_id).await,
            Self::Redis(repo) => repo.get_ledger_context(relayer_id).await,
        }
    }

    async fn set_last_synced_index(
        &self,
        relayer_id: &str,
        index: u64,
    ) -> Result<(), SyncStateError> {
        match self {
            Self::InMemory(repo) => repo.set_last_synced_index(relayer_id, index).await,
            Self::Redis(repo) => repo.set_last_synced_index(relayer_id, index).await,
        }
    }

    async fn set_ledger_context(
        &self,
        relayer_id: &str,
        context: Vec<u8>,
    ) -> Result<(), SyncStateError> {
        match self {
            Self::InMemory(repo) => repo.set_ledger_context(relayer_id, context).await,
            Self::Redis(repo) => repo.set_ledger_context(relayer_id, context).await,
        }
    }

    async fn set_sync_state(
        &self,
        relayer_id: &str,
        index: u64,
        context: Option<Vec<u8>>,
    ) -> Result<(), SyncStateError> {
        match self {
            Self::InMemory(repo) => repo.set_sync_state(relayer_id, index, context).await,
            Self::Redis(repo) => repo.set_sync_state(relayer_id, index, context).await,
        }
    }

    async fn update_if_greater(
        &self,
        relayer_id: &str,
        index: u64,
    ) -> Result<bool, SyncStateError> {
        match self {
            Self::InMemory(repo) => repo.update_if_greater(relayer_id, index).await,
            Self::Redis(repo) => repo.update_if_greater(relayer_id, index).await,
        }
    }

    async fn get_unshielded_balance(&self, relayer_id: &str) -> Result<u128, SyncStateError> {
        match self {
            Self::InMemory(repo) => repo.get_unshielded_balance(relayer_id).await,
            Self::Redis(repo) => repo.get_unshielded_balance(relayer_id).await,
        }
    }

    async fn set_unshielded_balance(
        &self,
        relayer_id: &str,
        balance: u128,
    ) -> Result<(), SyncStateError> {
        match self {
            Self::InMemory(repo) => repo.set_unshielded_balance(relayer_id, balance).await,
            Self::Redis(repo) => repo.set_unshielded_balance(relayer_id, balance).await,
        }
    }

    async fn reset(&self, relayer_id: &str) -> Result<(), SyncStateError> {
        match self {
            Self::InMemory(repo) => repo.reset(relayer_id).await,
            Self::Redis(repo) => repo.reset(relayer_id).await,
        }
    }

    async fn get_all(&self) -> Vec<(String, RelayerSyncState)> {
        match self {
            Self::InMemory(repo) => repo.get_all().await,
            Self::Redis(repo) => repo.get_all().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_storage_wrapper_basic_flow() {
        let repo = RelayerStateRepositoryStorage::new_in_memory();

        assert_eq!(repo.get_last_synced_index("relayer-1").await.unwrap(), None);

        repo.set_last_synced_index("relayer-1", 42).await.unwrap();
        assert_eq!(
            repo.get_last_synced_index("relayer-1").await.unwrap(),
            Some(42)
        );

        repo.reset("relayer-1").await.unwrap();
        assert_eq!(repo.get_last_synced_index("relayer-1").await.unwrap(), None);
    }
}
