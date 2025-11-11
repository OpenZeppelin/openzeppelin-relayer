//! Redis-backed implementation for relayer sync state management.
//!
//! This module provides persistent storage for sync state using Redis,
//! allowing the service to maintain state across restarts and scale horizontally.

use async_trait::async_trait;
use redis::aio::ConnectionManager;
use redis::AsyncCommands;
use std::sync::Arc;

use crate::models::RepositoryError;
use crate::repositories::redis_base::RedisRepository;

use super::{RelayerSyncState, SyncStateError, SyncStateTrait};

#[derive(Clone)]
pub struct RedisRelayerStateRepository {
    connection_manager: Arc<ConnectionManager>,
    key_prefix: String,
}

impl std::fmt::Debug for RedisRelayerStateRepository {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedisRelayerStateRepository")
            .field("key_prefix", &self.key_prefix)
            .finish()
    }
}

impl RedisRelayerStateRepository {
    pub fn new(
        connection_manager: Arc<ConnectionManager>,
        key_prefix: String,
    ) -> Result<Self, RepositoryError> {
        Ok(Self {
            connection_manager,
            key_prefix,
        })
    }

    fn get_key(&self, relayer_id: &str) -> String {
        format!("{}:relayer_state:{}", self.key_prefix, relayer_id)
    }

    async fn get_state(
        &self,
        relayer_id: &str,
    ) -> Result<Option<RelayerSyncState>, SyncStateError> {
        let key = self.get_key(relayer_id);
        let mut conn = self.connection_manager.as_ref().clone();

        let data: Option<String> = conn
            .get(&key)
            .await
            .map_err(|e| SyncStateError::SerializationError(format!("Redis get error: {e}")))?;

        match data {
            Some(json) => {
                let state = serde_json::from_str(&json).map_err(|e| {
                    SyncStateError::SerializationError(format!("Deserialization error: {e}"))
                })?;
                Ok(Some(state))
            }
            None => Ok(None),
        }
    }

    async fn set_state(
        &self,
        relayer_id: &str,
        state: &RelayerSyncState,
    ) -> Result<(), SyncStateError> {
        let key = self.get_key(relayer_id);
        let json = serde_json::to_string(state)
            .map_err(|e| SyncStateError::SerializationError(format!("Serialization error: {e}")))?;

        let mut conn = self.connection_manager.as_ref().clone();

        conn.set::<_, _, ()>(&key, json)
            .await
            .map_err(|e| SyncStateError::SerializationError(format!("Redis set error: {e}")))?;

        Ok(())
    }
}

impl RedisRepository for RedisRelayerStateRepository {}

#[async_trait]
impl SyncStateTrait for RedisRelayerStateRepository {
    async fn get_last_synced_index(&self, relayer_id: &str) -> Result<Option<u64>, SyncStateError> {
        Ok(self
            .get_state(relayer_id)
            .await?
            .map(|state| state.last_synced_index))
    }

    async fn get_ledger_context(
        &self,
        relayer_id: &str,
    ) -> Result<Option<Vec<u8>>, SyncStateError> {
        Ok(self
            .get_state(relayer_id)
            .await?
            .and_then(|state| state.ledger_context))
    }

    async fn set_last_synced_index(
        &self,
        relayer_id: &str,
        index: u64,
    ) -> Result<(), SyncStateError> {
        let mut state = self
            .get_state(relayer_id)
            .await?
            .unwrap_or(RelayerSyncState {
                last_synced_index: 0,
                ledger_context: None,
            });

        state.last_synced_index = index;
        self.set_state(relayer_id, &state).await
    }

    async fn set_ledger_context(
        &self,
        relayer_id: &str,
        context: Vec<u8>,
    ) -> Result<(), SyncStateError> {
        let mut state = self
            .get_state(relayer_id)
            .await?
            .unwrap_or(RelayerSyncState {
                last_synced_index: 0,
                ledger_context: None,
            });

        state.ledger_context = Some(context);
        self.set_state(relayer_id, &state).await
    }

    async fn set_sync_state(
        &self,
        relayer_id: &str,
        index: u64,
        context: Option<Vec<u8>>,
    ) -> Result<(), SyncStateError> {
        let state = RelayerSyncState {
            last_synced_index: index,
            ledger_context: context,
        };
        self.set_state(relayer_id, &state).await
    }

    async fn update_if_greater(
        &self,
        relayer_id: &str,
        index: u64,
    ) -> Result<bool, SyncStateError> {
        let current_state = self.get_state(relayer_id).await?;

        let should_update = current_state
            .as_ref()
            .map(|s| index > s.last_synced_index)
            .unwrap_or(true);

        if should_update {
            let state = RelayerSyncState {
                last_synced_index: index,
                ledger_context: current_state.and_then(|s| s.ledger_context),
            };
            self.set_state(relayer_id, &state).await?;
        }

        Ok(should_update)
    }

