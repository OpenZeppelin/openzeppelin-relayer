use std::sync::{Arc, Mutex};

use crate::{
    repositories::{InMemorySyncState, SyncStateTrait},
    services::midnight::{
        handler::{
            EventDispatcher, ProgressTracker, SyncConfig, SyncEvent, SyncEventHandler, SyncStrategy,
        },
        indexer::{ApplyStage, MidnightIndexerClient},
        utils::{derive_viewing_key, parse_collapsed_update, process_transaction},
        SyncError,
    },
};

use log::{debug, info, warn};
use midnight_ledger_prototype::transient_crypto::merkle_tree::MerkleTreeCollapsedUpdate;
use midnight_node_ledger_helpers::{
    mn_ledger_serialize::{deserialize, serialize},
    DefaultDB, LedgerContext, LedgerState, NetworkId, Proof, Transaction, WalletSeed, WalletState,
};

/// Enum to track updates in chronological order during wallet synchronization.
///
/// This enum is used to buffer and order both transaction and Merkle tree updates
/// as they are received from the indexer, ensuring correct application order.
///
/// - `Transaction`: Represents a transaction update with its index, transaction data, and apply stage.
/// - `MerkleUpdate`: Represents a Merkle tree update with its index and update data.
#[derive(Clone)]
enum ChronologicalUpdate {
    Transaction {
        index: u64,
        tx: Box<Transaction<Proof, DefaultDB>>,
        apply_stage: Option<ApplyStage>,
    },
    MerkleUpdate {
        index: u64,
        update: Box<MerkleTreeCollapsedUpdate>,
    },
}

struct EventHandler {
    network: NetworkId,
    updates_buffer: Arc<Mutex<Vec<ChronologicalUpdate>>>,
}

impl EventHandler {
    fn new(network: NetworkId, updates_buffer: Arc<Mutex<Vec<ChronologicalUpdate>>>) -> Self {
        Self {
            network,
            updates_buffer,
        }
    }
}

#[async_trait::async_trait]
impl SyncEventHandler for EventHandler {
    fn name(&self) -> &'static str {
        "EventHandler"
    }

    async fn handle(&mut self, event: &SyncEvent) -> Result<(), SyncError> {
        match event {
            SyncEvent::TransactionReceived {
                blockchain_index,
                transaction_data,
            } => {
                // Process transaction and buffer it (don't apply yet)
                if let Some(tx) = process_transaction(transaction_data, self.network)? {
                    // Buffer the transaction for later application with its apply stage
                    self.updates_buffer
                        .lock()
                        .unwrap()
                        .push(ChronologicalUpdate::Transaction {
                            index: *blockchain_index,
                            tx: Box::new(tx),
                            apply_stage: transaction_data.apply_stage.clone(),
                        });
                }
            }
            SyncEvent::MerkleUpdateReceived {
                update_info,
                blockchain_index,
            } => {
                // Process and buffer merkle update (don't apply yet)
                let update = parse_collapsed_update(update_info, self.network)?;

                // Buffer the update for later application
                self.updates_buffer
                    .lock()
                    .unwrap()
                    .push(ChronologicalUpdate::MerkleUpdate {
                        index: *blockchain_index,
                        update: Box::new(update),
                    });
            }
            SyncEvent::SyncCompleted => {
                // Sync completed, no additional processing needed
            }
            _ => {}
        }
        Ok(())
    }
}

/// Main sync manager that coordinates all sync components.
///
/// This struct is the entry point for wallet synchronization. It manages the lifecycle of the sync
/// process, wires together all services, and ensures that events are handled and state is updated
/// correctly. It is responsible for selecting the sync strategy, managing persistence, and tracking progress.
pub struct SyncManager<S: SyncStrategy> {
    // The ledger context
    // We use an Arc to allow for multiple references to the same context
    context: Arc<LedgerContext<DefaultDB>>,
    // The wallet seed
    seed: WalletSeed,
    // The sync strategy
    strategy: S,
    // The network
    network: NetworkId,
    // The sync state store
    sync_state_store: Arc<InMemorySyncState>,
    // The relayer ID for tracking sync state
    relayer_id: String,
}

