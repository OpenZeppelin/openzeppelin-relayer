//! Nonce management for EVM relayers.
//!
//! Handles nonce synchronization, gap detection, and gap resolution via NOOP transactions.
//! Also provides `handle_health_action` for targeted nonce health jobs dispatched
//! through the health check queue.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use dashmap::DashMap;
use tokio::sync::Mutex;
use tracing::{debug, error, info, instrument, warn};

use crate::{
    config::ServerConfig,
    constants::{
        EVM_STATUS_CHECK_INITIAL_DELAY_SECONDS, HEALTH_CHECK_ACTION_KEY,
        HEALTH_CHECK_ACTION_NONCE_HEALTH, HEALTH_CHECK_NONCE_HINT_KEY, MAX_GAP_SCAN_RANGE,
    },
    domain::{relayer::RelayerError, transaction::common::is_active_nonce_status},
    jobs::{JobProducerTrait, TransactionRequest, TransactionStatusCheck},
    models::{
        EvmNetwork, EvmTransactionData, NetworkRepoModel, NetworkType, RelayerRepoModel,
        TransactionRepoModel, TransactionStatus, TransactionUpdateRequest,
    },
    repositories::{NetworkRepository, RelayerRepository, Repository, TransactionRepository},
    services::{
        provider::EvmProviderTrait, signer::DataSignerTrait, TransactionCounterServiceTrait,
    },
    utils::{calculate_scheduled_timestamp, DistributedLock},
};

use super::EvmRelayer;

/// Settling pause before gap scanning. Gives in-flight `prepare_transaction` calls
/// time to persist their reserved nonces to the repository. Best-effort mitigation
/// for the counter-increment-before-persist race. See `detect_nonce_gaps` docs.
const NONCE_GAP_SETTLE_DURATION: Duration = Duration::from_secs(2);

/// Upper bound on the width of a drift region we are willing to scan for occupancy
/// before rewinding the counter. A rewind is NEVER performed on a partial scan, so
/// a region wider than this is skipped entirely rather than verified piecemeal.
const MAX_REWIND_SCAN_RANGE: u64 = 100_000;

/// Chunk size for the occupancy scan over the drift region (one `get_nonce_occupancy`
/// range query per chunk).
const REWIND_SCAN_CHUNK: u64 = 1000;

/// Per-relayer in-process locks serializing nonce-health runs when no distributed
/// lock is available (non-distributed mode, or missing connection info).
///
/// The rewind CAS alone is not enough there: concurrent health runs plus fresh
/// allocations can reproduce the observed counter value (ABA), letting a stale
/// rewind apply beneath live allocations. Serializing runs per relayer closes the
/// health-vs-health interleaving; the prepare-persist race remains covered by the
/// settle pause and the self-correcting collision class.
static NONCE_HEALTH_LOCAL_LOCKS: OnceLock<DashMap<String, Arc<Mutex<()>>>> = OnceLock::new();

// ── Nonce synchronization & gap detection ─────────────────────────────────────

