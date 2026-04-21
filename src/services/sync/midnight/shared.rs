//! Shared DUST sync task — one subscription per network, N wallet observers.
//!
//! # Why
//!
//! The single-relayer, one-shot sync model this module replaces had three
//! blockers we hit in earlier iterations:
//!
//! 1. **Staleness window.** After a per-relayer sync finished, chain advanced
//!    but our wallet's tree root didn't, so sequential tx submissions failed
//!    with `InvalidDustSpendProof` (error 170).
//! 2. **Multi-relayer scaling.** N relayers each ran their own subscription,
//!    each decoded the same ~34K events from scratch. Indexer load, bandwidth,
//!    and CPU all scaled linearly in N.
//! 3. **Blocking startup.** Process boot couldn't serve HTTP until every
//!    relayer finished its initial sync.
//!
//! # Design
//!
//! A [`SharedDustSyncTask`] owns **one WebSocket subscription to
//! `dustLedgerEvents` per network** and applies each decoded event to every
//! subscribed wallet's private `DustLocalState`. Per-wallet key filtering is
//! already handled inside the midnight-ledger `replay_events` library call, so
//! observers incur only O(1) state-check cost per event on top of the shared
//! decode.
//!
//! Each subscriber gets a [`WalletHandle`] exposing a `tokio::sync::watch`
//! channel for [`SyncStatus`] transitions. Callers await `await_ready()`
//! before building transactions — this replaces the old
//! `set_latest_block` anchor + one-shot sync pattern.
//!
//! Lifecycle:
//!
//! - Start the task once at process boot per active network via
//!   [`SharedDustSyncTask::new`] + [`SharedDustSyncTask::start`].
//! - Each relayer calls [`SharedDustSyncTask::subscribe_wallet`] to register
//!   its wallet seed and receive a `WalletHandle`.
//! - Relayer init path awaits `handle.await_ready()` in a background watcher
//!   task, then flips the relayer's `sync_status` from Syncing to Ready.
//! - Shutdown is signaled via a `tokio::sync::watch` flag; the task drops the
//!   WS and exits.
//!
//! This file introduces the primitives only. Process-boot wiring and relayer
//! state-machine integration land in follow-up steps.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio::sync::watch;
use tracing::{debug, info, warn};

use midnight_node_ledger_helpers::{
    make_block_context, DefaultDB, Event, HashOutput, LedgerContext, ProofMarker, SerdeTransaction,
    Signature, Timestamp, WalletSeed,
};

use super::indexer::MidnightIndexerClient;

/// Readiness of a wallet's DUST state relative to the chain tip.
///
/// Transition order is `Syncing → Ready`. Failures during bootstrap land in
/// `Failed`; transient reconnects keep the wallet in `Ready` once it has
/// caught up once (the task's event loop continues to apply new events in the
/// background).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncStatus {
    /// Initial state — catching up from the indexer's full event history.
    Syncing,
    /// Applied all events up to (at least) the chain's latest reported event.
    /// Transactions can be built from this wallet's state.
    Ready,
    /// Bootstrap failed (e.g. indexer unreachable). The task will retry; when
    /// it recovers, the status transitions back to `Syncing` and then `Ready`.
    Failed(String),
}

/// Per-wallet view into the shared task — observes status, reads state.
///
/// A handle is cheap to clone (`Arc` under the hood) and safe to share across
/// tasks. Dropping the last handle does NOT stop the sync task; the task's
/// lifecycle is managed by [`SharedDustSyncTask::stop`] explicitly.
pub struct WalletHandle {
    seed: WalletSeed,
    status_rx: watch::Receiver<SyncStatus>,
    context: Arc<LedgerContext<DefaultDB>>,
}

impl WalletHandle {
    /// The seed this handle observes.
    pub fn seed(&self) -> &WalletSeed {
        &self.seed
    }

    /// Shared `LedgerContext` holding this wallet's `DustLocalState`.
    ///
    /// Callers that need to read the wallet's DUST balance / UTXO set should
    /// go through `context.with_wallet_from_seed(self.seed().clone(), ...)`.
    pub fn context(&self) -> &Arc<LedgerContext<DefaultDB>> {
        &self.context
    }

    /// Current status snapshot. Cheap; use [`await_ready`] to block for
    /// `Ready`.
    pub fn status(&self) -> SyncStatus {
        self.status_rx.borrow().clone()
    }

