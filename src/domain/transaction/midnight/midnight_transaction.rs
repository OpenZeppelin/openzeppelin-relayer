use std::sync::Arc;

use async_trait::async_trait;
use tracing::{debug, info, instrument, warn};

use crate::{
    domain::Transaction,
    jobs::{JobProducerTrait, StatusCheckContext, TransactionSend, TransactionStatusCheck},
    models::{
        MidnightNetwork, NetworkTransactionRequest, NetworkType, RelayerRepoModel,
        TransactionError, TransactionRepoModel, TransactionStatus, TransactionUpdateRequest,
    },
    repositories::{
        RelayerRepository, RelayerStateRepositoryStorage, Repository, SyncStateTrait,
        TransactionRepository,
    },
    services::{
        provider::MidnightProviderTrait,
        signer::MidnightSigner,
        sync::midnight::{indexer::ApplyStage, LedgerContextManager, SyncManager},
    },
    utils::calculate_scheduled_timestamp,
};

/// Midnight transaction handler implementing the full lifecycle:
/// prepare → submit → check status.
///
/// Key differences from other networks:
/// - **No cancellation/replacement** — Midnight doesn't support nonce-based replacement
/// - **Dual hash tracking** — both extrinsic_tx_hash (Substrate) and pallet_tx_hash (application)
/// - **Status via indexer** — transaction status is queried from the GraphQL indexer, not RPC
pub struct MidnightTransaction<P, TR, RR, J, SS = RelayerStateRepositoryStorage>
where
    P: MidnightProviderTrait + Send + Sync,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync,
    J: JobProducerTrait + Send + Sync,
    SS: SyncStateTrait + Send + Sync,
{
    pub relayer: RelayerRepoModel,
    pub network: MidnightNetwork,
    pub provider: Arc<P>,
    pub signer: Arc<MidnightSigner>,
    pub sync_manager: SyncManager<SS>,
    pub ledger_ctx: Arc<LedgerContextManager>,
    pub transaction_repository: Arc<TR>,
    pub relayer_repository: Arc<RR>,
    pub job_producer: Arc<J>,
}

/// Type alias for the default concrete transaction handler.
pub type DefaultMidnightTransaction = MidnightTransaction<
    crate::services::provider::MidnightProvider,
    crate::repositories::TransactionRepositoryStorage,
    crate::repositories::RelayerRepositoryStorage,
    crate::jobs::JobProducer,
    RelayerStateRepositoryStorage,
>;

impl<P, TR, RR, J, SS> MidnightTransaction<P, TR, RR, J, SS>
where
    P: MidnightProviderTrait + Send + Sync,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync,
    J: JobProducerTrait + Send + Sync,
    SS: SyncStateTrait + Send + Sync,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        relayer: RelayerRepoModel,
        network: MidnightNetwork,
        provider: Arc<P>,
        signer: Arc<MidnightSigner>,
        sync_manager: SyncManager<SS>,
        ledger_ctx: Arc<LedgerContextManager>,
        transaction_repository: Arc<TR>,
        relayer_repository: Arc<RR>,
        job_producer: Arc<J>,
    ) -> Result<Self, TransactionError> {
        Ok(Self {
            relayer,
            network,
            provider,
            signer,
            sync_manager,
            ledger_ctx,
            transaction_repository,
            relayer_repository,
            job_producer,
        })
    }

    /// Schedule a status check job for a submitted transaction.
    async fn schedule_status_check(
        &self,
        tx: &TransactionRepoModel,
    ) -> Result<(), TransactionError> {
        // Use absolute timestamp consistent with EVM/Stellar patterns
        let delay_seconds = self
            .network
            .average_blocktime()
            .map(|d| d.as_secs() as i64)
            .unwrap_or(6);

        self.job_producer
            .produce_check_transaction_status_job(
                TransactionStatusCheck::new(
                    tx.id.clone(),
                    self.relayer.id.clone(),
                    NetworkType::Midnight,
                ),
                Some(calculate_scheduled_timestamp(delay_seconds)),
            )
            .await
            .map_err(|e| {
                TransactionError::UnexpectedError(format!("Failed to enqueue status check: {e}"))
            })?;

        Ok(())
    }
}

