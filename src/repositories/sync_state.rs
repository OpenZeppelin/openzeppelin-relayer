//! This module provides in-memory implementation for sync state management.
//!
//! The `InMemorySyncState` struct is used to track the last synced blockchain index
//! and the serialized ledger context for different relayers, enabling the sync service
//! to resume from the correct position with the proper wallet state.
//! This implementation uses a `DashMap` for concurrent access and modification of the sync state.

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[cfg(test)]
use mockall::automock;

/// Represents the sync state for a relayer, including blockchain index and ledger context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayerSyncState {
    /// The last synced blockchain index
    pub last_synced_index: u64,
    /// The serialized ledger context (optional, as it may not be available initially)
    pub ledger_context: Option<Vec<u8>>,
}

#[derive(Debug, Default, Clone)]
pub struct InMemorySyncState {
    store: DashMap<String, RelayerSyncState>, // relayer_id -> sync state
}

impl InMemorySyncState {
    pub fn new() -> Self {
        Self {
            store: DashMap::new(),
        }
    }
}

#[derive(Error, Debug, Serialize)]
pub enum SyncStateError {
    #[error("Sync state not found for relayer {relayer_id}")]
    NotFound { relayer_id: String },
    #[error("Invalid blockchain index {index} for relayer {relayer_id}")]
    InvalidIndex { relayer_id: String, index: u64 },
    #[error("Failed to serialize/deserialize ledger context: {0}")]
    SerializationError(String),
}

#[allow(dead_code)]
#[cfg_attr(test, automock)]
pub trait SyncStateTrait {
    /// Get the last synced blockchain index for a relayer
    fn get_last_synced_index(&self, relayer_id: &str) -> Result<Option<u64>, SyncStateError>;

    /// Get the serialized ledger context for a relayer
    fn get_ledger_context(&self, relayer_id: &str) -> Result<Option<Vec<u8>>, SyncStateError>;

    /// Set the last synced blockchain index for a relayer
    fn set_last_synced_index(&self, relayer_id: &str, index: u64) -> Result<(), SyncStateError>;

    /// Set the ledger context for a relayer
    fn set_ledger_context(&self, relayer_id: &str, context: Vec<u8>) -> Result<(), SyncStateError>;

    /// Set both the last synced index and ledger context for a relayer
    fn set_sync_state(
        &self,
        relayer_id: &str,
        index: u64,
        context: Option<Vec<u8>>,
    ) -> Result<(), SyncStateError>;

    /// Update the last synced blockchain index only if the new index is greater
    fn update_if_greater(&self, relayer_id: &str, index: u64) -> Result<bool, SyncStateError>;

    /// Reset the sync state for a relayer
    fn reset(&self, relayer_id: &str) -> Result<(), SyncStateError>;

    /// Get all sync states
    fn get_all(&self) -> Vec<(String, RelayerSyncState)>;
}

impl SyncStateTrait for InMemorySyncState {
    fn get_last_synced_index(&self, relayer_id: &str) -> Result<Option<u64>, SyncStateError> {
        Ok(self
            .store
            .get(relayer_id)
            .map(|state| state.last_synced_index))
    }

    fn get_ledger_context(&self, relayer_id: &str) -> Result<Option<Vec<u8>>, SyncStateError> {
        Ok(self
            .store
            .get(relayer_id)
            .and_then(|state| state.ledger_context.clone()))
    }

    fn set_last_synced_index(&self, relayer_id: &str, index: u64) -> Result<(), SyncStateError> {
        self.store
            .entry(relayer_id.to_string())
            .and_modify(|state| state.last_synced_index = index)
            .or_insert(RelayerSyncState {
                last_synced_index: index,
                ledger_context: None,
            });
        Ok(())
    }

    fn set_ledger_context(&self, relayer_id: &str, context: Vec<u8>) -> Result<(), SyncStateError> {
        self.store
            .entry(relayer_id.to_string())
            .and_modify(|state| state.ledger_context = Some(context.clone()))
            .or_insert(RelayerSyncState {
                last_synced_index: 0,
                ledger_context: Some(context),
            });
        Ok(())
    }

    fn set_sync_state(
        &self,
        relayer_id: &str,
        index: u64,
        context: Option<Vec<u8>>,
    ) -> Result<(), SyncStateError> {
        self.store.insert(
            relayer_id.to_string(),
            RelayerSyncState {
                last_synced_index: index,
                ledger_context: context,
            },
        );
        Ok(())
    }

