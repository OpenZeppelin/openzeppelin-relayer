use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
};

use tracing::{debug, info, warn};

use crate::repositories::{
    RelayerStateRepositoryStorage, ShieldedSpendReservation, ShieldedWalletState, SyncStateTrait,
    UnshieldedUtxo, UnshieldedWalletState,
};

use super::super::indexer::{
    IndexerError, MidnightIndexerClient, ViewingKeyFormat, WalletSyncEvent, ZswapChainStateUpdate,
};

use futures::{SinkExt, StreamExt};

type UnshieldedStateLock = Arc<tokio::sync::Mutex<()>>;
static UNSHIELDED_STATE_LOCKS: OnceLock<Mutex<HashMap<String, UnshieldedStateLock>>> =
    OnceLock::new();
type ShieldedStateLock = Arc<tokio::sync::Mutex<()>>;
static SHIELDED_STATE_LOCKS: OnceLock<Mutex<HashMap<String, ShieldedStateLock>>> = OnceLock::new();

fn unshielded_state_lock(relayer_id: &str) -> UnshieldedStateLock {
    let registry = UNSHIELDED_STATE_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut locks = registry
        .lock()
        .expect("unshielded state lock registry poisoned");
    locks
        .entry(relayer_id.to_string())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone()
}

fn shielded_state_lock(relayer_id: &str) -> ShieldedStateLock {
    let registry = SHIELDED_STATE_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut locks = registry
        .lock()
        .expect("shielded state lock registry poisoned");
    locks
        .entry(relayer_id.to_string())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone()
}

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("Indexer error: {0}")]
    Indexer(#[from] IndexerError),
    #[error("Sync state error: {0}")]
    SyncState(String),
}

fn build_zswap_events_subscription(start_index: Option<u64>) -> String {
    match start_index {
        Some(idx) if idx > 0 => format!(
            "subscription {{ zswapLedgerEvents(id: {idx}) {{ id protocolVersion maxId raw }} }}"
        ),
        _ => "subscription { zswapLedgerEvents { id protocolVersion maxId raw } }".to_string(),
    }
}

fn build_unshielded_transactions_subscription(
    address: &str,
    transaction_id: Option<u64>,
) -> String {
    let cursor = match transaction_id {
        Some(id) if id > 0 => format!(", transactionId: {id}"),
        _ => String::new(),
    };

    format!(
        "subscription {{ unshieldedTransactions(address: \"{address}\"{cursor}) {{ ... on UnshieldedTransaction {{ transaction {{ id protocolVersion ... on RegularTransaction {{ transactionResult {{ status segments {{ id success }} }} }} }} createdUtxos {{ owner value tokenType intentHash outputIndex ctime registeredForDustGeneration }} spentUtxos {{ owner value tokenType intentHash outputIndex ctime registeredForDustGeneration }} }} ... on UnshieldedTransactionsProgress {{ highestTransactionId }} }} }}"
    )
}

/// Midnight sync manager coordinating wallet state synchronization.
///
/// The sync flow:
/// 1. **Connect** — register a viewing key with the indexer, get session ID
/// 2. **Fetch events** — poll for `ViewingUpdate` and `ProgressUpdate` events
/// 3. **Process** — extract transaction data and merkle tree updates
/// 4. **Persist** — save the last synced index via `SyncStateTrait`
///
/// The ledger context bytes are opaque at this layer — actual ZK wallet state
/// will be handled when midnight-node crate types are integrated.
#[derive(Clone)]
pub struct SyncManager<SS: SyncStateTrait = RelayerStateRepositoryStorage> {
    indexer_client: MidnightIndexerClient,
    sync_state_store: Arc<SS>,
    relayer_id: String,
}

impl<SS: SyncStateTrait + Send + Sync> SyncManager<SS> {
    pub fn new(
        indexer_client: MidnightIndexerClient,
        sync_state_store: Arc<SS>,
        relayer_id: String,
    ) -> Self {
        Self {
            indexer_client,
            sync_state_store,
            relayer_id,
        }
    }

    pub fn indexer_client(&self) -> &MidnightIndexerClient {
        &self.indexer_client
    }

    pub async fn current_index(&self) -> Result<u64, SyncError> {
        Ok(self
            .sync_state_store
            .get_midnight_state(&self.relayer_id)
            .await
            .map_err(|e| SyncError::SyncState(e.to_string()))?
            .zswap_last_synced_index)
    }

    pub async fn load_context(&self) -> Result<Option<Vec<u8>>, SyncError> {
        Ok(self
            .sync_state_store
            .get_midnight_state(&self.relayer_id)
            .await
            .map_err(|e| SyncError::SyncState(e.to_string()))?
            .ledger_context)
    }

    pub async fn persist_state(
        &self,
        index: u64,
        context: Option<Vec<u8>>,
    ) -> Result<(), SyncError> {
        self.sync_state_store
            .update_midnight_state(&self.relayer_id, |state| {
                state.zswap_last_synced_index = index;
                state.ledger_context = context.clone();
                Ok(())
            })
            .await
            .map_err(|e| SyncError::SyncState(e.to_string()))
    }

