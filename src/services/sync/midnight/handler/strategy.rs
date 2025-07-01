use crate::services::midnight::{
    handler::{convert_indexer_event, EventDispatcher, ProgressTracker, SyncEvent},
    indexer::{MidnightIndexerClient, ViewingKeyFormat},
    SyncError,
};

use futures_util::StreamExt;
use log::{debug, info};
use tokio::time::{sleep_until, Duration, Instant};

/// Configuration for sync strategies.
///
/// Controls timeouts and whether to send progress events.
#[derive(Debug, Clone)]
pub struct SyncConfig {
    /// The viewing key to use for the sync.
    pub viewing_key: Option<ViewingKeyFormat>,
    /// Timeout for idle periods (no new events).
    pub idle_timeout: Option<Duration>,
    /// Whether to send progress events.
    pub send_progress_events: Option<bool>,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            viewing_key: None,
            // We set this to 5 seconds since 1 block is ~6 seconds
            // so we can safely say if we haven't received any events in 5 seconds,
            // we're likely just waiting for the next block to process
            idle_timeout: Some(Duration::from_secs(5)),
            send_progress_events: Some(true),
        }
    }
}

/// Trait for different synchronization strategies.
///
/// A sync strategy defines how the wallet fetches and processes blockchain data. It is responsible for
/// emitting events for transactions, Merkle updates, and progress, and for driving the sync lifecycle.
#[async_trait::async_trait]
pub trait SyncStrategy: Send + Sync {
    /// Create a new sync strategy.
    fn new(indexer_client: &MidnightIndexerClient, config: Option<SyncConfig>) -> Self;

    /// Execute the sync strategy from the given start height.
    ///
    /// This method should emit events via the dispatcher and update the progress tracker.
    async fn sync(
        &mut self,
        start_height: u64,
        event_dispatcher: &mut EventDispatcher,
        progress_tracker: &mut ProgressTracker,
    ) -> Result<(), SyncError>;
}

/// Strategy for quick synchronization using the indexer
/// This is faster but requires sharing the viewing key with the indexer
/// The indexer will only send us the transactions that are relevant to our wallet
pub struct QuickSyncStrategy {
    indexer_client: MidnightIndexerClient,
    config: Option<SyncConfig>,
}

impl QuickSyncStrategy {
    /// Ensure that the config is Some and that the viewing key is also Some
    fn ensure_config(&self) -> Result<&SyncConfig, SyncError> {
        self.config.as_ref().ok_or(SyncError::SessionError(
            "No config provided for quick sync".to_string(),
        ))
    }

    /// Establish a session with the indexer for the wallet's viewing key.
    async fn establish_session(&self) -> Result<String, SyncError> {
        let config = self.ensure_config()?;

        let viewing_key = config.viewing_key.as_ref().ok_or(SyncError::SessionError(
            "No viewing key provided for quick sync".to_string(),
        ))?;

        let session_id = self
            .indexer_client
            .connect_wallet(viewing_key)
            .await
            .map_err(|e| SyncError::SessionError(format!("Failed to connect wallet: {}", e)))?;

        debug!("Established wallet session: {}", session_id);
        Ok(session_id)
    }
}

#[async_trait::async_trait]
impl SyncStrategy for QuickSyncStrategy {
    /// Create a new relevant transaction sync strategy.
    fn new(indexer_client: &MidnightIndexerClient, config: Option<SyncConfig>) -> Self {
        Self {
            indexer_client: indexer_client.clone(),
            config,
        }
    }

    /// Execute the relevant transaction sync strategy.
    async fn sync(
        &mut self,
        start_height: u64,
        event_dispatcher: &mut EventDispatcher,
        progress_tracker: &mut ProgressTracker,
    ) -> Result<(), SyncError> {
        let config = self.ensure_config()?;
        let session_id = self.establish_session().await?;

        debug!("Starting quick sync from index {}", start_height);

        let send_progress_events = config.send_progress_events.unwrap_or(true);
        let idle_timeout = config.idle_timeout.unwrap_or(Duration::from_secs(5));

        // Subscribe to wallet events
        let mut wallet_stream = self
            .indexer_client
            .subscribe_wallet(&session_id, Some(start_height), Some(send_progress_events))
            .await?;

        let mut last_event_time = Instant::now();

        loop {
            let timeout = sleep_until(last_event_time + idle_timeout);
            tokio::pin!(timeout);

            tokio::select! {
                Some(event_result) = wallet_stream.next() => {
                    last_event_time = Instant::now();

                    match event_result {
                        Ok(indexer_event) => {
                            debug!("Processing indexer event: {:#?}", indexer_event);

                            // Convert and dispatch events
                            let sync_events = convert_indexer_event(indexer_event);
                            for event in sync_events {
                                // Update progress tracker based on event type
                                match &event {
                                    SyncEvent::TransactionReceived { blockchain_index, .. } => {
                                        progress_tracker.record_transaction(*blockchain_index);
                                    }
                                    SyncEvent::MerkleUpdateReceived { blockchain_index, .. } => {
                                        progress_tracker.record_merkle_update(*blockchain_index);
                                    }
                                    SyncEvent::ProgressUpdate {
                                        highest_index,
                                        highest_relevant_wallet_index,
                                        ..
                                    } => {
                                        if progress_tracker.is_sync_complete(*highest_index, *highest_relevant_wallet_index) {
                                            debug!("Sync completed based on progress update");
                                            // Dispatch completion event
                                            event_dispatcher.dispatch(&SyncEvent::SyncCompleted).await?;
                                            return Ok(());
                                        }
                                    }
                                    _ => {}
                                }

                                // Process the event
                                event_dispatcher.dispatch(&event).await?;
                            }
                        }
                        Err(e) => {
                            event_dispatcher.dispatch(&SyncEvent::SyncError { error: e }).await?;
                        }
                    }
                }
                _ = &mut timeout => {
                    debug!("No new events for {} seconds, sync is complete", idle_timeout.as_secs());
                    break;
                }
            }
        }

        // Dispatch final completion event
        info!("Sync completed");

        event_dispatcher.dispatch(&SyncEvent::SyncCompleted).await?;

        Ok(())
    }
}

/// Strategy for full synchronization
/// This is slower but does not require sharing the viewing key with the indexer
/// The indexer will send us all the transactions and Merkle updates
/// We apply them on a trial and error basis until we have the correct state
pub struct FullSyncStrategy {
    _indexer_client: MidnightIndexerClient,
}

#[async_trait::async_trait]
impl SyncStrategy for FullSyncStrategy {
    fn new(indexer_client: &MidnightIndexerClient, _config: Option<SyncConfig>) -> Self {
        Self {
            _indexer_client: indexer_client.clone(),
        }
    }

    async fn sync(
        &mut self,
        _start_height: u64,
        _event_dispatcher: &mut EventDispatcher,
        _progress_tracker: &mut ProgressTracker,
    ) -> Result<(), SyncError> {
        unimplemented!()
    }
}