    fn update_if_greater(&self, relayer_id: &str, index: u64) -> Result<bool, SyncStateError> {
        let mut updated = false;
        self.store
            .entry(relayer_id.to_string())
            .and_modify(|state| {
                if index > state.last_synced_index {
                    state.last_synced_index = index;
                    updated = true;
                }
            })
            .or_insert_with(|| {
                updated = true;
                RelayerSyncState {
                    last_synced_index: index,
                    ledger_context: None,
                }
            });
        Ok(updated)
    }

    fn reset(&self, relayer_id: &str) -> Result<(), SyncStateError> {
        self.store.remove(relayer_id);
        Ok(())
    }

    fn get_all(&self) -> Vec<(String, RelayerSyncState)> {
        self.store
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sync_state_basic_operations() {
        let store = InMemorySyncState::new();
        let relayer_id = "relayer_1";

        // Initially should be None
        assert_eq!(store.get_last_synced_index(relayer_id).unwrap(), None);

        // Set a value
        store.set_last_synced_index(relayer_id, 100).unwrap();
        assert_eq!(store.get_last_synced_index(relayer_id).unwrap(), Some(100));

        // Update to a higher value
        store.set_last_synced_index(relayer_id, 200).unwrap();
        assert_eq!(store.get_last_synced_index(relayer_id).unwrap(), Some(200));

        // Reset
        store.reset(relayer_id).unwrap();
        assert_eq!(store.get_last_synced_index(relayer_id).unwrap(), None);
    }

    #[test]
    fn test_update_if_greater() {
        let store = InMemorySyncState::new();
        let relayer_id = "relayer_1";

        // First update should succeed
        assert!(store.update_if_greater(relayer_id, 100).unwrap());
        assert_eq!(store.get_last_synced_index(relayer_id).unwrap(), Some(100));

        // Update with lower value should not change
        assert!(!store.update_if_greater(relayer_id, 50).unwrap());
        assert_eq!(store.get_last_synced_index(relayer_id).unwrap(), Some(100));

        // Update with equal value should not change
        assert!(!store.update_if_greater(relayer_id, 100).unwrap());
        assert_eq!(store.get_last_synced_index(relayer_id).unwrap(), Some(100));

        // Update with higher value should succeed
        assert!(store.update_if_greater(relayer_id, 150).unwrap());
        assert_eq!(store.get_last_synced_index(relayer_id).unwrap(), Some(150));
    }

    #[test]
    fn test_multiple_relayers() {
        let store = InMemorySyncState::new();

        // Set different indices for different relayers
        store.set_last_synced_index("relayer_1", 100).unwrap();
        store.set_last_synced_index("relayer_2", 200).unwrap();
        store.set_last_synced_index("relayer_3", 300).unwrap();

        // Verify independent states
        assert_eq!(store.get_last_synced_index("relayer_1").unwrap(), Some(100));
        assert_eq!(store.get_last_synced_index("relayer_2").unwrap(), Some(200));
        assert_eq!(store.get_last_synced_index("relayer_3").unwrap(), Some(300));

        // Update one relayer shouldn't affect others
        store.update_if_greater("relayer_1", 150).unwrap();
        assert_eq!(store.get_last_synced_index("relayer_1").unwrap(), Some(150));
        assert_eq!(store.get_last_synced_index("relayer_2").unwrap(), Some(200));
        assert_eq!(store.get_last_synced_index("relayer_3").unwrap(), Some(300));
    }

    #[test]
    fn test_ledger_context_operations() {
        let store = InMemorySyncState::new();
        let relayer_id = "relayer_1";

        // Initially should be None
        assert_eq!(store.get_ledger_context(relayer_id).unwrap(), None);

        // Set ledger context
        let context = vec![1, 2, 3, 4, 5];
        store
            .set_ledger_context(relayer_id, context.clone())
            .unwrap();
        assert_eq!(
            store.get_ledger_context(relayer_id).unwrap(),
            Some(context.clone())
        );

        // Set sync state with both index and context
        let new_context = vec![6, 7, 8, 9, 10];
        store
            .set_sync_state(relayer_id, 100, Some(new_context.clone()))
            .unwrap();
        assert_eq!(store.get_last_synced_index(relayer_id).unwrap(), Some(100));
        assert_eq!(
            store.get_ledger_context(relayer_id).unwrap(),
            Some(new_context)
        );
    }

    #[test]
    fn test_get_all() {
        let store = InMemorySyncState::new();

        // Initially empty
        assert_eq!(store.get_all().len(), 0);

        // Add some relayers
        store.set_last_synced_index("relayer_1", 100).unwrap();
        store
            .set_sync_state("relayer_2", 200, Some(vec![1, 2, 3]))
            .unwrap();

        let all = store.get_all();
        assert_eq!(all.len(), 2);

        // Convert to HashMap for easier testing
        let all_map: std::collections::HashMap<_, _> = all.into_iter().collect();
        assert_eq!(
            all_map.get("relayer_1").map(|s| s.last_synced_index),
            Some(100)
        );
        assert_eq!(
            all_map.get("relayer_2").map(|s| s.last_synced_index),
            Some(200)
        );
        assert!(all_map
            .get("relayer_2")
            .and_then(|s| s.ledger_context.as_ref())
            .is_some());
    }

    #[test]
    fn test_concurrent_access() {
        use std::sync::Arc;
        use std::thread;

        let store = Arc::new(InMemorySyncState::new());
        let mut handles = vec![];

        // Spawn multiple threads updating different relayers
        for i in 0..10 {
            let store_clone = Arc::clone(&store);
            let handle = thread::spawn(move || {
                let relayer_id = format!("relayer_{}", i);
                for j in 0..100 {
                    store_clone
                        .update_if_greater(&relayer_id, j)
                        .expect("Failed to update");
                }
            });
            handles.push(handle);
        }

        // Wait for all threads to complete
        for handle in handles {
            handle.join().unwrap();
        }

        // Verify all relayers have the expected final value
        for i in 0..10 {
            let relayer_id = format!("relayer_{}", i);
            assert_eq!(store.get_last_synced_index(&relayer_id).unwrap(), Some(99));
        }
    }

    #[test]
    fn test_set_sync_state_overwrite() {
        let store = InMemorySyncState::new();
        let relayer_id = "relayer_1";

        // Set initial state
        store
            .set_sync_state(relayer_id, 100, Some(vec![1, 2, 3]))
            .unwrap();
        assert_eq!(store.get_last_synced_index(relayer_id).unwrap(), Some(100));
        assert_eq!(
            store.get_ledger_context(relayer_id).unwrap(),
            Some(vec![1, 2, 3])
        );

        // Overwrite with new state
        store
            .set_sync_state(relayer_id, 200, Some(vec![4, 5, 6]))
            .unwrap();
        assert_eq!(store.get_last_synced_index(relayer_id).unwrap(), Some(200));
        assert_eq!(
            store.get_ledger_context(relayer_id).unwrap(),
            Some(vec![4, 5, 6])
        );

        // Overwrite with no context
        store.set_sync_state(relayer_id, 300, None).unwrap();
        assert_eq!(store.get_last_synced_index(relayer_id).unwrap(), Some(300));
        assert_eq!(store.get_ledger_context(relayer_id).unwrap(), None);
    }

    #[test]
    fn test_ledger_context_independence() {
        let store = InMemorySyncState::new();

        // Set ledger context without index
        store
            .set_ledger_context("relayer_1", vec![1, 2, 3])
            .unwrap();
        assert_eq!(store.get_last_synced_index("relayer_1").unwrap(), Some(0));
        assert_eq!(
            store.get_ledger_context("relayer_1").unwrap(),
            Some(vec![1, 2, 3])
        );

        // Update index without affecting context
        store.set_last_synced_index("relayer_1", 100).unwrap();
        assert_eq!(store.get_last_synced_index("relayer_1").unwrap(), Some(100));
        assert_eq!(
            store.get_ledger_context("relayer_1").unwrap(),
            Some(vec![1, 2, 3])
        );

        // Update context without affecting index
        store
            .set_ledger_context("relayer_1", vec![4, 5, 6])
            .unwrap();
        assert_eq!(store.get_last_synced_index("relayer_1").unwrap(), Some(100));
        assert_eq!(
            store.get_ledger_context("relayer_1").unwrap(),
            Some(vec![4, 5, 6])
        );
    }

    #[test]
    fn test_empty_ledger_context() {
        let store = InMemorySyncState::new();
        let relayer_id = "relayer_1";

        // Set empty context
        store.set_ledger_context(relayer_id, vec![]).unwrap();
        assert_eq!(store.get_ledger_context(relayer_id).unwrap(), Some(vec![]));

        // Empty context is different from None
        store.reset(relayer_id).unwrap();
        assert_eq!(store.get_ledger_context(relayer_id).unwrap(), None);
    }

    #[test]
    fn test_large_ledger_context() {
        let store = InMemorySyncState::new();
        let relayer_id = "relayer_1";

        // Create a large context (simulating serialized ledger state)
        let large_context: Vec<u8> = (0..10000).map(|i| (i % 256) as u8).collect();

        store
            .set_ledger_context(relayer_id, large_context.clone())
            .unwrap();
        let retrieved = store.get_ledger_context(relayer_id).unwrap().unwrap();

        assert_eq!(retrieved.len(), large_context.len());
        assert_eq!(retrieved, large_context);
    }
}
