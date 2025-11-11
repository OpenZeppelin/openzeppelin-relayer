use std::sync::{Arc, Mutex};

use crate::{
    repositories::{RelayerStateRepositoryStorage, SyncStateTrait},
    services::midnight::{
        handler::{
            ChronologicalUpdate, EventDispatcher, EventHandlerType, ProgressTracker, SyncConfig,
            SyncStrategy,
        },
        indexer::MidnightIndexerClient,
        utils::derive_viewing_key,
        SyncError,
    },
};

use log::{debug, info, warn};
use midnight_node_ledger_helpers::{
    mn_ledger_serialize::{deserialize, serialize},
    DefaultDB, LedgerContext, LedgerState, NetworkId, WalletSeed, WalletState,
};

/// Main sync manager that coordinates all sync components.
///
/// This struct is the entry point for wallet synchronization. It manages the lifecycle of the sync
/// process, wires together all services, and ensures that events are handled and state is updated
/// correctly. It is responsible for selecting the sync strategy, managing persistence, and tracking progress.
pub struct SyncManager<S: SyncStrategy, SS: SyncStateTrait = RelayerStateRepositoryStorage> {
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
    sync_state_store: Arc<SS>,
    // The relayer ID for tracking sync state
    relayer_id: String,
}

impl<S: SyncStrategy + Sync + Send, SS: SyncStateTrait + Send + Sync> SyncManager<S, SS> {
    /// Serialize the current ledger context to bytes
    #[allow(clippy::result_large_err)]
    fn serialize_context(&self) -> Result<Vec<u8>, SyncError> {
        // Serialize the wallet state for the current seed
        let wallet_state = {
            let wallets_guard = self
                .context
                .wallets
                .lock()
                .map_err(|e| SyncError::SyncError(format!("Failed to lock wallets: {e}")))?;

            wallets_guard
                .get(&self.seed)
                .map(|wallet| {
                    // We only serialize the wallet state, not the secret keys
                    let mut state_bytes = Vec::new();
                    serialize(&wallet.state, &mut state_bytes, self.network).map_err(|e| {
                        SyncError::SyncError(format!("Failed to serialize wallet state: {e:?}"))
                    })?;
                    Ok::<Vec<u8>, SyncError>(state_bytes)
                })
                .transpose()?
        };

        // Serialize the ledger state
        let ledger_state_bytes = {
            let ledger_state_guard =
                self.context.ledger_state.lock().map_err(|e| {
                    SyncError::SyncError(format!("Failed to lock ledger state: {e}"))
                })?;

            let mut bytes = Vec::new();
            serialize(&*ledger_state_guard, &mut bytes, self.network).map_err(|e| {
                SyncError::SyncError(format!("Failed to serialize ledger state: {e:?}"))
            })?;
            bytes
        };

        // Combine wallet state and ledger state into a single serialized context
        bincode::serialize(&(wallet_state, ledger_state_bytes))
            .map_err(|e| SyncError::SyncError(format!("Failed to serialize context: {e}")))
    }