    async fn reset(&self, relayer_id: &str) -> Result<(), SyncStateError> {
        let key = self.get_key(relayer_id);
        let mut conn = self.connection_manager.as_ref().clone();

        conn.del::<_, ()>(&key)
            .await
            .map_err(|e| SyncStateError::SerializationError(format!("Redis del error: {e}")))?;

        Ok(())
    }

    async fn get_all(&self) -> Vec<(String, RelayerSyncState)> {
        let pattern = format!("{}:relayer_state:*", self.key_prefix);

        let mut conn = self.connection_manager.as_ref().clone();

        let keys: Vec<String> = match conn.keys(&pattern).await {
            Ok(k) => k,
            Err(_) => return vec![],
        };

        let mut results = Vec::new();
        let prefix_len = format!("{}:relayer_state:", self.key_prefix).len();

        for key in keys {
            if let Ok(Some(data)) = conn.get::<_, Option<String>>(&key).await {
                if let Ok(state) = serde_json::from_str::<RelayerSyncState>(&data) {
                    let relayer_id = key[prefix_len..].to_string();
                    results.push((relayer_id, state));
                }
            }
        }

        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use redis::aio::ConnectionManager;
    use std::sync::Arc;

    async fn create_test_repository() -> RedisRelayerStateRepository {
        let redis_url =
            std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
        let client = redis::Client::open(redis_url).expect("Failed to create Redis client");
        let connection_manager = ConnectionManager::new(client)
            .await
            .expect("Failed to create Redis connection manager");
        RedisRelayerStateRepository::new(Arc::new(connection_manager), "test".to_string())
            .expect("Failed to create Redis relayer state repository")
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_basic_operations() {
        let repo = create_test_repository().await;
        let relayer_id = "test_relayer_1";

        // Initially should be None
        assert_eq!(repo.get_last_synced_index(relayer_id).await.unwrap(), None);

        // Set a value
        repo.set_last_synced_index(relayer_id, 100).await.unwrap();
        assert_eq!(
            repo.get_last_synced_index(relayer_id).await.unwrap(),
            Some(100)
        );

        // Update to a higher value
        repo.set_last_synced_index(relayer_id, 200).await.unwrap();
        assert_eq!(
            repo.get_last_synced_index(relayer_id).await.unwrap(),
            Some(200)
        );

        // Reset
        repo.reset(relayer_id).await.unwrap();
        assert_eq!(repo.get_last_synced_index(relayer_id).await.unwrap(), None);
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_update_if_greater() {
        let repo = create_test_repository().await;
        let relayer_id = "test_relayer_2";

        // First update should succeed
        assert!(repo.update_if_greater(relayer_id, 100).await.unwrap());
        assert_eq!(
            repo.get_last_synced_index(relayer_id).await.unwrap(),
            Some(100)
        );

        // Update with lower value should not change
        assert!(!repo.update_if_greater(relayer_id, 50).await.unwrap());
        assert_eq!(
            repo.get_last_synced_index(relayer_id).await.unwrap(),
            Some(100)
        );

        // Update with higher value should succeed
        assert!(repo.update_if_greater(relayer_id, 150).await.unwrap());
        assert_eq!(
            repo.get_last_synced_index(relayer_id).await.unwrap(),
            Some(150)
        );
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_ledger_context() {
        let repo = create_test_repository().await;
        let relayer_id = "test_relayer_3";

        // Initially should be None
        assert_eq!(repo.get_ledger_context(relayer_id).await.unwrap(), None);

        // Set ledger context
        let context = vec![1, 2, 3, 4, 5];
        repo.set_ledger_context(relayer_id, context.clone())
            .await
            .unwrap();
        assert_eq!(
            repo.get_ledger_context(relayer_id).await.unwrap(),
            Some(context.clone())
        );

        // Set sync state with both index and context
        let new_context = vec![6, 7, 8, 9, 10];
        repo.set_sync_state(relayer_id, 100, Some(new_context.clone()))
            .await
            .unwrap();
        assert_eq!(
            repo.get_last_synced_index(relayer_id).await.unwrap(),
            Some(100)
        );
        assert_eq!(
            repo.get_ledger_context(relayer_id).await.unwrap(),
            Some(new_context)
        );
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_get_all() {
        let repo = create_test_repository().await;

        // Add some relayers
        repo.set_last_synced_index("relayer_1", 100).await.unwrap();
        repo.set_sync_state("relayer_2", 200, Some(vec![1, 2, 3]))
            .await
            .unwrap();

        let all = repo.get_all().await;
        assert!(all.len() >= 2);

        // Convert to HashMap for easier testing
        let all_map: std::collections::HashMap<_, _> = all.into_iter().collect();
        assert!(all_map.contains_key("relayer_1"));
        assert!(all_map.contains_key("relayer_2"));
    }
}
