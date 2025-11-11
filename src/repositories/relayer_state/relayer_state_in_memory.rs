//! In-memory implementation for relayer sync state management.
//!
//! This module provides a fast, thread-safe in-memory storage implementation
//! for tracking relayer sync states during development and testing.
//! This implementation uses a `DashMap` for concurrent access and modification.

use async_trait::async_trait;
use dashmap::DashMap;

use super::{RelayerSyncState, SyncStateError, SyncStateTrait};

#[derive(Debug, Default, Clone)]
pub struct InMemoryRelayerStateRepository {
    store: DashMap<String, RelayerSyncState>, // relayer_id -> sync state
}

impl InMemoryRelayerStateRepository {
    pub fn new() -> Self {
        Self {
            store: DashMap::new(),
        }
    }
}

#[async_trait]
impl SyncStateTrait for InMemoryRelayerStateRepository {
    async fn get_last_synced_index(&self, relayer_id: &str) -> Result<Option<u64>, SyncStateError> {
        Ok(self
            .store
            .get(relayer_id)
            .map(|state| state.last_synced_index))
    }

    async fn get_ledger_context(
        &self,
        relayer_id: &str,
    ) -> Result<Option<Vec<u8>>, SyncStateError> {
        Ok(self
            .store
            .get(relayer_id)
            .and_then(|state| state.ledger_context.clone()))
    }

    async fn set_last_synced_index(
        &self,
        relayer_id: &str,
        index: u64,
    ) -> Result<(), SyncStateError> {
        self.store
            .entry(relayer_id.to_string())
            .and_modify(|state| state.last_synced_index = index)
            .or_insert(RelayerSyncState {
                last_synced_index: index,
                ledger_context: None,
            });
        Ok(())
    }

    async fn set_ledger_context(
        &self,
        relayer_id: &str,
        context: Vec<u8>,
    ) -> Result<(), SyncStateError> {
        self.store
            .entry(relayer_id.to_string())
            .and_modify(|state| state.ledger_context = Some(context.clone()))
            .or_insert(RelayerSyncState {
                last_synced_index: 0,
                ledger_context: Some(context),
            });
        Ok(())
    }