#[async_trait]
impl<P, TR, RR, J, SS> Transaction for MidnightTransaction<P, TR, RR, J, SS>
where
    P: MidnightProviderTrait + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    J: JobProducerTrait + Send + Sync + 'static,
    SS: SyncStateTrait + Send + Sync + 'static,
{
    #[instrument(
        level = "debug",
        skip(self, tx),
        fields(tx_id = %tx.id, relayer_id = %tx.relayer_id)
    )]
    async fn prepare_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        debug!(tx_id = %tx.id, "Preparing Midnight transaction");

        let midnight_data = tx.network_data.get_midnight_transaction_data()?;

        // Build the transaction using StandardTrasactionInfo.
        //
        // The flow:
        // 1. Create StandardTrasactionInfo with LedgerContext + proof provider
        // 2. Set unshielded offer from the request's inputs/outputs
        // 3. Call .build() which: computes TTL, builds offers, pays DUST fees, proves
        // 4. Serialize the proven transaction
        //
        // PREREQUISITE: The LedgerContext must have the current chain state
        // (parameters, network_id, UTXOs). This is populated during relayer
        // initialization via the shielded/unshielded sync, and the ledger state
        // can also be bootstrapped from the RPC node.
        let context = self.ledger_ctx.context().clone();

        // Check if the context has a valid network_id (indicates it was populated)
        let has_state = context.with_ledger_state(|state| !state.network_id.is_empty());

        if !has_state {
            return Err(TransactionError::NotSupported(
                "LedgerContext has no chain state. The relayer must complete \
                 initial sync before transactions can be prepared. Ensure the \
                 node RPC is reachable and the indexer sync completed."
                    .into(),
            ));
        }

        // Create proof provider and transaction builder
        use midnight_node_ledger_helpers::{FromContext, IntentInfo, StandardTrasactionInfo};

        let proof_server = Arc::new(crate::services::provider::RemoteProofServer::new(
            self.network.prover_url.clone(),
        ));

        // Diagnostic: log the `ctime` that pay_fees will use for DUST proof
        // construction. `pay_fees` internally does
        // `let now = self.context.latest_block_context().tblock;`
        // which is our LedgerContext's `latest_block_context().tblock`. The
        // chain-side proof verifier uses the block's actual tblock when
        // validating — if ours and chain's disagree, the zk public-input
        // `ctime` differs and the proof fails with InvalidDustSpendProof
        // (error 170). Log both our value and wall-clock so we can diff.
        let prepare_tblock = context.latest_block_context().tblock;
        let wall_now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let chain_latest = self
            .provider
            .get_indexer_client()
            .get_latest_block()
            .await
            .ok()
            .flatten();
        info!(
            tx_id = %tx.id,
            prepare_tblock = ?prepare_tblock,
            wall_now_secs,
            chain_latest_height = ?chain_latest.as_ref().and_then(|b| b.height),
            chain_latest_timestamp = ?chain_latest.as_ref().and_then(|b| b.timestamp),
            "ctime sources at prepare start"
        );

        if let Some(tblock_ms) = chain_latest.as_ref().and_then(|b| b.timestamp) {
            self.ledger_ctx
                .refresh_dust_spend_state(tblock_ms / 1000)
                .map_err(|e| {
                    TransactionError::UnexpectedError(format!(
                        "Failed to refresh DUST spend state: {e}"
                    ))
                })?;
        }

        let wallet_seed = self.ledger_ctx.wallet_seed().clone();
        let build_context = self.ledger_ctx.transaction_build_context().map_err(|e| {
            TransactionError::UnexpectedError(format!(
                "Failed to create isolated transaction build context: {e}"
            ))
        })?;
        let mut tx_info =
            StandardTrasactionInfo::new_from_context(build_context.clone(), proof_server, None);

        // Pay DUST fees from the wallet. Requires DUST to have been
        // registered and generated via the midnight-dust-generator tool.
        tx_info.set_funding_seeds(vec![wallet_seed.clone()]);

        // DUST trees are synced from wallet to LedgerState via the patched
        // midnight-ledger crate (DustLocalState fields made pub).

        // Build intents from the request's three surfaces:
        //
        // 1. `guaranteed_offer` — sugar for a single fallible unshielded offer
        //    at `GUARANTEED_OFFER_SEGMENT_ID` (segment 1). Named "guaranteed"
        //    at the API layer for caller-facing simplicity, but routed through
        //    a fallible segment because DUST fee payment requires it.
        // 2. `fallible_offers[]` — one intent per offer, each at its own
        //    segment_id, with `fallible_unshielded_offer: Some(...)`.
        // 3. `intents[]` — general case; callers choose which offer slots to
        //    populate. `guaranteed_unshielded_offer` + `fallible_unshielded_offer`
        //    within the same intent are both allowed by the library.
        //
        // Shielded offers and contract actions are rejected at the API layer
        // in PR-1; PR-2 lifts the action gate, PR-3 lifts the shielded gate.
        use crate::models::transaction::request::midnight::GUARANTEED_OFFER_SEGMENT_ID;

        if let Some(offer_req) = midnight_data.guaranteed_offer.as_ref() {
            if offer_req.is_shielded() {
                // Route shielded top-level offer to the tx's guaranteed
                // shielded slot (`StandardTransaction.guaranteed_coins`).
                // `validate()` guarantees this is the only place shielded
                // can appear in PR-3 v1, so there's no collision to guard.
                let shielded =
                    build_shielded_offer(offer_req, &wallet_seed, build_context.clone())?;
                tx_info.set_guaranteed_offer(shielded);
            } else {
                let offer = build_unshielded_offer(offer_req, &wallet_seed, build_context.clone())?;
                tx_info.add_intent(
                    GUARANTEED_OFFER_SEGMENT_ID,
                    Box::new(top_level_unshielded_intent(offer)),
                );
            }
        }

        for fallible in &midnight_data.fallible_offers {
            let offer =
                build_unshielded_offer(&fallible.offer, &wallet_seed, build_context.clone())?;
            tx_info.add_intent(
                fallible.segment_id,
                Box::new(IntentInfo {
                    guaranteed_unshielded_offer: None,
                    fallible_unshielded_offer: Some(offer),
                    actions: vec![],
                }),
            );
        }

        for intent in &midnight_data.intents {
            let guaranteed = intent
                .guaranteed_unshielded_offer
                .as_ref()
                .map(|o| build_unshielded_offer(o, &wallet_seed, build_context.clone()))
                .transpose()?;
            let fallible = intent
                .fallible_unshielded_offer
                .as_ref()
                .map(|o| build_unshielded_offer(o, &wallet_seed, build_context.clone()))
                .transpose()?;
            tx_info.add_intent(
                intent.segment_id,
                Box::new(IntentInfo {
                    guaranteed_unshielded_offer: guaranteed,
                    fallible_unshielded_offer: fallible,
                    actions: vec![],
                }),
            );
        }

        // PR-3 v2: fallible shielded offers — build each entry with
        // retargeting BuildInput/BuildOutput wrappers (§11 / §12 of the
        // architecture doc). These bypass the helpers' Segment::Guaranteed
        // hardcode by wrapping helpers' InputInfo/OutputInfo::build and
        // calling `retarget_segment(segment_id)` on the produced
        // zswap Input/Output. The resulting OfferInfo's Offer has its
        // Input/Output segment correctly tagged as fallible-N.
        if !midnight_data.fallible_shielded_offers.is_empty() {
            let mut shielded_fallible_map: std::collections::HashMap<
                u16,
                midnight_node_ledger_helpers::OfferInfo<midnight_node_ledger_helpers::DefaultDB>,
            > = std::collections::HashMap::new();
            for fallible in &midnight_data.fallible_shielded_offers {
                let offer = build_fallible_shielded_offer(
                    &fallible.offer,
                    &wallet_seed,
                    fallible.segment_id,
                    build_context.clone(),
                )?;
                shielded_fallible_map.insert(fallible.segment_id, offer);
            }
            tx_info.set_fallible_offers(shielded_fallible_map);
        }

        if tx_info.is_empty() {
            return Err(TransactionError::ValidationError(
                "Transaction has no offers or intents to build".into(),
            ));
        }

        // Build and prove the transaction.
        //
        // `RemoteProofServer::prove` panics on proof-server failures because
        // the library's `ProofProvider` trait signature is infallible. We
        // wrap the library's `tx_info.prove()` in `catch_unwind` so those
        // panics surface as normal job errors (retryable / dead-letter-able)
        // instead of crashing the worker task. See proof_server.rs for the
        // trait-signature constraint driving this pattern.
        use futures::FutureExt;
        use std::panic::AssertUnwindSafe;

        info!(tx_id = %tx.id, "Building and proving Midnight transaction");
        let proven_tx = match AssertUnwindSafe(tx_info.prove()).catch_unwind().await {
            Ok(Ok(proven)) => proven,
            Ok(Err(e)) => {
                return Err(TransactionError::UnexpectedError(format!(
                    "Transaction build/prove failed: {e}"
                )));
            }
            Err(payload) => {
                let msg = payload
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| {
                        payload
                            .downcast_ref::<&'static str>()
                            .map(|s| (*s).to_string())
                    })
                    .unwrap_or_else(|| "<non-string panic payload>".into());
                return Err(TransactionError::UnexpectedError(format!(
                    "Proof generation panicked (likely proof-server failure): {msg}"
                )));
            }
        };

        let pending_unshielded_keys = collect_unshielded_spend_keys(&proven_tx);

        // Serialize using tagged serialization
        let serialized_bytes =
            midnight_node_ledger_helpers::serialize(&proven_tx).map_err(|e| {
                TransactionError::UnexpectedError(format!("Transaction serialization failed: {e}"))
            })?;

        let serialized_hex = hex::encode(&serialized_bytes);

        // Compute the pallet transaction hash
        let tx_hash = proven_tx.transaction_hash();
        let pallet_hash = hex::encode(tx_hash.0 .0);

        info!(
            tx_id = %tx.id,
            serialized_len = serialized_bytes.len(),
            pallet_hash = %pallet_hash,
            "Midnight transaction prepared and serialized"
        );

        // Store the pallet-level (application) hash in `hash` — the value
        // users paste into midnightexplorer.com. Matches EVM/Stellar semantics
        // of "`hash` = explorer-searchable identifier". The substrate extrinsic
        // hash is stored separately in `extrinsic_hash` at submit time.
        let updated_network_data = crate::models::MidnightTransactionData {
            hash: Some(pallet_hash),
            extrinsic_hash: None,
            block_hash: None,
            serialized_tx: Some(serialized_hex),
            guaranteed_offer: midnight_data.guaranteed_offer.clone(),
            intents: midnight_data.intents.clone(),
            fallible_offers: midnight_data.fallible_offers.clone(),
            fallible_shielded_offers: midnight_data.fallible_shielded_offers.clone(),
            pending_unshielded_keys: pending_unshielded_keys.clone(),
        };

        self.transaction_repository
            .update_network_data(
                tx.id.clone(),
                crate::models::NetworkTransactionData::Midnight(updated_network_data),
            )
            .await
            .map_err(|e| TransactionError::UnexpectedError(e.to_string()))?;

        let updated = self
            .transaction_repository
            .partial_update(
                tx.id.clone(),
                TransactionUpdateRequest {
                    status: Some(TransactionStatus::Sent),
                    sent_at: Some(chrono::Utc::now().to_rfc3339()),
                    ..Default::default()
                },
            )
            .await
            .map_err(|e| TransactionError::UnexpectedError(e.to_string()))?;

        // Enqueue the submit job so the prepared transaction gets pushed to
        // the node. Every other network does this at the tail of prepare;
        // without it the tx would sit in Sent forever.
        self.job_producer
            .produce_submit_transaction_job(
                TransactionSend::submit(updated.id.clone(), updated.relayer_id.clone()),
                None,
            )
            .await
            .map_err(|e| {
                TransactionError::UnexpectedError(format!("Failed to enqueue submit job: {e}"))
            })?;

        if !pending_unshielded_keys.is_empty() {
            match self
                .sync_manager
                .mark_unshielded_pending_by_keys(&pending_unshielded_keys)
                .await
            {
                Ok(wallet_state) => {
                    if let Err(e) = self
                        .ledger_ctx
                        .reconcile_unshielded_utxos(&wallet_state.available_utxos)
                    {
                        warn!(
                            tx_id = %tx.id,
                            error = %e,
                            "failed to reconcile unshielded UTXOs after marking pending spends"
                        );
                    }
                }
                Err(e) => {
                    warn!(
                        tx_id = %tx.id,
                        error = %e,
                        "failed to mark unshielded UTXOs pending after prepare"
                    );
                }
            }
        }

        Ok(updated)
    }

    #[instrument(
        level = "debug",
        skip(self, tx),
        fields(tx_id = %tx.id, relayer_id = %tx.relayer_id)
    )]
    async fn submit_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        debug!(tx_id = %tx.id, "Submitting Midnight transaction");

        // Submit requires a serialized extrinsic in the network_data.
        // This is populated during prepare_transaction (which builds and proves the tx).
        // If prepare hasn't run or didn't produce serialized data, fail explicitly.
        let midnight_data = tx.network_data.get_midnight_transaction_data()?;

        let serialized_hex = midnight_data.serialized_tx.as_deref().ok_or_else(|| {
            TransactionError::NotSupported(
                "Midnight transaction has no serialized extrinsic. \
                     prepare_transaction must serialize the proven transaction first."
                    .into(),
            )
        })?;

        let result = self
            .provider
            .send_raw_extrinsic(serialized_hex)
            .await
            .map_err(|e| TransactionError::UnexpectedError(format!("Submit failed: {e}")))?;

        info!(
            tx_id = %tx.id,
            extrinsic_hash = %result.extrinsic_tx_hash,
            pallet_hash = ?result.pallet_tx_hash,
            "Midnight transaction submitted"
        );

        // Store extrinsic hash and pallet hash (if returned)
        let mut hashes = tx.hashes.clone();
        hashes.push(result.extrinsic_tx_hash.clone());

        // Preserve serialized_tx so resubmit paths can replay without re-proving.
        // `hash` (set during prepare as pallet_hash) is the explorer-searchable
        // identifier; the substrate extrinsic hash goes into `extrinsic_hash`
        // for node-level lookups.
        let updated_network_data = crate::models::MidnightTransactionData {
            hash: midnight_data.hash.clone().or(result.pallet_tx_hash.clone()),
            extrinsic_hash: Some(result.extrinsic_tx_hash.clone()),
            block_hash: midnight_data.block_hash.clone(),
            serialized_tx: midnight_data.serialized_tx.clone(),
            guaranteed_offer: midnight_data.guaranteed_offer.clone(),
            intents: midnight_data.intents.clone(),
            fallible_offers: midnight_data.fallible_offers.clone(),
            fallible_shielded_offers: midnight_data.fallible_shielded_offers.clone(),
            pending_unshielded_keys: midnight_data.pending_unshielded_keys.clone(),
        };

        self.transaction_repository
            .update_network_data(
                tx.id.clone(),
                crate::models::NetworkTransactionData::Midnight(updated_network_data),
            )
            .await
            .map_err(|e| TransactionError::UnexpectedError(e.to_string()))?;

        let updated = self
            .transaction_repository
            .partial_update(
                tx.id.clone(),
                TransactionUpdateRequest {
                    status: Some(TransactionStatus::Submitted),
                    hashes: Some(hashes),
                    ..Default::default()
                },
            )
            .await
            .map_err(|e| TransactionError::UnexpectedError(e.to_string()))?;

        // Schedule a status check
        self.schedule_status_check(&updated).await?;

        Ok(updated)
    }

    async fn resubmit_transaction(
        &self,
        _tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        Err(TransactionError::NotSupported(
            "Midnight does not support transaction resubmission".into(),
        ))
    }

    #[instrument(
        level = "debug",
        skip(self, tx, _context),
        fields(tx_id = %tx.id, relayer_id = %tx.relayer_id)
    )]
    async fn handle_transaction_status(
        &self,
        tx: TransactionRepoModel,
        _context: Option<StatusCheckContext>,
    ) -> Result<TransactionRepoModel, TransactionError> {
        let midnight_data = tx.network_data.get_midnight_transaction_data()?;

        // Preview indexer's `transactions(offset: { hash })` accepts ONLY the
        // pallet/application-level hash — querying by substrate extrinsic hash
        // returns an empty array. Use `midnight_data.hash` (the pallet hash
        // stored under the canonical `hash` field post-swap).
        let query_hash = midnight_data
            .hash
            .as_deref()
            .or(tx.hashes.last().map(String::as_str))
            .ok_or_else(|| {
                TransactionError::UnexpectedError(
                    "No hash available for status query. \
                     Transaction may not have been submitted successfully."
                        .into(),
                )
            })?;

        let indexer = self.provider.get_indexer_client();
        let tx_data = indexer
            .get_transaction_by_hash(query_hash)
            .await
            .map_err(|e| TransactionError::UnexpectedError(format!("Indexer query failed: {e}")))?;

        let (new_status, status_reason, block_hash) = match tx_data {
            Some(data) => {
                let block_hash = data.block_hash.clone();
                match data.apply_stage {
                    Some(ApplyStage::SucceedEntirely) => (
                        TransactionStatus::Confirmed,
                        Some("Transaction succeeded entirely".into()),
                        block_hash,
                    ),
                    Some(ApplyStage::SucceedPartially) => (
                        TransactionStatus::Failed,
                        Some("Transaction partially succeeded".into()),
                        block_hash,
                    ),
                    Some(ApplyStage::FailEntirely) => (
                        TransactionStatus::Failed,
                        Some("Transaction failed entirely".into()),
                        block_hash,
                    ),
                    Some(ApplyStage::Pending) | None => {
                        debug!(tx_id = %tx.id, "Transaction still pending, scheduling recheck");
                        self.schedule_status_check(&tx).await?;
                        return Ok(tx);
                    }
                }
            }
            None => {
                // Transaction not found in indexer yet — re-check later
                debug!(tx_id = %tx.id, "Transaction not found in indexer yet");
                self.schedule_status_check(&tx).await?;
                return Ok(tx);
            }
        };

        info!(
            tx_id = %tx.id,
            status = ?new_status,
            block_hash = ?block_hash,
            "Midnight transaction reached final state"
        );

        // Propagate block_hash into MidnightTransactionData so the API surfaces
        // where on-chain the tx landed. The indexer already returned it in the
        // status query; update_network_data is the only write path for it.
        if let Some(bh) = block_hash.as_ref() {
            let current = tx.network_data.get_midnight_transaction_data()?;
            let updated_network_data = crate::models::MidnightTransactionData {
                hash: current.hash.clone(),
                extrinsic_hash: current.extrinsic_hash.clone(),
                block_hash: Some(bh.clone()),
                serialized_tx: current.serialized_tx.clone(),
                guaranteed_offer: current.guaranteed_offer.clone(),
                intents: current.intents.clone(),
                fallible_offers: current.fallible_offers.clone(),
                fallible_shielded_offers: current.fallible_shielded_offers.clone(),
                pending_unshielded_keys: current.pending_unshielded_keys.clone(),
            };
            self.transaction_repository
                .update_network_data(
                    tx.id.clone(),
                    crate::models::NetworkTransactionData::Midnight(updated_network_data),
                )
                .await
                .map_err(|e| TransactionError::UnexpectedError(e.to_string()))?;
        }

        let confirmed_at = if new_status == TransactionStatus::Confirmed {
            Some(chrono::Utc::now().to_rfc3339())
        } else {
            None
        };

        if new_status == TransactionStatus::Failed
            && !midnight_data.pending_unshielded_keys.is_empty()
        {
            match self
                .sync_manager
                .release_unshielded_pending_by_keys(&midnight_data.pending_unshielded_keys)
                .await
            {
                Ok(wallet_state) => {
                    if let Err(e) = self
                        .ledger_ctx
                        .reconcile_unshielded_utxos(&wallet_state.available_utxos)
                    {
                        warn!(
                            tx_id = %tx.id,
                            error = %e,
                            "failed to reconcile unshielded UTXOs after failed transaction rollback"
                        );
                    }
                }
                Err(e) => {
                    warn!(
                        tx_id = %tx.id,
                        error = %e,
                        "failed to release pending unshielded UTXOs after failed transaction"
                    );
                }
            }
        }

        let updated = self
            .transaction_repository
            .partial_update(
                tx.id.clone(),
                TransactionUpdateRequest {
                    status: Some(new_status),
                    status_reason,
                    confirmed_at,
                    ..Default::default()
                },
            )
            .await
            .map_err(|e| TransactionError::UnexpectedError(e.to_string()))?;

        Ok(updated)
    }

    async fn cancel_transaction(
        &self,
        _tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        Err(TransactionError::NotSupported(
            "Midnight does not support transaction cancellation".into(),
        ))
    }

    async fn replace_transaction(
        &self,
        _old_tx: TransactionRepoModel,
        _new_tx_request: NetworkTransactionRequest,
    ) -> Result<TransactionRepoModel, TransactionError> {
        Err(TransactionError::NotSupported(
            "Midnight does not support transaction replacement".into(),
        ))
    }

    async fn sign_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        // Midnight transactions are signed as part of the prepare step
        // (ZK proof generation includes signing). This is a no-op.
        Ok(tx)
    }

    async fn validate_transaction(
        &self,
        _tx: TransactionRepoModel,
    ) -> Result<bool, TransactionError> {
        // Basic validation — a full implementation would check TTL, offer structure, etc.
        Ok(true)
    }
}

