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
    ///
    /// **Legacy constructor** — each instance owns its own `LedgerContext`,
    /// so concurrent relayers on the same network do NOT share tree state.
    /// Retained for existing tests; production code should use
    /// [`from_shared_context`](Self::from_shared_context) and let the
    /// process-wide `SharedDustSyncTask` own the context.
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

    /// Build a manager backed by a shared `LedgerContext`.
    ///
    /// The caller must have already registered `seed` with the context (via
    /// `LedgerContext::new_from_wallet_seeds(... &[...seed...])`). The
    /// process-wide shared task maintains the context's DUST state; this
    /// manager is a per-relayer handle that scopes reads to `seed`.
    pub fn from_shared_context(
        seed: WalletSeed,
        shared: Arc<LedgerContext<DefaultDB>>,
        network_id: &str,
    ) -> Self {
        Self {
            context: shared,
            wallet_seed: seed,
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

    /// Apply an observed transaction to the wallet's shielded state ONLY,
    /// bypassing the strict full-tx verification path.
    ///
    /// `update_from_tx` strict-verifies every proof in a tx (DUST, signatures,
    /// native, contract). For txs **we observe** off the indexer (someone
    /// else's tx, already accepted by the chain), that strict re-verification
    /// is needless and fails on `InvalidDustSpendProof` because the
    /// LedgerContext's local DUST/tblock state can't reproduce the proof's
    /// expected reference state.
    ///
    /// This method takes the same path the helpers' `update_from_tx` takes
    /// AFTER successful verification: extract the shielded offers, apply
    /// them directly to each registered wallet's `shielded.state` via
    /// `Wallet::update_state_from_offers`. The chain has already validated
    /// the tx; we just need the offers' coin commitments to land in the
    /// wallet's state so they're spendable. Mirrors the TS reference
    /// `CoreWallet.replayEventsWithChanges` shape.
    ///
    /// Returns the number of offers applied.
    pub fn apply_observed_tx_offers(&self, raw_hex: &str) -> Result<usize, LedgerContextError> {
        use midnight_node_ledger_helpers::Transaction;

        let bytes = hex::decode(raw_hex.trim_start_matches("0x"))
            .map_err(|e| LedgerContextError::DeserializationError(format!("hex decode: {e}")))?;
        let tx: MnFinalizedTransaction =
            midnight_node_ledger_helpers::deserialize(bytes.as_slice()).map_err(|e| {
                LedgerContextError::DeserializationError(format!("tx deserialize: {e}"))
            })?;

        // Only Standard txs carry shielded offers; ClaimRewards has none.
        let stx = match &tx {
            Transaction::Standard(stx) => stx,
            Transaction::ClaimRewards(_) => return Ok(0),
        };

        // Collect every shielded offer in the tx: the guaranteed slot plus
        // all fallible segments. We apply them all unconditionally — the
        // indexer only delivers txs the chain accepted, so partial-success
        // filtering on our side isn't necessary for state-tracking.
        let mut offers = Vec::new();
        if let Some(guaranteed) = &stx.guaranteed_coins {
            offers.push((**guaranteed).clone());
        }
        for entry in stx.fallible_coins.iter() {
            // storage::Map's iter yields `(K, Sp<V>)` — single deref to
            // unwrap Sp, then clone the inner Offer.
            offers.push((*entry.1).clone());
        }
        if offers.is_empty() {
            return Ok(0);
        }
        let n = offers.len();

        // Apply to OUR wallet's shielded state. `update_state_from_offers`
        // calls `state.apply(secret_keys, offer)` per offer — decrypts
        // outputs the secret keys can read, registers commitments, etc.
        // No proof verification.
        self.context
            .with_wallet_from_seed(self.wallet_seed.clone(), |wallet| {
                wallet.update_state_from_offers(&offers);
            });

        debug!(
            network_id = %self.network_id,
            offers_applied = n,
            "Applied observed tx offers to wallet shielded state"
        );

        Ok(n)
    }

    /// Sum the wallet's shielded coin balances by token type.
    ///
    /// Reads the wallet's `shielded.state.coins` map (the spendable set),
    /// excludes coins flagged as pending-spend (already nullified in an
    /// in-flight tx the indexer hasn't echoed back yet), and aggregates
    /// per `ShieldedTokenType`. Token types are returned as 64-char hex.
    pub fn shielded_balances(&self) -> std::collections::HashMap<String, u128> {
        let mut totals: std::collections::HashMap<String, u128> = std::collections::HashMap::new();
        self.context
            .with_wallet_from_seed(self.wallet_seed.clone(), |wallet| {
                for entry in wallet.shielded.state.coins.iter() {
                    let nullifier = entry.0;
                    // Skip coins that already have a pending nullifier (we've
                    // built a tx spending them but the chain hasn't confirmed
                    // it yet) — same logic the TS reference's
                    // `availableCoins` filter applies via `pendingSpends`.
                    if wallet
                        .shielded
                        .state
                        .pending_spends
                        .contains_key(&nullifier)
                    {
                        continue;
                    }
                    let qcoin = &entry.1;
                    let token_hex = hex::encode(qcoin.type_.0 .0);
                    let total = totals.entry(token_hex).or_insert(0u128);
                    *total = total.saturating_add(qcoin.value);
                }
            });
        totals
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
            HashOutput, IntentHash, Sp, Timestamp, UserAddress, Utxo, NIGHT,
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

/// Sync a wallet's DUST trees + params into the shared `LedgerState`.
///
/// `ParamChange` events update `DustLocalState.params` but NOT
/// `LedgerState.parameters.dust`, so the chain's current generation rate /
/// caps only live in the wallet — which means `speculative_spend` (reads
/// `state.parameters.dust`) computes `updated_value=0` unless we propagate.
///
/// Commitment root / generation root are keyed into `root_history[tblock]`
/// using the currently-latest block tblock — this is what the chain's
/// `dust_spend_check(ctime)` predecessor-lookup needs to find our wallet's
/// tree state.
pub fn sync_dust_trees_to_ledger_ctx(
    context: &Arc<LedgerContext<DefaultDB>>,
    seed: &WalletSeed,
) -> Result<(), LedgerContextError> {
    use midnight_node_ledger_helpers::Sp;

    let wallet_dust = context.with_wallet_from_seed(seed.clone(), |wallet| {
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

    let current_bytes = context
        .with_ledger_state(|state| midnight_node_ledger_helpers::serialize(state))
        .map_err(|e| LedgerContextError::SerializationError(e.to_string()))?;
    let mut state: midnight_node_ledger_helpers::LedgerState<DefaultDB> =
        midnight_node_ledger_helpers::deserialize(current_bytes.as_slice())
            .map_err(|e| LedgerContextError::DeserializationError(e.to_string()))?;

    let mut params = (*state.parameters).clone();
    params.dust = dust_params;
    state.parameters = Sp::new(params);

    let mut dust_state = (*state.dust).clone();

    dust_state.utxo.commitments = commitment_tree;
    dust_state.utxo.commitments_first_free = commitment_ff;

    let tblock = context.latest_block_context().tblock;
    if let Some(commit_root) = dust_state.utxo.commitments.root() {
        dust_state.utxo.root_history = dust_state.utxo.root_history.insert(tblock, commit_root);
    }

    dust_state.generation.generating_tree = generating_tree;
    dust_state.generation.generating_tree_first_free = generating_ff;

    if let Some(gen_root) = dust_state.generation.generating_tree.root() {
        dust_state.generation.root_history =
            dust_state.generation.root_history.insert(tblock, gen_root);
    }

    state.dust = Sp::new(dust_state);

    let new_bytes = midnight_node_ledger_helpers::serialize(&state)
        .map_err(|e| LedgerContextError::SerializationError(e.to_string()))?;
    context.update_ledger_state_from_bytes(&new_bytes);

    debug!(
        commitment_first_free = commitment_ff,
        generating_first_free = generating_ff,
        "Synced DUST trees to LedgerState"
    );

    Ok(())
}
