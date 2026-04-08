use async_trait::async_trait;
use dashmap::DashMap;

use super::{RelayerSyncState, SyncStateError, SyncStateTrait};

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
                unshielded_balance: 0,
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
                unshielded_balance: 0,
            });
        Ok(())
    }

    async fn set_sync_state(
        &self,
        relayer_id: &str,
        index: u64,
        context: Option<Vec<u8>>,
    ) -> Result<(), SyncStateError> {
        self.store
            .entry(relayer_id.to_string())
            .and_modify(|state| {
                state.last_synced_index = index;
                state.ledger_context = context.clone();
            })
            .or_insert(RelayerSyncState {
                last_synced_index: index,
                ledger_context: context,
                unshielded_balance: 0,
            });
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
                    unshielded_balance: 0,
                }
            });
        Ok(updated)
    }

    async fn get_unshielded_balance(&self, relayer_id: &str) -> Result<u128, SyncStateError> {
        Ok(self
            .store
            .get(relayer_id)
            .map(|state| state.unshielded_balance)
            .unwrap_or(0))
    }

    async fn set_unshielded_balance(
        &self,
        relayer_id: &str,
        balance: u128,
    ) -> Result<(), SyncStateError> {
        self.store
            .entry(relayer_id.to_string())
            .and_modify(|state| state.unshielded_balance = balance)
            .or_insert(RelayerSyncState {
                last_synced_index: 0,
                ledger_context: None,
                unshielded_balance: balance,
            });
        Ok(())
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