    pub async fn reset(&self) -> Result<(), SyncError> {
        self.sync_state_store
            .reset(&self.relayer_id)
            .await
            .map_err(|e| SyncError::SyncState(e.to_string()))
    }

    pub async fn health_check(&self) -> Result<(), SyncError> {
        self.indexer_client.health_check().await?;
        Ok(())
    }

    pub async fn get_unshielded_balance(&self) -> Result<u128, SyncError> {
        Ok(self
            .sync_state_store
            .get_midnight_state(&self.relayer_id)
            .await
            .map_err(|e| SyncError::SyncState(e.to_string()))?
            .unshielded_balance)
    }

    pub async fn get_unshielded_wallet_state(&self) -> Result<UnshieldedWalletState, SyncError> {
        Ok(self
            .sync_state_store
            .get_midnight_state(&self.relayer_id)
            .await
            .map_err(|e| SyncError::SyncState(e.to_string()))?
            .unshielded_wallet)
    }

    pub async fn mark_unshielded_pending_by_keys(
        &self,
        spent_keys: &[String],
    ) -> Result<UnshieldedWalletState, SyncError> {
        let lock = unshielded_state_lock(&self.relayer_id);
        let _guard = lock.lock().await;
        self.sync_state_store
            .update_midnight_state(&self.relayer_id, |state| {
                state.unshielded_wallet.mark_pending_by_keys(spent_keys);
                state.unshielded_balance = state.unshielded_wallet.available_balance();
                Ok(state.unshielded_wallet.clone())
            })
            .await
            .map_err(|e| SyncError::SyncState(e.to_string()))
    }

    pub async fn release_unshielded_pending_by_keys(
        &self,
        spent_keys: &[String],
    ) -> Result<UnshieldedWalletState, SyncError> {
        let lock = unshielded_state_lock(&self.relayer_id);
        let _guard = lock.lock().await;
        self.sync_state_store
            .update_midnight_state(&self.relayer_id, |state| {
                state.unshielded_wallet.release_pending_by_keys(spent_keys);
                state.unshielded_balance = state.unshielded_wallet.available_balance();
                Ok(state.unshielded_wallet.clone())
            })
            .await
            .map_err(|e| SyncError::SyncState(e.to_string()))
    }

    pub async fn get_shielded_wallet_state(&self) -> Result<ShieldedWalletState, SyncError> {
        Ok(self
            .sync_state_store
            .get_midnight_state(&self.relayer_id)
            .await
            .map_err(|e| SyncError::SyncState(e.to_string()))?
            .shielded_wallet)
    }

    pub async fn mark_shielded_pending(
        &self,
        transaction_id: &str,
        reservations: Vec<ShieldedSpendReservation>,
    ) -> Result<ShieldedWalletState, SyncError> {
        let lock = shielded_state_lock(&self.relayer_id);
        let _guard = lock.lock().await;
        let transaction_id = transaction_id.to_string();
        self.sync_state_store
            .update_midnight_state(&self.relayer_id, |state| {
                state
                    .shielded_wallet
                    .release_by_transaction(&transaction_id);
                state.shielded_wallet.mark_pending(reservations.clone());
                Ok(state.shielded_wallet.clone())
            })
            .await
            .map_err(|e| SyncError::SyncState(e.to_string()))
    }

    pub async fn release_shielded_pending_for_tx(
        &self,
        transaction_id: &str,
    ) -> Result<ShieldedWalletState, SyncError> {
        let lock = shielded_state_lock(&self.relayer_id);
        let _guard = lock.lock().await;
        let transaction_id = transaction_id.to_string();
        self.sync_state_store
            .update_midnight_state(&self.relayer_id, |state| {
                state
                    .shielded_wallet
                    .release_by_transaction(&transaction_id);
                Ok(state.shielded_wallet.clone())
            })
            .await
            .map_err(|e| SyncError::SyncState(e.to_string()))
    }

    pub async fn clear_confirmed_shielded_pending(
        &self,
        transaction_id: &str,
    ) -> Result<ShieldedWalletState, SyncError> {
        self.release_shielded_pending_for_tx(transaction_id).await
    }

    pub async fn retain_shielded_pending_transactions(
        &self,
        active_transaction_ids: &[String],
    ) -> Result<ShieldedWalletState, SyncError> {
        let lock = shielded_state_lock(&self.relayer_id);
        let _guard = lock.lock().await;
        let active_transaction_ids = active_transaction_ids.to_vec();
        self.sync_state_store
            .update_midnight_state(&self.relayer_id, |state| {
                state
                    .shielded_wallet
                    .retain_transactions(&active_transaction_ids);
                Ok(state.shielded_wallet.clone())
            })
            .await
            .map_err(|e| SyncError::SyncState(e.to_string()))
    }