    /// Restore the ledger context from serialized bytes
    #[allow(clippy::result_large_err)]
    pub fn restore_context(&self, serialized_context: &[u8]) -> Result<(), SyncError> {
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
                        warn!("Failed to deserialize ledger state: {e:?}, starting fresh");
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
                            warn!("Failed to deserialize wallet state: {e:?}, starting fresh");
                        }
                    }
                }
                Ok(())
            }
            Err(e) => {
                warn!("Failed to deserialize context: {e}, starting fresh");
                Ok(())
            }
        }
    }

    /// Create a new manager with the specified sync strategy and configuration.
    ///
    /// This method initializes all services and selects the sync strategy based on the provided options.
    /// It will also attempt to restore the ledger context from a previous sync if available.
    pub async fn new(
        indexer_client: &MidnightIndexerClient,
        seed: &WalletSeed,
        network: NetworkId,
        sync_state_store: Arc<SS>,
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
            .await
        {
            info!(
                "Found saved ledger context for relayer {}, attempting to restore",
                manager.relayer_id
            );

            if let Err(e) = manager.restore_context(&serialized_context) {
                warn!("Failed to restore context: {e}");
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
                    .await
                    .map_err(|e| SyncError::SyncError(format!("Failed to get sync state: {e}")))?
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
        let event_handler = EventHandlerType::EventHandler {
            network: self.network,
            updates_buffer: updates_buffer.clone(),
        };

        let mut event_dispatcher = EventDispatcher::new();
        event_dispatcher.register_handler(event_handler);

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
                    info!("Applying merkle update at index {index}");
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
                                "Failed to apply collapsed update to wallet state: {e}"
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
                        debug!("Applying transaction at index {index}");
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
                .await
                .map_err(|e| SyncError::SyncError(format!("Failed to save sync state: {e}")))?;

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
impl<S: SyncStrategy + Sync + Send, SS: SyncStateTrait + Send + Sync> super::SyncManagerTrait
    for SyncManager<S, SS>
{
    async fn sync(&mut self, start_index: u64) -> Result<(), SyncError> {
        self.sync(Some(start_index)).await
    }

    fn get_context(&self) -> Arc<LedgerContext<DefaultDB>> {
        self.get_context()
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        repositories::{RelayerStateRepositoryStorage, SyncStateTrait},
        services::{
            midnight::{
                handler::{QuickSyncStrategy, SyncManager, SyncManagerTrait},
                indexer::MidnightIndexerClient,
                SyncError,
            },
            sync::midnight::handler::{
                EventDispatcher, ProgressTracker, SyncConfig, SyncEvent, SyncStrategy,
            },
        },
    };
    use midnight_node_ledger_helpers::{NetworkId, WalletSeed};
    use std::sync::Arc;

    fn setup_test_env() {
        std::env::set_var(
            "MIDNIGHT_LEDGER_TEST_STATIC_DIR",
            "/tmp/midnight-test-static",
        );
    }
    // Mock sync strategy for testing
    struct MockSyncStrategy {
        _config: Option<SyncConfig>,
        sync_called: std::sync::Mutex<bool>,
        should_fail: bool,
    }

    #[async_trait::async_trait]
    impl SyncStrategy for MockSyncStrategy {
        fn new(_indexer_client: &MidnightIndexerClient, config: Option<SyncConfig>) -> Self {
            Self {
                _config: config,
                sync_called: std::sync::Mutex::new(false),
                should_fail: false,
            }
        }

        async fn sync(
            &mut self,
            start_index: u64,
            dispatcher: &mut EventDispatcher,
            progress_tracker: &mut ProgressTracker,
        ) -> Result<(), SyncError> {
            *self.sync_called.lock().unwrap() = true;

            if self.should_fail {
                return Err(SyncError::SyncError("Mock sync failed".to_string()));
            }

            // Simulate some sync activity
            progress_tracker.record_transaction(start_index + 1);
            dispatcher
                .dispatch(&SyncEvent::SyncCompleted)
                .await
                .unwrap();

            Ok(())
        }
    }

    #[tokio::test]
    async fn test_sync_manager_new() {
        setup_test_env();

        let seed = WalletSeed::from([1u8; 32]);
        let sync_state_store = Arc::new(RelayerStateRepositoryStorage::new_in_memory());
        let relayer_id = "test-relayer".to_string();
        let indexer_urls = crate::config::network::IndexerUrls {
            http: "http://localhost:8080".to_string(),
            ws: "ws://localhost:8080".to_string(),
        };
        let indexer_client = MidnightIndexerClient::new(indexer_urls);

        let manager = SyncManager::<MockSyncStrategy>::new(
            &indexer_client,
            &seed,
            NetworkId::TestNet,
            sync_state_store,
            relayer_id,
        )
        .await;

        assert!(manager.is_ok());
    }

    #[tokio::test]
    async fn test_sync_manager_sync_from_specific_index() {
        setup_test_env();

        let seed = WalletSeed::from([1u8; 32]);
        let sync_state_store = Arc::new(RelayerStateRepositoryStorage::new_in_memory());
        let relayer_id = "test-relayer".to_string();
        let indexer_urls = crate::config::network::IndexerUrls {
            http: "http://localhost:8080".to_string(),
            ws: "ws://localhost:8080".to_string(),
        };
        let indexer_client = MidnightIndexerClient::new(indexer_urls);

        let mut manager = SyncManager::<MockSyncStrategy>::new(
            &indexer_client,
            &seed,
            NetworkId::TestNet,
            sync_state_store.clone(),
            relayer_id.clone(),
        )
        .await
        .unwrap();

        // Sync from index 100
        let result = manager.sync(Some(100)).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_sync_manager_sync_incremental() {
        setup_test_env();

        let seed = WalletSeed::from([1u8; 32]);
        let sync_state_store = Arc::new(RelayerStateRepositoryStorage::new_in_memory());
        let relayer_id = "test-relayer".to_string();

        // Pre-set a sync state
        sync_state_store
            .set_last_synced_index(&relayer_id, 500)
            .await
            .unwrap();

        let indexer_urls = crate::config::network::IndexerUrls {
            http: "http://localhost:8080".to_string(),
            ws: "ws://localhost:8080".to_string(),
        };
        let indexer_client = MidnightIndexerClient::new(indexer_urls);

        let mut manager = SyncManager::<MockSyncStrategy>::new(
            &indexer_client,
            &seed,
            NetworkId::TestNet,
            sync_state_store.clone(),
            relayer_id.clone(),
        )
        .await
        .unwrap();

        // Sync incrementally (should start from 500)
        let result = manager.sync_incremental().await;
        assert!(result.is_ok());

        // Verify sync was called - the mock doesn't actually update the state
        // because SyncManager only saves state when there are real chronological updates
        let stored_index = sync_state_store
            .get_last_synced_index(&relayer_id)
            .await
            .unwrap();
        assert_eq!(stored_index, Some(500)); // Should remain at the pre-set value
    }

    #[tokio::test]
    async fn test_sync_manager_get_context() {
        setup_test_env();

        let seed = WalletSeed::from([1u8; 32]);
        let sync_state_store = Arc::new(RelayerStateRepositoryStorage::new_in_memory());
        let relayer_id = "test-relayer".to_string();
        let indexer_urls = crate::config::network::IndexerUrls {
            http: "http://localhost:8080".to_string(),
            ws: "ws://localhost:8080".to_string(),
        };
        let indexer_client = MidnightIndexerClient::new(indexer_urls);

        let manager = SyncManager::<MockSyncStrategy>::new(
            &indexer_client,
            &seed,
            NetworkId::TestNet,
            sync_state_store,
            relayer_id,
        )
        .await
        .unwrap();

        let context = manager.get_context();
        assert!(Arc::strong_count(&context) > 1); // Manager holds one, we hold one
    }

    #[tokio::test]
    async fn test_sync_manager_trait_implementation() {
        setup_test_env();

        let seed = WalletSeed::from([1u8; 32]);
        let sync_state_store = Arc::new(RelayerStateRepositoryStorage::new_in_memory());
        let relayer_id = "test-relayer".to_string();
        let indexer_urls = crate::config::network::IndexerUrls {
            http: "http://localhost:8080".to_string(),
            ws: "ws://localhost:8080".to_string(),
        };
        let indexer_client = MidnightIndexerClient::new(indexer_urls);

        let mut manager: Box<dyn SyncManagerTrait> = Box::new(
            SyncManager::<MockSyncStrategy>::new(
                &indexer_client,
                &seed,
                NetworkId::TestNet,
                sync_state_store,
                relayer_id,
            )
            .await
            .unwrap(),
        );

        // Test trait methods
        let result = manager.sync(200).await;
        assert!(result.is_ok());

        let context = manager.get_context();
        assert!(Arc::strong_count(&context) > 1);
    }

    #[test]
    fn test_sync_config_default() {
        let config = SyncConfig::default();
        assert!(config.viewing_key.is_none());
        assert_eq!(config.idle_timeout, Some(std::time::Duration::from_secs(5)));
        assert_eq!(config.send_progress_events, Some(true));
    }

    #[test]
    fn test_sync_config_custom() {
        let config = SyncConfig {
            viewing_key: None,
            idle_timeout: Some(std::time::Duration::from_secs(10)),
            send_progress_events: Some(false),
        };
        assert!(config.viewing_key.is_none());
        assert_eq!(
            config.idle_timeout,
            Some(std::time::Duration::from_secs(10))
        );
        assert_eq!(config.send_progress_events, Some(false));
    }

    #[tokio::test]
    async fn test_serialize_and_restore_context() {
        setup_test_env();

        let seed = WalletSeed::from([1u8; 32]);
        let sync_state_store = Arc::new(RelayerStateRepositoryStorage::new_in_memory());
        let relayer_id = "test-relayer".to_string();
        let indexer_urls = crate::config::network::IndexerUrls {
            http: "http://localhost:8080".to_string(),
            ws: "ws://localhost:8080".to_string(),
        };
        let indexer_client = MidnightIndexerClient::new(indexer_urls);

        // Create first manager
        let _manager1 = SyncManager::<QuickSyncStrategy>::new(
            &indexer_client,
            &seed,
            NetworkId::TestNet,
            sync_state_store.clone(),
            relayer_id.clone(),
        )
        .await
        .unwrap();

        // Can't test private serialize_context method directly
        // Save some dummy context to sync state
        let dummy_context = vec![1, 2, 3, 4, 5];
        sync_state_store
            .set_sync_state(&relayer_id, 100, Some(dummy_context.clone()))
            .await
            .unwrap();

        // Create second manager - should attempt to restore context
        let manager2 = SyncManager::<QuickSyncStrategy>::new(
            &indexer_client,
            &seed,
            NetworkId::TestNet,
            sync_state_store.clone(),
            relayer_id.clone(),
        )
        .await;

        assert!(manager2.is_ok()); // Should handle invalid data gracefully
    }

    #[tokio::test]
    async fn test_restore_context_invalid_data() {
        setup_test_env();

        let seed = WalletSeed::from([1u8; 32]);
        let sync_state_store = Arc::new(RelayerStateRepositoryStorage::new_in_memory());
        let relayer_id = "test-relayer".to_string();
        let indexer_urls = crate::config::network::IndexerUrls {
            http: "http://localhost:8080".to_string(),
            ws: "ws://localhost:8080".to_string(),
        };
        let indexer_client = MidnightIndexerClient::new(indexer_urls.clone());

        // Save invalid data
        sync_state_store
            .set_ledger_context(&relayer_id, vec![1, 2, 3, 4, 5]) // Invalid serialized data
            .await
            .unwrap();

        // Create manager - should handle invalid data gracefully
        let manager = SyncManager::<QuickSyncStrategy>::new(
            &indexer_client,
            &seed,
            NetworkId::TestNet,
            sync_state_store,
            relayer_id,
        )
        .await;

        assert!(manager.is_ok()); // Should not fail, just start fresh
    }
}
