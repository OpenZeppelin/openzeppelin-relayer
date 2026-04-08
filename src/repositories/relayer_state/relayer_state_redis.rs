use async_trait::async_trait;
use redis::AsyncCommands;
use std::fmt;
use std::sync::Arc;

use crate::models::RepositoryError;
use crate::repositories::redis_base::RedisRepository;
use crate::utils::RedisConnections;

use super::{RelayerSyncState, SyncStateError, SyncStateTrait};

const RELAYER_STATE_PREFIX: &str = "relayer_state";

#[derive(Clone)]
pub struct RedisRelayerStateRepository {
    connections: Arc<RedisConnections>,
    key_prefix: String,
}

impl RedisRepository for RedisRelayerStateRepository {}

impl fmt::Debug for RedisRelayerStateRepository {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RedisRelayerStateRepository")
            .field("key_prefix", &self.key_prefix)
            .finish()
    }
}

impl RedisRelayerStateRepository {
    pub fn new(
        connections: Arc<RedisConnections>,
        key_prefix: String,
    ) -> Result<Self, RepositoryError> {
        if key_prefix.is_empty() {
            return Err(RepositoryError::InvalidData(
                "Redis key prefix cannot be empty".to_string(),
            ));
        }

        Ok(Self {
            connections,
            key_prefix,
        })
    }

    fn key(&self, relayer_id: &str) -> String {
        format!(
            "{}:{}:{}",
            self.key_prefix, RELAYER_STATE_PREFIX, relayer_id
        )
    }

    async fn get_state(
        &self,
        relayer_id: &str,
    ) -> Result<Option<RelayerSyncState>, SyncStateError> {
        let key = self.key(relayer_id);
        let mut conn = self
            .get_connection(self.connections.reader(), "get_state")
            .await
            .map_err(|e| SyncStateError::SerializationError(e.to_string()))?;

        let value: Option<String> = conn
            .get(&key)
            .await
            .map_err(|e| SyncStateError::SerializationError(format!("Redis get error: {e}")))?;

        match value {
            Some(json) => serde_json::from_str(&json)
                .map(Some)
                .map_err(|e| SyncStateError::SerializationError(e.to_string())),
            None => Ok(None),
        }
    }

    async fn set_state(
        &self,
        relayer_id: &str,
        state: &RelayerSyncState,
    ) -> Result<(), SyncStateError> {
        let key = self.key(relayer_id);
        let payload = serde_json::to_string(state)
            .map_err(|e| SyncStateError::SerializationError(e.to_string()))?;

        let mut conn = self
            .get_connection(self.connections.primary(), "set_state")
            .await
            .map_err(|e| SyncStateError::SerializationError(e.to_string()))?;

        conn.set::<_, _, ()>(&key, payload)
            .await
            .map_err(|e| SyncStateError::SerializationError(format!("Redis set error: {e}")))?;

        Ok(())
    }
}

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
                unshielded_balance: 0,
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
                unshielded_balance: 0,
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
        // Preserve existing balance when updating sync cursor/context
        let existing_balance = self
            .get_state(relayer_id)
            .await?
            .map(|s| s.unshielded_balance)
            .unwrap_or(0);

        self.set_state(
            relayer_id,
            &RelayerSyncState {
                last_synced_index: index,
                ledger_context: context,
                unshielded_balance: existing_balance,
            },
        )
        .await
    }

    async fn update_if_greater(
        &self,
        relayer_id: &str,
        index: u64,
    ) -> Result<bool, SyncStateError> {
        let current_state = self.get_state(relayer_id).await?;
        let should_update = current_state
            .as_ref()
            .map(|state| index > state.last_synced_index)
            .unwrap_or(true);

        if should_update {
            self.set_state(
                relayer_id,
                &RelayerSyncState {
                    last_synced_index: index,
                    ledger_context: current_state
                        .as_ref()
                        .and_then(|state| state.ledger_context.clone()),
                    unshielded_balance: current_state.map(|s| s.unshielded_balance).unwrap_or(0),
                },
            )
            .await?;
        }

        Ok(should_update)
    }

    async fn get_unshielded_balance(&self, relayer_id: &str) -> Result<u128, SyncStateError> {
        Ok(self
            .get_state(relayer_id)
            .await?
            .map(|state| state.unshielded_balance)
            .unwrap_or(0))
    }

    async fn set_unshielded_balance(
        &self,
        relayer_id: &str,
        balance: u128,
    ) -> Result<(), SyncStateError> {
        let mut state = self
            .get_state(relayer_id)
            .await?
            .unwrap_or(RelayerSyncState {
                last_synced_index: 0,
                ledger_context: None,
                unshielded_balance: 0,
            });
        state.unshielded_balance = balance;
        self.set_state(relayer_id, &state).await
    }

    async fn reset(&self, relayer_id: &str) -> Result<(), SyncStateError> {
        let mut conn = self
            .get_connection(self.connections.primary(), "reset")
            .await
            .map_err(|e| SyncStateError::SerializationError(e.to_string()))?;

        conn.del::<_, ()>(self.key(relayer_id))
            .await
            .map_err(|e| SyncStateError::SerializationError(format!("Redis del error: {e}")))?;
        Ok(())
    }

    async fn get_all(&self) -> Vec<(String, RelayerSyncState)> {
        let pattern = format!("{}:{}:*", self.key_prefix, RELAYER_STATE_PREFIX);
        let mut conn = match self
            .get_connection(self.connections.reader(), "get_all")
            .await
        {
            Ok(conn) => conn,
            Err(_) => return vec![],
        };

        let keys: Vec<String> = match conn.keys(&pattern).await {
            Ok(keys) => keys,
            Err(_) => return vec![],
        };

        let prefix_len = format!("{}:{}:", self.key_prefix, RELAYER_STATE_PREFIX).len();
        let mut results = Vec::new();

        for key in keys {
            if let Ok(Some(payload)) = conn.get::<_, Option<String>>(&key).await {
                if let Ok(state) = serde_json::from_str::<RelayerSyncState>(&payload) {
                    results.push((key[prefix_len..].to_string(), state));
                }
            }
        }

        results
    }
}