    /// Perform an incremental sync using the indexer's wallet subscription.
    ///
    /// Connects with the given viewing key, fetches events since the last
    /// synced index, processes them, and persists the new sync cursor.
    ///
    /// Returns the number of events processed.
    pub async fn sync_incremental(
        &self,
        viewing_key: &ViewingKeyFormat,
    ) -> Result<SyncResult, SyncError> {
        let start_index = self.current_index().await?;

        // Connect wallet session
        let session_id = self.indexer_client.connect_wallet(viewing_key).await?;

        debug!(
            relayer_id = %self.relayer_id,
            session_id = %session_id,
            start_index,
            "Wallet session connected, fetching sync events"
        );

        // Fetch events since last sync
        let events = self
            .indexer_client
            .fetch_wallet_events(&session_id, Some(start_index))
            .await?;

        let mut result = SyncResult {
            events_processed: 0,
            transactions_found: 0,
            merkle_updates: 0,
            highest_index: start_index,
        };

        for event in &events {
            match event {
                WalletSyncEvent::ViewingUpdate { index, update, .. } => {
                    result.events_processed += 1;
                    if *index > result.highest_index {
                        result.highest_index = *index;
                    }

                    for chain_update in update {
                        match chain_update {
                            ZswapChainStateUpdate::RelevantTransaction { transaction, .. } => {
                                result.transactions_found += 1;
                                debug!(
                                    relayer_id = %self.relayer_id,
                                    tx_hash = %transaction.hash,
                                    index,
                                    "Sync found relevant transaction"
                                );
                            }
                            ZswapChainStateUpdate::MerkleTreeCollapsedUpdate { .. } => {
                                result.merkle_updates += 1;
                            }
                        }
                    }
                }
                WalletSyncEvent::ProgressUpdate {
                    highest_index,
                    highest_relevant_index,
                    ..
                } => {
                    debug!(
                        relayer_id = %self.relayer_id,
                        highest_index,
                        highest_relevant_index,
                        "Sync progress update"
                    );
                    if *highest_index > result.highest_index {
                        result.highest_index = *highest_index;
                    }
                }
            }
        }

        // Persist updated sync cursor
        if result.highest_index > start_index {
            self.persist_state(result.highest_index, None).await?;
            info!(
                relayer_id = %self.relayer_id,
                old_index = start_index,
                new_index = result.highest_index,
                transactions = result.transactions_found,
                merkle_updates = result.merkle_updates,
                "Sync state advanced"
            );
        }

        // Disconnect session (best-effort)
        if let Err(e) = self.indexer_client.disconnect_wallet(&session_id).await {
            warn!(
                relayer_id = %self.relayer_id,
                error = %e,
                "Failed to disconnect wallet session (non-fatal)"
            );
        }

        Ok(result)
    }

    /// Sync unshielded balance and collect UTXO details using WebSocket subscription.
    ///
    /// Returns the balance and a list of created/spent UTXOs with full details
    /// needed for UTXO injection into the LedgerContext.
    pub async fn sync_unshielded_balance(
        &self,
        address: &str,
    ) -> Result<UnshieldedSyncResult, SyncError> {
        #[derive(Clone)]
        enum UnshieldedStateUpdate {
            Progress(u64),
            Transaction {
                tx_id: u64,
                failed: bool,
                created: Vec<UnshieldedUtxo>,
                spent: Vec<UnshieldedUtxo>,
            },
        }

        let lock = unshielded_state_lock(&self.relayer_id);
        let _guard = lock.lock().await;
        let ws_url = self.indexer_client.ws_url();
        let mut wallet_state = self
            .sync_state_store
            .get_midnight_state(&self.relayer_id)
            .await
            .map_err(|e| SyncError::SyncState(e.to_string()))?
            .unshielded_wallet;
        info!(
            relayer_id = %self.relayer_id,
            address,
            ws_url,
            "Starting unshielded balance sync via WebSocket"
        );

        // Connect to the WebSocket endpoint using graphql-transport-ws protocol.
        // The indexer requires the Sec-WebSocket-Protocol: graphql-transport-ws header.
        let ws_request = tokio_tungstenite::tungstenite::http::Request::builder()
            .uri(ws_url)
            .header("Sec-WebSocket-Protocol", "graphql-transport-ws")
            .header(
                "Host",
                reqwest::Url::parse(ws_url)
                    .map(|u| u.host_str().unwrap_or("").to_string())
                    .unwrap_or_default(),
            )
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Version", "13")
            .header(
                "Sec-WebSocket-Key",
                tokio_tungstenite::tungstenite::handshake::client::generate_key(),
            )
            .body(())
            .map_err(|e| SyncError::SyncState(format!("WS request build failed: {e}")))?;

        let (ws_stream, _) = tokio_tungstenite::connect_async(ws_request)
            .await
            .map_err(|e| SyncError::SyncState(format!("WebSocket connect failed: {e}")))?;

        let (mut write, mut read) = ws_stream.split();

        // Send connection_init (graphql-transport-ws protocol)
        let init_msg = serde_json::json!({"type": "connection_init"});
        write
            .send(tokio_tungstenite::tungstenite::Message::Text(
                init_msg.to_string().into(),
            ))
            .await
            .map_err(|e| SyncError::SyncState(format!("WS send init failed: {e}")))?;

        // Wait for connection_ack
        let mut acked = false;
        let timeout = tokio::time::Duration::from_secs(10);
        let deadline = tokio::time::Instant::now() + timeout;

        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(timeout, read.next()).await {
                Ok(Some(Ok(msg))) => {
                    if let tokio_tungstenite::tungstenite::Message::Text(text) = &msg {
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(text) {
                            if val.get("type").and_then(|t| t.as_str()) == Some("connection_ack") {
                                acked = true;
                                break;
                            }
                        }
                    }
                }
                _ => break,
            }
        }