    /// Whether the wallet's DUST state is currently caught up.
    pub fn is_ready(&self) -> bool {
        matches!(*self.status_rx.borrow(), SyncStatus::Ready)
    }

    /// Wait until the wallet's status becomes `Ready`.
    ///
    /// Returns immediately if already `Ready`. A `Failed` status is returned
    /// as `Err` so callers can surface it.
    pub async fn await_ready(&self) -> Result<(), String> {
        let mut rx = self.status_rx.clone();
        loop {
            match &*rx.borrow() {
                SyncStatus::Ready => return Ok(()),
                SyncStatus::Failed(reason) => return Err(reason.clone()),
                SyncStatus::Syncing => {}
            }
            if rx.changed().await.is_err() {
                // sender dropped; task is gone. Treat as failure.
                return Err("sync task terminated".into());
            }
        }
    }
}

/// Per-observer bookkeeping held inside the task. Not exposed publicly.
struct WalletEntry {
    seed: WalletSeed,
    status_tx: watch::Sender<SyncStatus>,
}

/// Owns one WebSocket subscription per network and broadcasts decoded DUST
/// events to all subscribed wallets.
///
/// Construct with [`new`](Self::new). Subscribe wallets via
/// [`subscribe_wallet`](Self::subscribe_wallet) BEFORE starting the loop so
/// they receive events from the opening batch. Start the event loop with
/// [`start`](Self::start); stop it via [`stop`](Self::stop).
pub struct SharedDustSyncTask {
    network_id: String,
    indexer_client: MidnightIndexerClient,
    context: Arc<LedgerContext<DefaultDB>>,
    wallets: Mutex<Vec<WalletEntry>>,
    shutdown_tx: watch::Sender<bool>,
    shutdown_rx: watch::Receiver<bool>,
    task_handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Highest `applied_id` for which `try_mark_ready` has already done its
    /// anchor refresh + tree sync. Used to skip MB-scale serialize/deserialize
    /// work when the cursor hasn't advanced (common during inter-block quiet
    /// periods where the event loop keeps firing "caught up" checks).
    last_synced_applied_id: Mutex<i64>,
}

impl SharedDustSyncTask {
    /// Create a new (not-yet-running) task for a given network.
    ///
    /// `context` must be the process-wide `LedgerContext` that the relayer
    /// factory and transaction factory already share. This task registers
    /// wallets into that context as subscribers call `subscribe_wallet`.
    pub fn new(
        network_id: String,
        indexer_client: MidnightIndexerClient,
        context: Arc<LedgerContext<DefaultDB>>,
    ) -> Arc<Self> {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        Arc::new(Self {
            network_id,
            indexer_client,
            context,
            wallets: Mutex::new(Vec::new()),
            shutdown_tx,
            shutdown_rx,
            task_handle: Mutex::new(None),
            last_synced_applied_id: Mutex::new(-1),
        })
    }

    pub fn network_id(&self) -> &str {
        &self.network_id
    }

    pub fn context(&self) -> &Arc<LedgerContext<DefaultDB>> {
        &self.context
    }

    /// Register a wallet seed as an observer and receive a handle.
    ///
    /// The wallet MUST already exist in the shared `LedgerContext` (i.e. the
    /// seed was passed to `LedgerContext::new_from_wallet_seeds` at
    /// construction). This keeps the library's `wallet_for_seed` panic
    /// surface out of our event loop.
    pub fn subscribe_wallet(self: &Arc<Self>, seed: WalletSeed) -> Arc<WalletHandle> {
        let mut wallets = self
            .wallets
            .lock()
            .expect("SharedDustSyncTask wallets mutex poisoned");
        // Dedupe: repeated `subscribe_wallet(same_seed)` would otherwise
        // double-apply every event batch to the same DustLocalState, breaking
        // the library's tree invariants (NonLinearInsertion on the second
        // application). Return a fresh handle that observes the existing
        // entry's status.
        if let Some(existing) = wallets.iter().find(|e| e.seed == seed) {
            return Arc::new(WalletHandle {
                seed,
                status_rx: existing.status_tx.subscribe(),
                context: self.context.clone(),
            });
        }
        let (tx, rx) = watch::channel(SyncStatus::Syncing);
        wallets.push(WalletEntry {
            seed: seed.clone(),
            status_tx: tx,
        });
        Arc::new(WalletHandle {
            seed,
            status_rx: rx,
            context: self.context.clone(),
        })
    }

