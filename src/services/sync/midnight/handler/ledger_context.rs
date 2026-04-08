//! LedgerContext wrapper for processing indexer sync events.
//!
//! This module bridges the WebSocket sync events from the indexer into
//! the midnight-node-ledger-helpers `LedgerContext`, which maintains
//! the wallet's UTXO set and the chain's merkle tree state.

use std::sync::Arc;

use tracing::{debug, info, warn};

use midnight_node_ledger_helpers::{
    make_block_context, DefaultDB, LedgerContext, Signature, Timestamp, WalletSeed,
};

/// The concrete finalized transaction type from the chain.
/// This matches the tagged format `midnight:transaction[v9](signature[v1],proof,pedersen-schnorr[v1])`.
type MnFinalizedTransaction =
    midnight_node_ledger_helpers::transaction::FinalizedTransaction<DefaultDB>;

/// SerdeTransaction wrapping the finalized transaction type.
type MnSerdeTransaction = midnight_node_ledger_helpers::SerdeTransaction<
    Signature,
    midnight_node_ledger_helpers::ProofMarker,
    DefaultDB,
>;

/// Manages the LedgerContext lifecycle for a Midnight relayer.
///
/// This struct owns the `LedgerContext` and provides methods to:
/// - Apply raw transaction bytes from the indexer's shielded sync
/// - Serialize/deserialize the context for persistence
/// - Access the context for transaction building
pub struct LedgerContextManager {
    context: Arc<LedgerContext<DefaultDB>>,
    wallet_seed: WalletSeed,
    network_id: String,
    applied_tx_count: u64,
}

impl LedgerContextManager {
    /// Create a new context manager with a fresh LedgerContext.
    pub fn new(seed_bytes: &[u8; 32], network_id: &str) -> Self {
        let wallet_seed = WalletSeed::Medium(*seed_bytes);
        let context =
            LedgerContext::new_from_wallet_seeds(network_id.to_string(), &[wallet_seed.clone()]);

        Self {
            context: Arc::new(context),
            wallet_seed,
            network_id: network_id.to_string(),
            applied_tx_count: 0,
        }
    }

    /// Apply a raw transaction from the indexer to the LedgerContext.
    ///
    /// The `raw_hex` is the hex-encoded tagged serialization of a
    /// `SerdeTransaction<Signature, ProofMarker, DefaultDB>` as returned
    /// by the indexer's `transaction.raw` field.
    ///
    /// `block_timestamp_secs` is the block's timestamp in seconds since epoch.
    pub fn apply_transaction(
        &self,
        raw_hex: &str,
        block_timestamp_secs: u64,
    ) -> Result<(), LedgerContextError> {
        // Decode hex to bytes
        let bytes = hex::decode(raw_hex.trim_start_matches("0x"))
            .map_err(|e| LedgerContextError::DeserializationError(format!("hex decode: {e}")))?;

        // Deserialize using tagged deserialization.
        // The raw bytes use the format: midnight:transaction[v9](signature[v1],proof,pedersen-schnorr[v1])
        // This is a tagged Transaction, which we wrap in SerdeTransaction::Midnight.
        let inner_tx: MnFinalizedTransaction =
            midnight_node_ledger_helpers::deserialize(bytes.as_slice()).map_err(|e| {
                LedgerContextError::DeserializationError(format!("transaction deserialize: {e}"))
            })?;
        let tx = MnSerdeTransaction::Midnight(inner_tx);

        // Create a block context with the timestamp.
        // For wallet sync, we use a minimal context — the exact parent_block_hash
        // isn't critical for tracking UTXOs, but tblock is needed for fee calculations.
        let block_context = make_block_context(
            Timestamp::from_secs(block_timestamp_secs),
            Default::default(), // parent_block_hash — not available from indexer event
            Timestamp::from_secs(block_timestamp_secs.saturating_sub(6)), // approximate last_block_time
        );

        // Apply the transaction to the ledger context.
        // This updates both the ledger state and the wallet state.
        let (events, _cost) = self.context.update_from_tx(&tx, &block_context);

        debug!(
            network_id = %self.network_id,
            events = events.len(),
            "Applied transaction to LedgerContext"
        );

        Ok(())
    }

    /// Apply a collapsed merkle tree update.
    ///
    /// These updates come between transactions and keep the merkle tree
    /// in sync with the chain state. The `update_hex` contains the
    /// serialized tree update data.
    pub fn apply_merkle_update(
        &self,
        update_hex: &str,
        _start_index: u64,
        _end_index: u64,
    ) -> Result<(), LedgerContextError> {
        // Merkle tree updates are applied to the ZswapChainState within the ledger.
        // For now, we log and skip — the critical path for UTXO tracking is
        // update_from_tx which handles merkle roots internally.
        debug!(
            update_len = update_hex.len() / 2,
            "Merkle tree update received (not yet applied directly)"
        );
        Ok(())
    }

    /// Get the LedgerContext for transaction building.
    pub fn context(&self) -> &Arc<LedgerContext<DefaultDB>> {
        &self.context
    }

    /// Get the wallet seed.
    pub fn wallet_seed(&self) -> &WalletSeed {
        &self.wallet_seed
    }

    /// Serialize the current ledger state for persistence.
    ///
    /// This allows the context to be restored after a restart without
    /// re-syncing from genesis.
    pub fn serialize_state(&self) -> Result<Vec<u8>, LedgerContextError> {
        self.context.with_ledger_state(|state| {
            midnight_node_ledger_helpers::serialize_untagged(state)
                .map_err(|e| LedgerContextError::SerializationError(e.to_string()))
        })
    }

    /// Restore the ledger state from previously serialized bytes.
    pub fn restore_state(&self, bytes: &[u8]) -> Result<(), LedgerContextError> {
        self.context.update_ledger_state_from_bytes(bytes);
        info!("LedgerContext state restored from persisted bytes");
        Ok(())
    }

    /// Get the number of transactions applied since creation/restore.
    pub fn applied_tx_count(&self) -> u64 {
        self.applied_tx_count
    }

    /// List unshielded UTXOs for the wallet.
    pub fn unshielded_utxos(&self) -> Vec<midnight_node_ledger_helpers::Utxo> {
        self.context
            .with_wallet_from_seed(self.wallet_seed.clone(), |wallet| {
                self.context
                    .with_ledger_state(|state| wallet.unshielded_utxos(state))
            })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LedgerContextError {
    #[error("Deserialization error: {0}")]
    DeserializationError(String),
    #[error("Serialization error: {0}")]
    SerializationError(String),
    #[error("Context error: {0}")]
    ContextError(String),
}