        if !acked {
            return Err(SyncError::SyncState("WS connection_ack timeout".into()));
        }

        // Subscribe to unshieldedTransactions for our address.
        // The subscription returns UnshieldedTransaction (with createdUtxos/spentUtxos)
        // and UnshieldedTransactionsProgress events.
        let query = build_unshielded_transactions_subscription(
            address,
            Some(wallet_state.applied_transaction_id),
        );
        let subscribe_msg = serde_json::json!({
            "id": "unsub-1",
            "type": "subscribe",
            "payload": {
                "query": query
            }
        });

        write
            .send(tokio_tungstenite::tungstenite::Message::Text(
                subscribe_msg.to_string().into(),
            ))
            .await
            .map_err(|e| SyncError::SyncState(format!("WS subscribe failed: {e}")))?;

        // Collect events with a timeout — the subscription will replay history then go live
        let mut event_count: u64 = 0;
        let mut created_utxos: Vec<UnshieldedUtxo> = Vec::new();
        let mut spent_utxos: Vec<UnshieldedUtxo> = Vec::new();
        let mut state_updates: Vec<UnshieldedStateUpdate> = Vec::new();
        let idle_timeout = tokio::time::Duration::from_secs(5);

        loop {
            match tokio::time::timeout(idle_timeout, read.next()).await {
                Ok(Some(Ok(msg))) => {
                    if let tokio_tungstenite::tungstenite::Message::Text(text) = &msg {
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(text) {
                            let msg_type = val.get("type").and_then(|t| t.as_str()).unwrap_or("");

                            match msg_type {
                                "next" => {
                                    if let Some(data) = val
                                        .get("payload")
                                        .and_then(|p| p.get("data"))
                                        .and_then(|d| d.get("unshieldedTransactions"))
                                    {
                                        if let Some(highest) = data
                                            .get("highestTransactionId")
                                            .and_then(|h| h.as_u64())
                                        {
                                            wallet_state.apply_progress(highest);
                                            state_updates
                                                .push(UnshieldedStateUpdate::Progress(highest));
                                            debug!(
                                                relayer_id = %self.relayer_id,
                                                highest_transaction_id = highest,
                                                "Unshielded sync progress event"
                                            );
                                        } else if let Some(tx) = data.get("transaction") {
                                            let tx_id = tx
                                                .get("id")
                                                .and_then(|id| id.as_u64())
                                                .unwrap_or(0);
                                            let status = tx
                                                .get("transactionResult")
                                                .and_then(|r| r.get("status"))
                                                .and_then(|s| s.as_str())
                                                .unwrap_or("SUCCESS");

                                            let created = data
                                                .get("createdUtxos")
                                                .and_then(|c| c.as_array())
                                                .map(|items| {
                                                    items
                                                        .iter()
                                                        .filter_map(UnshieldedUtxo::from_json)
                                                        .collect::<Vec<_>>()
                                                })
                                                .unwrap_or_default();
                                            let spent = data
                                                .get("spentUtxos")
                                                .and_then(|s| s.as_array())
                                                .map(|items| {
                                                    items
                                                        .iter()
                                                        .filter_map(UnshieldedUtxo::from_json)
                                                        .collect::<Vec<_>>()
                                                })
                                                .unwrap_or_default();

                                            created_utxos.extend(created.clone());
                                            spent_utxos.extend(spent.clone());
                                            event_count += 1;

                                            if status == "FAILURE" {
                                                wallet_state.apply_failed(tx_id, spent.clone());
                                            } else {
                                                wallet_state.apply_success(
                                                    tx_id,
                                                    created.clone(),
                                                    spent.clone(),
                                                );
                                            }
                                            state_updates.push(
                                                UnshieldedStateUpdate::Transaction {
                                                    tx_id,
                                                    failed: status == "FAILURE",
                                                    created,
                                                    spent,
                                                },
                                            );
                                        }
                                    }
                                }
                                "complete" => {
                                    debug!(relayer_id = %self.relayer_id, "Subscription completed");
                                    break;
                                }
                                "error" => {
                                    let err_msg = val
                                        .get("payload")
                                        .map(|p| p.to_string())
                                        .unwrap_or_default();
                                    warn!(relayer_id = %self.relayer_id, error = %err_msg, "Subscription error");
                                    break;
                                }
                                _ => {} // ping, ka, etc.
                            }
                        }
                    }
                }
                Ok(Some(Err(e))) => {
                    warn!(relayer_id = %self.relayer_id, error = %e, "WS read error");
                    break;
                }
                Ok(None) => break, // Stream ended
                Err(_) => {
                    // Idle timeout — no more events, we've caught up
                    debug!(relayer_id = %self.relayer_id, events = event_count, "Idle timeout, sync complete");
                    break;
                }
            }
        }