    /// Spawn the event loop. Idempotent: subsequent calls are no-ops.
    pub fn start(self: &Arc<Self>) {
        let mut guard = self
            .task_handle
            .lock()
            .expect("SharedDustSyncTask task_handle mutex poisoned");
        if guard.is_some() {
            debug!(network_id = %self.network_id, "SharedDustSyncTask already started");
            return;
        }
        let this = self.clone();
        let handle = tokio::spawn(async move {
            if let Err(e) = this.clone().run().await {
                warn!(network_id = %this.network_id, error = %e, "SharedDustSyncTask exited with error");
                this.mark_all_failed(e);
            }
        });
        *guard = Some(handle);
        info!(network_id = %self.network_id, "SharedDustSyncTask started");
    }

    /// Signal the event loop to exit and await its completion.
    pub async fn stop(&self) {
        let _ = self.shutdown_tx.send(true);
        let handle = self
            .task_handle
            .lock()
            .expect("SharedDustSyncTask task_handle mutex poisoned")
            .take();
        if let Some(h) = handle {
            let _ = h.await;
        }
    }

    /// Top-level event loop. Opens a WS subscription, streams events, applies
    /// them in block-sized batches to every subscribed wallet, and maintains
    /// per-wallet readiness signals. Reconnects with a cursor on failure and
    /// exits cleanly on shutdown.
    async fn run(self: Arc<Self>) -> Result<(), String> {
        let mut last_applied_id: i64 = 0;
        let reconnect_backoff = Duration::from_secs(2);

        loop {
            if *self.shutdown_rx.borrow() {
                info!(network_id = %self.network_id, "SharedDustSyncTask shutdown requested");
                return Ok(());
            }
            match self.subscribe_and_process(last_applied_id).await {
                Ok(new_last) => {
                    // Clean exit from inner loop (shutdown path). Preserve
                    // cursor state in case the outer loop reconnects.
                    last_applied_id = new_last.max(last_applied_id);
                    return Ok(());
                }
                Err((e, high_water)) => {
                    last_applied_id = high_water.max(last_applied_id);
                    warn!(
                        network_id = %self.network_id,
                        error = %e,
                        last_applied_id,
                        "DUST subscription dropped; reconnecting"
                    );
                    tokio::time::sleep(reconnect_backoff).await;
                }
            }
        }
    }

    /// Inner subscription + processing loop. Returns `Ok(last_applied_id)` on
    /// clean shutdown, or `Err((reason, high_water))` on WS failure — the
    /// outer loop uses `high_water` as the resume cursor on reconnect.
    async fn subscribe_and_process(
        self: &Arc<Self>,
        resume_from: i64,
    ) -> Result<i64, (String, i64)> {
        use tokio_tungstenite::tungstenite;

        let ws_url = self.indexer_client.ws_url();
        let ws_request = tungstenite::http::Request::builder()
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
                tungstenite::handshake::client::generate_key(),
            )
            .body(())
            .map_err(|e| (format!("WS request build: {e}"), resume_from))?;

        let (ws_stream, _) = tokio_tungstenite::connect_async(ws_request)
            .await
            .map_err(|e| (format!("WS connect: {e}"), resume_from))?;
        let (mut write, mut read) = ws_stream.split();

        // Handshake: connection_init → wait for connection_ack.
        write
            .send(tungstenite::Message::Text(
                serde_json::json!({"type": "connection_init"})
                    .to_string()
                    .into(),
            ))
            .await
            .map_err(|e| (format!("WS init: {e}"), resume_from))?;

        let ack_timeout = Duration::from_secs(10);
        loop {
            match tokio::time::timeout(ack_timeout, read.next()).await {
                Ok(Some(Ok(tungstenite::Message::Text(text)))) => {
                    if text.contains("connection_ack") {
                        break;
                    }
                }
                Ok(Some(Ok(_))) => {}
                Ok(Some(Err(e))) => return Err((format!("WS ack: {e}"), resume_from)),
                Ok(None) | Err(_) => {
                    return Err(("WS ack timeout".into(), resume_from));
                }
            }
        }

