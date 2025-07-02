pub mod events;
pub mod manager;
pub mod strategy;
pub mod tracker;

use async_trait::async_trait;
use midnight_node_ledger_helpers::{DefaultDB, LedgerContext};
use std::sync::Arc;

use crate::services::midnight::SyncError;

pub use events::*;
pub use manager::*;
pub use strategy::*;
pub use tracker::*;

/// Trait for sync manager functionality required by the relayer
#[async_trait]
pub trait SyncManagerTrait: Send + Sync {
    /// Synchronize the wallet state from the given block height
    async fn sync(&mut self, start_height: u64) -> Result<(), SyncError>;

    /// Get the current ledger context
    fn get_context(&self) -> Arc<LedgerContext<DefaultDB>>;
}