        // Close WebSocket
        let _ = write
            .send(tokio_tungstenite::tungstenite::Message::Close(None))
            .await;

        let state_updates = state_updates.clone();
        wallet_state = self
            .sync_state_store
            .update_midnight_state(&self.relayer_id, |state| {
                for update in &state_updates {
                    match update {
                        UnshieldedStateUpdate::Progress(highest) => {
                            state.unshielded_wallet.apply_progress(*highest);
                        }
                        UnshieldedStateUpdate::Transaction {
                            tx_id,
                            failed,
                            created,
                            spent,
                        } => {
                            if *failed {
                                state.unshielded_wallet.apply_failed(*tx_id, spent.clone());
                            } else {
                                state.unshielded_wallet.apply_success(
                                    *tx_id,
                                    created.clone(),
                                    spent.clone(),
                                );
                            }
                        }
                    }
                }
                state.unshielded_balance = state.unshielded_wallet.available_balance();
                Ok(state.unshielded_wallet.clone())
            })
            .await
            .map_err(|e| SyncError::SyncState(e.to_string()))?;
        let final_balance = wallet_state.available_balance();

        info!(
            relayer_id = %self.relayer_id,
            balance = final_balance,
            events = event_count,
            created = created_utxos.len(),
            spent = spent_utxos.len(),
            "Unshielded balance synced"
        );