        // Subscribe with optional cursor. Omit `$id` entirely on first
        // connect — preview testnet's v4 indexer treats missing-or-null `id`
        // as "stream all events from the start"; passing `id: 0` appears to
        // skip historical events and begin near chain tip, which breaks the
        // library's strict `NonLinearInsertion` tree invariants during
        // replay. On reconnect, pass the last applied id as the resume
        // cursor.
        let sub_payload = if resume_from > 0 {
            serde_json::json!({
                "id": "dust-shared",
                "type": "subscribe",
                "payload": {
                    "query": "subscription DustLedgerEvents($id: Int) { dustLedgerEvents(id: $id) { type: __typename id maxId protocolVersion raw } }",
                    "variables": { "id": resume_from },
                }
            })
        } else {
            serde_json::json!({
                "id": "dust-shared",
                "type": "subscribe",
                "payload": {
                    "query": "subscription { dustLedgerEvents { type: __typename id maxId protocolVersion raw } }",
                }
            })
        };
        write
            .send(tungstenite::Message::Text(sub_payload.to_string().into()))
            .await
            .map_err(|e| (format!("WS subscribe: {e}"), resume_from))?;

        // Block-aware batching: events from the same block arrive in a burst
        // (milliseconds apart); inter-block gaps are ~6s. Flush when a quiet
        // period of `batch_quiet` elapses or when we hit `batch_max_size`.
        // This matches the library's `replay_events` batch semantics, where
        // generation collapses are deferred to batch-end.
        let batch_quiet = Duration::from_millis(150);
        let batch_max_size: usize = 200;

        let mut batch: Vec<Event<DefaultDB>> = Vec::new();
        let mut batch_last_event_id: i64 = 0;
        let mut chain_max_id: i64 = 0;
        let mut last_applied_id: i64 = resume_from;

        let mut shutdown_rx = self.shutdown_rx.clone();

        // Anchor refresh is driven by Ready transitions in `try_mark_ready`
        // (fires exactly when `applied_id >= chain_max_id`), so the
        // `latest_block_context.tblock` the `pay_fees` path reads always
        // corresponds to the chain-block the wallet is synced to. A purely
        // time-based refresh would race the wallet's tree and advance the
        // anchor past where the wallet actually is.

