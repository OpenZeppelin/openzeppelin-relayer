//! Midnight transaction builder using the `midnight-node-ledger-helpers` crate.
//!
//! This module provides the bridge between the relayer's transaction model
//! and the Midnight ledger's tagged serialization format.
//!
//! **Status:** Scaffold — the builder is structurally complete but the
//! `build_unshielded_transfer` method requires a populated `LedgerContext`
//! (from indexer wallet sync) before it can construct valid transactions.

use midnight_node_ledger_helpers::{DefaultDB, LedgerContext, WalletSeed};
use std::sync::Arc;

/// Errors during Midnight transaction building.
#[derive(Debug, thiserror::Error)]
pub enum TxBuildError {
    #[error("Serialization error: {0}")]
    SerializationError(String),
    #[error("Invalid input: {0}")]
    InvalidInput(String),
}

/// Wraps the midnight-node-ledger-helpers `LedgerContext` for transaction building.
pub struct MidnightTxBuilder {
    context: Arc<LedgerContext<DefaultDB>>,
    wallet_seed: WalletSeed,
}

impl MidnightTxBuilder {
    /// Create a new transaction builder from a raw 32-byte seed.
    pub fn new(seed_bytes: &[u8; 32], network_id: &str) -> Result<Self, TxBuildError> {
        let wallet_seed = WalletSeed::Medium(*seed_bytes);
        let context =
            LedgerContext::new_from_wallet_seeds(network_id.to_string(), &[wallet_seed.clone()]);

        Ok(Self {
            context: Arc::new(context),
            wallet_seed,
        })
    }

    /// Get the LedgerContext for sync operations that update wallet/merkle state.
    pub fn context(&self) -> &Arc<LedgerContext<DefaultDB>> {
        &self.context
    }

    /// Get the wallet seed.
    pub fn wallet_seed(&self) -> &WalletSeed {
        &self.wallet_seed
    }

    /// Serialize a proven transaction for RPC submission.
    pub fn serialize_for_submission<
        T: midnight_node_ledger_helpers::Serializable + midnight_node_ledger_helpers::Tagged,
    >(
        tx: &T,
    ) -> Result<String, TxBuildError> {
        let bytes = midnight_node_ledger_helpers::serialize(tx)
            .map_err(|e| TxBuildError::SerializationError(e.to_string()))?;
        Ok(hex::encode(bytes))
    }
}