        Ok(UnshieldedSyncResult {
            balance: final_balance,
            created_utxos,
            spent_utxos,
            wallet_state,
        })
    }

    /// Sync the shielded wallet state using the `zswapLedgerEvents` WebSocket subscription.
    ///
    /// The Midnight wallet SDK uses this event stream as the canonical input
    /// for shielded state. Each raw ledger event carries Merkle indexes for
    /// zswap outputs; replaying transaction offers cannot recover those indexes.
    pub async fn sync_shielded(
        &self,
        session_id: &str,
        start_index: Option<u64>,
        mut on_event: impl FnMut(ShieldedEvent) -> Result<(), SyncError>,
    ) -> Result<ShieldedSyncResult, SyncError> {
        let ws_url = self.indexer_client.ws_url();

        info!(
            relayer_id = %self.relayer_id,
            session_id,
            start_index,
            "Starting shielded wallet sync via WebSocket"
        );

        let ws_request = tokio_tungstenite::tungstenite::http::Request::builder()
            .uri(ws_url)
            .header("Sec-WebSocket-Protocol", "graphql-transport-ws")
            .header(
                "Host",
                reqwest::Url::parse(ws_url)
                    .map(|u| u.host_str().unwrap_or("").to_string())
                    .unwrap_or_default(),
            )
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Version", "13")
            .header(
                "Sec-WebSocket-Key",
                tokio_tungstenite::tungstenite::handshake::client::generate_key(),
            )
            .body(())
            .map_err(|e| SyncError::SyncState(format!("WS request build failed: {e}")))?;

        let (ws_stream, _) = tokio_tungstenite::connect_async(ws_request)
            .await
            .map_err(|e| SyncError::SyncState(format!("WebSocket connect failed: {e}")))?;

        let (mut write, mut read) = ws_stream.split();

        // connection_init
        let init = serde_json::json!({"type": "connection_init"});
        write
            .send(tokio_tungstenite::tungstenite::Message::Text(
                init.to_string().into(),
            ))
            .await
            .map_err(|e| SyncError::SyncState(format!("WS init failed: {e}")))?;

        // Wait for connection_ack
        let timeout = tokio::time::Duration::from_secs(10);
        loop {
            match tokio::time::timeout(timeout, read.next()).await {
                Ok(Some(Ok(msg))) => {
                    if let tokio_tungstenite::tungstenite::Message::Text(text) = &msg {
                        if text.contains("connection_ack") {
                            break;
                        }
                    }
                }
                _ => return Err(SyncError::SyncState("WS connection_ack timeout".into())),
            }
        }

        // `session_id` is retained in the signature because the caller still
        // owns connect/disconnect lifecycle. The zswap event stream itself is
        // global; wallet filtering happens during local replay via secret keys.
        let _ = session_id;
        let query = build_zswap_events_subscription(start_index);

        let sub_msg = serde_json::json!({
            "id": "shielded-1",
            "type": "subscribe",
            "payload": { "query": query }
        });

        write
            .send(tokio_tungstenite::tungstenite::Message::Text(
                sub_msg.to_string().into(),
            ))
            .await
            .map_err(|e| SyncError::SyncState(format!("WS subscribe failed: {e}")))?;

        let mut result = ShieldedSyncResult::default();
        let idle_timeout = tokio::time::Duration::from_secs(10);

        loop {
            match tokio::time::timeout(idle_timeout, read.next()).await {
                Ok(Some(Ok(msg))) => {
                    if let tokio_tungstenite::tungstenite::Message::Text(text) = &msg {
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(text.as_ref()) {
                            let msg_type = val.get("type").and_then(|t| t.as_str()).unwrap_or("");

                            match msg_type {
                                "next" => {
                                    if let Some(event) = val
                                        .get("payload")
                                        .and_then(|p| p.get("data"))
                                        .and_then(|d| d.get("zswapLedgerEvents"))
                                    {
                                        let raw_hex =
                                            event.get("raw").and_then(|r| r.as_str()).unwrap_or("");
                                        let event_id =
                                            event.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
                                        let max_id = event
                                            .get("maxId")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0);
                                        let protocol_version = event
                                            .get("protocolVersion")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0)
                                            as u32;

                                        debug!(
                                            relayer_id = %self.relayer_id,
                                            event_id,
                                            max_id,
                                            protocol_version,
                                            raw_len = raw_hex.len() / 2,
                                            "Received zswap ledger event"
                                        );

                                        on_event(ShieldedEvent::LedgerEvent {
                                            raw_hex: raw_hex.to_string(),
                                            event_id,
                                            max_id,
                                            protocol_version,
                                        })?;

                                        result.ledger_events += 1;
                                        if event_id > result.highest_index {
                                            result.highest_index = event_id;
                                        }
                                    }
                                }
                                "complete" => break,
                                "error" => {
                                    let err = val
                                        .get("payload")
                                        .map(|p| p.to_string())
                                        .unwrap_or_default();
                                    warn!(relayer_id = %self.relayer_id, error = %err, "Shielded subscription error");
                                    return Err(SyncError::SyncState(format!(
                                        "Subscription error: {err}"
                                    )));
                                }
                                _ => {}
                            }
                        }
                    }
                }
                Ok(Some(Err(e))) => {
                    warn!(relayer_id = %self.relayer_id, error = %e, "WS read error");
                    break;
                }
                Ok(None) => break,
                Err(_) => {
                    debug!(relayer_id = %self.relayer_id, "Shielded sync idle timeout, caught up");
                    break;
                }
            }
        }

        let _ = write
            .send(tokio_tungstenite::tungstenite::Message::Close(None))
            .await;

        info!(
            relayer_id = %self.relayer_id,
            ledger_events = result.ledger_events,
            highest_index = result.highest_index,
            "Shielded sync completed"
        );

        Ok(result)
    }

    /// Sync DUST ledger events via WebSocket subscription.
    ///
    /// Subscribes to `dustLedgerEvents` and collects the raw event hex data.
    /// Returns the collected events for the caller to feed into the LedgerContext.
    pub async fn sync_dust_events(&self) -> Result<Vec<String>, SyncError> {
        let ws_url = self.indexer_client.ws_url();

        info!(
            relayer_id = %self.relayer_id,
            "Starting DUST ledger events sync via WebSocket"
        );

        let ws_request = tokio_tungstenite::tungstenite::http::Request::builder()
            .uri(ws_url)
            .header("Sec-WebSocket-Protocol", "graphql-transport-ws")
            .header(
                "Host",
                reqwest::Url::parse(ws_url)
                    .map(|u| u.host_str().unwrap_or("").to_string())
                    .unwrap_or_default(),
            )
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Version", "13")
            .header(
                "Sec-WebSocket-Key",
                tokio_tungstenite::tungstenite::handshake::client::generate_key(),
            )
            .body(())
            .map_err(|e| SyncError::SyncState(format!("WS request build failed: {e}")))?;

        let (ws_stream, _) = tokio_tungstenite::connect_async(ws_request)
            .await
            .map_err(|e| SyncError::SyncState(format!("WebSocket connect failed: {e}")))?;

        let (mut write, mut read) = ws_stream.split();

        // connection_init
        let init = serde_json::json!({"type": "connection_init"});
        write
            .send(tokio_tungstenite::tungstenite::Message::Text(
                init.to_string().into(),
            ))
            .await
            .map_err(|e| SyncError::SyncState(format!("WS init failed: {e}")))?;

        // Wait for ack
        let timeout = tokio::time::Duration::from_secs(10);
        loop {
            match tokio::time::timeout(timeout, read.next()).await {
                Ok(Some(Ok(msg))) => {
                    if let tokio_tungstenite::tungstenite::Message::Text(text) = &msg {
                        if text.contains("connection_ack") {
                            break;
                        }
                    }
                }
                _ => return Err(SyncError::SyncState("WS connection_ack timeout".into())),
            }
        }

        // Subscribe to dustLedgerEvents — pull the same fields the TS wallet
        // SDK pulls (type, id, maxId, protocolVersion, raw) so we can compare
        // decoded-event metadata across stacks to triage the zero-balance bug.
        let sub_msg = serde_json::json!({
            "id": "dust-1",
            "type": "subscribe",
            "payload": {
                "query": "subscription { dustLedgerEvents { type: __typename id maxId protocolVersion raw } }"
            }
        });

        write
            .send(tokio_tungstenite::tungstenite::Message::Text(
                sub_msg.to_string().into(),
            ))
            .await
            .map_err(|e| SyncError::SyncState(format!("WS subscribe failed: {e}")))?;

        let mut raw_events: Vec<String> = Vec::new();

        // Termination strategy (Option 3 Part A): stop only once we've observed
        // an event whose `id == maxId` (i.e. chain's last emitted DUST event)
        // AND a short quiet period confirms no further events are in-flight.
        //
        // `maxId` is the chain's DUST-event high-water-mark at emission time.
        // When we receive an event where `id == maxId`, our wallet is caught
        // up to what the indexer has; waiting a couple seconds past that rules
        // out a stray straggler mid-block. The resulting tree root equals the
        // chain's most recent block-end root — the only state chain's
        // `root_history` records, which is what the DUST spend proof needs.
        //
        // If we never catch up (e.g. chain way ahead, indexer lagging), we
        // fall through the longer `not_caught_up_idle` timeout so the sync
        // still returns rather than hang forever.
        let caught_up_quiet = tokio::time::Duration::from_secs(2);
        let not_caught_up_idle = tokio::time::Duration::from_secs(30);
        let absolute_timeout = tokio::time::Duration::from_secs(300);
        let start = tokio::time::Instant::now();
        let mut is_caught_up = false;
        let mut caught_up_clean = false;

        // Diagnostic accumulators: per-variant counts, id range, distinct
        // protocolVersion / maxId values, and first/last raw-hex samples.
        let mut type_counts: std::collections::BTreeMap<String, u32> =
            std::collections::BTreeMap::new();
        let mut protocol_versions: std::collections::BTreeSet<i64> =
            std::collections::BTreeSet::new();
        let mut max_ids_seen: std::collections::BTreeSet<i64> = std::collections::BTreeSet::new();
        let mut min_event_id: Option<i64> = None;
        let mut max_event_id: Option<i64> = None;
        const SAMPLE_CAP: usize = 3;
        let mut first_samples: Vec<serde_json::Value> = Vec::new();
        let mut last_samples: std::collections::VecDeque<serde_json::Value> =
            std::collections::VecDeque::new();

        'read_loop: loop {
            if start.elapsed() > absolute_timeout {
                warn!(
                    relayer_id = %self.relayer_id,
                    events = raw_events.len(),
                    is_caught_up,
                    "DUST sync hit absolute timeout"
                );
                break;
            }
            let timeout = if is_caught_up {
                caught_up_quiet
            } else {
                not_caught_up_idle
            };
            match tokio::time::timeout(timeout, read.next()).await {
                Ok(Some(Ok(msg))) => {
                    if let tokio_tungstenite::tungstenite::Message::Text(text) = &msg {
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(text.as_ref()) {
                            let msg_type = val.get("type").and_then(|t| t.as_str()).unwrap_or("");
                            match msg_type {
                                "next" => {
                                    let ev = val
                                        .get("payload")
                                        .and_then(|p| p.get("data"))
                                        .and_then(|d| d.get("dustLedgerEvents"));
                                    if let Some(ev) = ev {
                                        if let Some(raw) = ev.get("raw").and_then(|r| r.as_str()) {
                                            raw_events.push(raw.to_string());

                                            let ev_type = ev
                                                .get("type")
                                                .and_then(|t| t.as_str())
                                                .unwrap_or("unknown")
                                                .to_string();
                                            *type_counts.entry(ev_type.clone()).or_insert(0) += 1;

                                            if let Some(pv) =
                                                ev.get("protocolVersion").and_then(|v| v.as_i64())
                                            {
                                                protocol_versions.insert(pv);
                                            }
                                            let ev_max_id =
                                                ev.get("maxId").and_then(|v| v.as_i64());
                                            if let Some(mx) = ev_max_id {
                                                max_ids_seen.insert(mx);
                                            }
                                            let ev_id = ev.get("id").and_then(|v| v.as_i64());
                                            if let Some(id) = ev_id {
                                                min_event_id =
                                                    Some(min_event_id.map_or(id, |m| m.min(id)));
                                                max_event_id =
                                                    Some(max_event_id.map_or(id, |m| m.max(id)));
                                            }

                                            // Caught-up signal: chain's max == the event's own id.
                                            // Any later event that bumps maxId drops us back
                                            // into "catching up" mode until we next see equality.
                                            if let (Some(id), Some(mx)) = (ev_id, ev_max_id) {
                                                is_caught_up = id == mx;
                                            }

                                            let sample = serde_json::json!({
                                                "type": ev_type,
                                                "id": ev.get("id"),
                                                "maxId": ev.get("maxId"),
                                                "pv": ev.get("protocolVersion"),
                                                "raw_len": raw.len(),
                                                "raw_prefix": raw.chars().take(64).collect::<String>(),
                                            });
                                            if first_samples.len() < SAMPLE_CAP {
                                                first_samples.push(sample.clone());
                                            }
                                            last_samples.push_back(sample);
                                            if last_samples.len() > SAMPLE_CAP {
                                                last_samples.pop_front();
                                            }
                                        }
                                    }
                                }
                                "complete" => break 'read_loop,
                                "error" => {
                                    let err = val
                                        .get("payload")
                                        .map(|p| p.to_string())
                                        .unwrap_or_default();
                                    warn!(error = %err, "DUST subscription error");
                                    break 'read_loop;
                                }
                                _ => {}
                            }
                        }
                    }
                }
                Ok(Some(Err(e))) => {
                    warn!(error = %e, "WS read error");
                    break;
                }
                Ok(None) => break,
                Err(_) => {
                    // Idle. If caught up, this is the clean stop — chain has
                    // emitted no new events for `caught_up_quiet`, confirming
                    // our tree root equals the most recent block-end root.
                    // Otherwise we fall out on `not_caught_up_idle` as a
                    // safety net (don't block the startup path forever).
                    if is_caught_up {
                        caught_up_clean = true;
                        debug!(
                            relayer_id = %self.relayer_id,
                            events = raw_events.len(),
                            id_max = ?max_event_id,
                            "DUST sync caught up — quiet period elapsed"
                        );
                    } else {
                        warn!(
                            relayer_id = %self.relayer_id,
                            events = raw_events.len(),
                            "DUST sync idle without catching up — tree may be stale"
                        );
                    }
                    break;
                }
            }
        }

        let _ = write
            .send(tokio_tungstenite::tungstenite::Message::Close(None))
            .await;

        // Structured diagnostic summary — mirrors the TS register.mjs dump so
        // the two can be diffed side-by-side.
        let type_counts_json = serde_json::to_string(&type_counts).unwrap_or_default();
        let pvs_json = serde_json::to_string(&protocol_versions).unwrap_or_default();
        let max_ids_json = serde_json::to_string(&max_ids_seen).unwrap_or_default();
        let first_json = serde_json::to_string(&first_samples).unwrap_or_default();
        let last_json =
            serde_json::to_string(&last_samples.iter().collect::<Vec<_>>()).unwrap_or_default();
        info!(
            relayer_id = %self.relayer_id,
            total = raw_events.len(),
            caught_up_clean,
            by_type = %type_counts_json,
            id_min = ?min_event_id,
            id_max = ?max_event_id,
            protocol_versions = %pvs_json,
            max_ids_seen = %max_ids_json,
            first_samples = %first_json,
            last_samples = %last_json,
            "Rust raw DUST events diagnostic"
        );

        info!(
            relayer_id = %self.relayer_id,
            events = raw_events.len(),
            "DUST ledger events synced"
        );

        Ok(raw_events)
    }
}