        loop {
            // Exit path: shutdown signaled.
            if *shutdown_rx.borrow() {
                if !batch.is_empty() {
                    self.apply_batch(&batch);
                    last_applied_id = batch_last_event_id.max(last_applied_id);
                    batch.clear();
                }
                let _ = write.send(tungstenite::Message::Close(None)).await;
                return Ok(last_applied_id);
            }

            // Wait for either: next message, shutdown, or the batch quiet
            // timer expiring (if we're holding a partial batch).
            let recv_timeout = if batch.is_empty() {
                Duration::from_secs(60)
            } else {
                batch_quiet
            };

            tokio::select! {
                _ = shutdown_rx.changed() => continue,
                msg = tokio::time::timeout(recv_timeout, read.next()) => {
                    match msg {
                        Ok(Some(Ok(tungstenite::Message::Text(text)))) => {
                            match self.parse_event_message(text.as_ref()) {
                                ParsedMessage::NextEvent { event, id, max_id } => {
                                    batch.push(event);
                                    batch_last_event_id = id;
                                    if max_id > chain_max_id {
                                        chain_max_id = max_id;
                                    }
                                    if batch.len() >= batch_max_size {
                                        self.apply_batch(&batch);
                                        last_applied_id = batch_last_event_id.max(last_applied_id);
                                        batch.clear();
                                        self.try_mark_ready(last_applied_id, chain_max_id).await;
                                    }
                                }
                                ParsedMessage::Complete | ParsedMessage::Error => {
                                    // Server ended or errored the subscription — bail to
                                    // outer loop for a reconnect with cursor preserved.
                                    if !batch.is_empty() {
                                        self.apply_batch(&batch);
                                        last_applied_id = batch_last_event_id.max(last_applied_id);
                                        batch.clear();
                                    }
                                    return Err((
                                        "subscription complete/error from server".into(),
                                        last_applied_id,
                                    ));
                                }
                                ParsedMessage::Other => {}
                            }
                        }
                        Ok(Some(Ok(_))) => {}
                        Ok(Some(Err(e))) => {
                            if !batch.is_empty() {
                                self.apply_batch(&batch);
                                last_applied_id = batch_last_event_id.max(last_applied_id);
                                batch.clear();
                            }
                            return Err((format!("WS read: {e}"), last_applied_id));
                        }
                        Ok(None) => {
                            if !batch.is_empty() {
                                self.apply_batch(&batch);
                                last_applied_id = batch_last_event_id.max(last_applied_id);
                                batch.clear();
                            }
                            return Err(("WS stream closed".into(), last_applied_id));
                        }
                        Err(_) => {
                            // Quiet-period timeout hit. If we have a partial
                            // batch, flush it — this is the block-boundary
                            // semantic that keeps tree state correct.
                            if !batch.is_empty() {
                                self.apply_batch(&batch);
                                last_applied_id = batch_last_event_id.max(last_applied_id);
                                batch.clear();
                                self.try_mark_ready(last_applied_id, chain_max_id).await;
                            } else if chain_max_id > 0 && last_applied_id >= chain_max_id {
                                // No events pending and nothing new seen —
                                // we're up-to-date with the server's view.
                                self.try_mark_ready(last_applied_id, chain_max_id).await;
                            }
                        }
                    }
                }
            }
        }
    }

    /// Apply a batch of decoded events to every subscribed wallet.
    ///
    /// Each wallet's `DustLocalState::replay_events` filters by its own
    /// `DustSecretKey`, so the decode cost is shared but the replay cost is
    /// per-wallet. Within a batch, generation collapses are deferred to
    /// batch-end by the library (dust.rs:1691) — batching is what keeps the
    /// tree consistent with chain's block-end roots.
    fn apply_batch(&self, batch: &[Event<DefaultDB>]) {
        let wallets = self
            .wallets
            .lock()
            .expect("SharedDustSyncTask wallets mutex poisoned");
        let seeds: Vec<WalletSeed> = wallets.iter().map(|w| w.seed.clone()).collect();
        drop(wallets); // release before taking the context's inner locks

        for seed in seeds {
            self.context.with_wallet_from_seed(seed, |wallet| {
                if let Err(e) = wallet.update_dust_from_tx(batch) {
                    warn!(
                        network_id = %self.network_id,
                        error = ?e,
                        batch_len = batch.len(),
                        "DUST replay failed for wallet"
                    );
                }
            });
        }
        debug!(
            network_id = %self.network_id,
            batch_len = batch.len(),
            "Applied DUST batch to subscribed wallets"
        );
    }

    /// Refresh the shared `LedgerContext`'s `latest_block_context` by fetching
    /// the chain's current block from the indexer. The `pay_fees` path reads
    /// `latest_block_context().tblock` as the DUST spend's declared `ctime`,
    /// and chain-side validation does a `root_history.get(ctime)`
    /// predecessor-by-time lookup — so keeping this aligned with the chain
    /// block our wallet is synced to is what makes proofs verifiable.
    async fn refresh_anchor(&self) {
        match self.indexer_client.get_latest_block().await {
            Ok(Some(block)) => {
                if let Some(tblock_ms) = block.timestamp {
                    let tblock_secs = tblock_ms / 1000;
                    let block_context = make_block_context(
                        Timestamp::from_secs(tblock_secs),
                        HashOutput::default(),
                        Timestamp::from_secs(tblock_secs.saturating_sub(6)),
                    );
                    let empty_txs: Vec<SerdeTransaction<Signature, ProofMarker, DefaultDB>> =
                        Vec::new();
                    self.context
                        .update_from_block(&empty_txs, &block_context, None, None);
                    debug!(
                        network_id = %self.network_id,
                        tblock = tblock_secs,
                        "latest_block_context refreshed"
                    );
                }
            }
            Ok(None) => {}
            Err(e) => {
                debug!(
                    network_id = %self.network_id,
                    error = %e,
                    "anchor refresh failed — will retry on next tick"
                );
            }
        }
    }

    /// Async version of [`maybe_mark_ready`] that also refreshes the chain
    /// anchor before transitioning to Ready. Call this whenever a batch has
    /// been applied and the cursor may have caught up to the chain's max —
    /// anchoring right before Ready ensures the `latest_block_context.tblock`
    /// the `pay_fees` path reads is the chain-block the wallet is synced to
    /// (not a periodic-timer snapshot that may drift past the wallet state).
    async fn try_mark_ready(&self, applied_id: i64, chain_max_id: i64) {
        if chain_max_id == 0 || applied_id < chain_max_id {
            return;
        }
        // Skip the anchor-refresh + tree-sync work if the cursor hasn't
        // advanced since the last ready tick — otherwise we serialize the
        // entire `LedgerState` (MB-scale) on every inter-block quiet period,
        // producing tens of pointless "Synced DUST trees" log lines per
        // minute.
        {
            let mut last = self
                .last_synced_applied_id
                .lock()
                .expect("last_synced_applied_id poisoned");
            if applied_id <= *last {
                return;
            }
            *last = applied_id;
        }
        // Refresh the anchor SYNCHRONOUSLY with the Ready transition. If this
        // fails (indexer blip), we keep any previous anchor rather than
        // silently advancing — the tx path can still try with the existing
        // tblock, and the next successful refresh will close the gap.
        self.refresh_anchor().await;
        // Snapshot wallet trees → LedgerState so the local well-formed check
        // (and by extension the `speculative_spend` + `pay_fees` path) uses
        // the same tree state we just caught up to. Uses the first wallet's
        // seed — every wallet's DustLocalState holds the same chain-wide
        // tree, so any seed works.
        let seed = {
            self.wallets
                .lock()
                .expect("SharedDustSyncTask wallets mutex poisoned")
                .first()
                .map(|e| e.seed.clone())
        };
        if let Some(seed) = seed {
            if let Err(e) = crate::services::sync::midnight::handler::ledger_context::sync_dust_trees_to_ledger_ctx(
                &self.context,
                &seed,
            ) {
                warn!(
                    network_id = %self.network_id,
                    error = %e,
                    "failed to sync DUST trees to LedgerState"
                );
            }
        }
        self.maybe_mark_ready(applied_id, chain_max_id);
    }

    /// Transition every wallet to `Ready` if our applied cursor has caught up
    /// to the chain's latest reported event id. Idempotent: sending the same
    /// status twice is a no-op from `watch`'s perspective.
    fn maybe_mark_ready(&self, applied_id: i64, chain_max_id: i64) {
        if chain_max_id == 0 || applied_id < chain_max_id {
            return;
        }
        let wallets = self
            .wallets
            .lock()
            .expect("SharedDustSyncTask wallets mutex poisoned");
        let mut transitioned = 0u32;
        for entry in wallets.iter() {
            let current = entry.status_tx.borrow().clone();
            if !matches!(current, SyncStatus::Ready) {
                let _ = entry.status_tx.send(SyncStatus::Ready);
                transitioned += 1;
            }
        }
        if transitioned > 0 {
            info!(
                network_id = %self.network_id,
                applied_id,
                chain_max_id,
                transitioned,
                "Wallets transitioned Syncing → Ready"
            );
        }
    }

    /// Parse a single `graphql-transport-ws` message into an actionable event.
    fn parse_event_message(&self, text: &str) -> ParsedMessage {
        let Ok(val) = serde_json::from_str::<serde_json::Value>(text) else {
            return ParsedMessage::Other;
        };
        let msg_type = val.get("type").and_then(|t| t.as_str()).unwrap_or("");
        match msg_type {
            "next" => {
                let Some(ev) = val
                    .get("payload")
                    .and_then(|p| p.get("data"))
                    .and_then(|d| d.get("dustLedgerEvents"))
                else {
                    return ParsedMessage::Other;
                };
                let Some(raw) = ev.get("raw").and_then(|r| r.as_str()) else {
                    return ParsedMessage::Other;
                };
                let id = ev.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
                let max_id = ev.get("maxId").and_then(|v| v.as_i64()).unwrap_or(0);
                let Ok(bytes) = hex::decode(raw.trim_start_matches("0x")) else {
                    return ParsedMessage::Other;
                };
                match midnight_node_ledger_helpers::deserialize::<Event<DefaultDB>, _>(
                    &mut &bytes[..],
                ) {
                    Ok(event) => ParsedMessage::NextEvent { event, id, max_id },
                    Err(_) => ParsedMessage::Other,
                }
            }
            "complete" => ParsedMessage::Complete,
            "error" => ParsedMessage::Error,
            _ => ParsedMessage::Other,
        }
    }

    fn mark_all_failed(&self, reason: String) {
        let wallets = self
            .wallets
            .lock()
            .expect("SharedDustSyncTask wallets mutex poisoned");
        for entry in wallets.iter() {
            let _ = entry.status_tx.send(SyncStatus::Failed(reason.clone()));
        }
    }
}