fn collect_unshielded_spend_keys(
    tx: &midnight_node_ledger_helpers::transaction::FinalizedTransaction<
        midnight_node_ledger_helpers::DefaultDB,
    >,
) -> Vec<String> {
    use midnight_node_ledger_helpers::Transaction;

    let Transaction::Standard(stx) = tx else {
        return Vec::new();
    };

    let mut keys = Vec::new();
    for entry in stx.intents.iter() {
        let intent = &*entry.1;
        for spend in intent
            .guaranteed_inputs()
            .into_iter()
            .chain(intent.fallible_inputs())
        {
            keys.push(format!(
                "{}#{}",
                hex::encode(spend.intent_hash.0 .0),
                spend.output_no
            ));
        }
    }
    keys
}

fn top_level_unshielded_intent(
    offer: midnight_node_ledger_helpers::UnshieldedOfferInfo<
        midnight_node_ledger_helpers::DefaultDB,
    >,
) -> midnight_node_ledger_helpers::IntentInfo<midnight_node_ledger_helpers::DefaultDB> {
    midnight_node_ledger_helpers::IntentInfo {
        guaranteed_unshielded_offer: Some(offer),
        fallible_unshielded_offer: None,
        actions: vec![],
    }
}

/// Translate one API `MidnightOfferRequest::Unshielded` into a library
/// `UnshieldedOfferInfo`, routing all spent inputs to the relayer's own
/// wallet seed and parsing destinations as bech32m unshielded addresses (or
/// the literal `"self"` loop-back).
///
/// Returns a `ValidationError` if the offer carries a shielded variant
/// (shouldn't happen post-`validate()`) or an input/output value that's not a
/// valid `u128`. The `token_type` field is presently hardcoded to `NIGHT`;
/// custom-token support lands in a follow-up once the builder-side parsing
/// helper exists.
fn build_unshielded_offer(
    offer: &crate::models::MidnightOfferRequest,
    wallet_seed: &midnight_node_ledger_helpers::WalletSeed,
    context: Arc<
        midnight_node_ledger_helpers::LedgerContext<midnight_node_ledger_helpers::DefaultDB>,
    >,
) -> Result<
    midnight_node_ledger_helpers::UnshieldedOfferInfo<midnight_node_ledger_helpers::DefaultDB>,
    TransactionError,