impl<S: SyncStrategy + Sync + Send> SyncManager<S> {
    /// Serialize the current ledger context to bytes
    fn serialize_context(&self) -> Result<Vec<u8>, SyncError> {
        // Serialize the wallet state for the current seed
        let wallet_state = {
            let wallets_guard = self
                .context
                .wallets
                .lock()
                .map_err(|e| SyncError::SyncError(format!("Failed to lock wallets: {}", e)))?;

            wallets_guard
                .get(&self.seed)
                .map(|wallet| {
                    // We only serialize the wallet state, not the secret keys
                    let mut state_bytes = Vec::new();
                    serialize(&wallet.state, &mut state_bytes, self.network).map_err(|e| {
                        SyncError::SyncError(format!("Failed to serialize wallet state: {:?}", e))
                    })?;
                    Ok::<Vec<u8>, SyncError>(state_bytes)
                })
                .transpose()?
        };

        // Serialize the ledger state
        let ledger_state_bytes = {
            let ledger_state_guard =
                self.context.ledger_state.lock().map_err(|e| {
                    SyncError::SyncError(format!("Failed to lock ledger state: {}", e))
                })?;

            let mut bytes = Vec::new();
            serialize(&*ledger_state_guard, &mut bytes, self.network).map_err(|e| {
                SyncError::SyncError(format!("Failed to serialize ledger state: {:?}", e))
            })?;
            bytes
        };

        // Combine wallet state and ledger state into a single serialized context
        bincode::serialize(&(wallet_state, ledger_state_bytes))
            .map_err(|e| SyncError::SyncError(format!("Failed to serialize context: {}", e)))
    }

    /// Restore the ledger context from serialized bytes
    fn restore_context(&self, serialized_context: &[u8]) -> Result<(), SyncError> {
        // Deserialize the combined context
        match bincode::deserialize::<(Option<Vec<u8>>, Vec<u8>)>(serialized_context) {
            Ok((wallet_state_bytes, ledger_state_bytes)) => {
                // Restore ledger state
                let mut reader = &ledger_state_bytes[..];
                match deserialize::<LedgerState<DefaultDB>, _>(&mut reader, self.network) {
                    Ok(ledger_state) => {
                        if let Ok(mut ledger_state_guard) = self.context.ledger_state.lock() {
                            *ledger_state_guard = ledger_state;
                            debug!("Successfully restored ledger state");
                        }
                    }
                    Err(e) => {
                        warn!(
                            "Failed to deserialize ledger state: {:?}, starting fresh",
                            e
                        );
                    }
                }

                // Restore wallet state if available
                if let Some(state_bytes) = wallet_state_bytes {
                    let mut reader = &state_bytes[..];
                    match deserialize::<WalletState<DefaultDB>, _>(&mut reader, self.network) {
                        Ok(wallet_state) => {
                            if let Ok(mut wallets_guard) = self.context.wallets.lock() {
                                if let Some(wallet) = wallets_guard.get_mut(&self.seed) {
                                    wallet.update_state(wallet_state);
                                    debug!("Successfully restored wallet state");
                                }
                            }
                        }
                        Err(e) => {
                            warn!(
                                "Failed to deserialize wallet state: {:?}, starting fresh",
                                e
                            );
                        }
                    }
                }
                Ok(())
            }
            Err(e) => {
                warn!("Failed to deserialize context: {}, starting fresh", e);
                Ok(())
            }
        }
    }

    /// Create a new manager with the specified sync strategy and configuration.
    ///
    /// This method initializes all services and selects the sync strategy based on the provided options.
    /// It will also attempt to restore the ledger context from a previous sync if available.
    pub fn new(
        indexer_client: &MidnightIndexerClient,
        seed: &WalletSeed,
        network: NetworkId,
        sync_state_store: Arc<InMemorySyncState>,
        relayer_id: String,
    ) -> Result<Self, SyncError> {
        // Temporarily add random destination seed to the context until Midnight fixes this
        // mn_shield-addr_test1cx5yug2suxqec6pzzfgwrg90crcvlk6ktlvg52eczkrw426suglqxqzl5ad0jpxv4mtdc0kpyswfjdjjqs8zh0fu0kupaha382r3py8wwqm03l5k
        // TODO: Remove this once Midnight fixes this
        let destination_seed =
            WalletSeed::from("8e0622a9987a7bef7b6a1417c693172b79e75f2308fe3ae9cc897f6108e3a067");

        let context = Arc::new(LedgerContext::new_from_wallet_seeds(&[
            *seed,
            destination_seed,
        ]));

        let wallet = context.wallet_from_seed(*seed);
        let viewing_key = derive_viewing_key(&wallet, network)?;

        let config = SyncConfig {
            viewing_key: Some(viewing_key),
            ..SyncConfig::default()
        };

        // Create sync strategy
        let strategy = S::new(indexer_client, Some(config));

        let manager = Self {
            context,
            seed: *seed,
            strategy,
            network,
            sync_state_store,
            relayer_id,
        };

        // Try to restore the context from saved state
        if let Ok(Some(serialized_context)) = manager
            .sync_state_store
            .get_ledger_context(&manager.relayer_id)
        {
            info!(
                "Found saved ledger context for relayer {}, attempting to restore",
                manager.relayer_id
            );

            if let Err(e) = manager.restore_context(&serialized_context) {
                warn!("Failed to restore context: {}", e);
            }
        }

        Ok(manager)
    }