/// Events emitted during shielded wallet sync.
pub enum ShieldedEvent {
    /// A raw `midnight:ledger:event` payload from `zswapLedgerEvents`.
    LedgerEvent {
        raw_hex: String,
        event_id: u64,
        max_id: u64,
        protocol_version: u32,
    },
}

/// Result of a shielded sync operation.
#[derive(Debug, Clone, Default)]
pub struct ShieldedSyncResult {
    pub ledger_events: u64,
    pub highest_index: u64,
}

/// Result of a sync operation.
#[derive(Debug, Clone, Default)]
pub struct SyncResult {
    pub events_processed: u64,
    pub transactions_found: u64,
    pub merkle_updates: u64,
    pub highest_index: u64,
}

/// Result of unshielded balance sync with full UTXO details.
#[derive(Debug, Clone)]
pub struct UnshieldedSyncResult {
    pub balance: u128,
    pub created_utxos: Vec<UnshieldedUtxo>,
    pub spent_utxos: Vec<UnshieldedUtxo>,
    pub wallet_state: UnshieldedWalletState,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shielded_sync_subscribes_to_zswap_ledger_events() {
        let query = build_zswap_events_subscription(Some(42));

        assert!(query.contains("zswapLedgerEvents"));
        assert!(query.contains("id protocolVersion maxId raw"));
        assert!(query.contains("42"));
        assert!(!query.contains("shieldedTransactions"));
        assert!(!query.contains("collapsedMerkleTree"));
    }

    #[test]
    fn initial_zswap_sync_omits_zero_cursor() {
        let query = build_zswap_events_subscription(Some(0));

        assert!(query.contains("zswapLedgerEvents"));
        assert!(!query.contains("$id"));
        assert!(!query.contains("id: 0"));
    }

    #[test]
    fn unshielded_sync_query_uses_cursor_and_status_fields() {
        let query = build_unshielded_transactions_subscription("mn_addr_preview1xyz", Some(77));

        assert!(query.contains("unshieldedTransactions"));
        assert!(query.contains("transactionId: 77"));
        assert!(query.contains("transactionResult"));
        assert!(query.contains("status"));
        assert!(query.contains("registeredForDustGeneration"));
    }
}
