use async_trait::async_trait;
use redis::AsyncCommands;
use std::fmt;
use std::sync::Arc;

use crate::models::RepositoryError;
use crate::repositories::redis_base::RedisRepository;
use crate::utils::RedisConnections;

use super::{MidnightRelayerSyncState, RelayerSyncState, SyncStateError, SyncStateTrait};

const RELAYER_STATE_PREFIX: &str = "relayer_state_v2";
const MAX_CAS_RETRIES: usize = 8;

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

    async fn get_state_from_primary(
        &self,
        relayer_id: &str,
    ) -> Result<Option<RelayerSyncState>, SyncStateError> {
        let key = self.key(relayer_id);
        let mut conn = self
            .get_connection(self.connections.primary(), "get_state_from_primary")
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

    async fn compare_and_set_state(
        &self,
        relayer_id: &str,
        expected_version: Option<u64>,
        state: &RelayerSyncState,
    ) -> Result<bool, SyncStateError> {
        let key = self.key(relayer_id);
        let expected = expected_version
            .map(|version| version.to_string())
            .unwrap_or_default();
        let payload = serde_json::to_string(state)
            .map_err(|e| SyncStateError::SerializationError(e.to_string()))?;
        let mut conn = self
            .get_connection(self.connections.primary(), "compare_and_set_state")
            .await
            .map_err(|e| SyncStateError::SerializationError(e.to_string()))?;

        let script = redis::Script::new(
            r#"
            local current = redis.call('GET', KEYS[1])
            local expected = ARGV[1]
            if current == false then
                if expected ~= '' then
                    return 0
                end
            else
                if expected == '' then
                    return 0
                end
                local decoded = cjson.decode(current)
                local version = decoded['version'] or 0
                if tostring(version) ~= expected then
                    return 0
                end
            end
            redis.call('SET', KEYS[1], ARGV[2])
            return 1
            "#,
        );

        let updated: i32 = script
            .key(key)
            .arg(expected)
            .arg(payload)
            .invoke_async(&mut conn)
            .await
            .map_err(|e| SyncStateError::SerializationError(format!("Redis CAS error: {e}")))?;

        Ok(updated == 1)
    }
}

#[async_trait]
impl SyncStateTrait for RedisRelayerStateRepository {
    async fn get_midnight_state(
        &self,
        relayer_id: &str,
    ) -> Result<MidnightRelayerSyncState, SyncStateError> {
        Ok(self
            .get_state(relayer_id)
            .await?
            .and_then(|state| state.midnight)
            .unwrap_or_default())
    }

    async fn update_midnight_state<F, R>(
        &self,
        relayer_id: &str,
        mut update: F,
    ) -> Result<R, SyncStateError>
    where
        F: FnMut(&mut MidnightRelayerSyncState) -> Result<R, SyncStateError> + Send,
        R: Send,
    {
        for _ in 0..MAX_CAS_RETRIES {
            let current = self.get_state_from_primary(relayer_id).await?;
            let expected_version = current.as_ref().map(|state| state.version);
            let mut state = current.unwrap_or_default();
            let result = update(state.midnight_or_default_mut())?;
            state.bump_version();

            if self
                .compare_and_set_state(relayer_id, expected_version, &state)
                .await?
            {
                return Ok(result);
            }
        }

        Err(SyncStateError::SerializationError(format!(
            "Redis CAS update failed after {MAX_CAS_RETRIES} retries"
        )))
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