/// Minimal parse result for a single `graphql-transport-ws` frame.
enum ParsedMessage {
    NextEvent {
        event: Event<DefaultDB>,
        id: i64,
        max_id: i64,
    },
    Complete,
    Error,
    Other,
}

// ---------------------------------------------------------------------------
// Per-network registry
// ---------------------------------------------------------------------------

/// A sync task + context tied to a registry key. Two flavors inhabit the
/// registry:
///
/// - **Shared slot** (key = `network_id`): built at boot with all known
///   midnight-relayer seeds on that network. One WS subscription, N wallets,
///   O(1) subscription cost regardless of N.
/// - **Isolated slot** (key = `{network_id}#runtime:{seed_debug}`): built
///   lazily when a relayer is CRUD-added at runtime with a seed the shared
///   slot doesn't know about. One WS subscription for exactly one wallet.
///   Works around the library's lack of a public `LedgerContext::add_wallet`
///   without rebuilding the shared slot and disrupting existing relayers.
///
/// On process restart, boot-time enumeration folds every configured relayer
/// back into the shared slot — isolated slots are a migration bridge, not a
/// permanent distinct class. Both share the same struct shape; callers don't
/// need to distinguish them.
pub struct NetworkSyncSlot {
    /// The *chain* network id (e.g. `"preview"`). Used by
    /// `LedgerContext::new_from_wallet_seeds` for network validation. Shared
    /// slots and their isolated siblings share this value.
    pub network_id: String,
    pub context: Arc<LedgerContext<DefaultDB>>,
    pub task: Arc<SharedDustSyncTask>,
    /// Seeds inserted at construction. Use `contains_seed` to check
    /// membership; direct access isn't exposed so the registry stays in
    /// charge of seed-to-slot resolution.
    seeds: Vec<WalletSeed>,
}