> {
    use crate::models::MidnightOfferRequest;
    use midnight_node_ledger_helpers::{
        BuildUtxoOutput, BuildUtxoSpend, DefaultDB, UnshieldedOfferInfo, UnshieldedWallet,
        UtxoOutputInfo, UtxoSpendInfo, WalletAddress, NIGHT,
    };
    use std::str::FromStr;

    let (inputs_req, outputs_req) = match offer {
        MidnightOfferRequest::Unshielded { inputs, outputs } => (inputs, outputs),
        MidnightOfferRequest::Shielded { .. } => {
            // Caller should dispatch to `build_shielded_offer` on
            // `offer.is_shielded()` — reaching here is a builder-dispatch
            // bug, not a user-input failure. Surface it clearly so the
            // test suite catches it instead of producing a wrong tx.
            return Err(TransactionError::UnexpectedError(
                "build_unshielded_offer called with a shielded offer variant".into(),
            ));
        }
    };

    let mut inputs: Vec<Box<dyn BuildUtxoSpend<DefaultDB>>> = Vec::with_capacity(inputs_req.len());
    let mut outputs: Vec<Box<dyn BuildUtxoOutput<DefaultDB>>> =
        Vec::with_capacity(outputs_req.len());

    let requested_input_total = inputs_req.iter().try_fold(0u128, |total, input| {
        let value = input.value.parse::<u128>().map_err(|_| {
            TransactionError::ValidationError(format!("Invalid input value: {}", input.value))
        })?;
        total
            .checked_add(value)
            .ok_or_else(|| TransactionError::ValidationError("Input value overflow".into()))
    })?;

    let mut requested_output_total = 0u128;
    for output in outputs_req {
        let value: u128 = output.value.parse().map_err(|_| {
            TransactionError::ValidationError(format!("Invalid output value: {}", output.value))
        })?;
        requested_output_total = requested_output_total
            .checked_add(value)
            .ok_or_else(|| TransactionError::ValidationError("Output value overflow".into()))?;
        if output.destination == "self" {
            outputs.push(Box::new(UtxoOutputInfo {
                value,
                owner: wallet_seed.clone(),
                token_type: NIGHT,
            }));
        } else {
            let wallet_addr = WalletAddress::from_str(&output.destination).map_err(|e| {
                TransactionError::ValidationError(format!(
                    "Invalid destination address {}: {e}",
                    output.destination
                ))
            })?;
            let unshielded = UnshieldedWallet::try_from(&wallet_addr).map_err(|e| {
                TransactionError::ValidationError(format!(
                    "Destination {} is not an unshielded address: {e:?}",
                    output.destination
                ))
            })?;
            outputs.push(Box::new(UtxoOutputInfo {
                value,
                owner: unshielded,
                token_type: NIGHT,
            }));
        }
    }

    if requested_output_total > requested_input_total {
        return Err(TransactionError::ValidationError(format!(
            "Unshielded outputs exceed inputs: outputs={requested_output_total}, inputs={requested_input_total}"
        )));
    }

    if requested_input_total > 0 {
        let (selected_inputs, selected_change) = UtxoSpendInfo::utxos_to_cover_value(
            context,
            wallet_seed.clone(),
            requested_input_total,
            NIGHT,
        )
        .map_err(|e| TransactionError::ValidationError(e.to_string()))?;

        let requested_change = requested_input_total
            .checked_sub(requested_output_total)
            .ok_or_else(|| TransactionError::ValidationError("Invalid unshielded change".into()))?;
        let change = selected_change
            .checked_add(requested_change)
            .ok_or_else(|| TransactionError::ValidationError("Change value overflow".into()))?;

        for input in selected_inputs {
            inputs.push(Box::new(input));
        }

        if change > 0 {
            outputs.push(Box::new(UtxoOutputInfo {
                value: change,
                owner: wallet_seed.clone(),
                token_type: NIGHT,
            }));
        }
    }

    Ok(UnshieldedOfferInfo { inputs, outputs })
}

