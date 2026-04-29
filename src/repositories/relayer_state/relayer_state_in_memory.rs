use async_trait::async_trait;
use dashmap::DashMap;

use super::{MidnightRelayerSyncState, RelayerSyncState, SyncStateError, SyncStateTrait};

#[derive(Debug, Default, Clone)]
pub struct InMemoryRelayerStateRepository {
    store: DashMap<String, RelayerSyncState>,
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
    async fn get_midnight_state(
        &self,
        relayer_id: &str,
    ) -> Result<MidnightRelayerSyncState, SyncStateError> {
        Ok(self
            .store
            .get(relayer_id)
            .and_then(|state| state.midnight.clone())
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
        let mut entry = self
            .store
            .entry(relayer_id.to_string())
            .or_insert_with(RelayerSyncState::default);
        let mut next_state = entry.clone();
        let result = update(next_state.midnight_or_default_mut())?;
        next_state.bump_version();
        *entry = next_state;

        Ok(result)
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
