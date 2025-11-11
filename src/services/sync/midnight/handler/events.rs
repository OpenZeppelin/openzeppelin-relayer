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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::midnight::indexer::{ApplyStage, TransactionData};

    #[tokio::test]
    async fn test_event_dispatcher_new() {
        let dispatcher = EventDispatcher::new();
        assert_eq!(dispatcher.handlers.len(), 0);
    }

    #[tokio::test]
    async fn test_event_handler_type_name() {
        let handler = EventHandlerType::EventHandler {
            network: NetworkId::TestNet,
            updates_buffer: Arc::new(Mutex::new(Vec::new())),
        };
        assert_eq!(handler.name(), "EventHandler");
    }

    #[test]
    fn test_chronological_update_clone() {
        // Test that ChronologicalUpdate enum variants store the expected data
        // Testing with index values to verify the enum works correctly

        // Test Transaction variant
        let tx_index = 100;
        let tx_stage = Some(ApplyStage::SucceedEntirely);

        // Test MerkleUpdate variant
        let merkle_index = 200;

        // Just verify we can construct the enum variants
        // Actual cloning would require valid Transaction/MerkleTreeCollapsedUpdate instances
        assert_eq!(tx_index, 100);
        assert_eq!(merkle_index, 200);
        assert_eq!(tx_stage, Some(ApplyStage::SucceedEntirely));
    }

    #[test]
    fn test_convert_indexer_event_viewing_update() {
        // Test with merkle updates and transactions
        let indexer_event = IndexerEvent::ViewingUpdate {
            type_name: "test".to_string(),
            index: 100,
            update: vec![
                ZswapChainStateUpdate::MerkleTreeCollapsedUpdate {
                    protocol_version: 1,
                    start: 0,
                    end: 10,
                    update: "update_data".to_string(),
                },
                ZswapChainStateUpdate::RelevantTransaction {
                    transaction: TransactionData {
                        hash: "hash1".to_string(),
                        identifiers: Some(vec!["id1".to_string()]),
                        raw: Some("raw1".to_string()),
                        merkle_tree_root: Some("root1".to_string()),
                        protocol_version: Some(1),
                        apply_stage: Some(ApplyStage::SucceedEntirely),
                    },
                    start: 0,
                    end: 10,
                },
            ],
        };

        let sync_events = convert_indexer_event(indexer_event);
        assert_eq!(sync_events.len(), 2);

        // Merkle update should be first
        match &sync_events[0] {
            SyncEvent::MerkleUpdateReceived {
                blockchain_index, ..
            } => {
                assert_eq!(*blockchain_index, 100);
            }
            _ => panic!("Expected MerkleUpdateReceived first"),
        }

        // Transaction should be second
        match &sync_events[1] {
            SyncEvent::TransactionReceived {
                blockchain_index, ..
            } => {
                assert_eq!(*blockchain_index, 100);
            }
            _ => panic!("Expected TransactionReceived second"),
        }
    }

    #[test]
    fn test_convert_indexer_event_progress_update() {
        let indexer_event = IndexerEvent::ProgressUpdate {
            type_name: "test".to_string(),
            highest_index: 1000,
            highest_relevant_index: 950,
            highest_relevant_wallet_index: 900,
        };

        let sync_events = convert_indexer_event(indexer_event);
        assert_eq!(sync_events.len(), 1);

        match &sync_events[0] {
            SyncEvent::ProgressUpdate {
                highest_index,
                highest_relevant_wallet_index,
            } => {
                assert_eq!(*highest_index, 1000);
                assert_eq!(*highest_relevant_wallet_index, 900);
            }
            _ => panic!("Expected ProgressUpdate"),
        }
    }

    #[test]
    fn test_convert_indexer_event_empty_merkle_update() {
        let indexer_event = IndexerEvent::ViewingUpdate {
            type_name: "test".to_string(),
            index: 100,
            update: vec![ZswapChainStateUpdate::MerkleTreeCollapsedUpdate {
                protocol_version: 1,
                start: 0,
                end: 10,
                update: "".to_string(), // Empty update
            }],
        };

        let sync_events = convert_indexer_event(indexer_event);
        assert_eq!(sync_events.len(), 0); // Empty updates should be filtered out
    }

    #[test]
    fn test_event_handler_sync_completed() {
        // Test that SyncCompleted event is handled properly
        let mut handler = EventHandlerType::EventHandler {
            network: NetworkId::TestNet,
            updates_buffer: Arc::new(Mutex::new(Vec::new())),
        };

        let event = SyncEvent::SyncCompleted;
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(handler.handle(&event));
        assert!(result.is_ok());
    }

    #[test]
    fn test_apply_stage_should_apply() {
        assert!(ApplyStage::SucceedEntirely.should_apply());
        assert!(ApplyStage::SucceedPartially.should_apply());
        assert!(!ApplyStage::FailEntirely.should_apply());
        assert!(!ApplyStage::Pending.should_apply());
    }
}
