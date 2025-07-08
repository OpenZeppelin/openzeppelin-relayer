//! Event system for wallet synchronization.
//!
//! This module defines the core event types, event handler traits, and the event dispatcher used
//! throughout the wallet sync process. Events are used to decouple the sync logic from the
//! processing of transactions, Merkle updates, progress, and errors. Sync strategies and services
//! emit events, which are then handled by registered event handlers. This enables flexible
//! composition and extension of sync behavior.
//!
//! The event system is central to the orchestration of wallet sync, allowing for modular and
//! testable components.

use crate::services::midnight::{
    indexer::{
        CollapsedUpdateInfo, IndexerError, TransactionData, WalletSyncEvent as IndexerEvent,
        ZswapChainStateUpdate,
    },
    utils::{parse_collapsed_update, process_transaction},
    SyncError,
};

use log::error;
use midnight_ledger_prototype::transient_crypto::merkle_tree::MerkleTreeCollapsedUpdate;
use midnight_node_ledger_helpers::{DefaultDB, NetworkId, Proof, Transaction};
use std::sync::{Arc, Mutex};

use crate::services::midnight::indexer::ApplyStage;

/// Enum to track updates in chronological order during wallet synchronization.
///
/// This enum is used to buffer and order both transaction and Merkle tree updates
/// as they are received from the indexer, ensuring correct application order.
///
/// - `Transaction`: Represents a transaction update with its index, transaction data, and apply stage.
/// - `MerkleUpdate`: Represents a Merkle tree update with its index and update data.
#[derive(Clone)]
pub enum ChronologicalUpdate {
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

/// Events that occur during wallet synchronization
pub enum SyncEvent {
    /// A relevant transaction was received
    TransactionReceived {
        blockchain_index: u64,
        transaction_data: TransactionData,
    },
    /// A Merkle tree update was received
    MerkleUpdateReceived {
        blockchain_index: u64,
        update_info: CollapsedUpdateInfo,
    },
    /// Progress update from the indexer
    ProgressUpdate {
        highest_index: u64,
        highest_relevant_wallet_index: u64,
    },
    /// Sync has completed
    SyncCompleted,
    /// An error occurred during sync
    SyncError { error: IndexerError },
}

/// Trait for handling sync events.
///
/// Implementors receive all sync events and can perform side effects or state updates.
#[async_trait::async_trait]
pub trait SyncEventHandler: Send + Sync {
    /// Handle a sync event.
    ///
    /// This method is called for every event dispatched by the orchestrator or sync strategy.
    async fn handle(&mut self, event: &SyncEvent) -> Result<(), SyncError>;

    /// Get the name of this handler for logging and diagnostics.
    fn name(&self) -> &'static str;
}

/// Enum representing all possible event handlers
pub enum EventHandlerType {
    /// The main event handler that buffers updates
    EventHandler {
        network: NetworkId,
        updates_buffer: Arc<Mutex<Vec<ChronologicalUpdate>>>,
    },
    // Add more handler variants as needed in the future
}

impl EventHandlerType {
    /// Handle a sync event based on the handler type
    async fn handle(&mut self, event: &SyncEvent) -> Result<(), SyncError> {
        match self {
            EventHandlerType::EventHandler {
                network,
                updates_buffer,
            } => {
                match event {
                    SyncEvent::TransactionReceived {
                        blockchain_index,
                        transaction_data,
                    } => {
                        // Process transaction and buffer it (don't apply yet)
                        if let Some(tx) = process_transaction(transaction_data, *network)? {
                            // Buffer the transaction for later application with its apply stage
                            updates_buffer
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
                        let update = parse_collapsed_update(update_info, *network)?;

                        // Buffer the update for later application
                        updates_buffer
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
    }

    /// Get the name of this handler for logging and diagnostics
    fn name(&self) -> &'static str {
        match self {
            EventHandlerType::EventHandler { .. } => "EventHandler",
        }
    }
}

/// Event dispatcher that manages multiple event handlers.
///
/// The dispatcher allows multiple handlers to be registered and ensures all are called for each event.
/// This enables logging, state updates, and persistence to be handled independently.
#[derive(Default)]
pub struct EventDispatcher {
    handlers: Vec<EventHandlerType>,
}

impl EventDispatcher {
    /// Create a new, empty event dispatcher.
    pub fn new() -> Self {
        Default::default()
    }

    /// Register a new event handler.
    ///
    /// Handlers are called in the order they are registered.
    pub fn register_handler(&mut self, handler: EventHandlerType) {
        self.handlers.push(handler);
    }

    /// Dispatch an event to all registered handlers.
    ///
    /// Errors from handlers are logged, but do not stop other handlers from running.
    pub async fn dispatch(&mut self, event: &SyncEvent) -> Result<(), SyncError> {
        for handler in &mut self.handlers {
            if let Err(e) = handler.handle(event).await {
                error!("Handler {} failed to process event: {}", handler.name(), e);
                // Continue processing with other handlers
            }
        }
        Ok(())
    }
}

/// Convert indexer events to sync events.
///
/// This function translates low-level indexer events into one or more high-level sync events,
/// ensuring Merkle updates are processed before transactions for consistency.
pub fn convert_indexer_event(event: IndexerEvent) -> Vec<SyncEvent> {
    let mut sync_events = Vec::new();

    match event {
        IndexerEvent::ViewingUpdate {
            type_name: _,
            index,
            update,
        } => {
            // IMPORTANT: Process merkle updates FIRST, then transactions
            // This ensures the merkle tree state is correct before transactions are applied

            // First, collect all merkle updates
            for update_item in &update {
                if let ZswapChainStateUpdate::MerkleTreeCollapsedUpdate {
                    protocol_version,
                    start,
                    end,
                    update,
                } = update_item
                {
                    if !update.is_empty() {
                        let update_info = CollapsedUpdateInfo {
                            blockchain_index: index,
                            protocol_version: *protocol_version,
                            start: *start,
                            end: *end,
                            update_data: update.clone(),
                        };
                        sync_events.push(SyncEvent::MerkleUpdateReceived {
                            blockchain_index: index,
                            update_info,
                        });
                    }
                }
            }

            // Then, collect all transactions
            for update_item in update {
                if let ZswapChainStateUpdate::RelevantTransaction {
                    transaction,
                    start: _,
                    end: _,
                } = update_item
                {
                    // Use the ViewingUpdate's index as the blockchain position
                    // All transactions in this update occurred at the same blockchain index
                    sync_events.push(SyncEvent::TransactionReceived {
                        blockchain_index: index,
                        transaction_data: transaction,
                    });
                }
            }
        }
        IndexerEvent::ProgressUpdate {
            type_name: _,
            highest_index,
            highest_relevant_index: _,
            highest_relevant_wallet_index,
        } => {
            sync_events.push(SyncEvent::ProgressUpdate {
                highest_index,
                highest_relevant_wallet_index,
            });
        }
    }

    sync_events
}
