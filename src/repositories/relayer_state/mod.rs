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
pub struct UnshieldedUtxo {
    pub owner: String,
    pub value: u128,
    pub token_type: String,
    pub intent_hash: String,
    pub output_index: u32,
    #[serde(default)]
    pub ctime: Option<u64>,
    #[serde(default)]
    pub registered_for_dust_generation: bool,
}

impl UnshieldedUtxo {
    pub fn key(&self) -> String {
        format!("{}#{}", self.intent_hash, self.output_index)
    }

    pub fn from_json(val: &serde_json::Value) -> Option<Self> {
        Some(Self {
            owner: val.get("owner")?.as_str()?.to_string(),
            value: val.get("value")?.as_str()?.parse().ok()?,
            token_type: val.get("tokenType")?.as_str().unwrap_or("").to_string(),
            intent_hash: val.get("intentHash")?.as_str().unwrap_or("").to_string(),
            output_index: val.get("outputIndex")?.as_u64().unwrap_or(0) as u32,
            ctime: val.get("ctime").and_then(parse_optional_u64),
            registered_for_dust_generation: val
                .get("registeredForDustGeneration")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        })
    }
}

fn parse_optional_u64(value: &serde_json::Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct UnshieldedWalletState {
    #[serde(default)]
    pub applied_transaction_id: u64,
    #[serde(default)]
    pub highest_transaction_id: u64,
    #[serde(default)]
    pub available_utxos: Vec<UnshieldedUtxo>,
    #[serde(default)]
    pub pending_utxos: Vec<UnshieldedUtxo>,
}

impl UnshieldedWalletState {
    pub fn available_balance(&self) -> u128 {
        self.available_utxos
            .iter()
            .fold(0u128, |acc, utxo| acc.saturating_add(utxo.value))
    }

    pub fn apply_success(
        &mut self,
        transaction_id: u64,
        created_utxos: Vec<UnshieldedUtxo>,
        spent_utxos: Vec<UnshieldedUtxo>,
    ) {
        if transaction_id <= self.applied_transaction_id {
            return;
        }

        let spent_keys: std::collections::HashSet<String> =
            spent_utxos.iter().map(UnshieldedUtxo::key).collect();
        self.available_utxos
            .retain(|utxo| !spent_keys.contains(&utxo.key()));
        self.pending_utxos
            .retain(|utxo| !spent_keys.contains(&utxo.key()));

        for utxo in created_utxos {
            self.upsert_available(utxo);
        }

        self.applied_transaction_id = transaction_id;
    }

    pub fn apply_failed(&mut self, transaction_id: u64, spent_utxos: Vec<UnshieldedUtxo>) {
        if transaction_id <= self.applied_transaction_id {
            return;
        }

        let spent_keys: std::collections::HashSet<String> =
            spent_utxos.iter().map(UnshieldedUtxo::key).collect();
        self.pending_utxos
            .retain(|utxo| !spent_keys.contains(&utxo.key()));

        for utxo in spent_utxos {
            self.upsert_available(utxo);
        }

        self.applied_transaction_id = transaction_id;
    }

    pub fn apply_progress(&mut self, highest_transaction_id: u64) {
        self.highest_transaction_id = self.highest_transaction_id.max(highest_transaction_id);
    }

    pub fn mark_pending_spent(&mut self, spent_utxos: Vec<UnshieldedUtxo>) {
        let spent_keys: std::collections::HashSet<String> =
            spent_utxos.iter().map(UnshieldedUtxo::key).collect();
        self.available_utxos
            .retain(|utxo| !spent_keys.contains(&utxo.key()));
        for utxo in spent_utxos {
            self.upsert_pending(utxo);
        }
    }

    pub fn mark_pending_by_keys(&mut self, spent_keys: &[String]) {
        let spent_keys: std::collections::HashSet<&str> =
            spent_keys.iter().map(String::as_str).collect();
        let mut moved = Vec::new();
        self.available_utxos.retain(|utxo| {
            if spent_keys.contains(utxo.key().as_str()) {
                moved.push(utxo.clone());
                false
            } else {
                true
            }
        });
        for utxo in moved {
            self.upsert_pending(utxo);
        }
    }

    pub fn release_pending_by_keys(&mut self, spent_keys: &[String]) {
        let spent_keys: std::collections::HashSet<&str> =
            spent_keys.iter().map(String::as_str).collect();
        let mut released = Vec::new();
        self.pending_utxos.retain(|utxo| {
            if spent_keys.contains(utxo.key().as_str()) {
                released.push(utxo.clone());
                false
            } else {
                true
            }
        });
        for utxo in released {
            self.upsert_available(utxo);
        }
    }

    fn upsert_available(&mut self, utxo: UnshieldedUtxo) {
        let key = utxo.key();
        self.available_utxos
            .retain(|existing| existing.key() != key);
        self.pending_utxos.retain(|existing| existing.key() != key);
        self.available_utxos.push(utxo);
    }

    fn upsert_pending(&mut self, utxo: UnshieldedUtxo) {
        let key = utxo.key();
        self.pending_utxos.retain(|existing| existing.key() != key);
        self.pending_utxos.push(utxo);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShieldedSpendReservation {
    pub coin_nonce: String,
    #[serde(default)]
    pub nullifier: Option<String>,
    #[serde(default)]
    pub commitment: Option<String>,
    pub transaction_id: String,
    pub token_type: String,
    pub value: u128,
    pub created_at: String,
    #[serde(default)]
    pub segment_id: Option<u16>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShieldedWalletState {
    #[serde(default)]
    pub pending_spends: Vec<ShieldedSpendReservation>,
}

impl ShieldedWalletState {
    pub fn mark_pending(&mut self, reservations: Vec<ShieldedSpendReservation>) {
        for reservation in reservations {
            self.upsert_pending(reservation);
        }
    }

    pub fn release_by_transaction(&mut self, transaction_id: &str) {
        self.pending_spends
            .retain(|reservation| reservation.transaction_id != transaction_id);
    }

    pub fn retain_transactions(&mut self, active_transaction_ids: &[String]) {
        let active: std::collections::HashSet<&str> =
            active_transaction_ids.iter().map(String::as_str).collect();
        self.pending_spends
            .retain(|reservation| active.contains(reservation.transaction_id.as_str()));
    }

    pub fn reserved_nonces(&self) -> std::collections::HashSet<String> {
        self.pending_spends
            .iter()
            .map(|reservation| reservation.coin_nonce.clone())
            .collect()
    }

    fn upsert_pending(&mut self, reservation: ShieldedSpendReservation) {
        let nonce = reservation.coin_nonce.clone();
        self.pending_spends
            .retain(|existing| existing.coin_nonce != nonce);
        self.pending_spends.push(reservation);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RelayerSyncState {
    pub last_synced_index: u64,
    pub ledger_context: Option<Vec<u8>>,
    #[serde(default)]
    pub unshielded_balance: u128,
    #[serde(default)]
    pub unshielded_wallet: UnshieldedWalletState,
    #[serde(default)]
    pub shielded_wallet: ShieldedWalletState,
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

    async fn get_unshielded_wallet_state(
        &self,
        relayer_id: &str,
    ) -> Result<UnshieldedWalletState, SyncStateError>;

    async fn set_unshielded_wallet_state(
        &self,
        relayer_id: &str,
        state: UnshieldedWalletState,
    ) -> Result<(), SyncStateError>;

    async fn get_shielded_wallet_state(
        &self,
        relayer_id: &str,
    ) -> Result<ShieldedWalletState, SyncStateError>;

    async fn set_shielded_wallet_state(
        &self,
        relayer_id: &str,
        state: ShieldedWalletState,
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

    async fn get_unshielded_wallet_state(
        &self,
        relayer_id: &str,
    ) -> Result<UnshieldedWalletState, SyncStateError> {
        match self {
            Self::InMemory(repo) => repo.get_unshielded_wallet_state(relayer_id).await,
            Self::Redis(repo) => repo.get_unshielded_wallet_state(relayer_id).await,
        }
    }

    async fn set_unshielded_wallet_state(
        &self,
        relayer_id: &str,
        state: UnshieldedWalletState,
    ) -> Result<(), SyncStateError> {
        match self {
            Self::InMemory(repo) => repo.set_unshielded_wallet_state(relayer_id, state).await,
            Self::Redis(repo) => repo.set_unshielded_wallet_state(relayer_id, state).await,
        }
    }

    async fn get_shielded_wallet_state(
        &self,
        relayer_id: &str,
    ) -> Result<ShieldedWalletState, SyncStateError> {
        match self {
            Self::InMemory(repo) => repo.get_shielded_wallet_state(relayer_id).await,
            Self::Redis(repo) => repo.get_shielded_wallet_state(relayer_id).await,
        }
    }

    async fn set_shielded_wallet_state(
        &self,
        relayer_id: &str,
        state: ShieldedWalletState,
    ) -> Result<(), SyncStateError> {
        match self {
            Self::InMemory(repo) => repo.set_shielded_wallet_state(relayer_id, state).await,
            Self::Redis(repo) => repo.set_shielded_wallet_state(relayer_id, state).await,
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

    #[test]
    fn legacy_sync_state_deserializes_with_empty_unshielded_wallet_state() {
        let json = r#"{
            "last_synced_index": 7,
            "ledger_context": null,
            "unshielded_balance": 123
        }"#;

        let state: RelayerSyncState = serde_json::from_str(json).unwrap();

        assert_eq!(state.unshielded_balance, 123);
        assert_eq!(state.unshielded_wallet.applied_transaction_id, 0);
        assert!(state.unshielded_wallet.available_utxos.is_empty());
        assert!(state.unshielded_wallet.pending_utxos.is_empty());
        assert!(state.shielded_wallet.pending_spends.is_empty());
    }

    fn shielded_reservation(nonce: &str, tx_id: &str, value: u128) -> ShieldedSpendReservation {
        ShieldedSpendReservation {
            coin_nonce: nonce.to_string(),
            nullifier: Some(format!("nullifier-{nonce}")),
            commitment: Some(format!("commitment-{nonce}")),
            transaction_id: tx_id.to_string(),
            token_type: "token".to_string(),
            value,
            created_at: "2026-04-29T00:00:00Z".to_string(),
            segment_id: Some(2),
        }
    }

    #[test]
    fn shielded_wallet_state_upserts_reservations_by_nonce() {
        let mut state = ShieldedWalletState::default();
        state.mark_pending(vec![shielded_reservation("nonce-1", "tx-1", 5)]);
        state.mark_pending(vec![shielded_reservation("nonce-1", "tx-2", 7)]);

        assert_eq!(state.pending_spends.len(), 1);
        assert_eq!(state.pending_spends[0].transaction_id, "tx-2");
        assert_eq!(state.pending_spends[0].value, 7);
        assert!(state.reserved_nonces().contains("nonce-1"));
    }

    #[test]
    fn shielded_wallet_state_releases_reservations_by_transaction() {
        let mut state = ShieldedWalletState {
            pending_spends: vec![
                shielded_reservation("nonce-1", "tx-1", 5),
                shielded_reservation("nonce-2", "tx-2", 7),
            ],
        };

        state.release_by_transaction("tx-1");

        assert_eq!(
            state.pending_spends,
            vec![shielded_reservation("nonce-2", "tx-2", 7)]
        );
    }

    #[test]
    fn shielded_wallet_state_prunes_non_active_transactions() {
        let mut state = ShieldedWalletState {
            pending_spends: vec![
                shielded_reservation("nonce-1", "tx-1", 5),
                shielded_reservation("nonce-2", "tx-2", 7),
            ],
        };

        state.retain_transactions(&["tx-2".to_string()]);

        assert_eq!(
            state.pending_spends,
            vec![shielded_reservation("nonce-2", "tx-2", 7)]
        );
    }

    #[tokio::test]
    async fn set_unshielded_wallet_state_derives_balance_from_available_utxos() {
        let repo = RelayerStateRepositoryStorage::new_in_memory();
        let wallet_state = UnshieldedWalletState {
            applied_transaction_id: 10,
            highest_transaction_id: 12,
            available_utxos: vec![UnshieldedUtxo {
                owner: "owner".to_string(),
                value: 5,
                token_type: "native".to_string(),
                intent_hash: "aa".repeat(32),
                output_index: 0,
                ctime: None,
                registered_for_dust_generation: false,
            }],
            pending_utxos: vec![UnshieldedUtxo {
                owner: "owner".to_string(),
                value: 7,
                token_type: "native".to_string(),
                intent_hash: "bb".repeat(32),
                output_index: 1,
                ctime: None,
                registered_for_dust_generation: false,
            }],
        };

        repo.set_unshielded_wallet_state("relayer-1", wallet_state.clone())
            .await
            .unwrap();

        assert_eq!(
            repo.get_unshielded_wallet_state("relayer-1").await.unwrap(),
            wallet_state
        );
        assert_eq!(repo.get_unshielded_balance("relayer-1").await.unwrap(), 5);
    }

    #[tokio::test]
    async fn set_shielded_wallet_state_round_trips_pending_reservations() {
        let repo = RelayerStateRepositoryStorage::new_in_memory();
        let wallet_state = ShieldedWalletState {
            pending_spends: vec![shielded_reservation("nonce-1", "tx-1", 5)],
        };

        repo.set_shielded_wallet_state("relayer-1", wallet_state.clone())
            .await
            .unwrap();

        assert_eq!(
            repo.get_shielded_wallet_state("relayer-1").await.unwrap(),
            wallet_state
        );
    }

    #[test]
    fn unshielded_success_update_moves_spent_out_and_adds_created() {
        let spent = UnshieldedUtxo {
            owner: "owner".to_string(),
            value: 5,
            token_type: "native".to_string(),
            intent_hash: "aa".repeat(32),
            output_index: 0,
            ctime: None,
            registered_for_dust_generation: false,
        };
        let created = UnshieldedUtxo {
            owner: "owner".to_string(),
            value: 3,
            token_type: "native".to_string(),
            intent_hash: "bb".repeat(32),
            output_index: 1,
            ctime: None,
            registered_for_dust_generation: false,
        };
        let mut state = UnshieldedWalletState {
            applied_transaction_id: 1,
            highest_transaction_id: 1,
            available_utxos: vec![spent.clone()],
            pending_utxos: vec![spent.clone()],
        };

        state.apply_success(2, vec![created.clone()], vec![spent]);

        assert_eq!(state.applied_transaction_id, 2);
        assert_eq!(state.available_utxos, vec![created]);
        assert!(state.pending_utxos.is_empty());
        assert_eq!(state.available_balance(), 3);
    }

    #[test]
    fn unshielded_success_update_releases_recreated_pending_utxo() {
        let stale_pending = UnshieldedUtxo {
            owner: "owner".to_string(),
            value: 5,
            token_type: "native".to_string(),
            intent_hash: "aa".repeat(32),
            output_index: 0,
            ctime: None,
            registered_for_dust_generation: false,
        };
        let spent = UnshieldedUtxo {
            owner: "owner".to_string(),
            value: 5,
            token_type: "native".to_string(),
            intent_hash: "bb".repeat(32),
            output_index: 0,
            ctime: None,
            registered_for_dust_generation: false,
        };
        let mut state = UnshieldedWalletState {
            applied_transaction_id: 1,
            highest_transaction_id: 1,
            available_utxos: Vec::new(),
            pending_utxos: vec![stale_pending.clone(), spent.clone()],
        };

        state.apply_success(2, vec![stale_pending.clone()], vec![spent]);

        assert_eq!(state.applied_transaction_id, 2);
        assert_eq!(state.available_utxos, vec![stale_pending]);
        assert!(state.pending_utxos.is_empty());
        assert_eq!(state.available_balance(), 5);
    }

    #[test]
    fn unshielded_failed_update_rolls_spent_back_from_pending() {
        let spent = UnshieldedUtxo {
            owner: "owner".to_string(),
            value: 5,
            token_type: "native".to_string(),
            intent_hash: "aa".repeat(32),
            output_index: 0,
            ctime: None,
            registered_for_dust_generation: false,
        };
        let mut state = UnshieldedWalletState {
            applied_transaction_id: 1,
            highest_transaction_id: 1,
            available_utxos: Vec::new(),
            pending_utxos: vec![spent.clone()],
        };

        state.apply_failed(2, vec![spent.clone()]);

        assert_eq!(state.applied_transaction_id, 2);
        assert_eq!(state.available_utxos, vec![spent]);
        assert!(state.pending_utxos.is_empty());
    }

    #[test]
    fn mark_pending_by_key_moves_selected_available_utxos() {
        let selected = UnshieldedUtxo {
            owner: "owner".to_string(),
            value: 5,
            token_type: "native".to_string(),
            intent_hash: "aa".repeat(32),
            output_index: 0,
            ctime: None,
            registered_for_dust_generation: false,
        };
        let untouched = UnshieldedUtxo {
            owner: "owner".to_string(),
            value: 7,
            token_type: "native".to_string(),
            intent_hash: "bb".repeat(32),
            output_index: 1,
            ctime: None,
            registered_for_dust_generation: false,
        };
        let mut state = UnshieldedWalletState {
            applied_transaction_id: 1,
            highest_transaction_id: 1,
            available_utxos: vec![selected.clone(), untouched.clone()],
            pending_utxos: Vec::new(),
        };

        state.mark_pending_by_keys(&[selected.key()]);

        assert_eq!(state.available_utxos, vec![untouched]);
        assert_eq!(state.pending_utxos, vec![selected]);
    }

    #[test]
    fn release_pending_by_key_moves_failed_spend_back_to_available() {
        let selected = UnshieldedUtxo {
            owner: "owner".to_string(),
            value: 5,
            token_type: "native".to_string(),
            intent_hash: "aa".repeat(32),
            output_index: 0,
            ctime: None,
            registered_for_dust_generation: false,
        };
        let untouched = UnshieldedUtxo {
            owner: "owner".to_string(),
            value: 7,
            token_type: "native".to_string(),
            intent_hash: "bb".repeat(32),
            output_index: 1,
            ctime: None,
            registered_for_dust_generation: false,
        };
        let mut state = UnshieldedWalletState {
            applied_transaction_id: 1,
            highest_transaction_id: 1,
            available_utxos: Vec::new(),
            pending_utxos: vec![selected.clone(), untouched.clone()],
        };

        state.release_pending_by_keys(&[selected.key()]);

        assert_eq!(state.available_utxos, vec![selected]);
        assert_eq!(state.pending_utxos, vec![untouched]);
    }
}
