use std::sync::Arc;

use tracing::{debug, info, warn};

use crate::repositories::{RelayerStateRepositoryStorage, SyncStateTrait};

use super::super::indexer::{
    IndexerError, MidnightIndexerClient, ViewingKeyFormat, WalletSyncEvent, ZswapChainStateUpdate,
};

use futures::{SinkExt, StreamExt};

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("Indexer error: {0}")]
    Indexer(#[from] IndexerError),
    #[error("Sync state error: {0}")]
    SyncState(String),
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
            .get_last_synced_index(&self.relayer_id)
            .await
            .map_err(|e| SyncError::SyncState(e.to_string()))?
            .unwrap_or(0))
    }

    pub async fn load_context(&self) -> Result<Option<Vec<u8>>, SyncError> {
        self.sync_state_store
            .get_ledger_context(&self.relayer_id)
            .await
            .map_err(|e| SyncError::SyncState(e.to_string()))
    }

    pub async fn persist_state(
        &self,
        index: u64,
        context: Option<Vec<u8>>,
    ) -> Result<(), SyncError> {
        self.sync_state_store
            .set_sync_state(&self.relayer_id, index, context)
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
        self.sync_state_store
            .get_unshielded_balance(&self.relayer_id)
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
        let ws_url = self.indexer_client.ws_url();
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
        let subscribe_msg = serde_json::json!({
            "id": "unsub-1",
            "type": "subscribe",
            "payload": {
                "query": format!(
                    "subscription {{ unshieldedTransactions(address: \"{address}\") {{ ... on UnshieldedTransaction {{ createdUtxos {{ owner value tokenType intentHash outputIndex }} spentUtxos {{ owner value tokenType intentHash outputIndex }} }} ... on UnshieldedTransactionsProgress {{ highestTransactionId }} }} }}"
                )
            }
        });

        write
            .send(tokio_tungstenite::tungstenite::Message::Text(
                subscribe_msg.to_string().into(),
            ))
            .await
            .map_err(|e| SyncError::SyncState(format!("WS subscribe failed: {e}")))?;

        // Collect events with a timeout — the subscription will replay history then go live
        let mut balance: i128 = 0;
        let mut event_count: u64 = 0;
        let mut created_utxos: Vec<UtxoDetail> = Vec::new();
        let mut spent_utxos: Vec<UtxoDetail> = Vec::new();
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
                                        // Collect created UTXOs with full details
                                        if let Some(created) =
                                            data.get("createdUtxos").and_then(|c| c.as_array())
                                        {
                                            for utxo in created {
                                                if let Some(detail) = UtxoDetail::from_json(utxo) {
                                                    balance += detail.value as i128;
                                                    created_utxos.push(detail);
                                                    event_count += 1;
                                                }
                                            }
                                        }
                                        // Collect spent UTXOs
                                        if let Some(spent) =
                                            data.get("spentUtxos").and_then(|s| s.as_array())
                                        {
                                            for utxo in spent {
                                                if let Some(detail) = UtxoDetail::from_json(utxo) {
                                                    balance -= detail.value as i128;
                                                    spent_utxos.push(detail);
                                                    event_count += 1;
                                                }
                                            }
                                        }
                                        // Progress events — just log
                                        if data.get("highestTransactionId").is_some() {
                                            debug!(relayer_id = %self.relayer_id, "Unshielded sync progress event");
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

        let final_balance = balance.max(0) as u128;

        // Persist balance in its own field (not ledger_context)
        self.sync_state_store
            .set_unshielded_balance(&self.relayer_id, final_balance)
            .await
            .map_err(|e| SyncError::SyncState(e.to_string()))?;

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
        })
    }

    /// Sync the shielded wallet state using the `shieldedTransactions` WebSocket subscription.
    ///
    /// This feeds transaction data and merkle tree updates into the `LedgerContext`,
    /// which is needed for constructing new transactions (the context tracks the
    /// wallet's coins and the chain's merkle tree state).
    ///
    /// The `raw_tx_handler` callback is called for each relevant transaction's raw hex.
    /// It should deserialize and apply it to the LedgerContext.
    pub async fn sync_shielded(
        &self,
        session_id: &str,
        start_index: Option<u64>,
        mut on_event: impl FnMut(ShieldedEvent),
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

        // Build the subscription query
        let mut query = format!("subscription {{ shieldedTransactions(sessionId: \"{session_id}\"");
        if let Some(idx) = start_index {
            query.push_str(&format!(", index: {idx}"));
        }
        query.push_str(") { ... on RelevantTransaction { transaction { id hash raw startIndex endIndex protocolVersion } collapsedMerkleTree { startIndex endIndex update protocolVersion } } ... on ShieldedTransactionsProgress { highestEndIndex highestCheckedEndIndex highestRelevantEndIndex } } }");

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
                                    if let Some(data) = val
                                        .get("payload")
                                        .and_then(|p| p.get("data"))
                                        .and_then(|d| d.get("shieldedTransactions"))
                                    {
                                        // RelevantTransaction event
                                        if let Some(tx) = data.get("transaction") {
                                            let raw_hex = tx
                                                .get("raw")
                                                .and_then(|r| r.as_str())
                                                .unwrap_or("");
                                            let tx_hash = tx
                                                .get("hash")
                                                .and_then(|h| h.as_str())
                                                .unwrap_or("");
                                            let start_idx = tx
                                                .get("startIndex")
                                                .and_then(|s| s.as_u64())
                                                .unwrap_or(0);
                                            let end_idx = tx
                                                .get("endIndex")
                                                .and_then(|e| e.as_u64())
                                                .unwrap_or(0);

                                            debug!(
                                                relayer_id = %self.relayer_id,
                                                tx_hash,
                                                start_idx,
                                                end_idx,
                                                raw_len = raw_hex.len() / 2,
                                                "Received shielded transaction"
                                            );

                                            on_event(ShieldedEvent::Transaction {
                                                raw_hex: raw_hex.to_string(),
                                                tx_hash: tx_hash.to_string(),
                                                start_index: start_idx,
                                                end_index: end_idx,
                                            });

                                            result.transactions += 1;
                                            if end_idx > result.highest_index {
                                                result.highest_index = end_idx;
                                            }
                                        }

                                        // CollapsedMerkleTree update
                                        if let Some(cmt) = data.get("collapsedMerkleTree") {
                                            let update_hex = cmt
                                                .get("update")
                                                .and_then(|u| u.as_str())
                                                .unwrap_or("");
                                            let start_idx = cmt
                                                .get("startIndex")
                                                .and_then(|s| s.as_u64())
                                                .unwrap_or(0);
                                            let end_idx = cmt
                                                .get("endIndex")
                                                .and_then(|e| e.as_u64())
                                                .unwrap_or(0);

                                            on_event(ShieldedEvent::MerkleUpdate {
                                                update_hex: update_hex.to_string(),
                                                start_index: start_idx,
                                                end_index: end_idx,
                                            });

                                            result.merkle_updates += 1;
                                            if end_idx > result.highest_index {
                                                result.highest_index = end_idx;
                                            }
                                        }

                                        // Progress event
                                        if let Some(highest) =
                                            data.get("highestEndIndex").and_then(|h| h.as_u64())
                                        {
                                            on_event(ShieldedEvent::Progress {
                                                highest_end_index: highest,
                                            });
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

        // Persist sync cursor
        if result.highest_index > 0 {
            self.sync_state_store
                .update_if_greater(&self.relayer_id, result.highest_index)
                .await
                .map_err(|e| SyncError::SyncState(e.to_string()))?;
        }

        info!(
            relayer_id = %self.relayer_id,
            transactions = result.transactions,
            merkle_updates = result.merkle_updates,
            highest_index = result.highest_index,
            "Shielded sync completed"
        );

        Ok(result)
    }
}

/// Events emitted during shielded wallet sync.
pub enum ShieldedEvent {
    /// A relevant transaction with raw serialized bytes.
    Transaction {
        raw_hex: String,
        tx_hash: String,
        start_index: u64,
        end_index: u64,
    },
    /// A collapsed merkle tree update.
    MerkleUpdate {
        update_hex: String,
        start_index: u64,
        end_index: u64,
    },
    /// Sync progress indicator.
    Progress { highest_end_index: u64 },
}

/// Result of a shielded sync operation.
#[derive(Debug, Clone, Default)]
pub struct ShieldedSyncResult {
    pub transactions: u64,
    pub merkle_updates: u64,
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
    pub created_utxos: Vec<UtxoDetail>,
    pub spent_utxos: Vec<UtxoDetail>,
}

/// Full details of an unshielded UTXO from the indexer.
#[derive(Debug, Clone)]
pub struct UtxoDetail {
    pub owner: String,
    pub value: u128,
    pub token_type: String,
    pub intent_hash: String,
    pub output_index: u32,
}

impl UtxoDetail {
    pub fn from_json(val: &serde_json::Value) -> Option<Self> {
        Some(Self {
            owner: val.get("owner")?.as_str()?.to_string(),
            value: val.get("value")?.as_str()?.parse().ok()?,
            token_type: val.get("tokenType")?.as_str().unwrap_or("").to_string(),
            intent_hash: val.get("intentHash")?.as_str().unwrap_or("").to_string(),
            output_index: val.get("outputIndex")?.as_u64().unwrap_or(0) as u32,
        })
    }
}