/// Translate one API `MidnightOfferRequest::Shielded` into a library
/// `OfferInfo`, producing a shielded (ZSWAP) offer.
///
/// PR-3 v1 constraint: the helpers' `BuildOutput` impls hardcode
/// `Segment::Guaranteed` inside `build()`, so the only correct home for
/// the resulting offer is `StandardTrasactionInfo::set_guaranteed_offer`.
/// Fallible-shielded offers would require bypassing these helpers and
/// going directly to the zswap crate's API — deferred until there's a
/// real caller need.
///
/// Inputs reference the relayer's own wallet seed; the library's
/// `InputInfo<WalletSeed>` selects a spendable coin from
/// `wallet.shielded.state.coins` at build time. If no matching coin is
/// available the library **panics** (`min_match_coin` panics on empty
/// iter, `state.spend` panics on failure). Those panics are caught by
/// the `AssertUnwindSafe(tx_info.prove()).catch_unwind()` wrapper at the
/// call site and surface as `TransactionError::UnexpectedError`.
///
/// `token_type` is parsed per-input/output via [`parse_shielded_token_type`]:
/// `"NIGHT"` shorthand maps to all-zeros, otherwise a 64-char hex string
/// is decoded to the 32-byte `ShieldedTokenType` hash.
/// Parse the request's `token_type` string into a `ShieldedTokenType`.
///
/// Accepts either of:
///   - `"NIGHT"` (case-insensitive) — the chain's primary shielded token,
///     stored as the all-zero `HashOutput([0u8; 32])`.
///   - 64-character hex (with optional `0x` prefix) — any other shielded
///     token type, decoded into the 32-byte hash that names it.
///
/// Used by both `build_shielded_offer` and `build_fallible_shielded_offer`
/// so the API surface accepts arbitrary token types instead of hardcoding
/// the relayer to NIGHT-only.
fn parse_shielded_token_type(
    s: &str,
) -> Result<midnight_node_ledger_helpers::ShieldedTokenType, TransactionError> {
    use midnight_node_ledger_helpers::{HashOutput, ShieldedTokenType};

    let trimmed = s.trim().trim_start_matches("0x");
    if trimmed.eq_ignore_ascii_case("NIGHT") {
        return Ok(ShieldedTokenType(HashOutput([0u8; 32])));
    }
    let bytes = hex::decode(trimmed).map_err(|e| {
        TransactionError::ValidationError(format!(
            "token_type must be 'NIGHT' or 64-char hex; failed to decode '{s}': {e}"
        ))
    })?;
    if bytes.len() != 32 {
        return Err(TransactionError::ValidationError(format!(
            "token_type hex must be 32 bytes (64 chars); got {} bytes",
            bytes.len()
        )));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(ShieldedTokenType(HashOutput(arr)))
}

fn build_shielded_offer(
    offer: &crate::models::MidnightOfferRequest,
    wallet_seed: &midnight_node_ledger_helpers::WalletSeed,
    context: Arc<
        midnight_node_ledger_helpers::LedgerContext<midnight_node_ledger_helpers::DefaultDB>,
    >,
) -> Result<
    midnight_node_ledger_helpers::OfferInfo<midnight_node_ledger_helpers::DefaultDB>,
    TransactionError,
> {
    use crate::models::MidnightOfferRequest;
    use midnight_node_ledger_helpers::{
        BuildInput, BuildOutput, DefaultDB, InputInfo, OfferInfo, OutputInfo, ShieldedWallet,
        WalletAddress,
    };
    use std::str::FromStr;

    let (inputs_req, outputs_req) = match offer {
        MidnightOfferRequest::Shielded { inputs, outputs } => (inputs, outputs),
        MidnightOfferRequest::Unshielded { .. } => {
            // Should not reach here when the caller dispatches on `is_shielded()`.
            return Err(TransactionError::ValidationError(
                "build_shielded_offer called with an unshielded offer variant".into(),
            ));
        }
    };

    let mut inputs: Vec<Box<dyn BuildInput<DefaultDB>>> = Vec::new();
    let mut outputs: Vec<Box<dyn BuildOutput<DefaultDB>>> = Vec::with_capacity(outputs_req.len());
    let mut requested_inputs: std::collections::HashMap<_, u128> = std::collections::HashMap::new();
    let mut requested_outputs: std::collections::HashMap<_, u128> =
        std::collections::HashMap::new();

    for input in inputs_req {
        let value: u128 = input.value.parse().map_err(|_| {
            TransactionError::ValidationError(format!(
                "Invalid shielded input value: {}",
                input.value
            ))
        })?;
        let token_type = parse_shielded_token_type(&input.token_type)?;
        let total = requested_inputs.entry(token_type).or_insert(0u128);
        *total = total
            .checked_add(value)
            .ok_or_else(|| TransactionError::ValidationError("Shielded input overflow".into()))?;
    }

    for output in outputs_req {
        let value: u128 = output.value.parse().map_err(|_| {
            TransactionError::ValidationError(format!(
                "Invalid shielded output value: {}",
                output.value
            ))
        })?;
        let token_type = parse_shielded_token_type(&output.token_type)?;
        let total = requested_outputs.entry(token_type).or_insert(0u128);
        *total = total
            .checked_add(value)
            .ok_or_else(|| TransactionError::ValidationError("Shielded output overflow".into()))?;
        if output.destination == "self" {
            outputs.push(Box::new(OutputInfo {
                destination: wallet_seed.clone(),
                token_type,
                value,
            }));
        } else {
            // Parse as bech32m `mn_shield-addr_*` → WalletAddress →
            // ShieldedWallet. Rejects unshielded addresses.
            let wallet_addr = WalletAddress::from_str(&output.destination).map_err(|e| {
                TransactionError::ValidationError(format!(
                    "Invalid shielded destination address {}: {e}",
                    output.destination
                ))
            })?;
            let shielded: ShieldedWallet<DefaultDB> = ShieldedWallet::try_from(&wallet_addr)
                .map_err(|e| {
                    TransactionError::ValidationError(format!(
                        "Destination {} is not a shielded address: {e:?}",
                        output.destination
                    ))
                })?;
            outputs.push(Box::new(OutputInfo {
                destination: shielded,
                token_type,
                value,
            }));
        }
    }

    for (token_type, requested_input_total) in requested_inputs {
        let requested_output_total = requested_outputs.get(&token_type).copied().unwrap_or(0);
        let requested_change = requested_input_total
            .checked_sub(requested_output_total)
            .ok_or_else(|| TransactionError::ValidationError("Invalid shielded change".into()))?;
        let (selected_inputs, selected_change) = InputInfo::coins_to_cover_value(
            context.clone(),
            wallet_seed.clone(),
            requested_input_total,
            token_type,
        )
        .map_err(|e| TransactionError::ValidationError(e.to_string()))?;
        let change = selected_change
            .checked_add(requested_change)
            .ok_or_else(|| TransactionError::ValidationError("Shielded change overflow".into()))?;

        for input in selected_inputs {
            inputs.push(Box::new(input));
        }
        if change > 0 {
            outputs.push(Box::new(OutputInfo {
                destination: wallet_seed.clone(),
                token_type,
                value: change,
            }));
        }
    }

    Ok(OfferInfo {
        inputs,
        outputs,
        transients: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn simple_unshielded_offer() -> crate::models::MidnightOfferRequest {
        crate::models::MidnightOfferRequest::Unshielded {
            inputs: vec![crate::models::MidnightUnshieldedInputRequest {
                origin: "self".into(),
                token_type: "NIGHT".into(),
                value: "1".into(),
            }],
            outputs: vec![crate::models::MidnightUnshieldedOutputRequest {
                destination: "self".into(),
                token_type: "NIGHT".into(),
                value: "1".into(),
            }],
        }
    }

    fn simple_shielded_offer(token_type: &str) -> crate::models::MidnightOfferRequest {
        crate::models::MidnightOfferRequest::Shielded {
            inputs: vec![crate::models::MidnightShieldedInputRequest {
                origin: "self".into(),
                token_type: token_type.into(),
                value: "1".into(),
            }],
            outputs: vec![crate::models::MidnightShieldedOutputRequest {
                destination: "self".into(),
                token_type: token_type.into(),
                value: "1".into(),
            }],
        }
    }

    fn credit_shielded_token(
        manager: &crate::services::sync::midnight::handler::LedgerContextManager,
        token_type: midnight_node_ledger_helpers::ShieldedTokenType,
        value: u128,
    ) {
        use midnight_node_ledger_helpers::{CoinInfo, Offer, OsRng, Output, Rng, SeedableRng};

        let wallet_seed = manager.wallet_seed().clone();
        let mut rng = midnight_node_ledger_helpers::StdRng::seed_from_u64(0x42);
        manager
            .context()
            .with_wallet_from_seed(wallet_seed, |wallet| {
                let secret_keys = wallet.shielded.secret_keys().clone();
                let coin = CoinInfo {
                    nonce: OsRng.r#gen(),
                    type_: token_type,
                    value,
                };
                let output = Output::new(
                    &mut rng,
                    &coin,
                    None,
                    &secret_keys.coin_public_key(),
                    Some(secret_keys.enc_public_key()),
                )
                .unwrap();
                let offer = Offer {
                    inputs: vec![].into(),
                    outputs: vec![output].into(),
                    transient: vec![].into(),
                    deltas: vec![].into(),
                };
                wallet.shielded.state = wallet.shielded.state.apply(&secret_keys, &offer);
            });
    }

    #[test]
    fn top_level_unshielded_offer_routes_to_guaranteed_unshielded_slot() {
        let manager = crate::services::sync::midnight::handler::LedgerContextManager::new(
            &[7u8; 32], "preview",
        );
        let wallet_seed = manager.wallet_seed().clone();
        manager
            .inject_utxos(&[crate::repositories::UnshieldedUtxo {
                owner: "owner".to_string(),
                value: 1_000_000_000,
                token_type: "native".to_string(),
                intent_hash: "aa".repeat(32),
                output_index: 0,
                ctime: Some(0),
                registered_for_dust_generation: false,
            }])
            .unwrap();
        let offer = build_unshielded_offer(
            &simple_unshielded_offer(),
            &wallet_seed,
            manager.context().clone(),
        )
        .unwrap();

        let intent = top_level_unshielded_intent(offer);

        assert!(intent.guaranteed_unshielded_offer.is_some());
        assert!(intent.fallible_unshielded_offer.is_none());
    }

    #[test]
    fn unshielded_offer_returns_change_for_oversized_selected_utxo() {
        let manager = crate::services::sync::midnight::handler::LedgerContextManager::new(
            &[7u8; 32], "preview",
        );
        let wallet_seed = manager.wallet_seed().clone();
        manager
            .inject_utxos(&[crate::repositories::UnshieldedUtxo {
                owner: "owner".to_string(),
                value: 1_000_000_000,
                token_type: "native".to_string(),
                intent_hash: "bb".repeat(32),
                output_index: 0,
                ctime: Some(0),
                registered_for_dust_generation: false,
            }])
            .unwrap();

        let offer = build_unshielded_offer(
            &simple_unshielded_offer(),
            &wallet_seed,
            manager.context().clone(),
        )
        .unwrap();
        let built = offer.build(manager.context().clone());

        let input_total: u128 = built.inputs.iter().map(|input| input.value).sum();
        let output_values: Vec<u128> = built.outputs.iter().map(|output| output.value).collect();
        let output_total: u128 = output_values.iter().sum();

        assert_eq!(input_total, 1_000_000_000);
        assert_eq!(output_total, input_total);
        assert!(output_values.contains(&1));
        assert!(output_values.contains(&999_999_999));
    }

    #[test]
    fn transaction_build_context_isolates_shielded_pending_spends() {
        use midnight_node_ledger_helpers::{HashOutput, SeedableRng, ShieldedTokenType};

        let manager = crate::services::sync::midnight::handler::LedgerContextManager::new(
            &[7u8; 32], "preview",
        );
        let wallet_seed = manager.wallet_seed().clone();
        let token = ShieldedTokenType(HashOutput([9u8; 32]));
        let token_hex = hex::encode(token.0 .0);
        credit_shielded_token(&manager, token, 1);
        assert_eq!(manager.shielded_balances().get(&token_hex), Some(&1));

        let build_context = manager.transaction_build_context().unwrap();
        let mut offer = build_shielded_offer(
            &simple_shielded_offer(&token_hex),
            &wallet_seed,
            build_context.clone(),
        )
        .unwrap();
        let mut rng = midnight_node_ledger_helpers::StdRng::seed_from_u64(0x42);
        offer.build(&mut rng, build_context).unwrap();

        assert_eq!(manager.shielded_balances().get(&token_hex), Some(&1));
    }

    #[test]
    fn shielded_offer_returns_change_for_oversized_selected_coin() {
        use midnight_node_ledger_helpers::{HashOutput, SeedableRng, ShieldedTokenType};

        let manager = crate::services::sync::midnight::handler::LedgerContextManager::new(
            &[7u8; 32], "preview",
        );
        let wallet_seed = manager.wallet_seed().clone();
        let token = ShieldedTokenType(HashOutput([10u8; 32]));
        let token_hex = hex::encode(token.0 .0);
        credit_shielded_token(&manager, token, 9);

        let build_context = manager.transaction_build_context().unwrap();
        let mut offer = build_shielded_offer(
            &simple_shielded_offer(&token_hex),
            &wallet_seed,
            build_context.clone(),
        )
        .unwrap();
        let mut rng = midnight_node_ledger_helpers::StdRng::seed_from_u64(0x43);
        let built = offer.build(&mut rng, build_context).unwrap();

        assert_eq!(built.inputs.iter().count(), 1);
        assert_eq!(built.outputs.iter().count(), 2);
    }
}

// ============================================================================
// PR-3 v2: Fallible-shielded offer support via segment retargeting
// ============================================================================
//
// The helpers crate's `InputInfo<WalletSeed>` / `OutputInfo<WalletSeed>` impls
// of `BuildInput` / `BuildOutput` hardcode `Segment::Guaranteed` when calling
// the underlying zswap `Input::new` / `Output::new`. This makes it impossible
// to go through the `OfferInfo::build` → `tx_info.set_fallible_offers` path
// for shielded offers at a fallible segment — the ZK proof's segment tag
// would be `Guaranteed` but the storage HashMap key says `N`, and the chain
// rejects the mismatch.
//
// The zswap crate exposes `Input::retarget_segment(u16)` and
// `Output::retarget_segment(u16)` (construct.rs:103 / :243) which rewrite the
// transcript segment bytes without re-proving. We compose: call the helpers'
// impls to get a `Guaranteed`-tagged Input/Output, then `retarget` it to the
// target fallible segment. The resulting ZK proof is still valid; only the
// public-transcript `segment` field changes.
//
// This implements `BuildInput<D>` / `BuildOutput<D>` traits on our wrapper
// types, so they can be stuffed into `OfferInfo { inputs: Vec<Box<dyn
// BuildInput>>, outputs: Vec<Box<dyn BuildOutput>>, ... }` and driven by the
// helpers' `OfferInfo::build` loop normally.

struct RetargetingShieldedInput {
    inner: midnight_node_ledger_helpers::InputInfo<midnight_node_ledger_helpers::WalletSeed>,
    segment_id: u16,
}

impl midnight_node_ledger_helpers::TokenInfo for RetargetingShieldedInput {
    fn token_type(&self) -> midnight_node_ledger_helpers::ShieldedTokenType {
        self.inner.token_type
    }
    fn value(&self) -> u128 {
        self.inner.value
    }
}

impl<D: midnight_node_ledger_helpers::DB + Clone> midnight_node_ledger_helpers::BuildInput<D>
    for RetargetingShieldedInput
{
    fn build(
        &mut self,
        rng: &mut midnight_node_ledger_helpers::StdRng,
        context: std::sync::Arc<midnight_node_ledger_helpers::LedgerContext<D>>,
    ) -> midnight_node_ledger_helpers::Input<midnight_node_ledger_helpers::ProofPreimage, D> {
        let guaranteed_input = <midnight_node_ledger_helpers::InputInfo<
            midnight_node_ledger_helpers::WalletSeed,
        > as midnight_node_ledger_helpers::BuildInput<D>>::build(
            &mut self.inner, rng, context
        );
        guaranteed_input.retarget_segment(self.segment_id)
    }
}

struct RetargetingShieldedOutputSelf {
    inner: midnight_node_ledger_helpers::OutputInfo<midnight_node_ledger_helpers::WalletSeed>,
    segment_id: u16,
}

impl midnight_node_ledger_helpers::TokenInfo for RetargetingShieldedOutputSelf {
    fn token_type(&self) -> midnight_node_ledger_helpers::ShieldedTokenType {
        self.inner.token_type
    }
    fn value(&self) -> u128 {
        self.inner.value
    }
}

impl<D: midnight_node_ledger_helpers::DB + Clone> midnight_node_ledger_helpers::BuildOutput<D>
    for RetargetingShieldedOutputSelf
{
    fn build(
        &self,
        rng: &mut midnight_node_ledger_helpers::StdRng,
        context: std::sync::Arc<midnight_node_ledger_helpers::LedgerContext<D>>,
    ) -> midnight_node_ledger_helpers::Output<midnight_node_ledger_helpers::ProofPreimage, D> {
        let guaranteed_output = <midnight_node_ledger_helpers::OutputInfo<
            midnight_node_ledger_helpers::WalletSeed,
        > as midnight_node_ledger_helpers::BuildOutput<D>>::build(
            &self.inner, rng, context
        );
        guaranteed_output.retarget_segment(self.segment_id)
    }
}

struct RetargetingShieldedOutputExternal {
    inner: midnight_node_ledger_helpers::OutputInfo<
        midnight_node_ledger_helpers::ShieldedWallet<midnight_node_ledger_helpers::DefaultDB>,
    >,
    segment_id: u16,
}

impl midnight_node_ledger_helpers::TokenInfo for RetargetingShieldedOutputExternal {
    fn token_type(&self) -> midnight_node_ledger_helpers::ShieldedTokenType {
        self.inner.token_type
    }
    fn value(&self) -> u128 {
        self.inner.value
    }
}

impl midnight_node_ledger_helpers::BuildOutput<midnight_node_ledger_helpers::DefaultDB>
    for RetargetingShieldedOutputExternal
{
    fn build(
        &self,
        rng: &mut midnight_node_ledger_helpers::StdRng,
        context: std::sync::Arc<
            midnight_node_ledger_helpers::LedgerContext<midnight_node_ledger_helpers::DefaultDB>,
        >,
    ) -> midnight_node_ledger_helpers::Output<
        midnight_node_ledger_helpers::ProofPreimage,
        midnight_node_ledger_helpers::DefaultDB,
    > {
        let guaranteed_output =
            <midnight_node_ledger_helpers::OutputInfo<
                midnight_node_ledger_helpers::ShieldedWallet<
                    midnight_node_ledger_helpers::DefaultDB,
                >,
            > as midnight_node_ledger_helpers::BuildOutput<
                midnight_node_ledger_helpers::DefaultDB,
            >>::build(&self.inner, rng, context);
        guaranteed_output.retarget_segment(self.segment_id)
    }
}

/// Translate an API shielded offer into an `OfferInfo` whose produced
/// zswap Inputs/Outputs are tagged with `segment_id` (fallible), not
/// `Segment::Guaranteed`. Used for shield-ops where the shielded half
/// of the tx lives in a fallible segment alongside an unshielded input
/// half in the same segment.
///
/// See the "PR-3 v2" comment block in this file for the segment-retarget
/// pattern explanation.
fn build_fallible_shielded_offer(
    offer: &crate::models::MidnightOfferRequest,
    wallet_seed: &midnight_node_ledger_helpers::WalletSeed,
    segment_id: u16,
    context: Arc<
        midnight_node_ledger_helpers::LedgerContext<midnight_node_ledger_helpers::DefaultDB>,
    >,
) -> Result<
    midnight_node_ledger_helpers::OfferInfo<midnight_node_ledger_helpers::DefaultDB>,
    TransactionError,
> {
    use crate::models::MidnightOfferRequest;
    use midnight_node_ledger_helpers::{
        BuildInput, BuildOutput, DefaultDB, InputInfo, OfferInfo, OutputInfo, ShieldedWallet,
        WalletAddress,
    };
    use std::str::FromStr;

    let (inputs_req, outputs_req) = match offer {
        MidnightOfferRequest::Shielded { inputs, outputs } => (inputs, outputs),
        MidnightOfferRequest::Unshielded { .. } => {
            return Err(TransactionError::UnexpectedError(
                "build_fallible_shielded_offer called with an unshielded offer variant".into(),
            ));
        }
    };

    let mut inputs: Vec<Box<dyn BuildInput<DefaultDB>>> = Vec::new();
    let mut outputs: Vec<Box<dyn BuildOutput<DefaultDB>>> = Vec::with_capacity(outputs_req.len());
    let mut requested_inputs: std::collections::HashMap<_, u128> = std::collections::HashMap::new();
    let mut requested_outputs: std::collections::HashMap<_, u128> =
        std::collections::HashMap::new();

    for input in inputs_req {
        let value: u128 = input.value.parse().map_err(|_| {
            TransactionError::ValidationError(format!(
                "Invalid shielded input value: {}",
                input.value
            ))
        })?;
        let token_type = parse_shielded_token_type(&input.token_type)?;
        let total = requested_inputs.entry(token_type).or_insert(0u128);
        *total = total
            .checked_add(value)
            .ok_or_else(|| TransactionError::ValidationError("Shielded input overflow".into()))?;
    }

    for output in outputs_req {
        let value: u128 = output.value.parse().map_err(|_| {
            TransactionError::ValidationError(format!(
                "Invalid shielded output value: {}",
                output.value
            ))
        })?;
        let token_type = parse_shielded_token_type(&output.token_type)?;
        let total = requested_outputs.entry(token_type).or_insert(0u128);
        *total = total
            .checked_add(value)
            .ok_or_else(|| TransactionError::ValidationError("Shielded output overflow".into()))?;
        if output.destination == "self" {
            outputs.push(Box::new(RetargetingShieldedOutputSelf {
                inner: OutputInfo {
                    destination: wallet_seed.clone(),
                    token_type,
                    value,
                },
                segment_id,
            }));
        } else {
            let wallet_addr = WalletAddress::from_str(&output.destination).map_err(|e| {
                TransactionError::ValidationError(format!(
                    "Invalid shielded destination address {}: {e}",
                    output.destination
                ))
            })?;
            let shielded: ShieldedWallet<DefaultDB> = ShieldedWallet::try_from(&wallet_addr)
                .map_err(|e| {
                    TransactionError::ValidationError(format!(
                        "Destination {} is not a shielded address: {e:?}",
                        output.destination
                    ))
                })?;
            outputs.push(Box::new(RetargetingShieldedOutputExternal {
                inner: OutputInfo {
                    destination: shielded,
                    token_type,
                    value,
                },
                segment_id,
            }));
        }
    }

    for (token_type, requested_input_total) in requested_inputs {
        let requested_output_total = requested_outputs.get(&token_type).copied().unwrap_or(0);
        let requested_change = requested_input_total
            .checked_sub(requested_output_total)
            .ok_or_else(|| TransactionError::ValidationError("Invalid shielded change".into()))?;
        let (selected_inputs, selected_change) = InputInfo::coins_to_cover_value(
            context.clone(),
            wallet_seed.clone(),
            requested_input_total,
            token_type,
        )
        .map_err(|e| TransactionError::ValidationError(e.to_string()))?;
        let change = selected_change
            .checked_add(requested_change)
            .ok_or_else(|| TransactionError::ValidationError("Shielded change overflow".into()))?;

        for input in selected_inputs {
            inputs.push(Box::new(RetargetingShieldedInput {
                inner: input,
                segment_id,
            }));
        }
        if change > 0 {
            outputs.push(Box::new(RetargetingShieldedOutputSelf {
                inner: OutputInfo {
                    destination: wallet_seed.clone(),
                    token_type,
                    value: change,
                },
                segment_id,
            }));
        }
    }

    Ok(OfferInfo {
        inputs,
        outputs,
        transients: Vec::new(),
    })
}