    /// Start synchronization.
    ///
    /// This method runs the sync strategy, dispatches events, buffers updates, and applies them in order.
    /// It also manages state persistence and progress tracking.
    /// If start_index is None, it will read from the sync state store.
    pub async fn sync(&mut self, start_index: Option<u64>) -> Result<(), SyncError> {
        // Determine the starting index
        let start_index = match start_index {
            Some(index) => index,
            None => {
                // Read from sync state store
                self.sync_state_store
                    .get_last_synced_index(&self.relayer_id)
                    .map_err(|e| SyncError::SyncError(format!("Failed to get sync state: {}", e)))?
                    .unwrap_or(0)
            }
        };

        info!(
            "Starting wallet synchronization from blockchain index: {} for relayer: {}",
            start_index, self.relayer_id
        );

        // Create progress tracker
        let mut progress_tracker = ProgressTracker::new(start_index);

        // Create shared buffer for chronological updates
        let updates_buffer = Arc::new(Mutex::new(Vec::<ChronologicalUpdate>::new()));

        // Execute sync strategy with a custom event handler
        let event_handler = EventHandler::new(self.network, updates_buffer.clone());

        let mut event_dispatcher = EventDispatcher::new();
        event_dispatcher.register_handler(Box::new(event_handler));

        self.strategy
            .sync(start_index, &mut event_dispatcher, &mut progress_tracker)
            .await?;

        // Apply all buffered updates in chronological order
        debug!("Applying buffered updates in chronological order");

        let mut updates = updates_buffer.lock().unwrap().clone();
        // Sort by blockchain index to ensure chronological order
        updates.sort_by_key(|update| match update {
            ChronologicalUpdate::Transaction { index, .. } => *index,
            ChronologicalUpdate::MerkleUpdate { index, .. } => *index,
        });

        info!("Applying {} updates in chronological order", updates.len());

        let mut last_blockchain_index = start_index;

        for update in updates {
            match update {
                ChronologicalUpdate::MerkleUpdate { index, update } => {
                    info!("Applying merkle update at index {}", index);
                    last_blockchain_index = index;

                    let mut wallets_guard = self.context.wallets.lock().unwrap();

                    let wallet = wallets_guard.get_mut(&self.seed).ok_or_else(|| {
                        SyncError::MerkleTreeUpdateError("Wallet not found in context".to_string())
                    })?;

                    match wallet.state.apply_collapsed_update(&update) {
                        Ok(new_state) => {
                            debug!(
									"Applied collapsed update: start={}, end={}, new first_free={} (was {})",
									update.start,
									update.end,
									new_state.first_free,
									wallet.state.first_free
								);
                            wallet.update_state(new_state);
                        }
                        Err(e) => {
                            return Err(SyncError::MerkleTreeUpdateError(format!(
                                "Failed to apply collapsed update to wallet state: {}",
                                e
                            )));
                        }
                    }
                }
                ChronologicalUpdate::Transaction {
                    index,
                    tx,
                    apply_stage,
                } => {
                    last_blockchain_index = index;

                    // Only apply transactions that succeeded
                    let should_apply = match &apply_stage {
                        Some(stage) => stage.should_apply(),
                        None => true, // We have to be careful here, it may potentially apply transactions that failed
                    };

                    if should_apply {
                        debug!("Applying transaction at index {}", index);
                        self.context.update_from_txs(&[*tx]);
                    }
                }
            }
        }

        // Save the last synced blockchain index and serialize the context
        if last_blockchain_index > start_index {
            // Serialize the current context
            let context_bytes = self.serialize_context()?;

            // Save both the index and the serialized context
            self.sync_state_store
                .set_sync_state(&self.relayer_id, last_blockchain_index, Some(context_bytes))
                .map_err(|e| SyncError::SyncError(format!("Failed to save sync state: {}", e)))?;

            info!(
                "Updated sync state to blockchain index {} for relayer {}",
                last_blockchain_index, self.relayer_id
            );
        }

        info!("Wallet synchronization completed successfully");
        Ok(())
    }

    pub fn get_context(&self) -> Arc<LedgerContext<DefaultDB>> {
        Arc::clone(&self.context)
    }

    /// Convenience method to sync from the last stored height
    pub async fn sync_incremental(&mut self) -> Result<(), SyncError> {
        self.sync(None).await
    }
}

#[async_trait::async_trait]
impl<S: SyncStrategy + Sync + Send> super::SyncManagerTrait for SyncManager<S> {
    async fn sync(&mut self, start_index: u64) -> Result<(), SyncError> {
        self.sync(Some(start_index)).await
    }

    fn get_context(&self) -> Arc<LedgerContext<DefaultDB>> {
        self.get_context()
    }
}
