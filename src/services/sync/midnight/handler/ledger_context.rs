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
        // In ledger-helpers 1.0.0+ this returns Result; treat failures as
        // deserialization errors (the tx shape didn't match chain expectations).
        let (events, _cost) = self
            .context
            .update_from_tx(&tx, &block_context)
            .map_err(|e| {
                LedgerContextError::DeserializationError(format!("update_from_tx: {e}"))
            })?;

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

    /// Align `latest_block_context` to the chain's current block.
    ///
    /// Without this, `latest_block_context().tblock` defaults to
    /// `SystemTime::now()` (wall-clock) via the library fallback, and the
    /// library's `pay_fees` picks up that wall-clock value as the DUST
    /// transaction's `ctime`. Chain-side validation does
    /// `root_history.get(tx.ctime)` with predecessor-by-time semantics — so
    /// with a wall-clock-future ctime, chain returns its LATEST block root,
    /// which diverges from whatever block our wallet last synced to. That
    /// mismatch causes MalformedTransaction::InvalidDustSpendProof (error 170).
    ///
    /// Passing chain's actual latest block tblock (fetched via indexer) keeps
    /// our declared ctime and chain's root-history lookup anchored to the
    /// same block, and makes the DUST-spend zk-proof's commitment_root PI
    /// match what chain recomputes.
    pub fn set_latest_block(
        &self,
        block_tblock_secs: u64,
        parent_block_hash: [u8; 32],
        last_block_time_secs: u64,
    ) {
        use midnight_node_ledger_helpers::{
            make_block_context, HashOutput, SerdeTransaction, Signature, Timestamp,
        };
        let block_context = make_block_context(
            Timestamp::from_secs(block_tblock_secs),
            HashOutput(parent_block_hash),
            Timestamp::from_secs(last_block_time_secs),
        );
        // Empty-tx update_from_block sets `latest_block_context` and advances
        // `process_ttls` on every wallet. `post_block_update` reruns on an
        // already-rehashed tree (idempotent) and appends a new root_history
        // entry at this tblock — harmless for our local checks.
        let empty_txs: Vec<
            SerdeTransaction<Signature, midnight_node_ledger_helpers::ProofMarker, DefaultDB>,
        > = Vec::new();
        self.context
            .update_from_block(&empty_txs, &block_context, None, None);
        info!(
            tblock = block_tblock_secs,
            "LedgerContext latest_block_context aligned to chain block"
        );
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

        // Read the full ledger state from on-chain storage.
        // The state_key() returns a Substrate storage key (not the state itself).
        // We use it to fetch the actual serialized LedgerState bytes via
        // the Subxt storage API which handles SCALE decoding.
        //
        // The state_key contains the raw serialized LedgerState as stored
        // in the Midnight pallet's storage. The .0 field is the raw bytes.
        let state_query = mn_meta::storage().midnight().state_key();
        let storage = api
            .storage()
            .at_latest()
            .await
            .map_err(|e| LedgerContextError::ContextError(format!("Subxt at_latest: {e}")))?;

        // Use fetch_raw to get the raw storage bytes.
        // Subxt's address() gives us the storage key, and fetch_raw
        // returns the raw SCALE-encoded value.
        let raw_bytes = storage
            .fetch_raw(state_query.to_root_bytes())
            .await
            .map_err(|e| LedgerContextError::ContextError(format!("fetch_raw failed: {e}")))?;

        match raw_bytes {
            Some(bytes) => {
                info!(
                    network_id = %network_id,
                    raw_bytes = bytes.len(),
                    "Read raw ledger state from chain storage"
                );

                // The raw storage value contains the SCALE-encoded LedgerState.
                // Try to deserialize with the tagged format that
                // update_ledger_state_from_bytes expects.
                // If the raw bytes start with the midnight tag, use directly.
                // Otherwise, fall back to parameters-only bootstrap.
                let tag_prefix = b"midnight:ledger-state";
                if bytes.len() > 20 && bytes.starts_with(tag_prefix) {
                    self.context.update_ledger_state_from_bytes(&bytes);
                    info!("LedgerContext bootstrapped with full chain state");
                } else {
                    warn!(
                        prefix = hex::encode(&bytes[..bytes.len().min(30)]),
                        "Storage bytes don't have expected tag, falling back to params"
                    );
                    Self::bootstrap_params_only(&api, &self.context, &network_id).await?;
                }
            }
            None => {
                warn!("No ledger state in storage, using block sync");
                Self::bootstrap_params_only(&api, &self.context, &network_id).await?;
            }
        }

        // Block sync disabled — panics on DUST-spending transactions.
        // DUST state is synced via event replay + tree sync instead.

        Ok(())
    }

    async fn bootstrap_params_only(
        api: &subxt::OnlineClient<subxt::PolkadotConfig>,
        context: &Arc<LedgerContext<DefaultDB>>,
        network_id: &str,
    ) -> Result<(), LedgerContextError> {
        let params_call = mn_meta::apis()
            .midnight_runtime_api()
            .get_ledger_parameters();
        let params_bytes = api
            .runtime_api()
            .at_latest()
            .await
            .map_err(|e| LedgerContextError::ContextError(format!("Subxt: {e}")))?
            .call(params_call)
            .await
            .map_err(|e| LedgerContextError::ContextError(format!("params: {e}")))?
            .map_err(|e| LedgerContextError::ContextError(format!("params err: {e:?}")))?;

        let parameters: LedgerParameters =
            midnight_node_ledger_helpers::deserialize(&mut &params_bytes[..])
                .map_err(|e| LedgerContextError::DeserializationError(format!("params: {e}")))?;

        use midnight_node_ledger_helpers::LedgerState;
        let mut new_state = LedgerState::<DefaultDB>::new(network_id);
        new_state.parameters = midnight_node_ledger_helpers::Sp::new(parameters);

        let bytes = midnight_node_ledger_helpers::serialize(&new_state)
            .map_err(|e| LedgerContextError::SerializationError(format!("serialize: {e}")))?;
        context.update_ledger_state_from_bytes(&bytes);

        info!("LedgerContext bootstrapped with parameters only");
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

        // Deserialize events up-front, then replay them as a SINGLE batch.
        //
        // DustLocalState::replay_events defers generation-tree collapses to the
        // end of the batch (see midnight-ledger dust.rs:1691 comment: "carry
        // out generation collapses *after* applying all the events, because
        // otherwise we might not have information around to process partial
        // dtime updates…"). Per-event invocation would collapse slots
        // prematurely, corrupting subsequent DustGenerationDtimeUpdate events
        // and yielding a tree root that never existed on chain — which is the
        // InvalidDustSpendProof / error 170 we hit on submit.
        let mut applied = 0u64;
        let mut errors = 0u64;
        let mut first_errors: Vec<(usize, String)> = Vec::new();
        let mut events: Vec<Event<DefaultDB>> = Vec::with_capacity(raw_events.len());

        for (idx, raw_hex) in raw_events.iter().enumerate() {
            let bytes = match hex::decode(raw_hex.trim_start_matches("0x")) {
                Ok(b) => b,
                Err(e) => {
                    errors += 1;
                    if first_errors.len() < 5 {
                        first_errors.push((idx, format!("hex: {e}")));
                    }
                    continue;
                }
            };
            match midnight_node_ledger_helpers::deserialize::<Event<DefaultDB>, _>(&mut &bytes[..])
            {
                Ok(e) => events.push(e),
                Err(e) => {
                    errors += 1;
                    if first_errors.len() < 5 {
                        first_errors.push((idx, format!("deserialize: {e}")));
                    }
                }
            }
        }

        self.context
            .with_wallet_from_seed(self.wallet_seed.clone(), |wallet| {
                // Single batched replay — generation collapses stay deferred.
                match wallet.update_dust_from_tx(&events) {
                    Ok(()) => applied = events.len() as u64,
                    Err(e) => {
                        errors += events.len() as u64;
                        first_errors.push((0, format!("batch replay: {e:?}")));
                    }
                }

                // Advance DUST timing to current block
                let tblock = self.context.latest_block_context().tblock;
                wallet.dust.process_ttls(tblock);
            });

        // Peek at the wallet's DUST state AFTER replay. Also capture tree roots
        // so they can be diffed against the chain's `Dust.root_history` storage
        // — mismatch means our root never was a chain block root (H1/H2 from
        // the InvalidDustSpendProof diagnostic plan).
        let (utxo_count, comm_first_free, gen_first_free, comm_root_hex, gen_root_hex) = self
            .context
            .with_wallet_from_seed(self.wallet_seed.clone(), |wallet| {
                wallet
                    .dust
                    .dust_local_state
                    .as_ref()
                    .map(|s| {
                        let comm_root = s
                            .commitment_tree
                            .root()
                            .map(|r| {
                                hex::encode(
                                    midnight_node_ledger_helpers::serialize(&r).unwrap_or_default(),
                                )
                            })
                            .unwrap_or_else(|| "<empty>".into());
                        let gen_root = s
                            .generating_tree
                            .root()
                            .map(|r| {
                                hex::encode(
                                    midnight_node_ledger_helpers::serialize(&r).unwrap_or_default(),
                                )
                            })
                            .unwrap_or_else(|| "<empty>".into());
                        (
                            s.utxos().count(),
                            s.commitment_tree_first_free,
                            s.generating_tree_first_free,
                            comm_root,
                            gen_root,
                        )
                    })
                    .unwrap_or((0, 0, 0, String::new(), String::new()))
            });

        info!(
            applied,
            errors,
            total = raw_events.len(),
            utxos_after = utxo_count,
            commitment_tree_first_free = comm_first_free,
            generating_tree_first_free = gen_first_free,
            commitment_root = %comm_root_hex,
            generating_root = %gen_root_hex,
            first_errors = ?first_errors,
            "Applied DUST events to wallet"
        );

        // Sync the wallet's DUST trees into the LedgerState so that
        // proof verification can find matching commitment roots.
        if applied > 0 {
            if let Err(e) = self.sync_dust_trees_to_ledger() {
                warn!(error = %e, "Failed to sync DUST trees to ledger");
            }
        }

        Ok(())
    }

    /// Sync the wallet's DUST merkle trees into the LedgerState.
    ///
    /// Copies commitment_tree and generating_tree from the wallet's
    /// DustLocalState into the LedgerState's DustState, and adds the
    /// current roots to root_history for proof verification.
    fn sync_dust_trees_to_ledger(&self) -> Result<(), LedgerContextError> {
        use midnight_node_ledger_helpers::Sp;

        // Read wallet's DUST tree data AND params (fields are pub via patched crate).
        // Params must be synced too: `ParamChange` events update
        // `DustLocalState.params` during replay but NOT `LedgerState.parameters.dust`,
        // so the chain's current generation rate / caps only live in the wallet.
        // `speculative_spend` reads `state.parameters.dust`; without propagating we
        // compute `updated_value=0` for a UTXO that wallet_balance correctly sums,
        // producing "Insufficient DUST" on pay_fees.
        let wallet_dust = self
            .context
            .with_wallet_from_seed(self.wallet_seed.clone(), |wallet| {
                wallet.dust.dust_local_state.as_ref().map(|state| {
                    (
                        state.commitment_tree.clone(),
                        state.commitment_tree_first_free,
                        state.generating_tree.clone(),
                        state.generating_tree_first_free,
                        state.params.clone(),
                    )
                })
            });

        let Some((commitment_tree, commitment_ff, generating_tree, generating_ff, dust_params)) =
            wallet_dust
        else {
            debug!("No DUST local state to sync");
            return Ok(());
        };

        // Deserialize current LedgerState, update DUST fields, re-inject
        let current_bytes = self.serialize_state()?;
        let mut state: midnight_node_ledger_helpers::LedgerState<DefaultDB> =
            midnight_node_ledger_helpers::deserialize(current_bytes.as_slice())
                .map_err(|e| LedgerContextError::DeserializationError(e.to_string()))?;

        // Propagate wallet's DUST params to LedgerState so downstream
        // speculative_spend / updated_value calls agree with wallet_balance.
        let mut params = (*state.parameters).clone();
        params.dust = dust_params;
        state.parameters = Sp::new(params);

        let mut dust_state = (*state.dust).clone();

        // Copy commitment tree
        dust_state.utxo.commitments = commitment_tree;
        dust_state.utxo.commitments_first_free = commitment_ff;

        // Add current root to root_history for proof verification
        let tblock = self.context.latest_block_context().tblock;
        if let Some(commit_root) = dust_state.utxo.commitments.root() {
            dust_state.utxo.root_history = dust_state.utxo.root_history.insert(tblock, commit_root);
        }

        // Copy generating tree
        dust_state.generation.generating_tree = generating_tree;
        dust_state.generation.generating_tree_first_free = generating_ff;

        if let Some(gen_root) = dust_state.generation.generating_tree.root() {
            dust_state.generation.root_history =
                dust_state.generation.root_history.insert(tblock, gen_root);
        }

        state.dust = Sp::new(dust_state);

        // Re-serialize and inject
        let new_bytes = midnight_node_ledger_helpers::serialize(&state)
            .map_err(|e| LedgerContextError::SerializationError(e.to_string()))?;
        self.context.update_ledger_state_from_bytes(&new_bytes);

        info!(
            commitment_first_free = commitment_ff,
            generating_first_free = generating_ff,
            "Synced DUST trees to LedgerState"
        );

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

    /// Sum the spendable DUST across the wallet's DustLocalState.
    ///
    /// DUST amounts grow over time, so each UTXO's current value depends
    /// on the evaluation timestamp — we use the latest observed block's
    /// `tblock`, matching what `speculative_spend` uses during fee payment.
    /// Returns 0 when DUST state is absent (no events replayed yet).
    pub fn dust_balance(&self) -> u128 {
        use midnight_node_ledger_helpers::Timestamp;
        use std::time::{SystemTime, UNIX_EPOCH};

        let ctime = Timestamp::from_secs(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        );
        let tblock_fallback = self.context.latest_block_context().tblock;

        self.context
            .with_wallet_from_seed(self.wallet_seed.clone(), |wallet| {
                let Some(state) = wallet.dust.dust_local_state.as_ref() else {
                    debug!("dust_balance: no DustLocalState");
                    return 0u128;
                };
                let utxo_count = state.utxos().count();
                let balance = state.wallet_balance(ctime);
                debug!(
                    utxo_count,
                    balance,
                    ctime = ?ctime,
                    tblock_fallback = ?tblock_fallback,
                    "dust_balance computed"
                );
                balance
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