impl NetworkSyncSlot {
    /// Whether this slot was constructed with the given seed. Shared slots
    /// include every midnight-relayer seed known at boot; isolated slots
    /// include exactly one.
    pub fn contains_seed(&self, seed: &WalletSeed) -> bool {
        self.seeds.contains(seed)
    }

    /// Number of seeds registered with this slot — diagnostic use only.
    pub fn seed_count(&self) -> usize {
        self.seeds.len()
    }
}

/// Process-wide registry mapping `network_id → NetworkSyncSlot`. Written once
/// per network at boot via [`init_network_sync`], read from many places
/// (factory, tx handlers) via [`get_network_sync`].
///
/// Using a `RwLock<HashMap>` rather than `OnceLock<HashMap>` so CRUD-style
/// runtime additions for brand-new networks remain possible; adding a new
/// relayer on an EXISTING network still needs the library-patch work flagged
/// in [`NetworkSyncSlot::seeds`].
fn registry() -> &'static RwLock<HashMap<String, Arc<NetworkSyncSlot>>> {
    static REGISTRY: OnceLock<RwLock<HashMap<String, Arc<NetworkSyncSlot>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Registry key for an isolated (runtime-registered) relayer's sync slot.
///
/// Uses a stable hash of the seed rather than its `Debug` representation — the
/// latter would tie our storage key to the library's Debug impl, which can
/// change between versions. Hashing with the standard `Hasher` keeps keys
/// stable as long as `WalletSeed` implements `Hash` (which it does).
fn isolated_registry_key(network_id: &str, seed: &WalletSeed) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    seed.hash(&mut h);
    format!("{network_id}#runtime:{:016x}", h.finish())
}

/// Core constructor shared by [`init_network_sync`] (boot-time shared) and
/// [`register_runtime_relayer`] (CRUD-time isolated). Accepts an explicit
/// `registry_key` distinct from `network_id`: the former is our bookkeeping
/// identity, the latter is the chain-network identity that the library's
/// `LedgerContext::new_from_wallet_seeds` uses for network validation.
///
/// Idempotent: returns an existing entry if `registry_key` is already
/// present. `seeds` / `indexer_client` are ignored in that case.
fn init_slot(
    registry_key: String,
    network_id: String,
    seeds: Vec<WalletSeed>,
    indexer_client: MidnightIndexerClient,
) -> Arc<NetworkSyncSlot> {
    {
        let read = registry().read().expect("registry lock poisoned");
        if let Some(existing) = read.get(&registry_key) {
            return existing.clone();
        }
    }

    let mut write = registry().write().expect("registry lock poisoned");
    // Race: another thread may have inserted while we released the read lock.
    if let Some(existing) = write.get(&registry_key) {
        return existing.clone();
    }

    let context = Arc::new(LedgerContext::new_from_wallet_seeds(
        network_id.clone(),
        &seeds,
    ));
    let task = SharedDustSyncTask::new(network_id.clone(), indexer_client, context.clone());
    // Pre-subscribe every seed BEFORE `task.start()` — see comment in event
    // loop: the opening burst of historical events would no-op otherwise, and
    // wallets would be stuck at tree-first-free=0 while events arrive with
    // mt_index far in the future, producing perpetual NonLinearInsertion.
    for seed in &seeds {
        let _handle = task.subscribe_wallet(seed.clone());
    }
    task.start();

    let slot = Arc::new(NetworkSyncSlot {
        network_id,
        context,
        task,
        seeds,
    });
    write.insert(registry_key.clone(), slot.clone());
    info!(
        registry_key = %registry_key,
        seed_count = slot.seed_count(),
        "Registered DUST sync slot"
    );
    slot
}