    async fn set_sync_state(
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

    async fn update_if_greater(
        &self,
        relayer_id: &str,
        index: u64,
    ) -> Result<bool, SyncStateError> {
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

    async fn reset(&self, relayer_id: &str) -> Result<(), SyncStateError> {
        self.store.remove(relayer_id);
        Ok(())
    }

    async fn get_all(&self) -> Vec<(String, RelayerSyncState)> {
        self.store
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_sync_state_basic_operations() {
        let store = InMemoryRelayerStateRepository::new();
        let relayer_id = "relayer_1";

        // Initially should be None
        assert_eq!(store.get_last_synced_index(relayer_id).await.unwrap(), None);

        // Set a value
        store.set_last_synced_index(relayer_id, 100).await.unwrap();
        assert_eq!(
            store.get_last_synced_index(relayer_id).await.unwrap(),
            Some(100)
        );

        // Update to a higher value
        store.set_last_synced_index(relayer_id, 200).await.unwrap();
        assert_eq!(
            store.get_last_synced_index(relayer_id).await.unwrap(),
            Some(200)
        );

        // Reset
        store.reset(relayer_id).await.unwrap();
        assert_eq!(store.get_last_synced_index(relayer_id).await.unwrap(), None);
    }

    #[tokio::test]
    async fn test_update_if_greater() {
        let store = InMemoryRelayerStateRepository::new();
        let relayer_id = "relayer_1";

        // First update should succeed
        assert!(store.update_if_greater(relayer_id, 100).await.unwrap());
        assert_eq!(
            store.get_last_synced_index(relayer_id).await.unwrap(),
            Some(100)
        );

        // Update with lower value should not change
        assert!(!store.update_if_greater(relayer_id, 50).await.unwrap());
        assert_eq!(
            store.get_last_synced_index(relayer_id).await.unwrap(),
            Some(100)
        );

        // Update with equal value should not change
        assert!(!store.update_if_greater(relayer_id, 100).await.unwrap());
        assert_eq!(
            store.get_last_synced_index(relayer_id).await.unwrap(),
            Some(100)
        );

        // Update with higher value should succeed
        assert!(store.update_if_greater(relayer_id, 150).await.unwrap());
        assert_eq!(
            store.get_last_synced_index(relayer_id).await.unwrap(),
            Some(150)
        );
    }

    #[tokio::test]
    async fn test_multiple_relayers() {
        let store = InMemoryRelayerStateRepository::new();

        // Set different indices for different relayers
        store.set_last_synced_index("relayer_1", 100).await.unwrap();
        store.set_last_synced_index("relayer_2", 200).await.unwrap();
        store.set_last_synced_index("relayer_3", 300).await.unwrap();

        // Verify independent states
        assert_eq!(
            store.get_last_synced_index("relayer_1").await.unwrap(),
            Some(100)
        );
        assert_eq!(
            store.get_last_synced_index("relayer_2").await.unwrap(),
            Some(200)
        );
        assert_eq!(
            store.get_last_synced_index("relayer_3").await.unwrap(),
            Some(300)
        );

        // Update one relayer shouldn't affect others
        store.update_if_greater("relayer_1", 150).await.unwrap();
        assert_eq!(
            store.get_last_synced_index("relayer_1").await.unwrap(),
            Some(150)
        );
        assert_eq!(
            store.get_last_synced_index("relayer_2").await.unwrap(),
            Some(200)
        );
        assert_eq!(
            store.get_last_synced_index("relayer_3").await.unwrap(),
            Some(300)
        );
    }

    #[tokio::test]
    async fn test_ledger_context_operations() {
        let store = InMemoryRelayerStateRepository::new();
        let relayer_id = "relayer_1";

        // Initially should be None
        assert_eq!(store.get_ledger_context(relayer_id).await.unwrap(), None);

        // Set ledger context
        let context = vec![1, 2, 3, 4, 5];
        store
            .set_ledger_context(relayer_id, context.clone())
            .await
            .unwrap();
        assert_eq!(
            store.get_ledger_context(relayer_id).await.unwrap(),
            Some(context.clone())
        );

        // Set sync state with both index and context
        let new_context = vec![6, 7, 8, 9, 10];
        store
            .set_sync_state(relayer_id, 100, Some(new_context.clone()))
            .await
            .unwrap();
        assert_eq!(
            store.get_last_synced_index(relayer_id).await.unwrap(),
            Some(100)
        );
        assert_eq!(
            store.get_ledger_context(relayer_id).await.unwrap(),
            Some(new_context)
        );
    }

    #[tokio::test]
    async fn test_get_all() {
        let store = InMemoryRelayerStateRepository::new();

        // Initially empty
        assert_eq!(store.get_all().await.len(), 0);

        // Add some relayers
        store.set_last_synced_index("relayer_1", 100).await.unwrap();
        store
            .set_sync_state("relayer_2", 200, Some(vec![1, 2, 3]))
            .await
            .unwrap();

        let all = store.get_all().await;
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
        assert!(
            all_map
                .get("relayer_2")
                .and_then(|s| s.ledger_context.as_ref())
                .is_some()
        );
    }

    #[tokio::test]
    async fn test_concurrent_access() {
        use std::sync::Arc;
        use tokio::task;

        let store = Arc::new(InMemoryRelayerStateRepository::new());
        let mut handles = vec![];

        // Spawn multiple tasks updating different relayers
        for i in 0..10 {
            let store_clone = Arc::clone(&store);
            let handle = task::spawn(async move {
                let relayer_id = format!("relayer_{}", i);
                for j in 0..100 {
                    store_clone
                        .update_if_greater(&relayer_id, j)
                        .await
                        .expect("Failed to update");
                }
            });
            handles.push(handle);
        }

        // Wait for all tasks to complete
        for handle in handles {
            handle.await.unwrap();
        }

        // Verify all relayers have the expected final value
        for i in 0..10 {
            let relayer_id = format!("relayer_{}", i);
            assert_eq!(
                store.get_last_synced_index(&relayer_id).await.unwrap(),
                Some(99)
            );
        }
    }

    #[tokio::test]
    async fn test_set_sync_state_overwrite() {
        let store = InMemoryRelayerStateRepository::new();
        let relayer_id = "relayer_1";

        // Set initial state
        store
            .set_sync_state(relayer_id, 100, Some(vec![1, 2, 3]))
            .await
            .unwrap();
        assert_eq!(
            store.get_last_synced_index(relayer_id).await.unwrap(),
            Some(100)
        );
        assert_eq!(
            store.get_ledger_context(relayer_id).await.unwrap(),
            Some(vec![1, 2, 3])
        );

        // Overwrite with new state
        store
            .set_sync_state(relayer_id, 200, Some(vec![4, 5, 6]))
            .await
            .unwrap();
        assert_eq!(
            store.get_last_synced_index(relayer_id).await.unwrap(),
            Some(200)
        );
        assert_eq!(
            store.get_ledger_context(relayer_id).await.unwrap(),
            Some(vec![4, 5, 6])
        );

        // Overwrite with no context
        store.set_sync_state(relayer_id, 300, None).await.unwrap();
        assert_eq!(
            store.get_last_synced_index(relayer_id).await.unwrap(),
            Some(300)
        );
        assert_eq!(store.get_ledger_context(relayer_id).await.unwrap(), None);
    }

    #[tokio::test]
    async fn test_ledger_context_independence() {
        let store = InMemoryRelayerStateRepository::new();

        // Set ledger context without index
        store
            .set_ledger_context("relayer_1", vec![1, 2, 3])
            .await
            .unwrap();
        assert_eq!(
            store.get_last_synced_index("relayer_1").await.unwrap(),
            Some(0)
        );
        assert_eq!(
            store.get_ledger_context("relayer_1").await.unwrap(),
            Some(vec![1, 2, 3])
        );

        // Update index without affecting context
        store.set_last_synced_index("relayer_1", 100).await.unwrap();
        assert_eq!(
            store.get_last_synced_index("relayer_1").await.unwrap(),
            Some(100)
        );
        assert_eq!(
            store.get_ledger_context("relayer_1").await.unwrap(),
            Some(vec![1, 2, 3])
        );

        // Update context without affecting index
        store
            .set_ledger_context("relayer_1", vec![4, 5, 6])
            .await
            .unwrap();
        assert_eq!(
            store.get_last_synced_index("relayer_1").await.unwrap(),
            Some(100)
        );
        assert_eq!(
            store.get_ledger_context("relayer_1").await.unwrap(),
            Some(vec![4, 5, 6])
        );
    }

    #[tokio::test]
    async fn test_empty_ledger_context() {
        let store = InMemoryRelayerStateRepository::new();
        let relayer_id = "relayer_1";

        // Set empty context
        store.set_ledger_context(relayer_id, vec![]).await.unwrap();
        assert_eq!(
            store.get_ledger_context(relayer_id).await.unwrap(),
            Some(vec![])
        );

        // Empty context is different from None
        store.reset(relayer_id).await.unwrap();
        assert_eq!(store.get_ledger_context(relayer_id).await.unwrap(), None);
    }

    #[tokio::test]
    async fn test_large_ledger_context() {
        let store = InMemoryRelayerStateRepository::new();
        let relayer_id = "relayer_1";

        // Create a large context (simulating serialized ledger state)
        let large_context: Vec<u8> = (0..10000).map(|i| (i % 256) as u8).collect();

        store
            .set_ledger_context(relayer_id, large_context.clone())
            .await
            .unwrap();
        let retrieved = store.get_ledger_context(relayer_id).await.unwrap().unwrap();

        assert_eq!(retrieved.len(), large_context.len());
        assert_eq!(retrieved, large_context);
    }
}
