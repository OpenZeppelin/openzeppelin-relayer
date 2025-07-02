use std::sync::{Arc, Mutex};

use crate::services::midnight::{
    handler::{
        EventDispatcher, ProgressTracker, SyncConfig, SyncEvent, SyncEventHandler, SyncStrategy,
    },
    indexer::{ApplyStage, MidnightIndexerClient},
    utils::{derive_viewing_key, parse_collapsed_update, process_transaction},
    SyncError,
};

use log::{debug, info};
use midnight_ledger_prototype::transient_crypto::merkle_tree::MerkleTreeCollapsedUpdate;
use midnight_node_ledger_helpers::{
    DefaultDB, LedgerContext, NetworkId, Proof, Transaction, WalletSeed,
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
}

impl<S: SyncStrategy + Sync + Send> SyncManager<S> {
    /// Create a new manager with the specified sync strategy and configuration.
    ///
    /// This method initializes all services and selects the sync strategy based on the provided options.
    pub fn new(
        indexer_client: &MidnightIndexerClient,
        seed: &WalletSeed,
        network: NetworkId,
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

        Ok(Self {
            context,
            seed: *seed,
            strategy,
            network,
        })
    }

    /// Start synchronization.
    ///
    /// This method runs the sync strategy, dispatches events, buffers updates, and applies them in order.
    /// It also manages state persistence and progress tracking.
    pub async fn sync(&mut self, start_height: u64) -> Result<(), SyncError> {
        info!("Starting wallet synchronization");

        // Create progress tracker
        let mut progress_tracker = ProgressTracker::new(start_height);

        // Create shared buffer for chronological updates
        let updates_buffer = Arc::new(Mutex::new(Vec::<ChronologicalUpdate>::new()));

        // Execute sync strategy with a custom event handler
        let event_handler = EventHandler::new(self.network, updates_buffer.clone());

        let mut event_dispatcher = EventDispatcher::new();
        event_dispatcher.register_handler(Box::new(event_handler));

        self.strategy
            .sync(start_height, &mut event_dispatcher, &mut progress_tracker)
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

        for update in updates {
            match update {
                ChronologicalUpdate::MerkleUpdate { index, update } => {
                    info!("Applying merkle update at index {}", index);

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

        info!("Wallet synchronization completed successfully");
        Ok(())
    }

    pub fn get_context(&self) -> Arc<LedgerContext<DefaultDB>> {
        Arc::clone(&self.context)
    }
}

#[async_trait::async_trait]
impl<S: SyncStrategy + Sync + Send> super::SyncManagerTrait for SyncManager<S> {
    async fn sync(&mut self, start_height: u64) -> Result<(), SyncError> {
        self.sync(start_height).await
    }

    fn get_context(&self) -> Arc<LedgerContext<DefaultDB>> {
        self.get_context()
    }
}
