//! LedgerContext wrapper for processing indexer sync events.
//!
//! This module bridges the WebSocket sync events from the indexer into
//! the midnight-node-ledger-helpers `LedgerContext`, which maintains
//! the wallet's UTXO set and the chain's merkle tree state.

use std::sync::Arc;

use tracing::{debug, info, warn};

use midnight_node_ledger_helpers::{
    make_block_context, DefaultDB, LedgerContext, LedgerParameters, Signature, Timestamp,
    WalletSeed,
};
use midnight_node_metadata::midnight_metadata_latest as mn_meta;

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

/// Process-wide shared LedgerContextManager.
/// Set by the relayer factory, read by the transaction factory.
static SHARED_LEDGER_CTX: std::sync::OnceLock<Arc<LedgerContextManager>> =
    std::sync::OnceLock::new();

/// Set the shared LedgerContextManager (called from relayer factory).
pub fn set_shared_ledger_ctx(ctx: Arc<LedgerContextManager>) {
    let _ = SHARED_LEDGER_CTX.set(ctx);
}

/// Get the shared LedgerContextManager (called from transaction factory).
/// Returns None if not initialized.
pub fn get_shared_ledger_ctx() -> Option<Arc<LedgerContextManager>> {
    SHARED_LEDGER_CTX.get().cloned()
}

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
            midnight_node_ledger_helpers::serialize(state)
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

    /// Bootstrap the LedgerContext from a live Midnight node via Subxt.
    ///
    /// Reads the network_id and ledger parameters from the node's runtime API.
    /// This populates the minimum state needed for `StandardTrasactionInfo::build()`
    /// to compute TTL, network_id, and fee calculations.
    ///
    /// NOTE: This does NOT populate the UTXO set — that requires either:
    /// - Block-by-block sync via `update_from_tx()`
    /// - Or injecting known UTXOs from the indexer
    pub async fn bootstrap_from_node(&self, rpc_url: &str) -> Result<(), LedgerContextError> {
        info!(rpc_url, "Bootstrapping LedgerContext from Midnight node");

        let api = subxt::OnlineClient::<subxt::PolkadotConfig>::from_url(rpc_url)
            .await
            .map_err(|e| LedgerContextError::ContextError(format!("Subxt connect failed: {e}")))?;

        // Read network_id via runtime API
        let network_id_call = mn_meta::apis().midnight_runtime_api().get_network_id();
        let network_id = api
            .runtime_api()
            .at_latest()
            .await
            .map_err(|e| LedgerContextError::ContextError(format!("Subxt at_latest: {e}")))?
            .call(network_id_call)
            .await
            .map_err(|e| LedgerContextError::ContextError(format!("get_network_id failed: {e}")))?;

        info!(network_id = %network_id, "Got network_id from node");

        // Read ledger parameters via runtime API
        let params_call = mn_meta::apis()
            .midnight_runtime_api()
            .get_ledger_parameters();
        let params_bytes = api
            .runtime_api()
            .at_latest()
            .await
            .map_err(|e| LedgerContextError::ContextError(format!("Subxt at_latest: {e}")))?
            .call(params_call)
            .await
            .map_err(|e| {
                LedgerContextError::ContextError(format!("get_ledger_parameters failed: {e}"))
            })?;

        let params_bytes = params_bytes.map_err(|e| {
            LedgerContextError::ContextError(format!("Ledger params API error: {e:?}"))
        })?;

        let parameters: LedgerParameters =
            midnight_node_ledger_helpers::deserialize(&mut &params_bytes[..]).map_err(|e| {
                LedgerContextError::DeserializationError(format!(
                    "LedgerParameters deserialize: {e}"
                ))
            })?;

        // Build a fresh LedgerState with the correct network_id and parameters.
        // Use LedgerState::blank() as the base and set the fields.
        use midnight_node_ledger_helpers::LedgerState;
        let mut new_state = LedgerState::<DefaultDB>::new(network_id.clone());
        new_state.parameters = midnight_node_ledger_helpers::Sp::new(parameters);

        // Serialize (tagged) and inject via update_ledger_state_from_bytes
        // which uses tagged deserialization internally
        let state_bytes = midnight_node_ledger_helpers::serialize(&new_state).map_err(|e| {
            LedgerContextError::SerializationError(format!(
                "Failed to serialize bootstrapped state: {e}"
            ))
        })?;

        self.context.update_ledger_state_from_bytes(&state_bytes);

        info!(
            network_id = %network_id,
            "LedgerContext bootstrapped with chain parameters"
        );

        Ok(())
    }

    /// Inject unshielded UTXOs into the LedgerState from indexer data.
    ///
    /// This populates the UTXO set so that `UtxoSpendInfo` can find
    /// matching UTXOs for spending during transaction building.
    pub fn inject_utxos(
        &self,
        utxos: &[super::manager::UtxoDetail],
    ) -> Result<(), LedgerContextError> {
        use midnight_node_ledger_helpers::{
            HashOutput, IntentHash, LedgerState, Sp, Timestamp, UserAddress, Utxo, NIGHT,
        };
        // UtxoMeta is not directly re-exported; access via the structure module
        use midnight_node_ledger_helpers::mn_ledger::structure::UtxoMeta;

        if utxos.is_empty() {
            return Ok(());
        }

        // Deserialize current state, inject UTXOs, re-serialize and inject
        let current_bytes = self.serialize_state()?;
        let mut state: midnight_node_ledger_helpers::LedgerState<DefaultDB> =
            midnight_node_ledger_helpers::deserialize(current_bytes.as_slice())
                .map_err(|e| LedgerContextError::DeserializationError(e.to_string()))?;

        for detail in utxos {
            let owner = self
                .context
                .with_wallet_from_seed(self.wallet_seed.clone(), |wallet| {
                    UserAddress::from(wallet.unshielded.signing_key().verifying_key())
                });

            let intent_hash_bytes = hex::decode(&detail.intent_hash).unwrap_or_default();
            let mut hash_arr = [0u8; 32];
            let copy_len = intent_hash_bytes.len().min(32);
            hash_arr[..copy_len].copy_from_slice(&intent_hash_bytes[..copy_len]);

            let utxo = Utxo {
                value: detail.value,
                owner,
                type_: NIGHT,
                intent_hash: IntentHash(HashOutput(hash_arr)),
                output_no: detail.output_index,
            };

            let meta = UtxoMeta {
                ctime: Timestamp::from_secs(0),
            };

            state.utxo = Sp::new(state.utxo.insert(utxo, meta));
        }

        // Re-serialize and inject
        let new_bytes = midnight_node_ledger_helpers::serialize(&state)
            .map_err(|e| LedgerContextError::SerializationError(e.to_string()))?;
        self.context.update_ledger_state_from_bytes(&new_bytes);

        // Verify injection by listing UTXOs
        let found_utxos = self.unshielded_utxos();
        info!(
            utxo_count = utxos.len(),
            found_after_injection = found_utxos.len(),
            "Injected unshielded UTXOs into LedgerContext"
        );

        for utxo in &found_utxos {
            debug!(
                value = utxo.value,
                owner = ?utxo.owner,
                token_type = ?utxo.type_,
                "UTXO in LedgerContext"
            );
        }

        Ok(())
    }

    /// Apply raw DUST ledger events to the wallet's DustWallet.
    ///
    /// Each event is a hex-encoded serialized `Event<DefaultDB>` from the
    /// indexer's `dustLedgerEvents` subscription. These update the wallet's
    /// DUST UTXO set, enabling fee payment.
    pub fn apply_dust_events(&self, raw_events: &[String]) -> Result<(), LedgerContextError> {
        use midnight_node_ledger_helpers::Event;

        if raw_events.is_empty() {
            return Ok(());
        }

        let mut events: Vec<Event<DefaultDB>> = Vec::new();
        for raw_hex in raw_events {
            let bytes = hex::decode(raw_hex.trim_start_matches("0x"))
                .map_err(|e| LedgerContextError::DeserializationError(format!("hex: {e}")))?;
            let event: Event<DefaultDB> =
                midnight_node_ledger_helpers::deserialize(&mut &bytes[..]).map_err(|e| {
                    LedgerContextError::DeserializationError(format!("dust event: {e}"))
                })?;
            events.push(event);
        }

        // Feed events into the wallet's DustWallet
        self.context
            .with_wallet_from_seed(self.wallet_seed.clone(), |wallet| {
                if let Err(e) = wallet.update_dust_from_tx(&events) {
                    warn!(error = ?e, "Failed to apply DUST events to wallet");
                }
            });

        info!(events = raw_events.len(), "Applied DUST events to wallet");
        Ok(())
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