/// Initialize (or return existing) **shared** per-network sync state. Called
/// at process boot from [`crate::bootstrap::initialize_midnight_shared_sync`]
/// with the full set of known relayer seeds on the network.
///
/// Registry key is the plain `network_id`. Every midnight relayer configured
/// at boot resolves to this slot via [`get_slot_for_seed`] when its seed is
/// in [`NetworkSyncSlot::seeds`].
pub fn init_network_sync(
    network_id: &str,
    seeds: Vec<WalletSeed>,
    indexer_client: MidnightIndexerClient,
) -> Arc<NetworkSyncSlot> {
    init_slot(
        network_id.to_string(),
        network_id.to_string(),
        seeds,
        indexer_client,
    )
}

/// Register a single runtime-added midnight relayer in its own isolated sync
/// slot. Used by the factory path when the shared slot's seed list doesn't
/// include `seed` (e.g. a relayer created via CRUD after boot).
///
/// Idempotent: repeated calls for the same `(network_id, seed)` return the
/// same slot. The slot starts its own WS subscription from `id=0` and
/// catches up the single wallet independently — existing relayers on the
/// shared slot are completely unaffected.
pub fn register_runtime_relayer(
    network_id: &str,
    seed: WalletSeed,
    indexer_client: MidnightIndexerClient,
) -> Arc<NetworkSyncSlot> {
    let key = isolated_registry_key(network_id, &seed);
    init_slot(key, network_id.to_string(), vec![seed], indexer_client)
}

/// Shared-slot accessor. Returns `None` until [`init_network_sync`] has run.
/// Prefer [`get_slot_for_seed`] in code paths that have a specific seed in
/// hand — it also handles isolated/runtime-registered relayers.
pub fn get_network_sync(network_id: &str) -> Option<Arc<NetworkSyncSlot>> {
    registry()
        .read()
        .expect("registry lock poisoned")
        .get(network_id)
        .cloned()
}

/// Resolve the correct sync slot for `(network_id, seed)`.
///
/// Returns the shared slot if it exists AND its seed list contains the given
/// seed. Otherwise returns the isolated slot for that seed, if one was
/// registered. Returns `None` when neither exists — the caller should fall
/// back to a non-shared `LedgerContextManager` or register the relayer at
/// runtime via [`register_runtime_relayer`] (or, more commonly, use
/// [`get_or_register_slot`] which handles that fallthrough).
pub fn get_slot_for_seed(network_id: &str, seed: &WalletSeed) -> Option<Arc<NetworkSyncSlot>> {
    let read = registry().read().expect("registry lock poisoned");
    if let Some(slot) = read.get(network_id) {
        if slot.contains_seed(seed) {
            return Some(slot.clone());
        }
    }
    read.get(&isolated_registry_key(network_id, seed)).cloned()
}

/// The canonical factory-side dispatch: returns the correct sync slot for
/// `(network_id, seed)`, registering an isolated runtime slot if neither the
/// shared nor the isolated variant exists yet.
///
/// Both the relayer factory and the transaction factory need this exact
/// pattern — "find my slot or register one" — so it lives here rather than
/// being duplicated at each call site.
pub fn get_or_register_slot(
    network_id: &str,
    seed: WalletSeed,
    indexer_client: MidnightIndexerClient,
) -> Arc<NetworkSyncSlot> {
    if let Some(existing) = get_slot_for_seed(network_id, &seed) {
        return existing;
    }
    info!(
        network_id,
        "seed not known to shared slot — registering isolated runtime sync for this relayer"
    );
    register_runtime_relayer(network_id, seed, indexer_client)
}

/// Stop a registered slot and drop it from the registry. `registry_key` is
/// the key used at init — `network_id` for shared slots, or the value
/// returned by [`isolated_registry_key`] for runtime-registered ones. Used
/// in tests and shutdown paths.
pub async fn shutdown_network_sync(registry_key: &str) {
    let slot = {
        let mut write = registry().write().expect("registry lock poisoned");
        write.remove(registry_key)
    };
    if let Some(slot) = slot {
        slot.task.stop().await;
    }
}