impl<P, RR, NR, TR, J, S, TCS> EvmRelayer<P, RR, NR, TR, J, S, TCS>
where
    P: EvmProviderTrait + Send + Sync,
    RR: Repository<RelayerRepoModel, String> + RelayerRepository + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    TR: Repository<TransactionRepoModel, String> + TransactionRepository + Send + Sync + 'static,
    J: JobProducerTrait + Send + Sync + 'static,
    S: DataSignerTrait + Send + Sync + 'static,
    TCS: TransactionCounterServiceTrait + Send + Sync + 'static,
{
    /// Synchronizes the nonce with the blockchain.
    #[instrument(
        level = "debug",
        skip(self),
        fields(
            request_id = ?crate::observability::request_id::get_request_id(),
            relayer_id = %self.relayer.id,
        )
    )]
    pub(crate) async fn sync_nonce(&self) -> Result<(), RelayerError> {
        let on_chain_nonce = self.get_on_chain_nonce().await?;

        // Treat the chain nonce as a floor only: atomically raise the counter to it
        // when the counter is behind, never lower it here. Concurrent allocations that
        // have already advanced the counter past the chain nonce are preserved.
        let effective = self
            .transaction_counter_service
            .sync_floor(on_chain_nonce)
            .await?;

        debug!(
            relayer_id = %self.relayer.id,
            on_chain_nonce = %on_chain_nonce,
            effective_nonce = %effective,
            "synced nonce counter to on-chain floor"
        );

        Ok(())
    }

    /// Fetches the on-chain nonce for this relayer's address.
    async fn get_on_chain_nonce(&self) -> Result<u64, RelayerError> {
        self.provider
            .get_transaction_count(&self.relayer.address)
            .await
            .map_err(|e| RelayerError::ProviderError(e.to_string()))
    }

    /// Checks if a transaction at the given nonce is still active (not a gap).
    ///
    /// Active statuses: Pending, Sent, Submitted, Mined — tx is still in-flight.
    /// Gap indicators: Failed, Canceled, Expired, Confirmed, or no tx at all.
    async fn find_active_tx_for_nonce(
        &self,
        nonce: u64,
    ) -> Result<Option<TransactionRepoModel>, RelayerError> {
        let tx = self
            .transaction_repository
            .find_by_nonce(&self.relayer.id, nonce)
            .await
            .map_err(|e| RelayerError::Internal(e.to_string()))?;

        match tx {
            Some(tx) if is_active_nonce_status(&tx.status) => Ok(Some(tx)),
            _ => Ok(None),
        }
    }

    /// Detects nonce gaps by scanning the nonce index ahead of the on-chain nonce.
    ///
    /// Scans `[on_chain_nonce, on_chain_nonce + MAX_GAP_SCAN_RANGE)` via batched
    /// `get_nonce_occupancy` (2 Redis MGET round trips). Finds the highest nonce
    /// that has any transaction and reports gaps below it — empty slots beyond the
    /// highest known tx are not gaps, just unassigned nonces.
    ///
    /// Accepts an optional pre-fetched on-chain nonce to avoid redundant RPC calls
    /// when the caller (e.g., `resolve_nonce_gaps`) already has it.
    ///
    /// # Known race condition (mitigated, not eliminated)
    ///
    /// The normal prepare path reserves a nonce via `get_and_increment()` before
    /// persisting it to the transaction repository. During that window, a nonce
    /// slot appears empty to this scanner — it could be misclassified as a gap.
    ///
    /// Mitigations in place:
    /// - **Settling pause**: `resolve_nonce_gaps` waits `NONCE_GAP_SETTLE_DURATION`
    ///   before calling this, giving in-flight `prepare_transaction` calls time to persist.
    /// - **Double-check**: `resolve_nonce_gaps` re-checks each gap candidate
    ///   via `find_active_tx_for_nonce` before creating a NOOP.
    /// - **Self-correcting**: if a false NOOP is created, it competes with the
    ///   real tx at the same nonce. The loser gets a nonce error which is handled
    ///   by the existing nonce recovery path — no funds lost, no stuck nonces.
    async fn detect_nonce_gaps(
        &self,
        on_chain_nonce: Option<u64>,
        nonce_hint: Option<u64>,
    ) -> Result<Vec<u64>, RelayerError> {
        let on_chain_nonce = match on_chain_nonce {
            Some(n) => n,
            None => self.get_on_chain_nonce().await?,
        };

        // Scan at least MAX_GAP_SCAN_RANGE ahead, but extend to cover the
        // nonce hint if it's beyond the default range. This handles cases
        // where a tx exists far ahead (e.g., counter bumped by 110).
        let hint_end = nonce_hint.map(|h| h + 1).unwrap_or(0);
        let scan_end = std::cmp::max(on_chain_nonce + MAX_GAP_SCAN_RANGE, hint_end);

        // Batch-scan using MGET (2 Redis round trips) instead of
        // N sequential find_by_nonce calls (N*3 round trips).
        let occupancy = self
            .transaction_repository
            .get_nonce_occupancy(&self.relayer.id, on_chain_nonce, scan_end)
            .await
            .map_err(|e| RelayerError::Internal(e.to_string()))?;

        // Find the highest nonce that has any transaction (active or not).
        // Only report gaps up to that point — empty slots beyond the highest
        // known tx are not gaps, they're just unassigned nonces.
        let highest_occupied = occupancy
            .iter()
            .rev()
            .find(|(_, status)| status.is_some())
            .map(|(nonce, _)| *nonce);

        let upper_bound = match highest_occupied {
            Some(n) => n + 1, // include the nonce itself, gaps are below it
            None => {
                // No transactions at all in the scan range — nothing to fill
                return Ok(vec![]);
            }
        };

        let gaps: Vec<u64> = occupancy
            .into_iter()
            .filter(|(nonce, status)| {
                *nonce < upper_bound && !status.as_ref().is_some_and(is_active_nonce_status)
            })
            .map(|(nonce, _)| nonce)
            .collect();

        Ok(gaps)
    }

    /// Detects and resolves nonce drift by trimming the empty head of the counter
    /// and filling genuine gaps with NOOP transactions.
    ///
    /// # Algorithm
    /// 1. Read `observed` (the current counter) BEFORE the settle pause. Any increment
    ///    that landed before this read is given the pause+scan window to persist its tx
    ///    record; any increment after this read makes the rewind CAS fail (safe no-op).
    /// 2. If a nonce hint is provided, `sync_floor(hint + 1)` so new txs don't collide
    ///    with the NOOPs we may create (handles counter resets). Order relative to the
    ///    `observed` read is immaterial — the hint path only raises, the rewind only lowers.
    /// 3. Settle pause: let in-flight `prepare_transaction` calls persist their reserved
    ///    nonces before we scan (best-effort, see `detect_nonce_gaps`).
    /// 4. `get_on_chain_nonce()` + `sync_floor(on_chain_nonce)` — raise the counter to the
    ///    chain floor.
    /// 5. Rewind trim: `try_rewind_counter` verifies the region `[on_chain, observed)` is
    ///    empty of any tx record and CAS-lowers the counter to the top of on-chain reality.
    /// 6. `detect_nonce_gaps()` + double-check + NOOP fill for any real gaps below the
    ///    highest known tx (runs in the same pass, regardless of the rewind outcome).
    ///
    /// # Residual race
    /// A `prepare` that INCRed the counter before our `observed` read but persists its tx
    /// record slower than pause+scan is protected only up to the read: if it persists after
    /// the scan, the rewind can move the counter past it and the nonce is handed out again.
    /// The slow `prepare` still persists, signs, and broadcasts, so the failure mode is two
    /// relayer-signed transactions competing for one nonce — exactly one mines, and the
    /// loser collapses to the existing self-correcting same-nonce collision class (see
    /// `detect_nonce_gaps` docs at the "Self-correcting" note).
    ///
    /// # Returns
    /// Number of gaps filled, or error.
    #[instrument(
        level = "debug",
        skip(self),
        fields(
            request_id = ?crate::observability::request_id::get_request_id(),
            relayer_id = %self.relayer.id,
        )
    )]
    async fn resolve_nonce_gaps(&self, nonce_hint: Option<u64>) -> Result<usize, RelayerError> {
        // Read the counter BEFORE the settle pause so it anchors the rewind CAS. A None
        // counter → 0 means there is nothing above the chain nonce to rewind.
        let observed = self.transaction_counter_service.get().await?.unwrap_or(0);

        // If a nonce hint is provided (e.g., from a tx stuck ahead of on-chain),
        // ensure the counter covers at least hint + 1 so new txs don't collide
        // with the NOOPs we're about to create. This handles counter resets.
        if let Some(hint) = nonce_hint {
            let required = hint + 1;
            let effective = self
                .transaction_counter_service
                .sync_floor(required)
                .await?;
            if observed < required {
                info!(
                    relayer_id = %self.relayer.id,
                    nonce_hint = hint,
                    effective = effective,
                    "raised counter to cover nonce hint"
                );
            }
        }

        // Settling pause: give in-flight prepare_transaction calls time to persist
        // their reserved nonces. This is a best-effort mitigation — not a guarantee.
        // See `detect_nonce_gaps` doc for the full race condition analysis.
        tokio::time::sleep(NONCE_GAP_SETTLE_DURATION).await;

        let on_chain_nonce = self.get_on_chain_nonce().await?;

        // Raise the counter to the chain floor without a second on-chain nonce fetch.
        let effective = self
            .transaction_counter_service
            .sync_floor(on_chain_nonce)
            .await?;
        debug!(
            relayer_id = %self.relayer.id,
            on_chain_nonce = %on_chain_nonce,
            effective_nonce = %effective,
            "synced nonce counter to on-chain floor"
        );

        // Trim the empty head: lower the counter back to on-chain reality over any
        // region verified free of tx records. Best-effort — outcome does not gate the fill.
        self.try_rewind_counter(on_chain_nonce, observed).await?;

        let gaps = self
            .detect_nonce_gaps(Some(on_chain_nonce), nonce_hint)
            .await?;
        if gaps.is_empty() {
            debug!("no nonce gaps detected");
            return Ok(0);
        }

        info!(
            relayer_id = %self.relayer.id,
            gap_count = gaps.len(),
            gaps = ?gaps,
            "filling nonce gaps with NOOP transactions"
        );

        let mut filled = 0;
        for nonce in &gaps {
            // Race guard — another instance may have filled this gap
            if self.find_active_tx_for_nonce(*nonce).await?.is_some() {
                debug!(
                    nonce = nonce,
                    "gap already filled by concurrent process, skipping"
                );
                continue;
            }

            match self.create_gap_filling_noop(*nonce).await {
                Ok(_) => {
                    filled += 1;
                    debug!(nonce = nonce, "created gap-filling NOOP transaction");
                }
                Err(e) => {
                    error!(
                        nonce = nonce,
                        error = %e,
                        "failed to create gap-filling NOOP, continuing with remaining gaps"
                    );
                }
            }
        }

        info!(
            relayer_id = %self.relayer.id,
            total_gaps = gaps.len(),
            filled = filled,
            "nonce gap resolution complete"
        );

        Ok(filled)
    }

    /// Trims the empty head of the nonce counter down to on-chain reality.
    ///
    /// Rewinds the counter from `observed` to `target`, where `target` is the highest
    /// slot in `[on_chain_nonce, observed)` holding a tx record of ANY status (plus one),
    /// or `on_chain_nonce` if the region is entirely empty. Records of any status bound
    /// the rewind: a `Failed` record may be a reverted-on-chain tx whose nonce was still
    /// consumed (a stale on-chain read), so it is never trimmed past.
    ///
    /// Invariants:
    /// - Never rewinds on a partial scan: a region wider than `MAX_REWIND_SCAN_RANGE` is
    ///   skipped entirely rather than verified in part.
    /// - The write is a CAS against `observed`; if the counter moved since we read it, the
    ///   write is a no-op and the next health run retries.
    async fn try_rewind_counter(
        &self,
        on_chain_nonce: u64,
        observed: u64,
    ) -> Result<(), RelayerError> {
        if observed <= on_chain_nonce {
            return Ok(());
        }

        if observed - on_chain_nonce > MAX_REWIND_SCAN_RANGE {
            error!(
                relayer_id = %self.relayer.id,
                on_chain_nonce = on_chain_nonce,
                observed = observed,
                "nonce drift region too large to verify; skipping rewind"
            );
            return Ok(());
        }

        // Scan chunks top-down from `observed` toward `on_chain_nonce`. The first chunk
        // containing any tx record holds the region's highest-occupied slot, so scanning
        // stops there; only a fully-empty region scans all the way to `on_chain_nonce`.
        let mut highest_occupied: Option<u64> = None;
        let mut chunk_end = observed;
        while chunk_end > on_chain_nonce {
            let chunk_start =
                std::cmp::max(on_chain_nonce, chunk_end.saturating_sub(REWIND_SCAN_CHUNK));
            let occupancy = self
                .transaction_repository
                .get_nonce_occupancy(&self.relayer.id, chunk_start, chunk_end)
                .await
                .map_err(|e| RelayerError::Internal(e.to_string()))?;

            if let Some((nonce, _)) = occupancy.iter().rev().find(|(_, status)| status.is_some()) {
                highest_occupied = Some(*nonce);
                break;
            }
            chunk_end = chunk_start;
        }

        let target = match highest_occupied {
            Some(n) => std::cmp::max(on_chain_nonce, n + 1),
            None => on_chain_nonce,
        };

        if target >= observed {
            return Ok(());
        }

        let applied = self
            .transaction_counter_service
            .set_if_equals(observed, target)
            .await?;

        if applied {
            info!(
                relayer_id = %self.relayer.id,
                observed = observed,
                target = target,
                on_chain_nonce = on_chain_nonce,
                "rewound nonce counter over verified-empty region"
            );
        } else {
            // warn, not debug: repeated misses are the only visible signal that the
            // rewind is being starved by concurrent counter movement.
            warn!(
                relayer_id = %self.relayer.id,
                observed = observed,
                target = target,
                "counter moved concurrently; skipping rewind, next health run will retry"
            );
        }

        Ok(())
    }

    /// Creates a gap-filling NOOP transaction for a specific nonce.
    ///
    /// The transaction is created as `Pending` with a preset nonce and pushed through
    /// the normal prepare/submit pipeline.
    async fn create_gap_filling_noop(
        &self,
        nonce: u64,
    ) -> Result<TransactionRepoModel, RelayerError> {
        let network_model = self
            .network_repository
            .get_by_name(NetworkType::Evm, &self.relayer.network)
            .await?
            .ok_or_else(|| {
                RelayerError::NetworkConfiguration(format!(
                    "Network {} not found",
                    self.relayer.network
                ))
            })?;

        let evm_network = EvmNetwork::try_from(network_model.clone())?;

        let mut evm_data = EvmTransactionData {
            gas_price: None,
            gas_limit: None,
            nonce: None,
            value: crate::models::U256::from(0u64),
            data: None,
            from: self.relayer.address.clone(),
            to: None,
            chain_id: evm_network.id(),
            hash: None,
            signature: None,
            speed: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            raw: None,
        };

        crate::domain::evm::make_noop(&mut evm_data, &evm_network, Some(&self.provider))
            .await
            .map_err(|e| RelayerError::Internal(format!("Failed to create NOOP data: {e}")))?;

        evm_data.nonce = Some(nonce);

        let now = chrono::Utc::now().to_rfc3339();
        let tx = TransactionRepoModel {
            id: uuid::Uuid::new_v4().to_string(),
            relayer_id: self.relayer.id.clone(),
            status: TransactionStatus::Pending,
            status_reason: Some(format!("Gap-filling NOOP for nonce {nonce}")),
            created_at: now,
            sent_at: None,
            confirmed_at: None,
            valid_until: None,
            delete_at: None,
            network_type: NetworkType::Evm,
            network_data: crate::models::NetworkTransactionData::Evm(evm_data),
            priced_at: None,
            hashes: Vec::new(),
            noop_count: Some(1),
            is_canceled: Some(false),
            metadata: None,
        };

        self.transaction_repository
            .create(tx.clone())
            .await
            .map_err(|e| RelayerError::Internal(e.to_string()))?;

        // Push through prepare pipeline first, then schedule delayed status check as safety
        // net. Attempt both jobs independently: the status-check job re-drives a stuck Pending
        // tx via handle_pending_state, so a single successful enqueue is enough to progress.
        let request_result = self
            .job_producer
            .produce_transaction_request_job(
                TransactionRequest::new(tx.id.clone(), tx.relayer_id.clone()),
                None,
            )
            .await;
        if let Err(e) = &request_result {
            error!(
                tx_id = %tx.id,
                nonce = nonce,
                error = %e,
                "failed to enqueue gap-filling NOOP request job, continuing"
            );
        }

        let status_result = self
            .job_producer
            .produce_check_transaction_status_job(
                TransactionStatusCheck::new(tx.id.clone(), tx.relayer_id.clone(), NetworkType::Evm),
                Some(calculate_scheduled_timestamp(
                    EVM_STATUS_CHECK_INITIAL_DELAY_SECONDS,
                )),
            )
            .await;
        if let Err(e) = &status_result {
            error!(
                tx_id = %tx.id,
                nonce = nonce,
                error = %e,
                "failed to enqueue gap-filling NOOP status-check job"
            );
        }

        if let (Err(request_err), Err(_)) = (request_result, status_result) {
            // Neither job scheduled: the Pending record would occupy the gap slot forever
            // (Pending counts as active occupancy). Fail it so a future health run re-fills.
            if let Err(update_err) = self
                .transaction_repository
                .partial_update(
                    tx.id.clone(),
                    TransactionUpdateRequest {
                        status: Some(TransactionStatus::Failed),
                        status_reason: Some("Gap-filling NOOP could not be scheduled".to_string()),
                        ..Default::default()
                    },
                )
                .await
            {
                error!(
                    tx_id = %tx.id,
                    error = %update_err,
                    "failed to mark unscheduled gap-filling NOOP as Failed"
                );
            }
            return Err(RelayerError::from(request_err));
        }

        info!(
            tx_id = %tx.id,
            nonce = nonce,
            "gap-filling NOOP transaction created and queued"
        );

        Ok(tx)
    }

    /// Handles a targeted health action dispatched via job metadata.
    ///
    /// Currently supported actions:
    /// - `nonce_health`: Detects and fills nonce gaps with NOOPs
    ///
    /// Returns `Ok(true)` if an action was handled, `Ok(false)` if no recognized action.
    pub(crate) async fn handle_health_action(
        &self,
        metadata: &HashMap<String, String>,
    ) -> Result<bool, RelayerError> {
        let action = match metadata.get(HEALTH_CHECK_ACTION_KEY) {
            Some(a) => a.as_str(),
            None => return Ok(false),
        };

        match action {
            HEALTH_CHECK_ACTION_NONCE_HEALTH => {
                let nonce_hint = metadata
                    .get(HEALTH_CHECK_NONCE_HINT_KEY)
                    .and_then(|v| v.parse::<u64>().ok());

                info!(
                    relayer_id = %self.relayer.id,
                    nonce_hint = ?nonce_hint,
                    "executing targeted nonce health action"
                );

                // Acquire distributed lock to prevent concurrent gap resolution.
                let _lock_guard = if ServerConfig::get_distributed_mode() {
                    if let Some((pool, prefix)) = self.relayer_repository.connection_info() {
                        let lock_key = format!("{prefix}:lock:nonce_health:{}", self.relayer.id);
                        let lock = DistributedLock::new(pool, &lock_key, Duration::from_secs(60));

                        match lock.try_acquire().await {
                            Ok(Some(guard)) => {
                                debug!(lock_key = %lock_key, "acquired distributed lock for nonce health");
                                Some(guard)
                            }
                            Ok(None) => {
                                info!(
                                    relayer_id = %self.relayer.id,
                                    "nonce health already running for this relayer, skipping"
                                );
                                return Ok(true);
                            }
                            Err(e) => {
                                warn!(
                                    relayer_id = %self.relayer.id,
                                    error = %e,
                                    "failed to acquire nonce health lock, skipping"
                                );
                                return Ok(true);
                            }
                        }
                    } else {
                        None
                    }
                } else {
                    None
                };

                // Without a distributed lock, serialize health runs in-process per
                // relayer (see `NONCE_HEALTH_LOCAL_LOCKS`). Mirror the distributed
                // behavior: if a run is already active, skip instead of queueing.
                let _local_guard = if _lock_guard.is_none() {
                    let locks = NONCE_HEALTH_LOCAL_LOCKS.get_or_init(DashMap::new);
                    let lock = locks
                        .entry(self.relayer.id.clone())
                        .or_insert_with(|| Arc::new(Mutex::new(())))
                        .clone();
                    match lock.try_lock_owned() {
                        Ok(guard) => Some(guard),
                        Err(_) => {
                            info!(
                                relayer_id = %self.relayer.id,
                                "nonce health already running in-process for this relayer, skipping"
                            );
                            return Ok(true);
                        }
                    }
                } else {
                    None
                };

                match self.resolve_nonce_gaps(nonce_hint).await {
                    Ok(filled) => {
                        info!(
                            relayer_id = %self.relayer.id,
                            gaps_filled = filled,
                            "nonce health action completed"
                        );
                    }
                    Err(e) => {
                        error!(
                            relayer_id = %self.relayer.id,
                            error = %e,
                            "nonce health action failed"
                        );
                        return Err(e);
                    }
                }

                Ok(true)
            }
            _ => {
                warn!(
                    relayer_id = %self.relayer.id,
                    action = %action,
                    "unknown targeted health action, skipping"
                );
                Ok(false)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domain::relayer::{SignDataRequest, SignDataResponse, SignTypedDataRequest},
        jobs::MockJobProducerTrait,
        models::{
            NetworkType, RelayerEvmPolicy, RelayerNetworkPolicy, RelayerRepoModel, RpcConfig,
            SignerError, TransactionStatus,
        },
        repositories::{MockNetworkRepository, MockRelayerRepository, MockTransactionRepository},
        services::{
            provider::MockEvmProviderTrait, signer::DataSignerTrait,
            MockTransactionCounterServiceTrait,
        },
    };
    use async_trait::async_trait;
    use std::future::ready;
    use std::sync::Arc;

    mockall::mock! {
        pub Signer {}

        #[async_trait]
        impl DataSignerTrait for Signer {
            async fn sign_data(&self, request: SignDataRequest) -> Result<SignDataResponse, SignerError>;
            async fn sign_typed_data(&self, request: SignTypedDataRequest) -> Result<SignDataResponse, SignerError>;
        }
    }

    fn create_test_evm_network() -> EvmNetwork {
        EvmNetwork {
            network: "mainnet".to_string(),
            rpc_urls: vec![RpcConfig::new(
                "https://mainnet.infura.io/v3/test".to_string(),
            )],
            average_blocktime_ms: 12000,
            is_testnet: false,
            tags: vec!["mainnet".to_string()],
            chain_id: 1,
            required_confirmations: 1,
            features: vec!["eip1559".to_string()],
            symbol: "ETH".to_string(),
            explorer_urls: None,
            gas_price_cache: None,
        }
    }

    fn create_test_relayer() -> RelayerRepoModel {
        RelayerRepoModel {
            id: "test-relayer-id".to_string(),
            name: "test-relayer".to_string(),
            network: "mainnet".to_string(),
            network_type: NetworkType::Evm,
            signer_id: "test-signer-id".to_string(),
            address: "0x1234567890abcdef".to_string(),
            policies: RelayerNetworkPolicy::Evm(RelayerEvmPolicy::default()),
            paused: false,
            notification_id: None,
            system_disabled: false,
            custom_rpc_urls: None,
            disabled_reason: None,
        }
    }

    #[allow(clippy::type_complexity)]
    fn setup_mocks() -> (
        MockEvmProviderTrait,
        MockRelayerRepository,
        MockNetworkRepository,
        MockTransactionRepository,
        MockJobProducerTrait,
        MockSigner,
        MockTransactionCounterServiceTrait,
    ) {
        (
            MockEvmProviderTrait::new(),
            MockRelayerRepository::new(),
            MockNetworkRepository::new(),
            MockTransactionRepository::new(),
            MockJobProducerTrait::new(),
            MockSigner::new(),
            MockTransactionCounterServiceTrait::new(),
        )
    }

    fn make_tx_with_status(status: TransactionStatus) -> TransactionRepoModel {
        TransactionRepoModel {
            id: "tx-id".to_string(),
            relayer_id: "test-relayer-id".to_string(),
            status,
            ..Default::default()
        }
    }

    fn create_test_network_model() -> NetworkRepoModel {
        use crate::config::{EvmNetworkConfig, NetworkConfigCommon};
        let config = EvmNetworkConfig {
            common: NetworkConfigCommon {
                network: "mainnet".to_string(),
                from: None,
                rpc_urls: Some(vec![crate::models::RpcConfig::new(
                    "https://mainnet.infura.io/v3/test".to_string(),
                )]),
                explorer_urls: None,
                average_blocktime_ms: Some(12000),
                is_testnet: Some(false),
                tags: Some(vec!["mainnet".to_string()]),
            },
            chain_id: Some(1),
            required_confirmations: Some(1),
            features: Some(vec!["eip1559".to_string()]),
            symbol: Some("ETH".to_string()),
            gas_price_cache: None,
        };
        NetworkRepoModel::new_evm(config)
    }

    #[tokio::test]
    async fn test_find_active_tx_for_nonce_with_submitted_tx_returns_some() {
        let (provider, relayer_repo, network_repo, mut tx_repo, job_producer, signer, counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        tx_repo
            .expect_find_by_nonce()
            .returning(|_, _| Ok(Some(make_tx_with_status(TransactionStatus::Submitted))));

        let relayer = EvmRelayer::new(
            relayer_model,
            signer,
            provider,
            create_test_evm_network(),
            Arc::new(relayer_repo),
            Arc::new(network_repo),
            Arc::new(tx_repo),
            Arc::new(counter),
            Arc::new(job_producer),
        )
        .unwrap();

        let result = relayer.find_active_tx_for_nonce(5).await.unwrap();
        assert!(result.is_some());
    }

    #[tokio::test]
    async fn test_find_active_tx_for_nonce_with_failed_tx_returns_none() {
        let (provider, relayer_repo, network_repo, mut tx_repo, job_producer, signer, counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        tx_repo
            .expect_find_by_nonce()
            .returning(|_, _| Ok(Some(make_tx_with_status(TransactionStatus::Failed))));

        let relayer = EvmRelayer::new(
            relayer_model,
            signer,
            provider,
            create_test_evm_network(),
            Arc::new(relayer_repo),
            Arc::new(network_repo),
            Arc::new(tx_repo),
            Arc::new(counter),
            Arc::new(job_producer),
        )
        .unwrap();

        let result = relayer.find_active_tx_for_nonce(5).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_find_active_tx_for_nonce_with_no_tx_returns_none() {
        let (provider, relayer_repo, network_repo, mut tx_repo, job_producer, signer, counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        tx_repo.expect_find_by_nonce().returning(|_, _| Ok(None));

        let relayer = EvmRelayer::new(
            relayer_model,
            signer,
            provider,
            create_test_evm_network(),
            Arc::new(relayer_repo),
            Arc::new(network_repo),
            Arc::new(tx_repo),
            Arc::new(counter),
            Arc::new(job_producer),
        )
        .unwrap();

        let result = relayer.find_active_tx_for_nonce(5).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_detect_nonce_gaps_with_gap() {
        let (
            mut provider,
            relayer_repo,
            network_repo,
            mut tx_repo,
            job_producer,
            signer,
            mut counter,
        ) = setup_mocks();
        let relayer_model = create_test_relayer();

        provider
            .expect_get_transaction_count()
            .returning(|_| Box::pin(ready(Ok(5u64))));

        counter
            .expect_get()
            .returning(|| Box::pin(ready(Ok(Some(8u64)))));

        // nonce 5 → Submitted (active), 6 → Failed (gap), 7 → Sent (active)
        tx_repo.expect_get_nonce_occupancy().returning(|_, _, _| {
            Ok(vec![
                (5, Some(TransactionStatus::Submitted)),
                (6, Some(TransactionStatus::Failed)),
                (7, Some(TransactionStatus::Sent)),
            ])
        });

        let relayer = EvmRelayer::new(
            relayer_model,
            signer,
            provider,
            create_test_evm_network(),
            Arc::new(relayer_repo),
            Arc::new(network_repo),
            Arc::new(tx_repo),
            Arc::new(counter),
            Arc::new(job_producer),
        )
        .unwrap();

        let gaps = relayer.detect_nonce_gaps(None, None).await.unwrap();
        assert_eq!(gaps, vec![6u64]);
    }

    #[tokio::test]
    async fn test_detect_nonce_gaps_no_gaps() {
        let (
            mut provider,
            relayer_repo,
            network_repo,
            mut tx_repo,
            job_producer,
            signer,
            mut counter,
        ) = setup_mocks();
        let relayer_model = create_test_relayer();

        provider
            .expect_get_transaction_count()
            .returning(|_| Box::pin(ready(Ok(5u64))));

        counter
            .expect_get()
            .returning(|| Box::pin(ready(Ok(Some(5u64)))));

        // Scan finds no txs at all → no highest_occupied → empty gaps
        tx_repo
            .expect_get_nonce_occupancy()
            .returning(|_, from, to| Ok((from..to).map(|n| (n, None)).collect()));

        let relayer = EvmRelayer::new(
            relayer_model,
            signer,
            provider,
            create_test_evm_network(),
            Arc::new(relayer_repo),
            Arc::new(network_repo),
            Arc::new(tx_repo),
            Arc::new(counter),
            Arc::new(job_producer),
        )
        .unwrap();

        let gaps = relayer.detect_nonce_gaps(None, None).await.unwrap();
        assert!(gaps.is_empty());
    }

    #[tokio::test]
    async fn test_detect_nonce_gaps_counter_behind_chain() {
        let (
            mut provider,
            relayer_repo,
            network_repo,
            mut tx_repo,
            job_producer,
            signer,
            mut counter,
        ) = setup_mocks();
        let relayer_model = create_test_relayer();

        provider
            .expect_get_transaction_count()
            .returning(|_| Box::pin(ready(Ok(10u64))));

        counter
            .expect_get()
            .returning(|| Box::pin(ready(Ok(Some(5u64)))));

        // Counter behind chain, scan finds no txs → empty gaps
        tx_repo
            .expect_get_nonce_occupancy()
            .returning(|_, from, to| Ok((from..to).map(|n| (n, None)).collect()));

        let relayer = EvmRelayer::new(
            relayer_model,
            signer,
            provider,
            create_test_evm_network(),
            Arc::new(relayer_repo),
            Arc::new(network_repo),
            Arc::new(tx_repo),
            Arc::new(counter),
            Arc::new(job_producer),
        )
        .unwrap();

        let gaps = relayer.detect_nonce_gaps(None, None).await.unwrap();
        assert!(gaps.is_empty());
    }

    #[tokio::test]
    async fn test_resolve_nonce_gaps_no_gaps() {
        let (
            mut provider,
            relayer_repo,
            network_repo,
            mut tx_repo,
            job_producer,
            signer,
            mut counter,
        ) = setup_mocks();
        let relayer_model = create_test_relayer();

        // get_on_chain_nonce + sync_nonce both call get_transaction_count
        provider
            .expect_get_transaction_count()
            .returning(|_| Box::pin(ready(Ok(5u64))));

        // resolve_nonce_gaps reads get() (observed); sync_nonce calls sync_floor()
        counter
            .expect_get()
            .returning(|| Box::pin(ready(Ok(Some(5u64)))));

        counter
            .expect_sync_floor()
            .returning(|floor| Box::pin(ready(Ok(floor))));

        // Scan finds no txs → empty gaps
        tx_repo
            .expect_get_nonce_occupancy()
            .returning(|_, from, to| Ok((from..to).map(|n| (n, None)).collect()));

        let relayer = EvmRelayer::new(
            relayer_model,
            signer,
            provider,
            create_test_evm_network(),
            Arc::new(relayer_repo),
            Arc::new(network_repo),
            Arc::new(tx_repo),
            Arc::new(counter),
            Arc::new(job_producer),
        )
        .unwrap();

        let result = relayer.resolve_nonce_gaps(None).await.unwrap();
        assert_eq!(result, 0);
    }

    #[tokio::test]
    async fn test_resolve_nonce_gaps_fills_gaps() {
        use crate::config::{EvmNetworkConfig, NetworkConfigCommon};

        let (
            mut provider,
            relayer_repo,
            mut network_repo,
            mut tx_repo,
            mut job_producer,
            signer,
            mut counter,
        ) = setup_mocks();
        let relayer_model = create_test_relayer();

        // get_on_chain_nonce + sync_nonce both call get_transaction_count
        provider
            .expect_get_transaction_count()
            .returning(|_| Box::pin(ready(Ok(5u64))));

        // resolve_nonce_gaps reads get() (observed); sync_nonce calls sync_floor()
        counter
            .expect_get()
            .returning(|| Box::pin(ready(Ok(Some(8u64)))));

        counter
            .expect_sync_floor()
            .returning(|floor| Box::pin(ready(Ok(floor))));

        // detect_nonce_gaps uses batch occupancy check
        tx_repo.expect_get_nonce_occupancy().returning(|_, _, _| {
            Ok(vec![
                (5, Some(TransactionStatus::Submitted)),
                (6, Some(TransactionStatus::Failed)),
                (7, Some(TransactionStatus::Sent)),
            ])
        });

        // resolve_nonce_gaps double-check: nonce 6 → still gap (Failed)
        tx_repo
            .expect_find_by_nonce()
            .returning(|_, nonce| match nonce {
                6 => Ok(Some(make_tx_with_status(TransactionStatus::Failed))),
                _ => Ok(None),
            });

        // create_gap_filling_noop needs network repo
        let config = EvmNetworkConfig {
            common: NetworkConfigCommon {
                network: "mainnet".to_string(),
                from: None,
                rpc_urls: Some(vec![crate::models::RpcConfig::new(
                    "https://mainnet.infura.io/v3/test".to_string(),
                )]),
                explorer_urls: None,
                average_blocktime_ms: Some(12000),
                is_testnet: Some(false),
                tags: Some(vec!["mainnet".to_string()]),
            },
            chain_id: Some(1),
            required_confirmations: Some(1),
            features: Some(vec!["eip1559".to_string()]),
            symbol: Some("ETH".to_string()),
            gas_price_cache: None,
        };
        let network_model = NetworkRepoModel::new_evm(config);

        network_repo
            .expect_get_by_name()
            .returning(move |_, _| Ok(Some(network_model.clone())));

        tx_repo.expect_create().returning(Ok);

        job_producer
            .expect_produce_transaction_request_job()
            .returning(|_, _| Box::pin(ready(Ok(()))));

        job_producer
            .expect_produce_check_transaction_status_job()
            .returning(|_, _| Box::pin(ready(Ok(()))));

        let relayer = EvmRelayer::new(
            relayer_model,
            signer,
            provider,
            create_test_evm_network(),
            Arc::new(relayer_repo),
            Arc::new(network_repo),
            Arc::new(tx_repo),
            Arc::new(counter),
            Arc::new(job_producer),
        )
        .unwrap();

        let result = relayer.resolve_nonce_gaps(None).await.unwrap();
        assert_eq!(result, 1);
    }

    /// When a nonce hint is provided and the counter is behind it, resolve_nonce_gaps raises the counter before scanning.
    #[tokio::test]
    async fn test_resolve_nonce_gaps_with_hint_raises_counter() {
        use crate::config::{EvmNetworkConfig, NetworkConfigCommon};

        let (
            mut provider,
            relayer_repo,
            mut network_repo,
            mut tx_repo,
            mut job_producer,
            signer,
            mut counter,
        ) = setup_mocks();
        let relayer_model = create_test_relayer();

        // on-chain nonce is 5
        provider
            .expect_get_transaction_count()
            .returning(|_| Box::pin(ready(Ok(5u64))));

        // Counter is at 5 (was reset), but hint says there's a tx at nonce 10
        // So resolve_nonce_gaps should raise counter to 11 via sync_floor before scanning.
        let counter_values = std::sync::Arc::new(std::sync::Mutex::new(5u64));
        let cv_get = counter_values.clone();
        counter.expect_get().returning(move || {
            let val = *cv_get.lock().unwrap();
            Box::pin(ready(Ok(Some(val))))
        });

        let cv_sf = counter_values.clone();
        counter.expect_sync_floor().returning(move |floor| {
            let mut guard = cv_sf.lock().unwrap();
            if *guard < floor {
                *guard = floor;
            }
            Box::pin(ready(Ok(*guard)))
        });

        // Occupancy scan: nonce 5-9 empty (gaps), nonce 10 has an active Submitted tx
        tx_repo
            .expect_get_nonce_occupancy()
            .returning(|_, from, to| {
                Ok((from..to)
                    .map(|n| {
                        if n == 10 {
                            (n, Some(TransactionStatus::Submitted))
                        } else {
                            (n, None)
                        }
                    })
                    .collect())
            });

        // Double-check for each gap nonce (5-9) — all empty
        tx_repo.expect_find_by_nonce().returning(|_, _| Ok(None));

        // create_gap_filling_noop needs network repo
        let config = EvmNetworkConfig {
            common: NetworkConfigCommon {
                network: "mainnet".to_string(),
                from: None,
                rpc_urls: Some(vec![crate::models::RpcConfig::new(
                    "https://mainnet.infura.io/v3/test".to_string(),
                )]),
                explorer_urls: None,
                average_blocktime_ms: Some(12000),
                is_testnet: Some(false),
                tags: Some(vec!["mainnet".to_string()]),
            },
            chain_id: Some(1),
            required_confirmations: Some(1),
            features: Some(vec!["eip1559".to_string()]),
            symbol: Some("ETH".to_string()),
            gas_price_cache: None,
        };
        let network_model = NetworkRepoModel::new_evm(config);
        network_repo
            .expect_get_by_name()
            .returning(move |_, _| Ok(Some(network_model.clone())));

        tx_repo.expect_create().returning(Ok);

        job_producer
            .expect_produce_transaction_request_job()
            .returning(|_, _| Box::pin(ready(Ok(()))));

        job_producer
            .expect_produce_check_transaction_status_job()
            .returning(|_, _| Box::pin(ready(Ok(()))));

        let relayer = EvmRelayer::new(
            relayer_model,
            signer,
            provider,
            create_test_evm_network(),
            Arc::new(relayer_repo),
            Arc::new(network_repo),
            Arc::new(tx_repo),
            Arc::new(counter),
            Arc::new(job_producer),
        )
        .unwrap();

        // Hint = 10 → counter should be raised to 11
        let result = relayer.resolve_nonce_gaps(Some(10)).await.unwrap();
        // Should fill gaps at nonces 5-9 (5 gaps)
        assert_eq!(result, 5);

        // Verify counter was raised
        assert_eq!(*counter_values.lock().unwrap(), 11);
    }

    /// Rewind (a): a fully-empty drift region rewinds the counter down to the chain nonce.
    #[tokio::test]
    async fn test_try_rewind_counter_empty_region_rewinds_to_chain() {
        use mockall::predicate::eq;

        let (provider, relayer_repo, network_repo, mut tx_repo, job_producer, signer, mut counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        // Region [100, 365) is entirely empty.
        tx_repo
            .expect_get_nonce_occupancy()
            .returning(|_, from, to| Ok((from..to).map(|n| (n, None)).collect()));

        // Empty region → target == on_chain_nonce; CAS lowers 365 → 100.
        counter
            .expect_set_if_equals()
            .with(eq(365u64), eq(100u64))
            .times(1)
            .returning(|_, _| Box::pin(ready(Ok(true))));

        let relayer = EvmRelayer::new(
            relayer_model,
            signer,
            provider,
            create_test_evm_network(),
            Arc::new(relayer_repo),
            Arc::new(network_repo),
            Arc::new(tx_repo),
            Arc::new(counter),
            Arc::new(job_producer),
        )
        .unwrap();

        relayer.try_rewind_counter(100, 365).await.unwrap();
    }

    /// Rewind (b): a terminal (Failed) record bounds the rewind — the counter stops just above it.
    #[tokio::test]
    async fn test_try_rewind_counter_bounded_by_terminal_record() {
        use mockall::predicate::eq;

        let (provider, relayer_repo, network_repo, mut tx_repo, job_producer, signer, mut counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        // A Failed record sits at nonce 150 within the region.
        tx_repo
            .expect_get_nonce_occupancy()
            .returning(|_, from, to| {
                Ok((from..to)
                    .map(|n| {
                        if n == 150 {
                            (n, Some(TransactionStatus::Failed))
                        } else {
                            (n, None)
                        }
                    })
                    .collect())
            });

        // Highest record at 150 → target 151; CAS lowers 365 → 151.
        counter
            .expect_set_if_equals()
            .with(eq(365u64), eq(151u64))
            .times(1)
            .returning(|_, _| Box::pin(ready(Ok(true))));

        let relayer = EvmRelayer::new(
            relayer_model,
            signer,
            provider,
            create_test_evm_network(),
            Arc::new(relayer_repo),
            Arc::new(network_repo),
            Arc::new(tx_repo),
            Arc::new(counter),
            Arc::new(job_producer),
        )
        .unwrap();

        relayer.try_rewind_counter(100, 365).await.unwrap();
    }

    /// Rewind (e): a region wider than `MAX_REWIND_SCAN_RANGE` is skipped without any scan or CAS write.
    #[tokio::test]
    async fn test_try_rewind_counter_region_too_large_skips() {
        let (provider, relayer_repo, network_repo, tx_repo, job_producer, signer, counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        // No get_nonce_occupancy and no set_if_equals expectations: either call panics,
        // proving the oversized region is skipped without a partial scan.
        let relayer = EvmRelayer::new(
            relayer_model,
            signer,
            provider,
            create_test_evm_network(),
            Arc::new(relayer_repo),
            Arc::new(network_repo),
            Arc::new(tx_repo),
            Arc::new(counter),
            Arc::new(job_producer),
        )
        .unwrap();

        relayer
            .try_rewind_counter(0, MAX_REWIND_SCAN_RANGE + 2)
            .await
            .unwrap();
    }

    /// Rewind (c): an active record bounds the rewind, and the gap-fill still runs in the same pass.
    #[tokio::test]
    async fn test_resolve_nonce_gaps_rewinds_and_fills_in_same_run() {
        use mockall::predicate::eq;

        let (
            mut provider,
            relayer_repo,
            mut network_repo,
            mut tx_repo,
            mut job_producer,
            signer,
            mut counter,
        ) = setup_mocks();
        let relayer_model = create_test_relayer();

        // on-chain nonce 100, counter (observed) at 105.
        provider
            .expect_get_transaction_count()
            .returning(|_| Box::pin(ready(Ok(100u64))));

        counter
            .expect_get()
            .returning(|| Box::pin(ready(Ok(Some(105u64)))));

        counter
            .expect_sync_floor()
            .returning(|floor| Box::pin(ready(Ok(floor))));

        // A single active record sits at nonce 102.
        tx_repo
            .expect_get_nonce_occupancy()
            .returning(|_, from, to| {
                Ok((from..to)
                    .map(|n| {
                        if n == 102 {
                            (n, Some(TransactionStatus::Submitted))
                        } else {
                            (n, None)
                        }
                    })
                    .collect())
            });

        // Highest record 102 → target 103; CAS lowers 105 → 103.
        counter
            .expect_set_if_equals()
            .with(eq(105u64), eq(103u64))
            .times(1)
            .returning(|_, _| Box::pin(ready(Ok(true))));

        // Double-check finds the gap slots empty.
        tx_repo.expect_find_by_nonce().returning(|_, _| Ok(None));

        let network_model = create_test_network_model();
        network_repo
            .expect_get_by_name()
            .returning(move |_, _| Ok(Some(network_model.clone())));

        tx_repo.expect_create().returning(Ok);

        job_producer
            .expect_produce_transaction_request_job()
            .returning(|_, _| Box::pin(ready(Ok(()))));
        job_producer
            .expect_produce_check_transaction_status_job()
            .returning(|_, _| Box::pin(ready(Ok(()))));

        let relayer = EvmRelayer::new(
            relayer_model,
            signer,
            provider,
            create_test_evm_network(),
            Arc::new(relayer_repo),
            Arc::new(network_repo),
            Arc::new(tx_repo),
            Arc::new(counter),
            Arc::new(job_producer),
        )
        .unwrap();

        // Gaps below the anchor at 102 are nonces 100 and 101.
        let filled = relayer.resolve_nonce_gaps(None).await.unwrap();
        assert_eq!(filled, 2);
    }

    /// Rewind (d): the CAS returns false (counter moved concurrently) — no error, and gap-fill still runs.
    #[tokio::test]
    async fn test_resolve_nonce_gaps_rewind_cas_false_continues_to_fill() {
        let (
            mut provider,
            relayer_repo,
            mut network_repo,
            mut tx_repo,
            mut job_producer,
            signer,
            mut counter,
        ) = setup_mocks();
        let relayer_model = create_test_relayer();

        provider
            .expect_get_transaction_count()
            .returning(|_| Box::pin(ready(Ok(100u64))));

        counter
            .expect_get()
            .returning(|| Box::pin(ready(Ok(Some(105u64)))));

        counter
            .expect_sync_floor()
            .returning(|floor| Box::pin(ready(Ok(floor))));

        // Active record at 103 → rewind target 104.
        tx_repo
            .expect_get_nonce_occupancy()
            .returning(|_, from, to| {
                Ok((from..to)
                    .map(|n| {
                        if n == 103 {
                            (n, Some(TransactionStatus::Submitted))
                        } else {
                            (n, None)
                        }
                    })
                    .collect())
            });

        // CAS fails — counter moved since observed; rewind is skipped without error.
        counter
            .expect_set_if_equals()
            .returning(|_, _| Box::pin(ready(Ok(false))));

        tx_repo.expect_find_by_nonce().returning(|_, _| Ok(None));

        let network_model = create_test_network_model();
        network_repo
            .expect_get_by_name()
            .returning(move |_, _| Ok(Some(network_model.clone())));

        tx_repo.expect_create().returning(Ok);

        job_producer
            .expect_produce_transaction_request_job()
            .returning(|_, _| Box::pin(ready(Ok(()))));
        job_producer
            .expect_produce_check_transaction_status_job()
            .returning(|_, _| Box::pin(ready(Ok(()))));

        let relayer = EvmRelayer::new(
            relayer_model,
            signer,
            provider,
            create_test_evm_network(),
            Arc::new(relayer_repo),
            Arc::new(network_repo),
            Arc::new(tx_repo),
            Arc::new(counter),
            Arc::new(job_producer),
        )
        .unwrap();

        // Gaps below the anchor at 103 are nonces 100, 101, 102.
        let filled = relayer.resolve_nonce_gaps(None).await.unwrap();
        assert_eq!(filled, 3);
    }

    /// Rewind (f): a failing counter `get()` propagates as an error.
    #[tokio::test]
    async fn test_resolve_nonce_gaps_propagates_counter_get_error() {
        let (provider, relayer_repo, network_repo, tx_repo, job_producer, signer, mut counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        counter.expect_get().returning(|| {
            Box::pin(ready(Err(
                crate::repositories::TransactionCounterError::NotFound(
                    "transient read failure".to_string(),
                ),
            )))
        });

        let relayer = EvmRelayer::new(
            relayer_model,
            signer,
            provider,
            create_test_evm_network(),
            Arc::new(relayer_repo),
            Arc::new(network_repo),
            Arc::new(tx_repo),
            Arc::new(counter),
            Arc::new(job_producer),
        )
        .unwrap();

        let result = relayer.resolve_nonce_gaps(None).await;
        assert!(result.is_err());
    }

    /// Gap-fill NOOP (g1): one job enqueues successfully — creation returns Ok and the record is not failed.
    #[tokio::test]
    async fn test_create_gap_filling_noop_one_job_enqueued_returns_ok() {
        let (
            provider,
            relayer_repo,
            mut network_repo,
            mut tx_repo,
            mut job_producer,
            signer,
            counter,
        ) = setup_mocks();
        let relayer_model = create_test_relayer();

        let network_model = create_test_network_model();
        network_repo
            .expect_get_by_name()
            .returning(move |_, _| Ok(Some(network_model.clone())));

        tx_repo.expect_create().returning(Ok);

        // Request job fails to enqueue.
        job_producer
            .expect_produce_transaction_request_job()
            .returning(|_, _| {
                Box::pin(ready(Err(crate::jobs::JobProducerError::QueueError(
                    "queue down".to_string(),
                ))))
            });
        // Status-check job succeeds — one enqueue is enough to progress.
        job_producer
            .expect_produce_check_transaction_status_job()
            .returning(|_, _| Box::pin(ready(Ok(()))));

        // partial_update must NOT be called (no expectation → any call panics).

        let relayer = EvmRelayer::new(
            relayer_model,
            signer,
            provider,
            create_test_evm_network(),
            Arc::new(relayer_repo),
            Arc::new(network_repo),
            Arc::new(tx_repo),
            Arc::new(counter),
            Arc::new(job_producer),
        )
        .unwrap();

        let tx = relayer.create_gap_filling_noop(7).await.unwrap();
        assert_eq!(tx.status, TransactionStatus::Pending);
    }

    /// Gap-fill NOOP (g2): both jobs fail to enqueue — the record is marked Failed and the enqueue error propagates.
    #[tokio::test]
    async fn test_create_gap_filling_noop_both_jobs_fail_marks_failed() {
        let (
            provider,
            relayer_repo,
            mut network_repo,
            mut tx_repo,
            mut job_producer,
            signer,
            counter,
        ) = setup_mocks();
        let relayer_model = create_test_relayer();

        let network_model = create_test_network_model();
        network_repo
            .expect_get_by_name()
            .returning(move |_, _| Ok(Some(network_model.clone())));

        tx_repo.expect_create().returning(Ok);

        job_producer
            .expect_produce_transaction_request_job()
            .returning(|_, _| {
                Box::pin(ready(Err(crate::jobs::JobProducerError::QueueError(
                    "queue down".to_string(),
                ))))
            });
        job_producer
            .expect_produce_check_transaction_status_job()
            .returning(|_, _| {
                Box::pin(ready(Err(crate::jobs::JobProducerError::QueueError(
                    "queue down".to_string(),
                ))))
            });

        // The stranded Pending record is failed so a future health run can re-fill.
        tx_repo
            .expect_partial_update()
            .withf(|_, update| update.status == Some(TransactionStatus::Failed))
            .times(1)
            .returning(|_, _| Ok(make_tx_with_status(TransactionStatus::Failed)));

        let relayer = EvmRelayer::new(
            relayer_model,
            signer,
            provider,
            create_test_evm_network(),
            Arc::new(relayer_repo),
            Arc::new(network_repo),
            Arc::new(tx_repo),
            Arc::new(counter),
            Arc::new(job_producer),
        )
        .unwrap();

        let result = relayer.create_gap_filling_noop(7).await;
        assert!(result.is_err());
    }

    /// `sync_nonce` raises the counter to the chain floor via a single atomic `sync_floor` call and never lowers it.
    #[tokio::test]
    async fn issue818_t2_sync_nonce_raises_counter_to_chain_floor_only() {
        use mockall::predicate::eq;

        let (mut provider, relayer_repo, network_repo, tx_repo, job_producer, signer, mut counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        // On-chain nonce is 100 (chain has moved less far than local counter).
        provider
            .expect_get_transaction_count()
            .returning(|_| Box::pin(ready(Ok(100u64))));

        // sync_nonce calls sync_floor(100) exactly once; the counter is already at 365
        // so the effective value stays 365. `get`/`set` are never called (no expectations).
        counter
            .expect_sync_floor()
            .with(eq(100u64))
            .times(1)
            .returning(|_| Box::pin(ready(Ok(365u64))));

        let relayer = EvmRelayer::new(
            relayer_model,
            signer,
            provider,
            create_test_evm_network(),
            Arc::new(relayer_repo),
            Arc::new(network_repo),
            Arc::new(tx_repo),
            Arc::new(counter),
            Arc::new(job_producer),
        )
        .unwrap();

        relayer.sync_nonce().await.unwrap();
    }

    /// A failing `sync_floor` store call propagates as an error from `sync_nonce`.
    #[tokio::test]
    async fn issue818_t3_sync_nonce_propagates_counter_store_error() {
        let (mut provider, relayer_repo, network_repo, tx_repo, job_producer, signer, mut counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        // On-chain nonce is 100.
        provider
            .expect_get_transaction_count()
            .returning(|_| Box::pin(ready(Ok(100u64))));

        // The atomic sync_floor fails with a transient store error.
        counter.expect_sync_floor().returning(|_| {
            Box::pin(ready(Err(
                crate::repositories::TransactionCounterError::NotFound(
                    "transient store failure".to_string(),
                ),
            )))
        });

        let relayer = EvmRelayer::new(
            relayer_model,
            signer,
            provider,
            create_test_evm_network(),
            Arc::new(relayer_repo),
            Arc::new(network_repo),
            Arc::new(tx_repo),
            Arc::new(counter),
            Arc::new(job_producer),
        )
        .unwrap();

        let result = relayer.sync_nonce().await;
        assert!(
            result.is_err(),
            "sync_nonce must propagate the counter store failure: {result:?}"
        );
    }

    /// A gap slot occupied by a `Pending` record (e.g. a stalled NOOP) counts as active occupancy, so `detect_nonce_gaps` does not report it as a gap.
    #[tokio::test]
    async fn issue818_t4_zombie_noop_pending_slot_hides_gap() {
        let (provider, relayer_repo, network_repo, mut tx_repo, job_producer, signer, counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        // Occupancy: nonce 6 is a stalled gap-fill NOOP still in `Pending`,
        // wedged between two genuinely active txs.
        tx_repo.expect_get_nonce_occupancy().returning(|_, _, _| {
            Ok(vec![
                (5, Some(TransactionStatus::Submitted)),
                (6, Some(TransactionStatus::Pending)),
                (7, Some(TransactionStatus::Sent)),
            ])
        });

        let relayer = EvmRelayer::new(
            relayer_model,
            signer,
            provider,
            create_test_evm_network(),
            Arc::new(relayer_repo),
            Arc::new(network_repo),
            Arc::new(tx_repo),
            Arc::new(counter),
            Arc::new(job_producer),
        )
        .unwrap();

        // on_chain_nonce provided (5) so no provider RPC is needed.
        let gaps = relayer.detect_nonce_gaps(Some(5), None).await.unwrap();

        assert!(
            gaps.is_empty(),
            "expected zero gaps because Pending slot 6 counts as active occupancy, got {gaps:?}"
        );
        assert!(
            !gaps.contains(&6),
            "slot 6 must NOT be reported as a gap while its NOOP record is Pending"
        );
    }

    /// When the scan window contains no tx record at all, `detect_nonce_gaps` has no anchor and reports zero gaps.
    #[tokio::test]
    async fn issue818_t4_no_anchor_empty_scan_window_reports_zero_gaps() {
        let (provider, relayer_repo, network_repo, mut tx_repo, job_producer, signer, counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        // Counter has drifted far ahead (365) while on-chain nonce is 100, but the
        // entire scan window [100, 200) is EMPTY — no tx records exist there.
        tx_repo
            .expect_get_nonce_occupancy()
            .returning(|_, from, to| Ok((from..to).map(|n| (n, None)).collect()));

        let relayer = EvmRelayer::new(
            relayer_model,
            signer,
            provider,
            create_test_evm_network(),
            Arc::new(relayer_repo),
            Arc::new(network_repo),
            Arc::new(tx_repo),
            Arc::new(counter),
            Arc::new(job_producer),
        )
        .unwrap();

        // on-chain nonce 100, NO nonce hint. Scan window is [100, 100+100) = [100, 200).
        let gaps = relayer.detect_nonce_gaps(Some(100), None).await.unwrap();

        assert!(
            gaps.is_empty(),
            "expected zero gaps because the scan window has no anchor tx, got {gaps:?}"
        );
    }

    #[tokio::test]
    async fn test_handle_health_action_no_action_returns_false() {
        let (provider, relayer_repo, network_repo, tx_repo, job_producer, signer, counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        let relayer = EvmRelayer::new(
            relayer_model,
            signer,
            provider,
            create_test_evm_network(),
            Arc::new(relayer_repo),
            Arc::new(network_repo),
            Arc::new(tx_repo),
            Arc::new(counter),
            Arc::new(job_producer),
        )
        .unwrap();

        let metadata = HashMap::new();
        let result = relayer.handle_health_action(&metadata).await.unwrap();
        assert!(!result);
    }

    #[tokio::test]
    async fn test_handle_health_action_unknown_action_returns_false() {
        let (provider, relayer_repo, network_repo, tx_repo, job_producer, signer, counter) =
            setup_mocks();
        let relayer_model = create_test_relayer();

        let relayer = EvmRelayer::new(
            relayer_model,
            signer,
            provider,
            create_test_evm_network(),
            Arc::new(relayer_repo),
            Arc::new(network_repo),
            Arc::new(tx_repo),
            Arc::new(counter),
            Arc::new(job_producer),
        )
        .unwrap();

        let mut metadata = HashMap::new();
        metadata.insert(HEALTH_CHECK_ACTION_KEY.to_string(), "unknown".to_string());
        let result = relayer.handle_health_action(&metadata).await.unwrap();
        assert!(!result);
    }
}
