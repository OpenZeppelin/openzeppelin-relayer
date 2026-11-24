//! This module contains the status-related functionality for EVM transactions.
//! It includes methods for checking transaction status, determining when to resubmit
//! or replace transactions with NOOPs, and updating transaction status in the repository.

use alloy::network::ReceiptResponse;
use alloy::primitives::{Address, TxKind, U256};
use alloy::rpc::types::{TransactionInput, TransactionRequest};
use chrono::{DateTime, Duration, Utc};
use dashmap::{mapref::entry::Entry, DashMap};
use eyre::Result;
use serde_json::json;
use std::sync::OnceLock;
use std::time::{Duration as StdDuration, Instant};
use tracing::{debug, error, info, warn};

use super::super::common::is_active_nonce_status;
use super::EvmRelayerTransaction;
use super::{
    ensure_status_one_of, evm_transaction::TX_NONCE_RECONCILE_TRIGGER, get_age_since_priced,
    has_enough_confirmations, is_noop, is_too_early_to_resubmit, is_transaction_valid, is_unsent,
    make_noop, too_many_attempts, too_many_noop_attempts,
};
use crate::constants::{
    get_evm_min_age_for_hash_recovery, get_evm_nonce_used_failure_max_age,
    get_evm_nonce_used_failure_min_age, get_evm_pending_recovery_trigger_timeout,
    get_evm_prepare_timeout, get_evm_resend_timeout, ARBITRUM_TIME_TO_RESUBMIT,
    DEFAULT_EVM_INCLUDE_REVERT_DATA, EVM_MIN_HASHES_FOR_RECOVERY, MAX_GAP_SCAN_RANGE,
};
use crate::domain::transaction::common::{is_final_state, is_pending_transaction};
use crate::domain::transaction::util::get_age_since_created;
use crate::models::{EvmNetwork, NetworkRepoModel, NetworkType};
use crate::repositories::{NetworkRepository, RelayerRepository};
use crate::{
    domain::transaction::evm::price_calculator::PriceCalculatorTrait,
    jobs::{JobProducerTrait, StatusCheckContext},
    models::{
        EvmTransactionData, NetworkTransactionData, RelayerRepoModel, TransactionError,
        TransactionReceipt, TransactionRepoModel, TransactionStatus, TransactionUpdateRequest,
    },
    repositories::{Repository, TransactionCounterTrait, TransactionRepository},
    services::{provider::EvmProviderTrait, signer::Signer},
    utils::{get_resubmit_timeout_for_speed, get_resubmit_timeout_with_backoff},
};

/// The nonce and gap result from one on-chain nonce lookup.
struct NonceGapCheck {
    on_chain_nonce: u64,
    has_gap: bool,
}

/// The reason recorded on a failed EVM transaction when the on-chain revert payload cannot be
/// recovered (or recovery is disabled). Kept byte-for-byte for consumers that match on it.
const REVERT_REASON_GENERIC: &str = "Transaction reverted on-chain (receipt status: failed)";

/// Upper bound (in characters) on the revert-data hex embedded in `status_reason`. Revert payloads
/// are normally tiny, but they are RPC/contract-controlled, so we cap the recovered hex before it
/// is persisted to avoid an oversized DB write or notification payload.
const MAX_REVERT_DATA_HEX_LEN: usize = 4096;

/// Wait at least twice the confirmation depth, 12 blocks, and one minute.
fn nonce_used_failure_span(required_confirmations: u64, average_blocktime_ms: u64) -> u64 {
    let minute_in_blocks = match average_blocktime_ms {
        0 => 0,
        ms => 60_000_u64.div_ceil(ms),
    };
    required_confirmations
        .saturating_mul(2)
        .max(12)
        .max(minute_in_blocks)
}

/// Whether a "nonce used, no receipt" sighting is recorded.
fn has_nonce_used_seen(tx: &TransactionRepoModel) -> bool {
    tx.metadata.as_ref().is_some_and(|metadata| {
        metadata.nonce_used_seen_at_block.is_some() || metadata.nonce_used_seen_at.is_some()
    })
}

/// Metadata with the "nonce used" sighting cleared, or `None` when it is not set.
/// A receipt or an unused nonce ends the wait; a stale sighting would let a later
/// empty receipt mark a mined transaction `Failed` without a new wait.
fn without_nonce_used_seen(
    tx: &TransactionRepoModel,
) -> Option<crate::models::TransactionMetadata> {
    if !has_nonce_used_seen(tx) {
        return None;
    }
    let mut metadata = tx.metadata.clone()?;
    metadata.nonce_used_seen_at_block = None;
    metadata.nonce_used_seen_at = None;
    Some(metadata)
}

/// `current` with the payload fields of an `eth_getTransactionByHash` result.
/// Addresses equal to a stored one (ignoring case) keep the stored spelling, so
/// NOOP detection (`to == from`) still works. Signature and raw bytes belong to
/// the stored attempt, so they are cleared.
fn payload_from_rpc(
    current: &EvmTransactionData,
    rpc: &serde_json::Value,
) -> Result<EvmTransactionData, String> {
    if rpc.is_null() {
        return Err("transaction not found".to_string());
    }
    let field = |name: &str| rpc.get(name).and_then(|value| value.as_str());
    let hex = |name: &str| field(name).map(|value| value.trim_start_matches("0x"));
    let quantity = |name: &str| -> Result<Option<u128>, String> {
        hex(name)
            .map(|value| u128::from_str_radix(value, 16).map_err(|e| format!("{name}: {e}")))
            .transpose()
    };

    let mut data = current.clone();
    data.to = field("to").map(|to| {
        [current.to.as_deref(), Some(current.from.as_str())]
            .into_iter()
            .flatten()
            .find(|stored| stored.eq_ignore_ascii_case(to))
            .unwrap_or(to)
            .to_string()
    });
    data.value = U256::from_str_radix(hex("value").ok_or("value missing")?, 16)
        .map_err(|e| format!("value: {e}"))?;
    data.data = Some(field("input").ok_or("input missing")?.to_string());
    data.gas_limit = quantity("gas")?
        .map(u64::try_from)
        .transpose()
        .map_err(|e| format!("gas: {e}"))?;
    data.max_fee_per_gas = quantity("maxFeePerGas")?;
    data.max_priority_fee_per_gas = quantity("maxPriorityFeePerGas")?;
    // For an EIP-1559 attempt `gasPrice` is the effective price, not a setting.
    data.gas_price = match data.max_fee_per_gas {
        Some(_) => None,
        None => quantity("gasPrice")?,
    };
    data.signature = None;
    data.raw = None;
    Ok(data)
}

/// Outcome of `reconcile_tx_nonce_state`.
#[derive(Debug)]
enum NonceReconcile {
    /// Reconciliation decided this check (held, recovered or failed).
    Handled(Box<TransactionRepoModel>),
    /// Continue with the normal status flow.
    Continue,
    /// The current hash has a receipt: evaluate it without reading it again, since
    /// a second read can hit a lagging node and return null.
    Receipt(Box<TransactionReceipt>),
}

#[cfg(test)]
impl NonceReconcile {
    fn handled(self) -> Option<TransactionRepoModel> {
        match self {
            NonceReconcile::Handled(tx) => Some(*tx),
            _ => None,
        }
    }
}

/// Outcome of checking a transaction's historical hashes for a mined receipt.
#[derive(Debug)]
enum HistoricalHashRecovery {
    /// A historical hash was mined; the transaction was updated to use it.
    Recovered(Box<TransactionRepoModel>),
    /// Every historical hash was checked and none was mined.
    NotFound,
    /// No hash was found, but at least one receipt lookup failed.
    Incomplete,
}

/// Caps an overlong revert-data hex string to [`MAX_REVERT_DATA_HEX_LEN`], appending a marker so
/// consumers can tell the payload was clipped. The hex is ASCII, so slicing on a byte index is safe.
fn truncate_revert_hex(hex: &str) -> String {
    if hex.len() <= MAX_REVERT_DATA_HEX_LEN {
        return hex.to_string();
    }
    format!("{}...(truncated)", &hex[..MAX_REVERT_DATA_HEX_LEN])
}

/// Reconstructs an `eth_call` request from persisted transaction data to reproduce the call as a
/// state read. The fee fields (gas price / EIP-1559 caps) are carried through because contracts
/// can branch on `tx.gasprice`, which would otherwise change the revert path versus the mined tx.
fn build_revert_call_request(
    evm_data: &EvmTransactionData,
) -> Result<TransactionRequest, TransactionError> {
    let from = evm_data.from.parse::<Address>().map_err(|e| {
        TransactionError::UnexpectedError(format!("Invalid from address for revert recovery: {e}"))
    })?;
    let to = match evm_data.to.as_ref() {
        Some(addr) => TxKind::Call(addr.parse::<Address>().map_err(|e| {
            TransactionError::UnexpectedError(format!(
                "Invalid to address for revert recovery: {e}"
            ))
        })?),
        None => TxKind::Create,
    };
    let input = evm_data.data_to_bytes().map_err(|e| {
        TransactionError::UnexpectedError(format!("Invalid input data for revert recovery: {e}"))
    })?;

    Ok(TransactionRequest {
        from: Some(from),
        to: Some(to),
        value: Some(evm_data.value),
        input: TransactionInput::from(input),
        gas: evm_data.gas_limit,
        gas_price: evm_data.gas_price,
        max_fee_per_gas: evm_data.max_fee_per_gas,
        max_priority_fee_per_gas: evm_data.max_priority_fee_per_gas,
        ..Default::default()
    })
}

/// Extracts the revert payload from a `callTracer` trace result. The top-level `output` field
/// carries the revert bytes; an empty/`"0x"` value means no payload. The output is
/// RPC-controlled, so it is validated as `0x`-prefixed hex before being accepted — this rejects
/// malformed/non-ASCII payloads that would otherwise be recorded verbatim and could panic the
/// byte-index slicing in [`truncate_revert_hex`].
fn extract_trace_output(trace: &serde_json::Value) -> Option<String> {
    trace
        .get("output")
        .and_then(|v| v.as_str())
        .filter(|s| is_revert_hex(s))
        .map(|s| s.to_string())
}

/// Returns `true` when `s` is `0x` followed by at least one ASCII hex digit and nothing else.
fn is_revert_hex(s: &str) -> bool {
    match s.strip_prefix("0x") {
        Some(rest) => !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_hexdigit()),
        None => false,
    }
}

/// Per-key TTL gate: `should_fire` returns true at most once per `ttl` per key.
struct TtlDebounce {
    entries: DashMap<String, Instant>,
    ttl: StdDuration,
}

impl TtlDebounce {
    fn new(ttl: StdDuration) -> Self {
        Self {
            entries: DashMap::new(),
            ttl,
        }
    }

    fn should_fire(&self, key: &str) -> bool {
        // Entry holds the shard lock across check-and-set, so concurrent
        // callers cannot both fire within the same TTL window.
        match self.entries.entry(key.to_string()) {
            Entry::Occupied(mut entry) => {
                if entry.get().elapsed() < self.ttl {
                    false
                } else {
                    entry.insert(Instant::now());
                    true
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(Instant::now());
                true
            }
        }
    }

    /// Releases the window for `key` so the next caller can fire immediately.
    /// Used when the action gated by `should_fire` failed and should be retried.
    fn clear(&self, key: &str) {
        self.entries.remove(key);
    }
}

/// Per-relayer debounce for nonce-health job *production* in `detect_nonce_gap_ahead`.
///
/// The distributed lock taken when the health job runs already dedupes concurrent
/// *execution*, but with many transactions stuck on the same gapped relayer, every
/// status-check cycle for every one of them would otherwise enqueue its own job.
/// This bounds enqueue volume to at most one per relayer per TTL window.
static NONCE_HEALTH_DEBOUNCE: OnceLock<TtlDebounce> = OnceLock::new();

/// TTL for the `NONCE_HEALTH_DEBOUNCE` window.
const NONCE_HEALTH_DEBOUNCE_TTL: StdDuration = StdDuration::from_secs(30);

impl<P, RR, NR, TR, J, S, TCR, PC> EvmRelayerTransaction<P, RR, NR, TR, J, S, TCR, PC>
where
    P: EvmProviderTrait + Send + Sync,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    J: JobProducerTrait + Send + Sync + 'static,
    S: Signer + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PC: PriceCalculatorTrait + Send + Sync,
{
    pub(super) async fn check_transaction_status(
        &self,
        tx: &TransactionRepoModel,
    ) -> Result<TransactionStatus, TransactionError> {
        // Early return if transaction is already in a final state
        if is_final_state(&tx.status) {
            return Ok(tx.status.clone());
        }

        if tx.status == TransactionStatus::Pending {
            return Ok(tx.status.clone());
        }

        let evm_data = tx.network_data.get_evm_transaction_data()?;
        let tx_hash = match evm_data.hash.as_ref() {
            Some(hash) => hash,
            None if tx.status == TransactionStatus::Sent => return Ok(TransactionStatus::Sent),
            None => {
                return Err(TransactionError::UnexpectedError(
                    "Transaction hash is missing".to_string(),
                ))
            }
        };

        let receipt_result = match self.provider().get_transaction_receipt(tx_hash).await {
            Ok(receipt) => receipt,
            // An RPC error must not count toward the circuit breaker for Sent.
            Err(e) if tx.status == TransactionStatus::Sent => {
                warn!(
                    tx_id = %tx.id,
                    relayer_id = %tx.relayer_id,
                    tx_hash = %tx_hash,
                    error = %e,
                    "receipt lookup failed for Sent transaction; keeping Sent"
                );
                return Ok(TransactionStatus::Sent);
            }
            Err(e) => return Err(e.into()),
        };

        if let Some(receipt) = receipt_result {
            self.status_from_receipt(tx, &receipt).await
        } else {
            debug!(
                tx_id = %tx.id,
                relayer_id = %tx.relayer_id,
                tx_hash = %tx_hash,
                "transaction not yet mined"
            );

            // Check saved hashes if the current payload was replaced or is stale.
            if self.should_try_hash_recovery(tx)? {
                match self
                    .try_recover_with_historical_hashes(tx, &evm_data)
                    .await?
                {
                    // Return the status from the recovered (updated) transaction
                    HistoricalHashRecovery::Recovered(recovered_tx) => {
                        return Ok(recovered_tx.status)
                    }
                    HistoricalHashRecovery::NotFound => {}
                    // Best-effort fallback: a lookup error leaves the tx Submitted
                    HistoricalHashRecovery::Incomplete => {
                        warn!(
                            tx_id = %tx.id,
                            relayer_id = %tx.relayer_id,
                            "historical hash recovery incomplete due to RPC errors"
                        );
                    }
                }
            }

            Ok(if tx.status == TransactionStatus::Sent {
                TransactionStatus::Sent
            } else {
                TransactionStatus::Submitted
            })
        }
    }

    /// Status of a transaction whose current hash has `receipt`.
    async fn status_from_receipt(
        &self,
        tx: &TransactionRepoModel,
        receipt: &TransactionReceipt,
    ) -> Result<TransactionStatus, TransactionError> {
        let evm_data = tx.network_data.get_evm_transaction_data()?;
        let tx_hash = evm_data.hash.as_deref().unwrap_or_default();
        if !receipt.inner.status() {
            return Ok(TransactionStatus::Failed);
        }
        let last_block_number = match self.provider().get_block_number().await {
            Ok(block) => block,
            // The receipt proves the transaction mined; confirm it on a later check
            // rather than feed the circuit breaker.
            Err(e) => {
                warn!(
                    tx_id = %tx.id,
                    relayer_id = %tx.relayer_id,
                    tx_hash = %tx_hash,
                    error = %e,
                    "block read failed after receipt; marking Mined"
                );
                return Ok(TransactionStatus::Mined);
            }
        };
        let tx_block_number = receipt
            .block_number
            .ok_or(TransactionError::UnexpectedError(
                "Transaction receipt missing block number".to_string(),
            ))?;

        let network_model = self
            .network_repository()
            .get_by_chain_id(NetworkType::Evm, evm_data.chain_id)
            .await?
            .ok_or(TransactionError::UnexpectedError(format!(
                "Network with chain id {} not found",
                evm_data.chain_id
            )))?;

        let network = EvmNetwork::try_from(network_model).map_err(|e| {
            TransactionError::UnexpectedError(format!(
                "Error converting network model to EvmNetwork: {e}"
            ))
        })?;

        if !has_enough_confirmations(
            tx_block_number,
            last_block_number,
            network.required_confirmations,
        ) {
            debug!(
                tx_id = %tx.id,
                relayer_id = %tx.relayer_id,
                tx_hash = %tx_hash,
                "transaction mined but not confirmed"
            );
            return Ok(TransactionStatus::Mined);
        }
        // For a user-cancelled transaction, attribute the terminal status by what
        // ACTUALLY mined. Cancellation replaces the tx with a self-send NOOP
        // (to == from). If that NOOP is what confirmed, the transaction was truly
        // cancelled. If the original mined instead (the cancellation lost the nonce
        // race), it executed for real and must be reported Confirmed, not Canceled.
        // We key off the mined receipt rather than the locally-stored network_data,
        // which may still describe the NOOP even while the original hash is what
        // confirmed (e.g. before the replacement has been broadcast).
        if tx.is_canceled == Some(true) {
            if receipt.to == Some(receipt.from) {
                debug!(
                    tx_id = %tx.id,
                    relayer_id = %tx.relayer_id,
                    tx_hash = %tx_hash,
                    "cancellation NOOP confirmed on-chain; marking transaction as Canceled"
                );
                return Ok(TransactionStatus::Canceled);
            }
            debug!(
                tx_id = %tx.id,
                relayer_id = %tx.relayer_id,
                tx_hash = %tx_hash,
                "cancelled transaction's original mined before the NOOP could replace it; reporting Confirmed"
            );
        }
        Ok(TransactionStatus::Confirmed)
    }

    /// Determines if a transaction should be resubmitted.
    pub(super) async fn should_resubmit(
        &self,
        tx: &TransactionRepoModel,
    ) -> Result<bool, TransactionError> {
        // Validate transaction is in correct state for resubmission
        ensure_status_one_of(
            tx,
            &[TransactionStatus::Sent, TransactionStatus::Submitted],
            Some("should_resubmit"),
        )?;

        let evm_data = tx.network_data.get_evm_transaction_data()?;
        let age = get_age_since_priced(tx)?;

        // Check if network lacks mempool and determine appropriate timeout
        let network_model = self
            .network_repository()
            .get_by_chain_id(NetworkType::Evm, evm_data.chain_id)
            .await?
            .ok_or(TransactionError::UnexpectedError(format!(
                "Network with chain id {} not found",
                evm_data.chain_id
            )))?;

        let network = EvmNetwork::try_from(network_model).map_err(|e| {
            TransactionError::UnexpectedError(format!(
                "Error converting network model to EvmNetwork: {e}"
            ))
        })?;

        let timeout = match network.is_arbitrum() {
            true => ARBITRUM_TIME_TO_RESUBMIT,
            false => get_resubmit_timeout_for_speed(&evm_data.speed),
        };

        let timeout_with_backoff = match network.is_arbitrum() {
            true => timeout, // Use base timeout without backoff for Arbitrum
            false => get_resubmit_timeout_with_backoff(timeout, tx.hashes.len()),
        };

        if age > Duration::milliseconds(timeout_with_backoff) {
            debug!(
                tx_id = %tx.id,
                relayer_id = %tx.relayer_id,
                age_ms = %age.num_milliseconds(),
                "transaction has been pending for too long, resubmitting"
            );
            return Ok(true);
        }
        Ok(false)
    }

    /// Determines if a transaction should be replaced with a NOOP transaction.
    ///
    /// Returns a tuple `(should_noop, reason)` where:
    /// - `should_noop`: `true` if transaction should be replaced with NOOP
    /// - `reason`: Optional reason string explaining why NOOP is needed (only set when `should_noop` is `true`)
    ///
    /// # Arguments
    ///
    /// * `tx` - The transaction to check
    pub(super) async fn should_noop(
        &self,
        tx: &TransactionRepoModel,
    ) -> Result<(bool, Option<String>), TransactionError> {
        if too_many_noop_attempts(tx) {
            debug!("Transaction has too many NOOP attempts already");
            return Ok((false, None));
        }

        let evm_data = tx.network_data.get_evm_transaction_data()?;
        if is_noop(&evm_data) {
            return Ok((false, None));
        }

        let network_model = self
            .network_repository()
            .get_by_chain_id(NetworkType::Evm, evm_data.chain_id)
            .await?
            .ok_or(TransactionError::UnexpectedError(format!(
                "Network with chain id {} not found",
                evm_data.chain_id
            )))?;

        let network = EvmNetwork::try_from(network_model).map_err(|e| {
            TransactionError::UnexpectedError(format!(
                "Error converting network model to EvmNetwork: {e}"
            ))
        })?;

        if network.is_rollup() && too_many_attempts(tx) {
            let reason =
                "Rollup transaction has too many attempts. Replacing with NOOP.".to_string();
            debug!(
                tx_id = %tx.id,
                relayer_id = %tx.relayer_id,
                reason = %reason,
                "replacing transaction with NOOP"
            );
            return Ok((true, Some(reason)));
        }

        if !is_transaction_valid(&tx.created_at, &tx.valid_until) {
            let reason = "Transaction is expired. Replacing with NOOP.".to_string();
            debug!(
                tx_id = %tx.id,
                relayer_id = %tx.relayer_id,
                reason = %reason,
                "replacing transaction with NOOP"
            );
            return Ok((true, Some(reason)));
        }

        if tx.status == TransactionStatus::Pending {
            let created_at = &tx.created_at;
            let created_time = DateTime::parse_from_rfc3339(created_at)
                .map_err(|e| {
                    TransactionError::UnexpectedError(format!("Invalid created_at timestamp: {e}"))
                })?
                .with_timezone(&Utc);
            let age = Utc::now().signed_duration_since(created_time);
            if age > get_evm_prepare_timeout() {
                let reason = format!(
                    "Transaction in Pending state for over {} minutes. Replacing with NOOP.",
                    get_evm_prepare_timeout().num_minutes()
                );
                debug!(
                    tx_id = %tx.id,
                    relayer_id = %tx.relayer_id,
                    reason = %reason,
                    "replacing transaction with NOOP"
                );
                return Ok((true, Some(reason)));
            }
        }

        let latest_block = self.provider().get_block_by_number().await;
        if let Ok(block) = latest_block {
            let block_gas_limit = block.header.gas_limit;
            if let Some(gas_limit) = evm_data.gas_limit {
                if gas_limit > block_gas_limit {
                    let reason = format!(
                                "Transaction gas limit ({gas_limit}) exceeds block gas limit ({block_gas_limit}). Replacing with NOOP.",
                            );
                    warn!(
                        tx_id = %tx.id,
                        tx_gas_limit = %gas_limit,
                        block_gas_limit = %block_gas_limit,
                        "transaction gas limit exceeds block gas limit, replacing with NOOP"
                    );
                    return Ok((true, Some(reason)));
                }
            }
        }

        Ok((false, None))
    }

    /// Helper method that updates transaction status only if it's different from the current status.
    pub(super) async fn update_transaction_status_if_needed(
        &self,
        tx: TransactionRepoModel,
        new_status: TransactionStatus,
        status_reason: Option<String>,
    ) -> Result<TransactionRepoModel, TransactionError> {
        if tx.status != new_status {
            return self
                .update_transaction_status(tx, new_status, status_reason)
                .await;
        }
        Ok(tx)
    }

    /// Prepares a NOOP transaction update request.
    pub(super) async fn prepare_noop_update_request(
        &self,
        tx: &TransactionRepoModel,
        is_cancellation: bool,
        reason: Option<String>,
    ) -> Result<TransactionUpdateRequest, TransactionError> {
        let mut evm_data = tx.network_data.get_evm_transaction_data()?;
        let network_model = self
            .network_repository()
            .get_by_chain_id(NetworkType::Evm, evm_data.chain_id)
            .await?
            .ok_or(TransactionError::UnexpectedError(format!(
                "Network with chain id {} not found",
                evm_data.chain_id
            )))?;

        let network = EvmNetwork::try_from(network_model).map_err(|e| {
            TransactionError::UnexpectedError(format!(
                "Error converting network model to EvmNetwork: {e}"
            ))
        })?;

        make_noop(&mut evm_data, &network, Some(self.provider())).await?;

        let noop_count = tx.noop_count.unwrap_or(0) + 1;
        let update_request = TransactionUpdateRequest {
            network_data: Some(NetworkTransactionData::Evm(evm_data)),
            noop_count: Some(noop_count),
            status_reason: reason,
            is_canceled: if is_cancellation {
                Some(true)
            } else {
                tx.is_canceled
            },
            ..Default::default()
        };
        Ok(update_request)
    }

    /// Handles transactions in the Submitted state.
    ///
    /// Before resubmitting, checks whether the transaction's nonce is ahead of
    /// the on-chain nonce. If so, the tx can never mine because there's a gap
    /// below it — schedules a nonce health job to fill the gap instead of
    /// resubmitting (which would be futile).
    async fn handle_submitted_state(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        if tx.network_data.get_evm_transaction_data()?.raw.is_none() {
            self.send_transaction_resubmit_job(&tx).await?;
            return Ok(tx);
        }
        if self.should_resubmit(&tx).await? {
            // Before resubmitting, check if there's a nonce gap blocking this tx.
            // Only worth the RPC call when we're already going to resubmit (tx is stale).
            if let Some(check) = self.detect_nonce_gap_ahead(&tx).await {
                if check.has_gap {
                    // Tx can't mine — nonce gap below it. Trigger health job, skip resubmit.
                    return self
                        .update_transaction_status_if_needed(tx, TransactionStatus::Submitted, None)
                        .await;
                }
            }

            let resubmitted_tx = self.handle_resubmission(tx).await?;
            return Ok(resubmitted_tx);
        }

        if is_unsent(&tx)? && get_age_since_priced(&tx)? > get_evm_resend_timeout() {
            // No node accepted these bytes yet, so check expiry before sending them.
            let (should_noop, reason) = self.should_noop(&tx).await?;
            if should_noop {
                let noop_tx = self.process_noop_transaction(&tx, reason).await?;
                self.send_transaction_resubmit_job(&noop_tx).await?;
                return Ok(noop_tx);
            }
            // A nonce gap below this tx makes a resend futile; the detector
            // schedules the nonce health job itself.
            if self
                .detect_nonce_gap_ahead(&tx)
                .await
                .is_some_and(|check| check.has_gap)
            {
                return self
                    .update_transaction_status_if_needed(tx, TransactionStatus::Submitted, None)
                    .await;
            }
            self.send_transaction_submit_job(&tx).await?;
        }

        self.update_transaction_status_if_needed(tx, TransactionStatus::Submitted, None)
            .await
    }

    /// Checks whether the tx is blocked by a nonce gap below it.
    ///
    /// 1. Fetches on-chain nonce (single RPC call).
    /// 2. If `tx_nonce <= on_chain_nonce` — no gap, tx should be mineable.
    /// 3. Otherwise scans `on_chain_nonce..tx_nonce` using the Redis nonce index
    ///    to check if every slot has an active (Pending/Sent/Submitted/Mined) tx.
    /// 4. If any slot is empty or has a terminal-status tx — that's a real gap.
    ///    Schedules a nonce health job and sets `has_gap`.
    ///
    /// Returns the on-chain nonce too, so the Arbitrum head gate needs no second RPC call:
    /// - `Some` with `has_gap: true` — gap confirmed, health job scheduled
    /// - `Some` with `has_gap: false` — no gap, or the index lookup failed
    /// - `None` — couldn't determine (missing nonce, RPC error)
    async fn detect_nonce_gap_ahead(&self, tx: &TransactionRepoModel) -> Option<NonceGapCheck> {
        let evm_data = match tx.network_data.get_evm_transaction_data() {
            Ok(d) => d,
            Err(_) => return None,
        };
        let tx_nonce = match evm_data.nonce {
            Some(n) => n,
            None => return None,
        };

        let on_chain_nonce = match self
            .provider()
            .get_transaction_count(&self.relayer().address)
            .await
        {
            Ok(n) => n,
            Err(e) => {
                debug!(
                    tx_id = %tx.id,
                    error = %e,
                    "nonce gap check: failed to get on-chain nonce, skipping"
                );
                return None;
            }
        };

        // tx is the next expected nonce or already behind — no gap possible.
        if tx_nonce <= on_chain_nonce {
            return Some(NonceGapCheck {
                on_chain_nonce,
                has_gap: false,
            });
        }

        // Cap the scan to avoid an unbounded MGET if tx_nonce is very far ahead.
        // If the gap is larger, we scan what we can — the health job's
        // detect_nonce_gaps will use the nonce hint to extend its own scan.
        let scan_to = std::cmp::min(tx_nonce, on_chain_nonce + MAX_GAP_SCAN_RANGE);

        let occupancy = match self
            .transaction_repository()
            .get_nonce_occupancy(&tx.relayer_id, on_chain_nonce, scan_to)
            .await
        {
            Ok(o) => o,
            Err(e) => {
                debug!(
                    tx_id = %tx.id,
                    error = %e,
                    "nonce gap check: occupancy lookup failed, skipping gap recovery"
                );
                return Some(NonceGapCheck {
                    on_chain_nonce,
                    has_gap: false,
                });
            }
        };

        let gap_nonces: Vec<u64> = occupancy
            .into_iter()
            .filter(|(_, status)| !status.as_ref().is_some_and(is_active_nonce_status))
            .map(|(nonce, _)| nonce)
            .collect();

        if gap_nonces.is_empty() {
            // All slots between on-chain and tx_nonce are actively filled — no gap.
            return Some(NonceGapCheck {
                on_chain_nonce,
                has_gap: false,
            });
        }

        warn!(
            tx_id = %tx.id,
            relayer_id = %tx.relayer_id,
            tx_nonce = tx_nonce,
            on_chain_nonce = on_chain_nonce,
            gap_count = gap_nonces.len(),
            gaps = ?gap_nonces,
            "nonce gaps confirmed below tx, scheduling nonce health to fill"
        );

        // Debounce job *production* per relayer — detection/return value is unaffected.
        let debounce =
            NONCE_HEALTH_DEBOUNCE.get_or_init(|| TtlDebounce::new(NONCE_HEALTH_DEBOUNCE_TTL));
        if debounce.should_fire(&tx.relayer_id) {
            if let Err(e) = self.schedule_relayer_nonce_health_job(tx).await {
                warn!(
                    tx_id = %tx.id,
                    error = %e,
                    "failed to schedule nonce health job for nonce gap"
                );
                // A failed enqueue must not hold the window: no job was produced, so
                // let the next detection on this relayer retry immediately.
                debounce.clear(&tx.relayer_id);
            }
        } else {
            debug!(
                tx_id = %tx.id,
                relayer_id = %tx.relayer_id,
                "nonce health job production debounced for relayer"
            );
        }

        Some(NonceGapCheck {
            on_chain_nonce,
            has_gap: true,
        })
    }

    /// Processes transaction resubmission logic
    async fn handle_resubmission(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        debug!(
            tx_id = %tx.id,
            relayer_id = %tx.relayer_id,
            status = ?tx.status,
            "scheduling resubmit job for transaction"
        );

        // Check if transaction gas limit exceeds block gas limit before resubmitting
        let (should_noop, reason) = self.should_noop(&tx).await?;
        let tx_to_process = if should_noop {
            self.process_noop_transaction(&tx, reason).await?
        } else {
            tx
        };

        self.send_transaction_resubmit_job(&tx_to_process).await?;
        Ok(tx_to_process)
    }

    /// Handles NOOP transaction processing before resubmission
    async fn process_noop_transaction(
        &self,
        tx: &TransactionRepoModel,
        reason: Option<String>,
    ) -> Result<TransactionRepoModel, TransactionError> {
        debug!(
            tx_id = %tx.id,
            relayer_id = %tx.relayer_id,
            status = ?tx.status,
            "preparing transaction NOOP before resubmission"
        );
        let update = self.prepare_noop_update_request(tx, false, reason).await?;
        let updated_tx = self.cas_update(tx, update).await?;

        let res = self.send_transaction_update_notification(&updated_tx).await;
        if let Err(e) = res {
            error!(
                tx_id = %updated_tx.id,
                relayer_id = %updated_tx.relayer_id,
                status = ?updated_tx.status,
                error = %e,
                "sending transaction update notification failed for NOOP transaction"
            );
        }
        Ok(updated_tx)
    }

    /// Handles transactions in the Pending state.
    async fn handle_pending_state(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        let (should_noop, reason) = self.should_noop(&tx).await?;
        if should_noop {
            // A prior prepare can have stored a nonce. The guarded write schedules its repair.
            debug!(
                tx_id = %tx.id,
                relayer_id = %tx.relayer_id,
                reason = %reason.as_ref().unwrap_or(&"unknown".to_string()),
                "marking pending transaction as Failed"
            );
            let update = TransactionUpdateRequest {
                status: Some(TransactionStatus::Failed),
                status_reason: reason,
                ..Default::default()
            };
            let updated_tx = self.cas_update(&tx, update).await?;

            let res = self.send_transaction_update_notification(&updated_tx).await;
            if let Err(e) = res {
                error!(
                    tx_id = %updated_tx.id,
                    relayer_id = %updated_tx.relayer_id,
                    status = ?updated_tx.status,
                    error = %e,
                    "sending transaction update notification failed for Pending state NOOP"
                );
            }
            return Ok(updated_tx);
        }

        // Check if transaction is stuck in Pending (prepare job may have failed)
        let age = get_age_since_created(&tx)?;
        if age > get_evm_pending_recovery_trigger_timeout() {
            warn!(
                tx_id = %tx.id,
                relayer_id = %tx.relayer_id,
                age_seconds = age.num_seconds(),
                "transaction stuck in Pending, queuing prepare job"
            );

            // Re-queue prepare job
            self.send_transaction_request_job(&tx).await?;
        }

        Ok(tx)
    }

    /// Handles transactions in the Mined state.
    async fn handle_mined_state(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        self.update_transaction_status_if_needed(tx, TransactionStatus::Mined, None)
            .await
    }

    /// Handles transactions in final states (Confirmed, Failed, Expired).
    async fn handle_final_state(
        &self,
        tx: TransactionRepoModel,
        status: TransactionStatus,
        status_reason: Option<String>,
    ) -> Result<TransactionRepoModel, TransactionError> {
        self.update_transaction_status_if_needed(tx, status, status_reason)
            .await
    }

    /// Marks a transaction as Failed with a given reason.
    async fn mark_as_failed(
        &self,
        tx: TransactionRepoModel,
        reason: String,
    ) -> Result<TransactionRepoModel, TransactionError> {
        warn!(
            tx_id = %tx.id,
            relayer_id = %tx.relayer_id,
            reason = %reason,
            "force-failing transaction due to circuit breaker"
        );

        let update = TransactionUpdateRequest {
            status: Some(TransactionStatus::Failed),
            status_reason: Some(reason),
            ..Default::default()
        };

        let updated_tx = self.cas_update(&tx, update).await?;

        // Send notification (best effort)
        if let Err(e) = self.send_transaction_update_notification(&updated_tx).await {
            error!(
                tx_id = %updated_tx.id,
                relayer_id = %updated_tx.relayer_id,
                error = %e,
                "failed to send notification for force-failed transaction"
            );
        }

        Ok(updated_tx)
    }

    /// Reconciles a single transaction's nonce state against on-chain reality.
    ///
    /// This is the fast-path reconciliation triggered by nonce errors during submission.
    /// Reads the nonce, block number, then receipts. A used nonce without a receipt
    /// must remain unresolved for the network's block span and a wall-clock minimum
    /// before it can fail. A failed nonce or block read gives no verdict: with a
    /// recorded sighting it holds the transaction after checking receipts, otherwise
    /// it returns to the normal flow.
    ///
    /// Returns `Handled(tx)` if reconciliation decided this check (caller returns early),
    /// `Receipt` when the current hash is mined, or `Continue` for the normal flow.
    /// Clearing a sighting updates `tx` to the stored record.
    async fn reconcile_tx_nonce_state(
        &self,
        tx: &mut TransactionRepoModel,
    ) -> Result<NonceReconcile, TransactionError> {
        let evm_data = tx.network_data.get_evm_transaction_data()?;

        let Some(tx_nonce) = evm_data.nonce else {
            return Ok(NonceReconcile::Continue);
        };
        // A failed nonce or block read is inconclusive. While a sighting is recorded
        // the nonce is known to be used, so check receipts and hold; resubmitting or
        // replacing with a NOOP cannot help. Without a sighting, use the normal flow.
        let holding = has_nonce_used_seen(tx);
        let mut on_chain_nonce = match self
            .provider()
            .get_transaction_count(&self.relayer().address)
            .await
        {
            Ok(nonce) => Some(nonce),
            Err(e) if holding => {
                warn!(tx_id = %tx.id, error = %e, "nonce recovery: nonce read failed, holding");
                None
            }
            Err(e) => {
                warn!(
                    tx_id = %tx.id,
                    error = %e,
                    "nonce recovery: nonce read failed, using normal status flow"
                );
                return Ok(NonceReconcile::Continue);
            }
        };
        if on_chain_nonce.is_some_and(|nonce| nonce <= tx_nonce) {
            // While holding, trust an unused nonce only when the head is at or past the
            // sighting's block (a reorg) and a second nonce read agrees; otherwise a node
            // lags, so hold instead of letting the normal flow resubmit and add another
            // hinted job. Heuristic: a load balancer may still answer from different
            // nodes. After the maximum age, trust it so the hold cannot last forever.
            let metadata = tx.metadata.as_ref();
            let held_past_max_age = metadata
                .and_then(|metadata| metadata.nonce_used_seen_at.as_deref())
                .and_then(|at| DateTime::parse_from_rfc3339(at).ok())
                .is_some_and(|at| {
                    Utc::now().signed_duration_since(at) >= get_evm_nonce_used_failure_max_age()
                });
            let trusted = match metadata.and_then(|metadata| metadata.nonce_used_seen_at_block) {
                None => true,
                Some(_) if held_past_max_age => true,
                Some(seen) => {
                    self.provider()
                        .get_block_number()
                        .await
                        .is_ok_and(|head| head >= seen)
                        && self
                            .provider()
                            .get_transaction_count(&self.relayer().address)
                            .await
                            .is_ok_and(|nonce| nonce <= tx_nonce)
                }
            };
            if trusted {
                // No saved payload can be mined while the nonce is unused; the normal flow checks receipts.
                self.clear_nonce_used_seen(tx).await?;
                return Ok(NonceReconcile::Continue);
            }
            warn!(
                tx_id = %tx.id,
                "nonce recovery: unused nonce not confirmed at the sighting's head, holding"
            );
            on_chain_nonce = None;
        }
        // Keep this read after the nonce and before every receipt lookup.
        let block_number = match on_chain_nonce {
            None => None,
            Some(_) => match self.provider().get_block_number().await {
                Ok(block) => Some(block),
                Err(e) if holding => {
                    warn!(tx_id = %tx.id, error = %e, "nonce recovery: block read failed, holding");
                    None
                }
                Err(e) => {
                    warn!(
                        tx_id = %tx.id,
                        error = %e,
                        "nonce recovery: block read failed, using normal status flow"
                    );
                    return Ok(NonceReconcile::Continue);
                }
            },
        };
        let mut had_rpc_errors = false;

        // A current receipt returns to the normal status flow.
        if let Some(ref hash) = evm_data.hash {
            match self.provider().get_transaction_receipt(hash).await {
                Ok(Some(receipt)) => {
                    debug!(
                        tx_id = %tx.id,
                        hash = %hash,
                        "nonce recovery: receipt found for current hash, deferring to normal flow"
                    );
                    self.clear_nonce_used_seen(tx).await?;
                    return Ok(NonceReconcile::Receipt(Box::new(receipt)));
                }
                Ok(None) => {
                    // No receipt for current hash — continue recovery
                }
                Err(e) => {
                    warn!(
                        tx_id = %tx.id,
                        hash = %hash,
                        error = %e,
                        "nonce recovery: error checking receipt for current hash"
                    );
                    had_rpc_errors = true;
                }
            }
        }

        match self
            .try_recover_with_historical_hashes(tx, &evm_data)
            .await?
        {
            HistoricalHashRecovery::Recovered(recovered_tx) => {
                return Ok(NonceReconcile::Handled(recovered_tx))
            }
            HistoricalHashRecovery::NotFound => {}
            HistoricalHashRecovery::Incomplete => had_rpc_errors = true,
        }

        let (Some(on_chain_nonce), Some(block_number)) = (on_chain_nonce, block_number) else {
            return Ok(NonceReconcile::Handled(Box::new(tx.clone())));
        };
        if had_rpc_errors {
            return Ok(NonceReconcile::Handled(Box::new(tx.clone())));
        }

        let mut metadata = tx.metadata.clone().unwrap_or_default();
        let seen_at = metadata
            .nonce_used_seen_at
            .as_deref()
            .and_then(|at| DateTime::parse_from_rfc3339(at).ok());
        let (Some(seen_at_block), Some(seen_at)) = (metadata.nonce_used_seen_at_block, seen_at)
        else {
            metadata.nonce_used_seen_at_block = Some(block_number);
            metadata.nonce_used_seen_at = Some(Utc::now().to_rfc3339());
            let updated_tx = self
                .cas_update(
                    tx,
                    TransactionUpdateRequest {
                        metadata: Some(metadata),
                        ..Default::default()
                    },
                )
                .await?;
            // If another key used the nonce, the counter is behind the chain. Resync it
            // now rather than after the wait, so later transactions do not reuse used
            // nonces. Harmless when our own transaction used it. Best-effort.
            if let Err(e) = self.schedule_relayer_nonce_health_job(tx).await {
                warn!(
                    tx_id = %tx.id,
                    error = %e,
                    "nonce recovery: failed to schedule nonce health on first sighting"
                );
            }
            return Ok(NonceReconcile::Handled(Box::new(updated_tx)));
        };
        // A network config error gives no verdict; hold rather than feed the breaker.
        let network = match self
            .network_repository()
            .get_by_chain_id(NetworkType::Evm, evm_data.chain_id)
            .await
        {
            Ok(Some(model)) => EvmNetwork::try_from(model).map_err(|e| e.to_string()),
            Ok(None) => Err(format!(
                "network with chain id {} not found",
                evm_data.chain_id
            )),
            Err(e) => Err(e.to_string()),
        };
        let network = match network {
            Ok(network) => network,
            Err(error) => {
                warn!(tx_id = %tx.id, %error, "nonce recovery: network lookup failed, holding");
                return Ok(NonceReconcile::Handled(Box::new(tx.clone())));
            }
        };
        let span =
            nonce_used_failure_span(network.required_confirmations, network.average_blocktime_ms);
        // Both the block span and the wall-clock minimum must pass: blocks bound
        // reorgs and confirmations, time bounds provider lag on fast chains. The
        // maximum age ends the wait on a head that stopped advancing.
        let waited = Utc::now().signed_duration_since(seen_at);
        let span_passed = block_number.saturating_sub(seen_at_block) >= span;
        if waited < get_evm_nonce_used_failure_max_age()
            && (!span_passed || waited < get_evm_nonce_used_failure_min_age())
        {
            return Ok(NonceReconcile::Handled(Box::new(tx.clone())));
        }

        // Nonce was consumed but no known hash found — consumed externally
        let reason = format!(
            "Nonce {tx_nonce} consumed externally (on-chain nonce: {on_chain_nonce}). \
             No matching transaction hash found on-chain."
        );
        warn!(
            tx_id = %tx.id,
            relayer_id = %tx.relayer_id,
            tx_nonce = tx_nonce,
            on_chain_nonce = on_chain_nonce,
            "nonce recovery: nonce consumed externally, marking as Failed"
        );

        let updated_tx = self
            .update_transaction_status(tx.clone(), TransactionStatus::Failed, Some(reason))
            .await?;

        // External nonce consumption may have left the internal transaction counter
        // behind the on-chain nonce, or created gaps. Schedule a nonce health job
        // to sync the counter and fill any gaps with NOOPs. Best-effort — failure
        // here doesn't block the recovery; the periodic health check will catch it.
        if let Err(e) = self.schedule_relayer_nonce_health_job(tx).await {
            warn!(
                tx_id = %tx.id,
                error = %e,
                "nonce recovery: failed to schedule nonce health after external consumption"
            );
        }

        Ok(NonceReconcile::Handled(Box::new(updated_tx)))
    }

    /// Handles circuit breaker safely based on transaction status.
    ///
    /// This method implements the safe circuit breaker logic:
    /// - **Pending/Sent**: Safe to mark as Failed (never broadcast to network)
    /// - **Submitted**: Must trigger NOOP to clear nonce slot (regardless of expiry)
    ///
    /// For Submitted transactions, we always issue a NOOP because the nonce slot is
    /// occupied and the original transaction could still execute. Simply marking as
    /// Failed/Expired would leave the nonce blocked and risk the relayer stopping.
    ///
    /// Note: NOOP transactions are filtered out before entering this function.
    async fn handle_circuit_breaker_safely(
        &self,
        tx: TransactionRepoModel,
        ctx: &StatusCheckContext,
    ) -> Result<TransactionRepoModel, TransactionError> {
        let reason = format!(
            "Transaction status monitoring failed after {} consecutive errors (total: {}). \
             Last status: {:?}.",
            ctx.consecutive_failures, ctx.total_failures, tx.status
        );

        match tx.status {
            TransactionStatus::Pending => {
                // The guarded failure write also schedules repair of an assigned nonce.
                debug!(
                    tx_id = %tx.id,
                    relayer_id = %tx.relayer_id,
                    "circuit breaker: marking Pending transaction as Failed"
                );
                self.mark_as_failed(tx, reason).await
            }
            TransactionStatus::Sent => {
                // Sent: nonce assigned but never broadcast to network.
                // If a nonce is assigned, we must issue a NOOP to clear the nonce slot
                // rather than just marking as Failed (which would leak the nonce).
                let has_nonce = tx
                    .network_data
                    .get_evm_transaction_data()
                    .map(|d| d.nonce.is_some())
                    .unwrap_or(false);

                if has_nonce {
                    warn!(
                        tx_id = %tx.id,
                        relayer_id = %tx.relayer_id,
                        "circuit breaker: Sent transaction with nonce assigned - triggering NOOP to clear nonce slot"
                    );
                    let noop_reason = Some(format!(
                        "{reason}. Replacing with NOOP to clear nonce slot (Sent state with assigned nonce)."
                    ));
                    let updated_tx = self.process_noop_transaction(&tx, noop_reason).await?;
                    // Must use resubmit (not submit): make_noop dropped the signed bytes,
                    // and resubmit signs the NOOP with new gas pricing.
                    self.send_noop_resubmit_job(&updated_tx).await?;
                    Ok(updated_tx)
                } else {
                    // Defensive: Sent without nonce shouldn't normally happen
                    debug!(
                        tx_id = %tx.id,
                        relayer_id = %tx.relayer_id,
                        "circuit breaker: Sent transaction without nonce - safe to mark as Failed"
                    );
                    self.mark_as_failed(tx, reason).await
                }
            }
            TransactionStatus::Submitted => {
                // Submitted transactions occupy a nonce slot and could still execute.
                // Regardless of expiry status, we MUST issue a NOOP to:
                // 1. Clear the nonce slot so subsequent transactions can proceed
                // 2. Prevent the original transaction from executing later
                // Note: NOOP transactions are filtered out before entering this function.
                warn!(
                    tx_id = %tx.id,
                    relayer_id = %tx.relayer_id,
                    "circuit breaker: Submitted transaction - triggering NOOP to safely clear nonce"
                );
                let noop_reason = Some(format!(
                    "{reason}. Replacing with NOOP to clear nonce slot."
                ));
                let updated_tx = self.process_noop_transaction(&tx, noop_reason).await?;
                self.send_transaction_resubmit_job(&updated_tx).await?;
                Ok(updated_tx)
            }
            _ => {
                // Final states shouldn't reach here, but handle gracefully
                debug!(
                    tx_id = %tx.id,
                    relayer_id = %tx.relayer_id,
                    status = ?tx.status,
                    "circuit breaker: unexpected status, returning transaction unchanged"
                );
                Ok(tx)
            }
        }
    }

    /// Inherent status-handling method.
    ///
    /// This method encapsulates the full logic for handling transaction status,
    /// including resubmission, NOOP replacement, timeout detection, and updating status.
    pub async fn handle_status_impl(
        &self,
        mut tx: TransactionRepoModel,
        context: Option<StatusCheckContext>,
    ) -> Result<TransactionRepoModel, TransactionError> {
        debug!(
            tx_id = %tx.id,
            relayer_id = %tx.relayer_id,
            status = ?tx.status,
            "checking transaction status"
        );

        // 1. Early return if final state
        if is_final_state(&tx.status) {
            debug!(
                tx_id = %tx.id,
                relayer_id = %tx.relayer_id,
                status = ?tx.status,
                "transaction already in final state"
            );
            return Ok(tx);
        }

        // 1.1. Check if circuit breaker should force finalization
        // Skip circuit breaker for NOOP transactions - they're already safe (just clearing nonce)
        // and should be handled by normal status logic which will eventually resolve them.
        if let Some(ref ctx) = context {
            let is_noop_tx = tx
                .network_data
                .get_evm_transaction_data()
                .map(|data| is_noop(&data))
                .unwrap_or(false);

            // No node accepted these bytes yet, so a run of failed checks
            // (an RPC outage) is no reason to cancel them. The total limit
            // still applies, so the breaker fires in the end.
            let never_accepted = tx.status == TransactionStatus::Submitted
                && tx.sent_at.is_none()
                && !ctx.triggered_by_total();
            if never_accepted && ctx.should_force_finalize() {
                warn!(
                    tx_id = %tx.id,
                    consecutive_failures = ctx.consecutive_failures,
                    relayer_id = %tx.relayer_id,
                    "circuit breaker skipped: no node accepted the transaction yet"
                );
            }

            if ctx.should_force_finalize() && !is_noop_tx && !never_accepted {
                warn!(
                    tx_id = %tx.id,
                    consecutive_failures = ctx.consecutive_failures,
                    total_failures = ctx.total_failures,
                    max_consecutive = ctx.max_consecutive_failures,
                    status = ?tx.status,
                    "circuit breaker triggered - handling safely based on transaction state"
                );
                return self.handle_circuit_breaker_safely(tx, ctx).await;
            }

            if ctx.should_force_finalize() && is_noop_tx {
                debug!(
                    tx_id = %tx.id,
                    consecutive_failures = ctx.consecutive_failures,
                    relayer_id = %tx.relayer_id,
                    "circuit breaker would trigger but transaction is NOOP - continuing with normal status logic"
                );
            }
        }

        let has_recovery_hint = context
            .as_ref()
            .and_then(|ctx| ctx.job_metadata.as_ref())
            .is_some_and(|metadata| metadata.contains_key(TX_NONCE_RECONCILE_TRIGGER));
        let nonce_used_seen = has_nonce_used_seen(&tx);
        // Continue the observation window on ordinary status jobs too.
        let mut receipt = None;
        if has_recovery_hint || nonce_used_seen {
            match self.reconcile_tx_nonce_state(&mut tx).await? {
                NonceReconcile::Handled(recovered_tx) => return Ok(*recovered_tx),
                NonceReconcile::Receipt(found) => receipt = Some(found),
                NonceReconcile::Continue => {}
            }
        }

        // 2. Check transaction status first
        // This allows fast transactions to update their status immediately,
        // even if they're young (<20s). Pending returns without an on-chain query.
        let status = match receipt {
            Some(receipt) => self.status_from_receipt(&tx, &receipt).await?,
            None => self.check_transaction_status(&tx).await?,
        };

        debug!(
            tx_id = %tx.id,
            previous_status = ?tx.status,
            new_status = ?status,
            relayer_id = %tx.relayer_id,
            "transaction status check completed"
        );

        // 2.1. Reload transaction from DB if status changed
        // This ensures we have fresh data if check_transaction_status triggered a recovery
        // or any other update that modified the transaction in the database.
        let tx = if status != tx.status {
            debug!(
                tx_id = %tx.id,
                old_status = ?tx.status,
                new_status = ?status,
                relayer_id = %tx.relayer_id,
                "status changed during check, reloading transaction from DB to ensure fresh data"
            );
            // Primary: a lagging replica would make the next guarded write lose.
            let stored = self
                .transaction_repository()
                .get_by_id_on_primary(tx.id.clone())
                .await?;
            // Hash recovery above writes Mined. A newer Mined record is accepted
            // because handle_mined_state then writes nothing; keep it that way.
            // Any other change means the status was read for older data.
            let own_recovery =
                status == TransactionStatus::Mined && stored.status == TransactionStatus::Mined;
            if stored.version != tx.version && !own_recovery {
                return Err(TransactionError::ConcurrentUpdateConflict(tx.id.clone()));
            }
            stored
        } else {
            tx
        };

        // 3. Check if too early for resubmission on in-progress transactions
        // For Pending/Sent/Submitted states, defer resubmission logic and timeout checks
        // if the transaction is too young. Just update status and return.
        // For other states (Mined/Confirmed/Failed/etc), process immediately regardless of age.
        if is_too_early_to_resubmit(&tx)?
            && is_pending_transaction(&status)
            && !(matches!(
                status,
                TransactionStatus::Sent | TransactionStatus::Submitted
            ) && tx.network_data.get_evm_transaction_data()?.raw.is_none())
        {
            // Update status if it changed, then return
            return self
                .update_transaction_status_if_needed(tx, status, None)
                .await;
        }

        // 4. Handle based on status (including complex operations like resubmission)
        match status {
            TransactionStatus::Pending => self.handle_pending_state(tx).await,
            TransactionStatus::Sent => self.handle_sent_state(tx).await,
            TransactionStatus::Submitted => self.handle_submitted_state(tx).await,
            TransactionStatus::Mined => self.handle_mined_state(tx).await,
            TransactionStatus::Failed => {
                // On the transition into Failed, attempt best-effort recovery of the on-chain
                // revert payload; on re-polls of an already-Failed tx, leave the reason untouched.
                let status_reason = if tx.status != TransactionStatus::Failed {
                    Some(self.build_failed_status_reason(&tx).await)
                } else {
                    None
                };
                self.handle_final_state(tx, status, status_reason).await
            }
            TransactionStatus::Confirmed
            | TransactionStatus::Expired
            | TransactionStatus::Canceled => self.handle_final_state(tx, status, None).await,
        }
    }

    /// Builds the `status_reason` recorded when a transaction transitions into Failed from a
    /// failed on-chain receipt. When revert-data recovery is enabled (default) and a payload is
    /// recovered, returns the enriched reason; otherwise returns the generic string.
    async fn build_failed_status_reason(&self, tx: &TransactionRepoModel) -> String {
        if !self
            .relayer()
            .policies
            .get_evm_policy()
            .include_revert_data
            .unwrap_or(DEFAULT_EVM_INCLUDE_REVERT_DATA)
        {
            debug!(
                tx_id = %tx.id,
                "revert-data recovery disabled by policy; using generic reason"
            );
            return REVERT_REASON_GENERIC.to_string();
        }

        match self.recover_revert_data(tx).await {
            Some(hex) => {
                let hex = truncate_revert_hex(&hex);
                format!("Transaction reverted on-chain (revert_data: {hex})")
            }
            None => REVERT_REASON_GENERIC.to_string(),
        }
    }

    /// Best-effort recovery of the on-chain revert payload for a transaction entering Failed.
    ///
    /// Prefers `debug_traceTransaction` with `callTracer`; falls back to an `eth_call` re-executed
    /// at the transaction's execution block. Every error is contained and mapped to `None`, so a
    /// recovery failure can never propagate out of the status flow. Returns the `0x`-prefixed
    /// revert hex when recovered.
    async fn recover_revert_data(&self, tx: &TransactionRepoModel) -> Option<String> {
        let evm_data = tx.network_data.get_evm_transaction_data().ok()?;
        let hash = evm_data.hash.as_ref()?;

        // Preferred: debug_traceTransaction. An Err means tracing is unavailable on this RPC,
        // so fall through to eth_call.
        match self
            .provider()
            .raw_request_dyn_best_effort(
                "debug_traceTransaction",
                json!([hash, {"tracer": "callTracer"}]),
            )
            .await
        {
            Ok(trace) => {
                if let Some(hex) = extract_trace_output(&trace) {
                    info!(
                        tx_id = %tx.id,
                        method = "trace",
                        "recovered on-chain revert data"
                    );
                    return Some(hex);
                }
                debug!(
                    tx_id = %tx.id,
                    "trace returned no revert output; trying eth_call fallback"
                );
            }
            Err(e) => {
                warn!(
                    tx_id = %tx.id,
                    error = %e,
                    "debug_traceTransaction unavailable; trying eth_call fallback"
                );
            }
        }

        // Fallback: re-fetch the receipt to learn the execution block, then re-run the
        // transaction as an eth_call at that block.
        let block_number = match self.provider().get_transaction_receipt(hash).await {
            Ok(Some(receipt)) => match receipt.block_number {
                Some(bn) => bn,
                None => {
                    debug!(
                        tx_id = %tx.id,
                        "receipt missing block number; skipping eth_call recovery"
                    );
                    return None;
                }
            },
            Ok(None) => {
                debug!(
                    tx_id = %tx.id,
                    "no receipt found; skipping eth_call recovery"
                );
                return None;
            }
            Err(e) => {
                warn!(
                    tx_id = %tx.id,
                    error = %e,
                    "failed to re-fetch receipt for revert recovery"
                );
                return None;
            }
        };

        let request = match build_revert_call_request(&evm_data) {
            Ok(req) => req,
            Err(e) => {
                warn!(
                    tx_id = %tx.id,
                    error = %e,
                    "failed to reconstruct eth_call request for revert recovery"
                );
                return None;
            }
        };

        match self
            .provider()
            .get_call_revert_data(&request, block_number)
            .await
        {
            Ok(Some(bytes)) if !bytes.is_empty() => {
                let hex = format!("0x{}", hex::encode(&bytes));
                info!(
                    tx_id = %tx.id,
                    method = "eth_call",
                    "recovered on-chain revert data"
                );
                Some(hex)
            }
            Ok(_) => {
                debug!(tx_id = %tx.id, "no revert data recovered");
                None
            }
            Err(e) => {
                warn!(
                    tx_id = %tx.id,
                    error = %e,
                    "eth_call revert recovery failed"
                );
                None
            }
        }
    }

    /// Handle transactions stuck in Sent (prepared but not submitted)
    async fn handle_sent_state(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        debug!(
            tx_id = %tx.id,
            relayer_id = %tx.relayer_id,
            "handling Sent state"
        );

        // Check if transaction should be replaced with NOOP (expired, too many attempts, etc.)
        let (should_noop, reason) = self.should_noop(&tx).await?;
        if should_noop {
            debug!(
                tx_id = %tx.id,
                relayer_id = %tx.relayer_id,
                "preparing NOOP for sent transaction"
            );
            let update = self.prepare_noop_update_request(&tx, false, reason).await?;
            let updated_tx = self.cas_update(&tx, update).await?;

            // Resubmit, not submit: make_noop dropped the signed bytes, and the
            // resubmit path re-prices and signs the NOOP.
            self.send_noop_resubmit_job(&updated_tx).await?;
            let res = self.send_transaction_update_notification(&updated_tx).await;
            if let Err(e) = res {
                error!(
                    tx_id = %updated_tx.id,
                    relayer_id = %updated_tx.relayer_id,
                    status = ?updated_tx.status,
                    error = %e,
                    "sending transaction update notification failed for Sent state NOOP"
                );
            }
            return Ok(updated_tx);
        }

        // Transaction was prepared but submission job may have failed
        // Re-queue a resend job if it's been stuck for a while
        let age_since_sent = get_age_since_priced(&tx)?;
        let missing_raw = tx.network_data.get_evm_transaction_data()?.raw.is_none();

        if missing_raw {
            self.send_transaction_resubmit_job(&tx).await?;
            return Ok(tx);
        }

        if age_since_sent > get_evm_resend_timeout() {
            // A nonce gap below this tx makes any resubmit futile; the detector schedules
            // the nonce-health job itself. Only worth the RPC once the tx is already stale.
            let nonce_check = self.detect_nonce_gap_ahead(&tx).await;
            if nonce_check.as_ref().is_some_and(|check| check.has_gap) {
                return self
                    .update_transaction_status_if_needed(tx, TransactionStatus::Sent, None)
                    .await;
            }

            // Arbitrum head gate: the sequencer holds a nonce-above-head tx ~1s and
            // then rejects it, so a resubmit of anything but the head always fails.
            // Only the tx at the on-chain nonce is broadcast. Successors are delivered
            // by the broadcast-success chain link, or by this gate once they become
            // the head.
            if let Some(check) = nonce_check {
                if self
                    .is_behind_arbitrum_head(&tx, check.on_chain_nonce)
                    .await
                {
                    debug!(
                        tx_id = %tx.id,
                        relayer_id = %tx.relayer_id,
                        "stuck Sent tx is queued behind the chain head on Arbitrum; skipping resubmit"
                    );
                    return self
                        .update_transaction_status_if_needed(tx, TransactionStatus::Sent, None)
                        .await;
                }
            }

            warn!(
                tx_id = %tx.id,
                relayer_id = %tx.relayer_id,
                age_seconds = age_since_sent.num_seconds(),
                "transaction stuck in Sent, scheduling delivery"
            );

            // Reuse the stored bytes until the price timer is due.
            if self.should_resubmit(&tx).await? {
                self.send_transaction_resubmit_job(&tx).await?;
            } else {
                self.send_transaction_submit_job(&tx).await?;
            }
        }

        self.update_transaction_status_if_needed(tx, TransactionStatus::Sent, None)
            .await
    }

    /// Arbitrum only: returns `true` when the tx nonce is above the on-chain account
    /// nonce (queued behind other txs, guaranteed rejection if broadcast). Non-Arbitrum
    /// networks and lookup failures return `false` so resubmit behavior is preserved.
    async fn is_behind_arbitrum_head(
        &self,
        tx: &TransactionRepoModel,
        on_chain_nonce: u64,
    ) -> bool {
        let Ok(evm_data) = tx.network_data.get_evm_transaction_data() else {
            return false;
        };
        let Some(tx_nonce) = evm_data.nonce else {
            return false;
        };

        // Compare first, so the network lookup only runs for a tx behind the head.
        tx_nonce > on_chain_nonce && self.is_arbitrum_network(tx).await
    }

    /// Queues the resubmit job that signs and sends a NOOP replacement. On Arbitrum, a
    /// `Sent` NOOP above the chain head would be rejected: it stays persisted and is
    /// sent by the chain link, or by a later status check once it is the head. Other
    /// networks always get the job.
    pub(super) async fn send_noop_resubmit_job(
        &self,
        tx: &TransactionRepoModel,
    ) -> Result<(), TransactionError> {
        if tx.status == TransactionStatus::Sent && self.is_above_arbitrum_head(tx).await {
            debug!(
                tx_id = %tx.id,
                relayer_id = %tx.relayer_id,
                "Sent NOOP is above the chain head on Arbitrum; deferring resubmit"
            );
            return Ok(());
        }
        self.send_transaction_resubmit_job(tx).await
    }

    /// Arbitrum only: fetches the on-chain nonce and returns `true` when the tx nonce
    /// is above it. The network check runs first, so other networks pay no RPC call.
    /// Lookup and RPC failures return `false`, so the caller still sends.
    async fn is_above_arbitrum_head(&self, tx: &TransactionRepoModel) -> bool {
        let Some(tx_nonce) = tx
            .network_data
            .get_evm_transaction_data()
            .ok()
            .and_then(|data| data.nonce)
        else {
            return false;
        };
        if !self.is_arbitrum_network(tx).await {
            return false;
        }
        match self
            .provider()
            .get_transaction_count(&self.relayer().address)
            .await
        {
            Ok(on_chain_nonce) => tx_nonce > on_chain_nonce,
            Err(e) => {
                debug!(tx_id = %tx.id, error = %e, "head check: failed to get on-chain nonce");
                false
            }
        }
    }

    /// Determines if we should attempt hash recovery for a stuck transaction.
    ///
    /// Check replacements at once. Check resubmissions after the recovery timeout.
    fn should_try_hash_recovery(
        &self,
        tx: &TransactionRepoModel,
    ) -> Result<bool, TransactionError> {
        if !matches!(
            tx.status,
            TransactionStatus::Submitted | TransactionStatus::Sent
        ) {
            return Ok(false);
        }
        let evm_data = tx.network_data.get_evm_transaction_data()?;
        // Nothing to recover when the only saved hash is the current one.
        if !tx
            .hashes
            .iter()
            .any(|hash| Some(hash) != evm_data.hash.as_ref())
        {
            return Ok(false);
        }
        if tx.status == TransactionStatus::Sent
            || !tx
                .hashes
                .iter()
                .any(|hash| Some(hash) == evm_data.hash.as_ref())
        {
            return Ok(true);
        }
        if tx.hashes.len() < EVM_MIN_HASHES_FOR_RECOVERY {
            return Ok(false);
        }

        // Only try if transaction has been stuck for a while
        let age = get_age_since_priced(tx)?;
        let min_age_for_recovery = get_evm_min_age_for_hash_recovery();

        if age < min_age_for_recovery {
            return Ok(false);
        }

        Ok(true)
    }

    /// Attempts to recover transaction status by checking all historical hashes.
    ///
    /// When a transaction is resubmitted multiple times due to timeouts, the database
    /// may contain multiple hashes. The "current" hash (network_data.hash) might not
    /// be the one that actually got mined. This method checks all historical hashes
    /// to find if any were mined, and updates the database with the correct one.
    ///
    /// Returns `Recovered` with the updated transaction model if a historical hash was mined.
    /// Receipt lookup errors give `Incomplete`; only repository errors are returned as `Err`.
    async fn try_recover_with_historical_hashes(
        &self,
        tx: &TransactionRepoModel,
        evm_data: &crate::models::EvmTransactionData,
    ) -> Result<HistoricalHashRecovery, TransactionError> {
        debug!(
            tx_id = %tx.id,
            relayer_id = %tx.relayer_id,
            current_hash = ?evm_data.hash,
            total_hashes = %tx.hashes.len(),
            "attempting hash recovery - checking historical hashes"
        );

        let mut had_lookup_error = false;

        // Check each historical hash (most recent first, since it's more likely)
        for (idx, historical_hash) in tx.hashes.iter().rev().enumerate() {
            // Skip if this is the current hash (already checked)
            if Some(historical_hash) == evm_data.hash.as_ref() {
                continue;
            }

            debug!(
                tx_id = %tx.id,
                relayer_id = %tx.relayer_id,
                hash = %historical_hash,
                index = %idx,
                "checking historical hash"
            );

            // Try to get receipt for this hash
            match self
                .provider()
                .get_transaction_receipt(historical_hash)
                .await
            {
                Ok(Some(receipt)) => {
                    warn!(
                        tx_id = %tx.id,
                        relayer_id = %tx.relayer_id,
                        mined_hash = %historical_hash,
                        wrong_hash = ?evm_data.hash,
                        block_number = ?receipt.block_number,
                        "RECOVERED: found mined transaction with historical hash - correcting database"
                    );

                    // The stored payload may be a later replacement or NOOP; publish the
                    // payload that actually mined with its hash. If it cannot be read,
                    // still record Mined with the stored payload: dropping the receipt
                    // would let the normal flow resubmit or NOOP a mined transaction.
                    let mut mined_data = self.payload_for_hash(evm_data, historical_hash).await;
                    if mined_data.is_err() {
                        // One retry: a lagging node may not have the transaction yet.
                        mined_data = self.payload_for_hash(evm_data, historical_hash).await;
                    }
                    let mined_data = match mined_data {
                        Ok(data) => data,
                        Err(error) => {
                            warn!(
                                tx_id = %tx.id,
                                hash = %historical_hash,
                                %error,
                                "could not read the mined payload; keeping the stored payload"
                            );
                            // The stored signature and raw bytes belong to another attempt.
                            let mut data = evm_data.clone();
                            data.signature = None;
                            data.raw = None;
                            data
                        }
                    };

                    // Update with correct hash and Mined status
                    // Let the normal status check flow handle confirmation checking
                    let updated_tx = self
                        .update_transaction_with_corrected_hash(
                            tx,
                            &mined_data,
                            historical_hash,
                            TransactionStatus::Mined,
                        )
                        .await?;

                    return Ok(HistoricalHashRecovery::Recovered(Box::new(updated_tx)));
                }
                Ok(None) => {
                    // This hash not found either, continue to next
                    continue;
                }
                Err(e) => {
                    // Network error, log but continue checking other hashes
                    warn!(
                        tx_id = %tx.id,
                        relayer_id = %tx.relayer_id,
                        hash = %historical_hash,
                        error = %e,
                        "error checking historical hash, continuing to next"
                    );
                    had_lookup_error = true;
                    continue;
                }
            }
        }

        // The mined hash may be one we failed to check, so callers must not treat
        // the nonce as consumed externally.
        if had_lookup_error {
            return Ok(HistoricalHashRecovery::Incomplete);
        }

        // None of the historical hashes found on-chain
        debug!(
            tx_id = %tx.id,
            relayer_id = %tx.relayer_id,
            "hash recovery completed - no historical hashes found on-chain"
        );
        Ok(HistoricalHashRecovery::NotFound)
    }

    /// Clears the "nonce used" sighting, if set, and replaces `tx` with the stored
    /// record so the caller's next guarded write is not refused by this one.
    /// See `without_nonce_used_seen`.
    async fn clear_nonce_used_seen(
        &self,
        tx: &mut TransactionRepoModel,
    ) -> Result<(), TransactionError> {
        if let Some(metadata) = without_nonce_used_seen(tx) {
            *tx = self
                .cas_update(
                    tx,
                    TransactionUpdateRequest {
                        metadata: Some(metadata),
                        ..Default::default()
                    },
                )
                .await?;
        }
        Ok(())
    }

    /// The payload of attempt `hash` as the chain has it, applied over `current`.
    async fn payload_for_hash(
        &self,
        current: &EvmTransactionData,
        hash: &str,
    ) -> Result<EvmTransactionData, String> {
        let rpc = self
            .provider()
            .raw_request_dyn("eth_getTransactionByHash", json!([hash]))
            .await
            .map_err(|e| e.to_string())?;
        payload_from_rpc(current, &rpc)
    }

    /// Updates transaction with the corrected hash and status
    ///
    /// Returns the updated transaction model and sends a notification about the status change.
    async fn update_transaction_with_corrected_hash(
        &self,
        tx: &TransactionRepoModel,
        evm_data: &crate::models::EvmTransactionData,
        correct_hash: &str,
        status: TransactionStatus,
    ) -> Result<TransactionRepoModel, TransactionError> {
        let mut corrected_data = evm_data.clone();
        corrected_data.hash = Some(correct_hash.to_string());

        let updated_tx = self
            .cas_update(
                tx,
                TransactionUpdateRequest {
                    network_data: Some(NetworkTransactionData::Evm(corrected_data)),
                    status: Some(status),
                    metadata: without_nonce_used_seen(tx),
                    ..Default::default()
                },
            )
            .await?;

        // Send notification about the recovered transaction
        if let Err(e) = self.send_transaction_update_notification(&updated_tx).await {
            error!(
                tx_id = %updated_tx.id,
                relayer_id = %updated_tx.relayer_id,
                error = %e,
                "failed to send notification for hash recovery"
            );
        }

        Ok(updated_tx)
    }
}

#[cfg(test)]
pub(super) mod tests {
    use crate::{
        config::{EvmNetworkConfig, NetworkConfigCommon},
        domain::transaction::evm::{EvmRelayerTransaction, MockPriceCalculatorTrait},
        jobs::MockJobProducerTrait,
        models::{
            evm::Speed, EvmTransactionData, NetworkConfigData, NetworkRepoModel,
            NetworkTransactionData, NetworkType, RelayerEvmPolicy, RelayerNetworkPolicy,
            RelayerRepoModel, RpcConfig, TransactionReceipt, TransactionRepoModel,
            TransactionStatus, U256,
        },
        repositories::{
            MockNetworkRepository, MockRelayerRepository, MockTransactionCounterTrait,
            MockTransactionRepository,
        },
        services::{provider::MockEvmProviderTrait, signer::MockSigner},
    };
    use alloy::{
        consensus::{Eip658Value, Receipt, ReceiptWithBloom},
        network::AnyReceiptEnvelope,
        primitives::{b256, Address, BlockHash, Bloom, TxHash},
    };
    use chrono::{Duration, Utc};
    use std::sync::Arc;

    /// Helper struct holding all the mocks we often need
    pub struct TestMocks {
        pub provider: MockEvmProviderTrait,
        pub relayer_repo: MockRelayerRepository,
        pub network_repo: MockNetworkRepository,
        pub tx_repo: MockTransactionRepository,
        pub job_producer: MockJobProducerTrait,
        pub signer: MockSigner,
        pub counter: MockTransactionCounterTrait,
        pub price_calc: MockPriceCalculatorTrait,
    }

    /// Returns a default `TestMocks` with zero-configuration stubs.
    /// You can override expectations in each test as needed.
    pub fn default_test_mocks() -> TestMocks {
        let mut provider = MockEvmProviderTrait::new();
        // A recovered hash reads its mined payload; echo the default test payload.
        provider
            .expect_raw_request_dyn()
            .withf(|method, _| method == "eth_getTransactionByHash")
            .returning(|_, _| {
                Box::pin(async {
                    Ok(serde_json::json!({
                        "to": "0xRecipient",
                        "value": "0x0",
                        "input": "0xData",
                        "gas": "0x5208",
                        "gasPrice": "0x4a817c800"
                    }))
                })
            });
        TestMocks {
            provider,
            relayer_repo: MockRelayerRepository::new(),
            network_repo: MockNetworkRepository::new(),
            tx_repo: MockTransactionRepository::new(),
            job_producer: MockJobProducerTrait::new(),
            signer: MockSigner::new(),
            counter: MockTransactionCounterTrait::new(),
            price_calc: MockPriceCalculatorTrait::new(),
        }
    }

    /// Returns a `TestMocks` with network repository configured for prepare_noop_update_request tests.
    pub fn default_test_mocks_with_network() -> TestMocks {
        let mut mocks = default_test_mocks();
        // Set up default expectation for get_by_chain_id that prepare_noop_update_request tests need
        mocks
            .network_repo
            .expect_get_by_chain_id()
            .returning(|network_type, chain_id| {
                if network_type == NetworkType::Evm && chain_id == 1 {
                    Ok(Some(create_test_network_model()))
                } else {
                    Ok(None)
                }
            });
        mocks
    }

    /// Creates a test NetworkRepoModel for chain_id 1 (mainnet)
    pub fn create_test_network_model() -> NetworkRepoModel {
        let evm_config = EvmNetworkConfig {
            common: NetworkConfigCommon {
                network: "mainnet".to_string(),
                from: None,
                rpc_urls: Some(vec![RpcConfig::new("https://rpc.example.com".to_string())]),
                explorer_urls: Some(vec!["https://explorer.example.com".to_string()]),
                average_blocktime_ms: Some(12000),
                is_testnet: Some(false),
                tags: Some(vec!["mainnet".to_string()]),
            },
            chain_id: Some(1),
            required_confirmations: Some(12),
            status_check: None,
            features: Some(vec!["eip1559".to_string()]),
            symbol: Some("ETH".to_string()),
            gas_price_cache: None,
        };
        NetworkRepoModel {
            id: "evm:mainnet".to_string(),
            name: "mainnet".to_string(),
            network_type: NetworkType::Evm,
            config: NetworkConfigData::Evm(evm_config),
        }
    }

    /// Creates a test NetworkRepoModel for chain_id 42161 (Arbitrum-like) with no-mempool tag
    pub fn create_test_no_mempool_network_model() -> NetworkRepoModel {
        let evm_config = EvmNetworkConfig {
            common: NetworkConfigCommon {
                network: "arbitrum".to_string(),
                from: None,
                rpc_urls: Some(vec![crate::models::RpcConfig::new(
                    "https://arb-rpc.example.com".to_string(),
                )]),
                explorer_urls: Some(vec!["https://arb-explorer.example.com".to_string()]),
                average_blocktime_ms: Some(1000),
                is_testnet: Some(false),
                tags: Some(vec![
                    "arbitrum".to_string(),
                    "rollup".to_string(),
                    "no-mempool".to_string(),
                ]),
            },
            chain_id: Some(42161),
            required_confirmations: Some(12),
            status_check: None,
            features: Some(vec!["eip1559".to_string()]),
            symbol: Some("ETH".to_string()),
            gas_price_cache: None,
        };
        NetworkRepoModel {
            id: "evm:arbitrum".to_string(),
            name: "arbitrum".to_string(),
            network_type: NetworkType::Evm,
            config: NetworkConfigData::Evm(evm_config),
        }
    }

    /// Creates a test NetworkRepoModel for chain_id 1 with Arbitrum-based rollup tags,
    /// so `is_arbitrum()` and `is_rollup()` both return true.
    pub fn create_test_arbitrum_network_model() -> NetworkRepoModel {
        let evm_config = EvmNetworkConfig {
            common: NetworkConfigCommon {
                network: "arbitrum-one".to_string(),
                from: None,
                rpc_urls: Some(vec![RpcConfig::new(
                    "https://arb-rpc.example.com".to_string(),
                )]),
                explorer_urls: Some(vec!["https://arb-explorer.example.com".to_string()]),
                average_blocktime_ms: Some(250),
                is_testnet: Some(false),
                tags: Some(vec!["rollup".to_string(), "arbitrum-based".to_string()]),
            },
            chain_id: Some(1),
            required_confirmations: Some(1),
            features: Some(vec!["eip1559".to_string()]),
            symbol: Some("ETH".to_string()),
            gas_price_cache: None,
            status_check: None,
        };
        NetworkRepoModel {
            id: "evm:arbitrum-one".to_string(),
            name: "arbitrum-one".to_string(),
            network_type: NetworkType::Evm,
            config: NetworkConfigData::Evm(evm_config),
        }
    }

    /// Minimal "builder" for TransactionRepoModel.
    /// Allows quick creation of a test transaction with default fields,
    /// then updates them based on the provided status or overrides.
    pub fn make_test_transaction(status: TransactionStatus) -> TransactionRepoModel {
        let raw = matches!(
            status,
            TransactionStatus::Sent | TransactionStatus::Submitted
        )
        .then(|| vec![1, 2, 3]);
        TransactionRepoModel {
            version: 0,
            id: "test-tx-id".to_string(),
            relayer_id: "test-relayer-id".to_string(),
            status,
            status_reason: None,
            created_at: Utc::now().to_rfc3339(),
            sent_at: None,
            confirmed_at: None,
            valid_until: None,
            delete_at: None,
            network_type: NetworkType::Evm,
            network_data: NetworkTransactionData::Evm(EvmTransactionData {
                chain_id: 1,
                from: "0xSender".to_string(),
                to: Some("0xRecipient".to_string()),
                value: U256::from(0),
                data: Some("0xData".to_string()),
                gas_limit: Some(21000),
                gas_price: Some(20000000000),
                max_fee_per_gas: None,
                max_priority_fee_per_gas: None,
                nonce: None,
                signature: None,
                hash: None,
                speed: Some(Speed::Fast),
                raw,
            }),
            priced_at: None,
            hashes: Vec::new(),
            noop_count: None,
            is_canceled: Some(false),
            metadata: None,
        }
    }

    /// Minimal "builder" for EvmRelayerTransaction.
    /// Takes mock dependencies as arguments.
    pub fn make_test_evm_relayer_transaction(
        relayer: RelayerRepoModel,
        mocks: TestMocks,
    ) -> EvmRelayerTransaction<
        MockEvmProviderTrait,
        MockRelayerRepository,
        MockNetworkRepository,
        MockTransactionRepository,
        MockJobProducerTrait,
        MockSigner,
        MockTransactionCounterTrait,
        MockPriceCalculatorTrait,
    > {
        EvmRelayerTransaction::new(
            relayer,
            mocks.provider,
            Arc::new(mocks.relayer_repo),
            Arc::new(mocks.network_repo),
            Arc::new(mocks.tx_repo),
            Arc::new(mocks.counter),
            Arc::new(mocks.job_producer),
            mocks.price_calc,
            mocks.signer,
        )
        .unwrap()
    }

    fn create_test_relayer() -> RelayerRepoModel {
        RelayerRepoModel {
            id: "test-relayer-id".to_string(),
            name: "Test Relayer".to_string(),
            paused: false,
            system_disabled: false,
            network: "test_network".to_string(),
            network_type: NetworkType::Evm,
            policies: RelayerNetworkPolicy::Evm(RelayerEvmPolicy::default()),
            signer_id: "test_signer".to_string(),
            address: "0x".to_string(),
            notification_id: None,
            custom_rpc_urls: None,
            ..Default::default()
        }
    }

    pub(super) fn make_mock_receipt(status: bool, block_number: Option<u64>) -> TransactionReceipt {
        // Use some placeholder values for minimal completeness
        let tx_hash = TxHash::from(b256!(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        ));
        let block_hash = BlockHash::from(b256!(
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        ));
        let from_address = Address::from([0x11; 20]);

        TransactionReceipt {
            inner: alloy::rpc::types::TransactionReceipt {
                inner: AnyReceiptEnvelope {
                    inner: ReceiptWithBloom {
                        receipt: Receipt {
                            status: Eip658Value::Eip658(status), // determines success/fail
                            cumulative_gas_used: 0,
                            logs: vec![],
                        },
                        logs_bloom: Bloom::ZERO,
                    },
                    r#type: 0, // Legacy transaction type
                },
                transaction_hash: tx_hash,
                transaction_index: Some(0),
                block_hash: block_number.map(|_| block_hash), // only set if mined
                block_number,
                gas_used: 21000,
                effective_gas_price: 1000,
                blob_gas_used: None,
                blob_gas_price: None,
                from: from_address,
                to: None,
                contract_address: None,
            },
            other: Default::default(),
        }
    }

    /// The fixed `from` address used by [`make_mock_receipt`].
    fn mock_receipt_from() -> Address {
        Address::from([0x11; 20])
    }

    /// Like [`make_mock_receipt`] but sets the receipt's `to`, so cancellation
    /// attribution (a NOOP is a self-send: `to == from`) can be exercised.
    fn make_mock_receipt_with_to(
        status: bool,
        block_number: Option<u64>,
        to: Option<Address>,
    ) -> TransactionReceipt {
        let mut receipt = make_mock_receipt(status, block_number);
        receipt.inner.to = to;
        receipt
    }

    // Tests for `check_transaction_status`
    mod check_transaction_status_tests {
        use super::*;

        #[tokio::test]
        async fn test_not_mined() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let mut tx = make_test_transaction(TransactionStatus::Submitted);

            // Provide a hash so we can check for receipt
            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.hash = Some("0xFakeHash".to_string());
            }

            // Mock that get_transaction_receipt returns None (not mined)
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|_| Box::pin(async { Ok(None) }));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);

            let status = evm_transaction.check_transaction_status(&tx).await.unwrap();
            assert_eq!(status, TransactionStatus::Submitted);
        }

        /// A failed historical lookup must not fail the status check; the tx stays Submitted.
        #[tokio::test]
        async fn test_not_mined_historical_lookup_error_stays_submitted() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.hashes = vec![
                "0xHash1".to_string(),
                "0xHash2".to_string(),
                "0xHash3".to_string(),
            ];
            tx.sent_at = Some((Utc::now() - Duration::minutes(3)).to_rfc3339());
            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.hash = Some("0xHash3".to_string());
            }

            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|hash| {
                    if hash == "0xHash2" {
                        Box::pin(async { Err(crate::services::provider::ProviderError::Timeout) })
                    } else {
                        Box::pin(async { Ok(None) })
                    }
                });

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);

            let status = evm_transaction.check_transaction_status(&tx).await.unwrap();
            assert_eq!(status, TransactionStatus::Submitted);
        }

        /// A failed write of the recovered hash must fail the status check so it is retried,
        /// not look like a lookup error. Two hashes (one resubmission) are enough to recover.
        #[tokio::test]
        async fn test_not_mined_recovered_hash_write_error_propagates() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.hashes = vec!["0xHash1".to_string(), "0xHash2".to_string()];
            tx.sent_at = Some((Utc::now() - Duration::minutes(3)).to_rfc3339());
            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.hash = Some("0xHash1".to_string());
            }

            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|hash| {
                    if hash == "0xHash2" {
                        Box::pin(async { Ok(Some(make_mock_receipt(true, Some(100)))) })
                    } else {
                        Box::pin(async { Ok(None) })
                    }
                });
            mocks
                .tx_repo
                .expect_partial_update_if_version()
                .times(1)
                .returning(|_, _, _| {
                    Err(crate::models::RepositoryError::ConnectionError(
                        "redis down".to_string(),
                    ))
                });

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);

            let result = evm_transaction.check_transaction_status(&tx).await;
            assert!(result.is_err(), "Expected Err, got: {result:?}");
        }

        #[tokio::test]
        async fn test_mined_but_not_confirmed() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let mut tx = make_test_transaction(TransactionStatus::Submitted);

            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.hash = Some("0xFakeHash".to_string());
            }

            // Mock a mined receipt with block_number = 100
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|_| Box::pin(async { Ok(Some(make_mock_receipt(true, Some(100)))) }));

            // Mock block_number that hasn't reached the confirmation threshold
            mocks
                .provider
                .expect_get_block_number()
                .return_once(|| Box::pin(async { Ok(100) }));

            // Mock network repository to return a test network model
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);

            let status = evm_transaction.check_transaction_status(&tx).await.unwrap();
            assert_eq!(status, TransactionStatus::Mined);
        }

        #[tokio::test]
        async fn test_confirmed() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let mut tx = make_test_transaction(TransactionStatus::Submitted);

            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.hash = Some("0xFakeHash".to_string());
            }

            // Mock a mined receipt with block_number = 100
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|_| Box::pin(async { Ok(Some(make_mock_receipt(true, Some(100)))) }));

            // Mock block_number that meets the confirmation threshold
            mocks
                .provider
                .expect_get_block_number()
                .return_once(|| Box::pin(async { Ok(113) }));

            // Mock network repository to return a test network model
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);

            let status = evm_transaction.check_transaction_status(&tx).await.unwrap();
            assert_eq!(status, TransactionStatus::Confirmed);
        }

        /// When a user-cancelled tx confirms and the mined tx is the NOOP (a self-send,
        /// receipt.to == receipt.from), it must terminate as Canceled.
        #[tokio::test]
        async fn test_cancellation_noop_confirmed_becomes_canceled() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.is_canceled = Some(true);

            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.hash = Some("0xNoopHash".to_string());
            }

            // Mined receipt for a NOOP: to == from.
            let noop_to = Some(mock_receipt_from());
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(move |_| {
                    Box::pin(async move {
                        Ok(Some(make_mock_receipt_with_to(true, Some(100), noop_to)))
                    })
                });
            mocks
                .provider
                .expect_get_block_number()
                .return_once(|| Box::pin(async { Ok(113) }));
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);

            let status = evm_transaction.check_transaction_status(&tx).await.unwrap();
            assert_eq!(status, TransactionStatus::Canceled);
        }

        /// If a user-cancelled tx confirms but the ORIGINAL transaction is what mined
        /// (cancellation lost the nonce race: receipt.to != receipt.from), it actually
        /// executed and must be reported Confirmed, not Canceled.
        #[tokio::test]
        async fn test_cancellation_original_mined_stays_confirmed() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.is_canceled = Some(true);

            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.hash = Some("0xOriginalHash".to_string());
            }

            // Mined receipt for the ORIGINAL transfer: to is a different address than from.
            let original_to = Some(Address::from([0x22; 20]));
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(move |_| {
                    Box::pin(async move {
                        Ok(Some(make_mock_receipt_with_to(
                            true,
                            Some(100),
                            original_to,
                        )))
                    })
                });
            mocks
                .provider
                .expect_get_block_number()
                .return_once(|| Box::pin(async { Ok(113) }));
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);

            let status = evm_transaction.check_transaction_status(&tx).await.unwrap();
            assert_eq!(status, TransactionStatus::Confirmed);
        }

        /// A confirmed self-send that is NOT a user cancellation (is_canceled=false) — e.g.
        /// a nonce-clearing NOOP from timeout handling — must still confirm normally.
        #[tokio::test]
        async fn test_non_cancellation_noop_confirmed_stays_confirmed() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            // is_canceled stays Some(false) from the helper.

            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.hash = Some("0xNoopHash".to_string());
            }

            // Even with a self-send receipt (to == from), a non-cancelled tx stays Confirmed.
            let noop_to = Some(mock_receipt_from());
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(move |_| {
                    Box::pin(async move {
                        Ok(Some(make_mock_receipt_with_to(true, Some(100), noop_to)))
                    })
                });
            mocks
                .provider
                .expect_get_block_number()
                .return_once(|| Box::pin(async { Ok(113) }));
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);

            let status = evm_transaction.check_transaction_status(&tx).await.unwrap();
            assert_eq!(status, TransactionStatus::Confirmed);
        }

        #[tokio::test]
        async fn test_failed() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let mut tx = make_test_transaction(TransactionStatus::Submitted);

            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.hash = Some("0xFakeHash".to_string());
            }

            // Mock a mined receipt with failure
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|_| Box::pin(async { Ok(Some(make_mock_receipt(false, Some(100)))) }));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);

            let status = evm_transaction.check_transaction_status(&tx).await.unwrap();
            assert_eq!(status, TransactionStatus::Failed);
        }
    }

    // Tests for `should_resubmit`
    mod should_resubmit_tests {
        use super::*;
        use crate::models::TransactionError;

        #[tokio::test]
        async fn test_should_resubmit_true() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            // Set sent_at to 600 seconds ago to force resubmission
            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.sent_at = Some((Utc::now() - Duration::seconds(600)).to_rfc3339());

            // Mock network repository to return a regular network model
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let res = evm_transaction.should_resubmit(&tx).await.unwrap();
            assert!(res, "Transaction should be resubmitted after timeout.");
        }

        #[tokio::test]
        async fn test_should_resubmit_false() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            // Make a transaction with status Submitted but recently sent
            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.sent_at = Some(Utc::now().to_rfc3339());

            // Mock network repository to return a regular network model
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let res = evm_transaction.should_resubmit(&tx).await.unwrap();
            assert!(!res, "Transaction should not be resubmitted immediately.");
        }

        #[tokio::test]
        async fn test_should_resubmit_true_for_no_mempool_network() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            // Set up a transaction that would normally be resubmitted (sent_at long ago)
            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.sent_at = Some((Utc::now() - Duration::seconds(600)).to_rfc3339());

            // Set chain_id to match the no-mempool network
            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.chain_id = 42161; // Arbitrum chain ID
            }

            // Mock network repository to return a no-mempool network model
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_no_mempool_network_model())));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let res = evm_transaction.should_resubmit(&tx).await.unwrap();
            assert!(
                res,
                "Transaction should be resubmitted for no-mempool networks."
            );
        }

        #[tokio::test]
        async fn test_should_resubmit_network_not_found() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.sent_at = Some((Utc::now() - Duration::seconds(600)).to_rfc3339());

            // Mock network repository to return None (network not found)
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(None));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.should_resubmit(&tx).await;

            assert!(
                result.is_err(),
                "should_resubmit should return error when network not found"
            );
            let error = result.unwrap_err();
            match error {
                TransactionError::UnexpectedError(msg) => {
                    assert!(msg.contains("Network with chain id 1 not found"));
                }
                _ => panic!("Expected UnexpectedError for network not found"),
            }
        }

        #[tokio::test]
        async fn test_should_resubmit_network_conversion_error() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.sent_at = Some((Utc::now() - Duration::seconds(600)).to_rfc3339());

            // Create a network model with invalid EVM config (missing chain_id)
            let invalid_evm_config = EvmNetworkConfig {
                common: NetworkConfigCommon {
                    network: "invalid-network".to_string(),
                    from: None,
                    rpc_urls: Some(vec![crate::models::RpcConfig::new(
                        "https://rpc.example.com".to_string(),
                    )]),
                    explorer_urls: Some(vec!["https://explorer.example.com".to_string()]),
                    average_blocktime_ms: Some(12000),
                    is_testnet: Some(false),
                    tags: Some(vec!["testnet".to_string()]),
                },
                chain_id: None, // This will cause the conversion to fail
                required_confirmations: Some(12),
                status_check: None,
                features: Some(vec!["eip1559".to_string()]),
                symbol: Some("ETH".to_string()),
                gas_price_cache: None,
            };
            let invalid_network = NetworkRepoModel {
                id: "evm:invalid".to_string(),
                name: "invalid-network".to_string(),
                network_type: NetworkType::Evm,
                config: NetworkConfigData::Evm(invalid_evm_config),
            };

            // Mock network repository to return the invalid network model
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(move |_, _| Ok(Some(invalid_network.clone())));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.should_resubmit(&tx).await;

            assert!(
                result.is_err(),
                "should_resubmit should return error when network conversion fails"
            );
            let error = result.unwrap_err();
            match error {
                TransactionError::UnexpectedError(msg) => {
                    assert!(msg.contains("Error converting network model to EvmNetwork"));
                }
                _ => panic!("Expected UnexpectedError for network conversion failure"),
            }
        }
    }

    // Tests for `should_noop`
    mod should_noop_tests {
        use super::*;

        #[tokio::test]
        async fn test_expired_transaction_triggers_noop() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            // Force the transaction to be "expired" by setting valid_until in the past
            tx.valid_until = Some((Utc::now() - Duration::seconds(10)).to_rfc3339());

            // Mock network repository to return a test network model
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let (res, reason) = evm_transaction.should_noop(&tx).await.unwrap();
            assert!(res, "Expired transaction should be replaced with a NOOP.");
            assert!(
                reason.is_some(),
                "Reason should be provided for expired transaction"
            );
            assert!(
                reason.unwrap().contains("expired"),
                "Reason should mention expiration"
            );
        }

        #[tokio::test]
        async fn test_too_many_noop_attempts_returns_false() {
            let mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.noop_count = Some(51); // Max is 50, so this should return false

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let (res, reason) = evm_transaction.should_noop(&tx).await.unwrap();
            assert!(
                !res,
                "Transaction with too many NOOP attempts should not be replaced."
            );
            assert!(
                reason.is_none(),
                "Reason should not be provided when should_noop is false"
            );
        }

        #[tokio::test]
        async fn test_already_noop_returns_false() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            // Make it a NOOP by setting to=None and value=0
            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.to = None;
                evm_data.value = U256::from(0);
            }

            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            // Mock get_block_by_number for gas limit validation (won't be called since is_noop returns early, but needed for compilation)
            mocks.provider.expect_get_block_by_number().returning(|| {
                Box::pin(async {
                    use alloy::{network::AnyRpcBlock, rpc::types::Block};
                    let mut block: Block = Block::default();
                    block.header.gas_limit = 30_000_000u64;
                    Ok(AnyRpcBlock::from(block))
                })
            });

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let (res, reason) = evm_transaction.should_noop(&tx).await.unwrap();
            assert!(
                !res,
                "Transaction that is already a NOOP should not be replaced."
            );
            assert!(
                reason.is_none(),
                "Reason should not be provided when should_noop is false"
            );
        }

        #[tokio::test]
        async fn test_rollup_with_too_many_attempts_triggers_noop() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            // Set chain_id to Arbitrum (rollup network)
            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.chain_id = 42161; // Arbitrum
            }
            tx.metadata = Some(crate::models::TransactionMetadata {
                accepted_sends: 51,
                ..Default::default()
            });

            // Mock network repository to return Arbitrum network
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_no_mempool_network_model())));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let (res, reason) = evm_transaction.should_noop(&tx).await.unwrap();
            assert!(
                res,
                "Rollup transaction with too many attempts should be replaced with NOOP."
            );
            assert!(
                reason.is_some(),
                "Reason should be provided for rollup transaction"
            );
            assert!(
                reason.unwrap().contains("too many attempts"),
                "Reason should mention too many attempts"
            );
        }

        #[tokio::test]
        async fn test_pending_state_timeout_triggers_noop() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Pending);
            // Set created_at to 3 minutes ago (> 2 minute timeout)
            tx.created_at = (Utc::now() - Duration::minutes(3)).to_rfc3339();

            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let (res, reason) = evm_transaction.should_noop(&tx).await.unwrap();
            assert!(
                res,
                "Pending transaction stuck for >2 minutes should be replaced with NOOP."
            );
            assert!(
                reason.is_some(),
                "Reason should be provided for pending timeout"
            );
            assert!(
                reason.unwrap().contains("Pending state"),
                "Reason should mention Pending state"
            );
        }

        #[tokio::test]
        async fn test_valid_transaction_returns_false() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let tx = make_test_transaction(TransactionStatus::Submitted);
            // Transaction is recent, not expired, not on rollup, no issues

            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            // Mock get_block_by_number for gas limit validation
            mocks.provider.expect_get_block_by_number().returning(|| {
                Box::pin(async {
                    use alloy::{network::AnyRpcBlock, rpc::types::Block};
                    let mut block: Block = Block::default();
                    block.header.gas_limit = 30_000_000u64;
                    Ok(AnyRpcBlock::from(block))
                })
            });

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let (res, reason) = evm_transaction.should_noop(&tx).await.unwrap();
            assert!(!res, "Valid transaction should not be replaced with NOOP.");
            assert!(
                reason.is_none(),
                "Reason should not be provided when should_noop is false"
            );
        }
    }

    // Tests for `update_transaction_status_if_needed`
    mod update_transaction_status_tests {
        use super::*;

        #[tokio::test]
        async fn test_no_update_when_status_is_same() {
            // Create mocks, relayer, and a transaction with status Submitted.
            let mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let tx = make_test_transaction(TransactionStatus::Submitted);
            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);

            // When new status is the same as current, update_transaction_status_if_needed
            // should simply return the original transaction.
            let updated_tx = evm_transaction
                .update_transaction_status_if_needed(tx.clone(), TransactionStatus::Submitted, None)
                .await
                .unwrap();
            assert_eq!(updated_tx.status, TransactionStatus::Submitted);
            assert_eq!(updated_tx.id, tx.id);
        }

        #[tokio::test]
        async fn test_updates_when_status_differs() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let tx = make_test_transaction(TransactionStatus::Submitted);

            // Mock partial_update to return a transaction with new status
            mocks
                .tx_repo
                .expect_partial_update_if_version()
                .returning(|_, _, update| {
                    let mut updated_tx = make_test_transaction(TransactionStatus::Submitted);
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    Ok(updated_tx)
                });

            // Mock notification job
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let updated_tx = evm_transaction
                .update_transaction_status_if_needed(tx.clone(), TransactionStatus::Mined, None)
                .await
                .unwrap();

            assert_eq!(updated_tx.status, TransactionStatus::Mined);
        }

        #[tokio::test]
        async fn test_updates_with_status_reason() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let tx = make_test_transaction(TransactionStatus::Submitted);

            mocks
                .tx_repo
                .expect_partial_update_if_version()
                .withf(|_, _, update| {
                    update.status == Some(TransactionStatus::Failed)
                        && update.status_reason == Some("Transaction reverted on-chain".to_string())
                })
                .returning(|_, _, update| {
                    let mut updated_tx = make_test_transaction(TransactionStatus::Submitted);
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    updated_tx.status_reason = update.status_reason.clone();
                    Ok(updated_tx)
                });

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let updated_tx = evm_transaction
                .update_transaction_status_if_needed(
                    tx.clone(),
                    TransactionStatus::Failed,
                    Some("Transaction reverted on-chain".to_string()),
                )
                .await
                .unwrap();

            assert_eq!(updated_tx.status, TransactionStatus::Failed);
            assert_eq!(
                updated_tx.status_reason.as_deref(),
                Some("Transaction reverted on-chain")
            );
        }
    }

    // Tests for `handle_sent_state`
    mod handle_sent_state_tests {
        use super::*;

        /// A Sent tx over the rollup attempt threshold is NOOP-replaced AND the
        /// replacement goes out as a RESUBMIT job: the stored raw/signature still
        /// hold the original payload, so only the re-signing resubmit path
        /// actually broadcasts the NOOP.
        #[tokio::test]
        async fn sent_state_noop_enqueues_resubmit_job() {
            use crate::jobs::TransactionCommand;

            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Sent);
            tx.hashes = vec!["0xHash1".to_string(); crate::constants::MAXIMUM_TX_ATTEMPTS + 1];
            tx.sent_at = Some((Utc::now() - Duration::seconds(60)).to_rfc3339());

            // should_noop + prepare_noop_update_request both look the network up.
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_arbitrum_network_model())));

            // make_noop estimates gas on Arbitrum.
            mocks
                .provider
                .expect_estimate_gas()
                .returning(|_| Box::pin(async { Ok(50_000u64) }));

            // The persisted update must carry the NOOP'd network_data.
            let tx_clone = tx.clone();
            mocks
                .tx_repo
                .expect_partial_update_if_version()
                .times(1)
                .withf(|_, _, update| {
                    update.network_data.as_ref().is_some_and(|data| {
                        data.get_evm_transaction_data().is_ok_and(|d| {
                            d.value == U256::ZERO && d.to.as_deref() == Some(&d.from)
                        })
                    })
                })
                .returning(move |_, _, update| {
                    let mut updated_tx = tx_clone.clone();
                    updated_tx.network_data = update.network_data.clone().unwrap();
                    updated_tx.noop_count = update.noop_count;
                    Ok(updated_tx)
                });

            // The job must be a RESUBMIT (re-price + re-sign), not a plain submit.
            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .times(1)
                .withf(|job, _| matches!(job.command, TransactionCommand::Resubmit))
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.handle_sent_state(tx).await.unwrap();
            assert!(result.noop_count.unwrap_or(0) > 0, "NOOP must be recorded");
        }

        /// Arbitrum: a Sent-state NOOP above the chain head is persisted but gets no
        /// resubmit job (the sequencer would reject it); at the head it is resubmitted.
        /// Arbitrum is resolved by the relayer's network name, not by chain id.
        #[tokio::test]
        async fn arbitrum_sent_state_noop_resubmits_only_at_head() {
            for (on_chain_nonce, expect_job) in [(5u64, false), (7u64, true)] {
                let mut mocks = default_test_mocks();
                let relayer = create_test_relayer();

                let mut tx = make_test_transaction(TransactionStatus::Sent);
                tx.hashes = vec!["0xHash1".to_string(); crate::constants::MAXIMUM_TX_ATTEMPTS + 1];
                tx.sent_at = Some((Utc::now() - Duration::seconds(60)).to_rfc3339());
                tx.network_data = NetworkTransactionData::Evm(EvmTransactionData {
                    nonce: Some(7),
                    ..tx.network_data.get_evm_transaction_data().unwrap()
                });

                // should_noop + prepare_noop_update_request look the network up by
                // chain id; the head check resolves it by the relayer's name.
                mocks
                    .network_repo
                    .expect_get_by_chain_id()
                    .returning(|_, _| Ok(Some(create_test_arbitrum_network_model())));
                mocks
                    .network_repo
                    .expect_get_by_name()
                    .with(
                        mockall::predicate::eq(NetworkType::Evm),
                        mockall::predicate::eq(relayer.network.clone()),
                    )
                    .returning(|_, _| Ok(Some(create_test_arbitrum_network_model())));
                mocks
                    .provider
                    .expect_estimate_gas()
                    .returning(|_| Box::pin(async { Ok(50_000u64) }));
                mocks
                    .provider
                    .expect_get_transaction_count()
                    .times(1)
                    .returning(move |_| Box::pin(async move { Ok(on_chain_nonce) }));

                let tx_clone = tx.clone();
                mocks
                    .tx_repo
                    .expect_partial_update_if_version()
                    .times(1)
                    .returning(move |_, _, update| {
                        let mut updated_tx = tx_clone.clone();
                        updated_tx.network_data = update.network_data.clone().unwrap();
                        updated_tx.noop_count = update.noop_count;
                        Ok(updated_tx)
                    });

                mocks
                    .job_producer
                    .expect_produce_submit_transaction_job()
                    .times(usize::from(expect_job))
                    .returning(|_, _| Box::pin(async { Ok(()) }));

                let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
                let result = evm_transaction.handle_sent_state(tx).await.unwrap();
                assert!(result.noop_count.unwrap_or(0) > 0, "NOOP must be persisted");
            }
        }

        #[tokio::test]
        async fn test_sent_state_recent_no_resend() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Sent);
            // Set sent_at to recent (e.g., 10 seconds ago)
            tx.sent_at = Some((Utc::now() - Duration::seconds(10)).to_rfc3339());

            // Mock network repository to return a test network model for should_noop check
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            // Mock get_block_by_number for gas limit validation in handle_sent_state
            mocks.provider.expect_get_block_by_number().returning(|| {
                Box::pin(async {
                    use alloy::{network::AnyRpcBlock, rpc::types::Block};
                    let mut block: Block = Block::default();
                    block.header.gas_limit = 30_000_000u64;
                    Ok(AnyRpcBlock::from(block))
                })
            });

            // Mock status check job scheduling
            mocks
                .job_producer
                .expect_produce_check_transaction_status_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.handle_sent_state(tx.clone()).await.unwrap();

            assert_eq!(result.status, TransactionStatus::Sent);
        }

        #[tokio::test]
        async fn test_sent_state_stuck_schedules_resubmit() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Sent);
            // Set sent_at to long ago (> 30 seconds for resend timeout)
            tx.sent_at = Some((Utc::now() - Duration::seconds(60)).to_rfc3339());

            // Mock network repository to return a test network model for should_noop check
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            // Mock get_block_by_number for gas limit validation in handle_sent_state
            mocks.provider.expect_get_block_by_number().returning(|| {
                Box::pin(async {
                    use alloy::{network::AnyRpcBlock, rpc::types::Block};
                    let mut block: Block = Block::default();
                    block.header.gas_limit = 30_000_000u64;
                    Ok(AnyRpcBlock::from(block))
                })
            });

            // Mock resubmit job scheduling
            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            // Mock status check job scheduling
            mocks
                .job_producer
                .expect_produce_check_transaction_status_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.handle_sent_state(tx.clone()).await.unwrap();

            assert_eq!(result.status, TransactionStatus::Sent);
        }

        /// An aged `Sent` tx with a confirmed nonce gap below it skips the resubmit
        /// and schedules a nonce-health job instead (tx stays `Sent`).
        #[tokio::test]
        async fn issue818_t5_sent_state_nonce_gap_ahead_skips_resubmit() {
            use std::sync::atomic::{AtomicUsize, Ordering};
            use std::sync::Arc as StdArc;

            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            // Aged Sent tx (60s > 25s resend timeout) with nonce 300. Unique relayer id
            // so the nonce-health debounce doesn't interact with other tests.
            let mut tx = make_test_transaction(TransactionStatus::Sent);
            tx.relayer_id = "issue818-t5-relayer".to_string();
            tx.sent_at = Some((Utc::now() - Duration::seconds(60)).to_rfc3339());
            tx.network_data = NetworkTransactionData::Evm(EvmTransactionData {
                nonce: Some(300),
                ..tx.network_data.get_evm_transaction_data().unwrap()
            });

            // should_noop: network lookup + block gas limit (does not noop).
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            mocks.provider.expect_get_block_by_number().returning(|| {
                Box::pin(async {
                    use alloy::{network::AnyRpcBlock, rpc::types::Block};
                    let mut block: Block = Block::default();
                    block.header.gas_limit = 30_000_000u64;
                    Ok(AnyRpcBlock::from(block))
                })
            });

            // Chain nonce is far behind tx nonce (100 vs 300).
            mocks
                .provider
                .expect_get_transaction_count()
                .returning(|_| Box::pin(async { Ok(100) }));

            // Scan is capped to on_chain_nonce + MAX_GAP_SCAN_RANGE (100 + 100 = 200).
            // All slots empty -> confirmed gap.
            mocks
                .tx_repo
                .expect_get_nonce_occupancy()
                .withf(|relayer_id, from, to| {
                    relayer_id == "issue818-t5-relayer" && *from == 100 && *to == 200
                })
                .returning(|_, from, to| Ok((from..to).map(|n| (n, None)).collect()));

            // Nonce health job should be produced (unique relayer id, so not debounced).
            let health_calls = StdArc::new(AtomicUsize::new(0));
            let health_calls_clone = health_calls.clone();
            mocks
                .job_producer
                .expect_produce_relayer_health_check_job()
                .withf(|job, _| {
                    job.metadata.as_ref().map_or(false, |m| {
                        m.get("health_check_action") == Some(&"nonce_health".to_string())
                    })
                })
                .returning(move |_, _| {
                    health_calls_clone.fetch_add(1, Ordering::SeqCst);
                    Box::pin(async { Ok(()) })
                });

            // Should NOT call produce_submit_transaction_job (resubmit skipped) —
            // mockall will panic if an unexpected call is made.

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.handle_sent_state(tx.clone()).await.unwrap();

            assert_eq!(result.status, TransactionStatus::Sent);
            assert_eq!(
                health_calls.load(Ordering::SeqCst),
                1,
                "nonce health job should be produced when the Sent-state gap check confirms a gap"
            );
        }

        /// When `detect_nonce_gap_ahead` can't determine an answer (RPC error),
        /// the Sent-state resubmit fails open and proceeds.
        #[tokio::test]
        async fn issue818_sent_state_gap_check_rpc_failure_proceeds_to_resubmit() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Sent);
            tx.relayer_id = "issue818-rpc-failure-relayer".to_string();
            tx.sent_at = Some((Utc::now() - Duration::seconds(60)).to_rfc3339());
            tx.network_data = NetworkTransactionData::Evm(EvmTransactionData {
                nonce: Some(300),
                ..tx.network_data.get_evm_transaction_data().unwrap()
            });

            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            mocks.provider.expect_get_block_by_number().returning(|| {
                Box::pin(async {
                    use alloy::{network::AnyRpcBlock, rpc::types::Block};
                    let mut block: Block = Block::default();
                    block.header.gas_limit = 30_000_000u64;
                    Ok(AnyRpcBlock::from(block))
                })
            });

            // RPC fails for the on-chain nonce lookup — gap check is skipped (None).
            mocks
                .provider
                .expect_get_transaction_count()
                .times(1)
                .returning(|_| {
                    Box::pin(async {
                        Err(crate::services::provider::ProviderError::Other(
                            "rpc timeout".to_string(),
                        ))
                    })
                });

            // Resubmission should still proceed (fail-open).
            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            mocks
                .job_producer
                .expect_produce_check_transaction_status_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.handle_sent_state(tx.clone()).await.unwrap();

            assert_eq!(result.status, TransactionStatus::Sent);
        }

        /// With no nonce gap below the tx, the Sent-state resubmit proceeds normally.
        #[tokio::test]
        async fn issue818_sent_state_no_gap_proceeds_to_resubmit() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Sent);
            tx.relayer_id = "issue818-no-gap-relayer".to_string();
            tx.sent_at = Some((Utc::now() - Duration::seconds(60)).to_rfc3339());
            tx.network_data = NetworkTransactionData::Evm(EvmTransactionData {
                nonce: Some(270),
                ..tx.network_data.get_evm_transaction_data().unwrap()
            });

            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));
            mocks
                .network_repo
                .expect_get_by_name()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            mocks.provider.expect_get_block_by_number().returning(|| {
                Box::pin(async {
                    use alloy::{network::AnyRpcBlock, rpc::types::Block};
                    let mut block: Block = Block::default();
                    block.header.gas_limit = 30_000_000u64;
                    Ok(AnyRpcBlock::from(block))
                })
            });

            // tx_nonce=270, on_chain=269 -> single slot to check.
            mocks
                .provider
                .expect_get_transaction_count()
                .times(1)
                .returning(|_| Box::pin(async { Ok(269) }));

            // Nonce 269 has an active Submitted tx -> no gap.
            mocks
                .tx_repo
                .expect_get_nonce_occupancy()
                .returning(|_, from, to| {
                    Ok((from..to)
                        .map(|n| (n, Some(TransactionStatus::Submitted)))
                        .collect())
                });

            // Should proceed to resubmit (no health job expected).
            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            mocks
                .job_producer
                .expect_produce_check_transaction_status_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.handle_sent_state(tx.clone()).await.unwrap();

            assert_eq!(result.status, TransactionStatus::Sent);
        }

        /// Arbitrum head gate: a stale Sent tx whose nonce is above the on-chain
        /// nonce gets NO resubmit job — broadcasting it is a guaranteed rejection.
        #[tokio::test]
        async fn arbitrum_sent_state_behind_head_skips_resubmit() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Sent);
            tx.relayer_id = "arb-head-gate-behind-relayer".to_string();
            tx.sent_at = Some((Utc::now() - Duration::seconds(60)).to_rfc3339());
            tx.network_data = NetworkTransactionData::Evm(EvmTransactionData {
                nonce: Some(7),
                ..tx.network_data.get_evm_transaction_data().unwrap()
            });

            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_arbitrum_network_model())));
            mocks
                .network_repo
                .expect_get_by_name()
                .returning(|_, _| Ok(Some(create_test_arbitrum_network_model())));

            mocks.provider.expect_get_block_by_number().returning(|| {
                Box::pin(async {
                    use alloy::{network::AnyRpcBlock, rpc::types::Block};
                    let mut block: Block = Block::default();
                    block.header.gas_limit = 30_000_000u64;
                    Ok(AnyRpcBlock::from(block))
                })
            });

            // On-chain nonce is 5: the tx at nonce 7 is queued behind others.
            mocks
                .provider
                .expect_get_transaction_count()
                .times(1)
                .returning(|_| Box::pin(async { Ok(5) }));

            // Slots 5 and 6 are actively occupied — no gap, so only the head gate
            // stands between this tx and a resubmit job.
            mocks
                .tx_repo
                .expect_get_nonce_occupancy()
                .returning(|_, from, to| {
                    Ok((from..to)
                        .map(|n| (n, Some(TransactionStatus::Sent)))
                        .collect())
                });

            // No resubmit job may be queued for a tx behind the head.
            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .never();

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.handle_sent_state(tx.clone()).await.unwrap();

            assert_eq!(result.status, TransactionStatus::Sent);
        }

        /// Arbitrum head gate: the stale Sent tx AT the on-chain nonce (the head)
        /// does get its resubmit job — it is the only deliverable transaction.
        #[tokio::test]
        async fn arbitrum_sent_state_at_head_proceeds_to_resubmit() {
            use std::sync::atomic::{AtomicUsize, Ordering};
            use std::sync::Arc as StdArc;

            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Sent);
            tx.relayer_id = "arb-head-gate-at-head-relayer".to_string();
            tx.sent_at = Some((Utc::now() - Duration::seconds(60)).to_rfc3339());
            tx.network_data = NetworkTransactionData::Evm(EvmTransactionData {
                nonce: Some(5),
                ..tx.network_data.get_evm_transaction_data().unwrap()
            });

            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_arbitrum_network_model())));
            mocks
                .network_repo
                .expect_get_by_name()
                .returning(|_, _| Ok(Some(create_test_arbitrum_network_model())));

            mocks.provider.expect_get_block_by_number().returning(|| {
                Box::pin(async {
                    use alloy::{network::AnyRpcBlock, rpc::types::Block};
                    let mut block: Block = Block::default();
                    block.header.gas_limit = 30_000_000u64;
                    Ok(AnyRpcBlock::from(block))
                })
            });

            // On-chain nonce equals the tx nonce — this tx is the head.
            mocks
                .provider
                .expect_get_transaction_count()
                .times(1)
                .returning(|_| Box::pin(async { Ok(5) }));

            let submit_calls = StdArc::new(AtomicUsize::new(0));
            let submit_calls_clone = submit_calls.clone();
            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .returning(move |_, _| {
                    submit_calls_clone.fetch_add(1, Ordering::SeqCst);
                    Box::pin(async { Ok(()) })
                });

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.handle_sent_state(tx.clone()).await.unwrap();

            assert_eq!(result.status, TransactionStatus::Sent);
            assert_eq!(
                submit_calls.load(Ordering::SeqCst),
                1,
                "the head transaction must get its resubmit job"
            );
        }

        /// A failed enqueue must not hold the debounce window: after `clear`, the
        /// next caller fires immediately instead of waiting out the TTL.
        #[test]
        fn ttl_debounce_clear_releases_window() {
            use crate::domain::transaction::evm::status::TtlDebounce;

            let debounce = TtlDebounce::new(std::time::Duration::from_secs(3600));
            assert!(debounce.should_fire("relayer-1"));
            assert!(!debounce.should_fire("relayer-1"));

            debounce.clear("relayer-1");
            assert!(debounce.should_fire("relayer-1"));

            // Clearing one key must not affect another relayer's window.
            assert!(debounce.should_fire("relayer-2"));
            debounce.clear("relayer-1");
            assert!(!debounce.should_fire("relayer-2"));
        }

        /// A second gap detection for the same relayer within the debounce TTL skips
        /// resubmission both times but enqueues only one nonce-health job.
        #[tokio::test]
        async fn issue818_nonce_health_debounce_produces_once_within_ttl() {
            use std::sync::atomic::{AtomicUsize, Ordering};
            use std::sync::Arc as StdArc;

            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Sent);
            tx.relayer_id = "issue818-debounce-relayer".to_string();
            tx.sent_at = Some((Utc::now() - Duration::seconds(60)).to_rfc3339());
            tx.network_data = NetworkTransactionData::Evm(EvmTransactionData {
                nonce: Some(300),
                ..tx.network_data.get_evm_transaction_data().unwrap()
            });

            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            mocks.provider.expect_get_block_by_number().returning(|| {
                Box::pin(async {
                    use alloy::{network::AnyRpcBlock, rpc::types::Block};
                    let mut block: Block = Block::default();
                    block.header.gas_limit = 30_000_000u64;
                    Ok(AnyRpcBlock::from(block))
                })
            });

            mocks
                .provider
                .expect_get_transaction_count()
                .returning(|_| Box::pin(async { Ok(100) }));

            mocks
                .tx_repo
                .expect_get_nonce_occupancy()
                .returning(|_, from, to| Ok((from..to).map(|n| (n, None)).collect()));

            let health_calls = StdArc::new(AtomicUsize::new(0));
            let health_calls_clone = health_calls.clone();
            mocks
                .job_producer
                .expect_produce_relayer_health_check_job()
                .returning(move |_, _| {
                    health_calls_clone.fetch_add(1, Ordering::SeqCst);
                    Box::pin(async { Ok(()) })
                });

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);

            // First detection for this relayer: produces the health job.
            let result1 = evm_transaction.handle_sent_state(tx.clone()).await.unwrap();
            assert_eq!(result1.status, TransactionStatus::Sent);

            // Second detection immediately after, same relayer: debounced, no
            // second job produced. Resubmit is still skipped in both cases —
            // debounce only affects job *production*, not gap detection.
            let result2 = evm_transaction.handle_sent_state(tx.clone()).await.unwrap();
            assert_eq!(result2.status, TransactionStatus::Sent);

            assert_eq!(
                health_calls.load(Ordering::SeqCst),
                1,
                "nonce health job should be produced only once within the debounce TTL"
            );
        }
    }

    // Tests for `prepare_noop_update_request`
    mod prepare_noop_update_request_tests {
        use super::*;

        #[tokio::test]
        async fn test_noop_request_without_cancellation() {
            // Create a transaction with an initial noop_count of 2 and is_canceled set to false.
            let mocks = default_test_mocks_with_network();
            let relayer = create_test_relayer();
            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.noop_count = Some(2);
            tx.is_canceled = Some(false);

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let update_req = evm_transaction
                .prepare_noop_update_request(&tx, false, None)
                .await
                .unwrap();

            // NOOP count should be incremented: 2 becomes 3.
            assert_eq!(update_req.noop_count, Some(3));
            // When not cancelling, the is_canceled flag should remain as in the original transaction.
            assert_eq!(update_req.is_canceled, Some(false));
        }

        #[tokio::test]
        async fn test_noop_request_with_cancellation() {
            // Create a transaction with no initial noop_count (None) and is_canceled false.
            let mocks = default_test_mocks_with_network();
            let relayer = create_test_relayer();
            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.noop_count = None;
            tx.is_canceled = Some(false);

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let update_req = evm_transaction
                .prepare_noop_update_request(&tx, true, None)
                .await
                .unwrap();

            // NOOP count should default to 1.
            assert_eq!(update_req.noop_count, Some(1));
            // When cancelling, the is_canceled flag should be forced to true.
            assert_eq!(update_req.is_canceled, Some(true));
        }
    }

    // Tests for `handle_submitted_state`
    mod handle_submitted_state_tests {
        use super::*;

        #[tokio::test]
        async fn test_schedules_resubmit_job() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            // Set sent_at far in the past to force resubmission
            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.sent_at = Some((Utc::now() - Duration::seconds(600)).to_rfc3339());

            // Mock network repository to return a test network model for should_noop check
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            // Mock get_block_by_number for gas limit validation
            mocks.provider.expect_get_block_by_number().returning(|| {
                Box::pin(async {
                    use alloy::{network::AnyRpcBlock, rpc::types::Block};
                    let mut block: Block = Block::default();
                    block.header.gas_limit = 30_000_000u64;
                    Ok(AnyRpcBlock::from(block))
                })
            });

            // On-chain nonce <= tx nonce (no gap), so resubmission proceeds normally
            mocks
                .provider
                .expect_get_transaction_count()
                .returning(|_| Box::pin(async { Ok(10) }));

            // Expect the resubmit job to be produced
            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            // Expect status check to be scheduled
            mocks
                .job_producer
                .expect_produce_check_transaction_status_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let updated_tx = evm_transaction.handle_submitted_state(tx).await.unwrap();

            // We remain in "Submitted" after scheduling the resubmit
            assert_eq!(updated_tx.status, TransactionStatus::Submitted);
        }

        /// When tx_nonce > on_chain_nonce and nonce slots are empty, the tx is
        /// blocked by a gap. Should schedule nonce health job and skip resubmission.
        #[tokio::test]
        async fn test_nonce_gap_detected_schedules_health_skips_resubmit() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.sent_at = Some((Utc::now() - Duration::seconds(600)).to_rfc3339());
            tx.network_data = NetworkTransactionData::Evm(EvmTransactionData {
                nonce: Some(274),
                hash: Some("0xhash".to_string()),
                raw: Some(vec![1, 2, 3]),
                ..tx.network_data.get_evm_transaction_data().unwrap()
            });

            // Mock network repository for should_resubmit
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            // On-chain nonce is 269, tx nonce is 274
            mocks
                .provider
                .expect_get_transaction_count()
                .returning(|_| Box::pin(async { Ok(269) }));

            // Batch nonce scan: nonces 269-273 are all empty → confirmed gap
            mocks
                .tx_repo
                .expect_get_nonce_occupancy()
                .withf(|relayer_id, from, to| {
                    relayer_id == "test-relayer-id" && *from == 269 && *to == 274
                })
                .returning(|_, from, to| Ok((from..to).map(|n| (n, None)).collect()));

            // Should schedule nonce health job
            mocks
                .job_producer
                .expect_produce_relayer_health_check_job()
                .withf(|job, _| {
                    job.metadata.as_ref().map_or(false, |m| {
                        m.get("health_check_action") == Some(&"nonce_health".to_string())
                    })
                })
                .returning(|_, _| Box::pin(async { Ok(()) }));

            // Should NOT call produce_submit_transaction_job (resubmit skipped)
            // mockall will panic if unexpected calls are made

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let updated_tx = evm_transaction.handle_submitted_state(tx).await.unwrap();

            assert_eq!(updated_tx.status, TransactionStatus::Submitted);
        }

        /// When get_transaction_count fails, the gap check should be skipped
        /// and resubmission should proceed normally.
        #[tokio::test]
        async fn test_nonce_gap_check_rpc_failure_proceeds_to_resubmit() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.sent_at = Some((Utc::now() - Duration::seconds(600)).to_rfc3339());

            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            mocks.provider.expect_get_block_by_number().returning(|| {
                Box::pin(async {
                    use alloy::{network::AnyRpcBlock, rpc::types::Block};
                    let mut block: Block = Block::default();
                    block.header.gas_limit = 30_000_000u64;
                    Ok(AnyRpcBlock::from(block))
                })
            });

            // RPC fails for nonce check — should be gracefully skipped
            mocks
                .provider
                .expect_get_transaction_count()
                .returning(|_| {
                    Box::pin(async {
                        Err(crate::services::provider::ProviderError::Other(
                            "rpc timeout".to_string(),
                        ))
                    })
                });

            // Resubmission should still proceed
            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            mocks
                .job_producer
                .expect_produce_check_transaction_status_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let updated_tx = evm_transaction.handle_submitted_state(tx).await.unwrap();

            assert_eq!(updated_tx.status, TransactionStatus::Submitted);
        }

        /// When all nonce slots between on-chain and tx_nonce are filled by active
        /// transactions, no gap exists — resubmission proceeds normally.
        #[tokio::test]
        async fn test_no_gap_when_slots_filled_proceeds_to_resubmit() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.sent_at = Some((Utc::now() - Duration::seconds(600)).to_rfc3339());
            tx.network_data = NetworkTransactionData::Evm(EvmTransactionData {
                nonce: Some(270),
                hash: Some("0xhash".to_string()),
                raw: Some(vec![1, 2, 3]),
                ..tx.network_data.get_evm_transaction_data().unwrap()
            });

            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            mocks.provider.expect_get_block_by_number().returning(|| {
                Box::pin(async {
                    use alloy::{network::AnyRpcBlock, rpc::types::Block};
                    let mut block: Block = Block::default();
                    block.header.gas_limit = 30_000_000u64;
                    Ok(AnyRpcBlock::from(block))
                })
            });

            // tx_nonce=270, on_chain=269 → 1 slot to check
            mocks
                .provider
                .expect_get_transaction_count()
                .returning(|_| Box::pin(async { Ok(269) }));

            // Nonce 269 has an active Submitted tx → no gap
            mocks
                .tx_repo
                .expect_get_nonce_occupancy()
                .returning(|_, from, to| {
                    Ok((from..to)
                        .map(|n| (n, Some(TransactionStatus::Submitted)))
                        .collect())
                });

            // Should proceed to resubmit (no health job expected)
            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            mocks
                .job_producer
                .expect_produce_check_transaction_status_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let updated_tx = evm_transaction.handle_submitted_state(tx).await.unwrap();

            assert_eq!(updated_tx.status, TransactionStatus::Submitted);
        }
    }

    // Tests for `handle_pending_state`
    mod handle_pending_state_tests {
        use super::*;

        #[tokio::test]
        async fn test_pending_state_no_noop() {
            // Create a pending transaction that is fresh (created now).
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let mut tx = make_test_transaction(TransactionStatus::Pending);
            tx.created_at = Utc::now().to_rfc3339(); // less than one minute old

            // Mock network repository to return a test network model
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            // Mock get_block_by_number for gas limit validation
            mocks.provider.expect_get_block_by_number().returning(|| {
                Box::pin(async {
                    use alloy::{network::AnyRpcBlock, rpc::types::Block};
                    let mut block: Block = Block::default();
                    block.header.gas_limit = 30_000_000u64;
                    Ok(AnyRpcBlock::from(block))
                })
            });

            // Expect status check to be scheduled when not doing NOOP
            mocks
                .job_producer
                .expect_produce_check_transaction_status_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction
                .handle_pending_state(tx.clone())
                .await
                .unwrap();

            // When should_noop returns false the original transaction is returned unchanged.
            assert_eq!(result.id, tx.id);
            assert_eq!(result.status, tx.status);
            assert_eq!(result.noop_count, tx.noop_count);
        }

        #[tokio::test]
        async fn test_pending_state_with_noop() {
            // Create a pending transaction that is old (created 2 minutes ago)
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let mut tx = make_test_transaction(TransactionStatus::Pending);
            tx.created_at = (Utc::now() - Duration::minutes(2)).to_rfc3339();

            // Mock network repository to return a test network model
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            // Mock get_block_by_number for gas limit validation
            mocks.provider.expect_get_block_by_number().returning(|| {
                Box::pin(async {
                    use alloy::{network::AnyRpcBlock, rpc::types::Block};
                    let mut block: Block = Block::default();
                    block.header.gas_limit = 30_000_000u64;
                    Ok(AnyRpcBlock::from(block))
                })
            });

            // Expect partial_update to be called and simulate a Failed update
            // (Pending state transactions are marked as Failed, not NOOP, since nonces aren't assigned)
            let tx_clone = tx.clone();
            mocks
                .tx_repo
                .expect_partial_update_if_version()
                .withf(move |id, _, update| {
                    id == "test-tx-id"
                        && update.status == Some(TransactionStatus::Failed)
                        && update.status_reason.is_some()
                })
                .returning(move |_, _, update| {
                    let mut updated_tx = tx_clone.clone();
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    updated_tx.status_reason = update.status_reason.clone();
                    Ok(updated_tx)
                });
            // Expect that a notification is produced (no submit job needed for Failed status)
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction
                .handle_pending_state(tx.clone())
                .await
                .unwrap();

            // Since should_noop returns true for pending timeout, transaction should be marked as Failed
            assert_eq!(result.status, TransactionStatus::Failed);
            assert!(result.status_reason.is_some());
            assert!(result.status_reason.unwrap().contains("Pending state"));
        }
    }

    // Tests for `handle_mined_state`
    mod handle_mined_state_tests {
        use super::*;

        #[tokio::test]
        async fn test_updates_status_and_schedules_check() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            // Create a transaction in Submitted state (the mined branch is reached via status check).
            let tx = make_test_transaction(TransactionStatus::Submitted);

            // Expect schedule_status_check to be called with delay 5.
            mocks
                .job_producer
                .expect_produce_check_transaction_status_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));
            // Expect partial_update to update the transaction status to Mined.
            mocks
                .tx_repo
                .expect_partial_update_if_version()
                .returning(|_, _, update| {
                    let mut updated_tx = make_test_transaction(TransactionStatus::Submitted);
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    Ok(updated_tx)
                });

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction
                .handle_mined_state(tx.clone())
                .await
                .unwrap();
            assert_eq!(result.status, TransactionStatus::Mined);
        }
    }

    // Tests for `handle_final_state`
    mod handle_final_state_tests {
        use super::*;

        #[tokio::test]
        async fn test_final_state_confirmed() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let tx = make_test_transaction(TransactionStatus::Submitted);

            // Expect partial_update to update status to Confirmed.
            mocks
                .tx_repo
                .expect_partial_update_if_version()
                .returning(|_, _, update| {
                    let mut updated_tx = make_test_transaction(TransactionStatus::Submitted);
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    Ok(updated_tx)
                });

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction
                .handle_final_state(tx.clone(), TransactionStatus::Confirmed, None)
                .await
                .unwrap();
            assert_eq!(result.status, TransactionStatus::Confirmed);
        }

        #[tokio::test]
        async fn test_final_state_failed() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let tx = make_test_transaction(TransactionStatus::Submitted);

            // Expect partial_update to update status to Failed with status_reason.
            mocks
                .tx_repo
                .expect_partial_update_if_version()
                .returning(|_, _, update| {
                    let mut updated_tx = make_test_transaction(TransactionStatus::Submitted);
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    updated_tx.status_reason = update.status_reason.clone();
                    Ok(updated_tx)
                });

            let reason = "Transaction reverted on-chain (receipt status: failed)".to_string();
            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction
                .handle_final_state(tx.clone(), TransactionStatus::Failed, Some(reason.clone()))
                .await
                .unwrap();
            assert_eq!(result.status, TransactionStatus::Failed);
            assert_eq!(result.status_reason.as_deref(), Some(reason.as_str()));
        }

        #[tokio::test]
        async fn test_final_state_expired() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let tx = make_test_transaction(TransactionStatus::Submitted);

            // Expect partial_update to update status to Expired.
            mocks
                .tx_repo
                .expect_partial_update_if_version()
                .returning(|_, _, update| {
                    let mut updated_tx = make_test_transaction(TransactionStatus::Submitted);
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    Ok(updated_tx)
                });

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction
                .handle_final_state(tx.clone(), TransactionStatus::Expired, None)
                .await
                .unwrap();
            assert_eq!(result.status, TransactionStatus::Expired);
        }
    }

    // Integration tests for `handle_status_impl`
    mod handle_status_impl_tests {
        use super::*;

        #[tokio::test]
        async fn test_impl_submitted_branch() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.sent_at = Some((Utc::now() - Duration::seconds(120)).to_rfc3339());
            // Set a dummy hash so check_transaction_status can proceed.
            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.hash = Some("0xFakeHash".to_string());
            }
            // Simulate no receipt found.
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|_| Box::pin(async { Ok(None) }));
            // Mock network repository for should_resubmit check
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));
            // Expect that a status check job is scheduled.
            mocks
                .job_producer
                .expect_produce_check_transaction_status_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));
            // Expect update_transaction_status_if_needed to update status to Submitted.
            mocks
                .tx_repo
                .expect_partial_update_if_version()
                .returning(|_, _, update| {
                    let mut updated_tx = make_test_transaction(TransactionStatus::Submitted);
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    Ok(updated_tx)
                });

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.handle_status_impl(tx, None).await.unwrap();
            assert_eq!(result.status, TransactionStatus::Submitted);
        }

        #[tokio::test]
        async fn test_impl_mined_branch() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            // Set created_at to be old enough to pass is_too_early_to_resubmit
            tx.created_at = (Utc::now() - Duration::minutes(1)).to_rfc3339();
            // Set a dummy hash.
            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.hash = Some("0xFakeHash".to_string());
            }
            // Simulate a receipt with a block number of 100 and a successful receipt.
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|_| Box::pin(async { Ok(Some(make_mock_receipt(true, Some(100)))) }));
            // Simulate that the current block number is 100 (so confirmations are insufficient).
            mocks
                .provider
                .expect_get_block_number()
                .return_once(|| Box::pin(async { Ok(100) }));
            // Mock network repository to return a test network model
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));
            // Mock the notification job that gets sent after status update
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));
            // Expect a primary read to reload the transaction after status change
            mocks.tx_repo.expect_get_by_id_on_primary().returning(|_| {
                let updated_tx = make_test_transaction(TransactionStatus::Mined);
                Ok(updated_tx)
            });
            // Expect update_transaction_status_if_needed to update status to Mined.
            mocks
                .tx_repo
                .expect_partial_update_if_version()
                .returning(|_, _, update| {
                    let mut updated_tx = make_test_transaction(TransactionStatus::Submitted);
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    Ok(updated_tx)
                });

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.handle_status_impl(tx, None).await.unwrap();
            assert_eq!(result.status, TransactionStatus::Mined);
        }

        /// A replace that commits during the receipt RPC must not get the old hash's status.
        #[tokio::test]
        async fn test_impl_status_for_replaced_record_conflicts() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.created_at = (Utc::now() - Duration::minutes(1)).to_rfc3339();
            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.hash = Some("0xOldHash".to_string());
            }
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|_| Box::pin(async { Ok(Some(make_mock_receipt(true, Some(100)))) }));
            mocks
                .provider
                .expect_get_block_number()
                .return_once(|| Box::pin(async { Ok(100) }));
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));
            let replaced = {
                let mut r = tx.clone();
                r.version = tx.version + 1;
                r
            };
            mocks
                .tx_repo
                .expect_get_by_id_on_primary()
                .returning(move |_| Ok(replaced.clone()));
            mocks.tx_repo.expect_partial_update_if_version().never();

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let err = evm_transaction
                .handle_status_impl(tx, None)
                .await
                .unwrap_err();
            assert!(err.is_concurrent_update_conflict());
        }

        /// A matching status does not make another writer's newer record safe to use.
        #[tokio::test]
        async fn test_impl_same_status_on_replaced_record_conflicts() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let mut tx = make_test_transaction(TransactionStatus::Mined);
            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.hash = Some("0xOldHash".to_string());
            }
            // The receipt is gone (reorg), so the observed status is Submitted.
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|_| Box::pin(async { Ok(None) }));
            let replaced = {
                let mut r = tx.clone();
                r.version = tx.version + 2;
                r.status = TransactionStatus::Submitted;
                r
            };
            mocks
                .tx_repo
                .expect_get_by_id_on_primary()
                .returning(move |_| Ok(replaced.clone()));
            mocks.tx_repo.expect_partial_update_if_version().never();

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let err = evm_transaction
                .handle_status_impl(tx, None)
                .await
                .unwrap_err();
            assert!(err.is_concurrent_update_conflict());
        }

        /// Hash recovery in the status check bumps the version; that is not a conflict.
        #[tokio::test]
        async fn test_impl_status_after_own_recovery_proceeds() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.created_at = (Utc::now() - Duration::hours(1)).to_rfc3339();
            tx.sent_at = Some((Utc::now() - Duration::hours(1)).to_rfc3339());
            tx.hashes = vec!["0xH1".into(), "0xH2".into(), "0xH3".into()];
            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.hash = Some("0xH3".to_string());
            }
            // The current hash has no receipt; an earlier one mined.
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|hash| {
                    let found = hash == "0xH2";
                    Box::pin(async move { Ok(found.then(|| make_mock_receipt(true, Some(100)))) })
                });
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));
            let stored = Arc::new(std::sync::Mutex::new(tx.clone()));
            let written = stored.clone();
            mocks
                .tx_repo
                .expect_partial_update_if_version()
                .times(1)
                .withf(|_, _, update| update.status == Some(TransactionStatus::Mined))
                .returning(move |_, _, update| {
                    let mut t = written.lock().unwrap();
                    t.status = TransactionStatus::Mined;
                    t.network_data = update.network_data.clone().unwrap();
                    t.version += 1;
                    Ok(t.clone())
                });
            let read = stored.clone();
            mocks
                .tx_repo
                .expect_get_by_id_on_primary()
                .returning(move |_| Ok(read.lock().unwrap().clone()));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.handle_status_impl(tx, None).await.unwrap();
            assert_eq!(result.status, TransactionStatus::Mined);
            assert_eq!(
                result.network_data.get_evm_transaction_data().unwrap().hash,
                Some("0xH2".to_string())
            );
        }

        #[tokio::test]
        async fn test_impl_final_confirmed_branch() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            // Create a transaction with status Confirmed.
            let tx = make_test_transaction(TransactionStatus::Confirmed);

            // In this branch, check_transaction_status returns the final status immediately,
            // so we expect partial_update to update the transaction status to Confirmed.
            mocks
                .tx_repo
                .expect_partial_update_if_version()
                .returning(|_, _, update| {
                    let mut updated_tx = make_test_transaction(TransactionStatus::Submitted);
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    Ok(updated_tx)
                });

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.handle_status_impl(tx, None).await.unwrap();
            assert_eq!(result.status, TransactionStatus::Confirmed);
        }

        #[tokio::test]
        async fn test_impl_final_failed_branch() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            // Create a transaction with status Failed.
            let tx = make_test_transaction(TransactionStatus::Failed);

            mocks
                .tx_repo
                .expect_partial_update_if_version()
                .returning(|_, _, update| {
                    let mut updated_tx = make_test_transaction(TransactionStatus::Submitted);
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    Ok(updated_tx)
                });

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.handle_status_impl(tx, None).await.unwrap();
            assert_eq!(result.status, TransactionStatus::Failed);
        }

        /// Verifies that a Submitted transaction with a failed on-chain receipt
        /// transitions to Failed status with a descriptive status_reason.
        #[tokio::test]
        async fn test_impl_submitted_to_failed_sets_status_reason() {
            let mut mocks = default_test_mocks();
            let mut relayer = create_test_relayer();
            // Opt in to revert-data recovery so the transition-to-Failed path exercises it.
            relayer.policies = RelayerNetworkPolicy::Evm(RelayerEvmPolicy {
                include_revert_data: Some(true),
                ..Default::default()
            });
            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.created_at = (Utc::now() - Duration::minutes(1)).to_rfc3339();
            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.hash = Some("0xFakeHash".to_string());
            }

            // Simulate a receipt with status=false (reverted on-chain).
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|_| Box::pin(async { Ok(Some(make_mock_receipt(false, Some(100)))) }));

            // Revert-data recovery runs on the transition to Failed. The trace path returns no
            // usable output here, and the placeholder tx data can't be reconstructed into an
            // eth_call, so recovery yields the legacy generic reason.
            mocks
                .provider
                .expect_raw_request_dyn_best_effort()
                .returning(|_, _| Box::pin(async { Ok(serde_json::json!({})) }));

            // Mock get_by_id for the DB reload after status change.
            let tx_clone = tx.clone();
            mocks
                .tx_repo
                .expect_get_by_id_on_primary()
                .returning(move |_| {
                    let mut reloaded = tx_clone.clone();
                    reloaded.status = TransactionStatus::Submitted;
                    Ok(reloaded)
                });

            // Expect partial_update with status=Failed and a status_reason.
            mocks
                .tx_repo
                .expect_partial_update_if_version()
                .withf(|_, _, update| {
                    update.status == Some(TransactionStatus::Failed)
                        && update.status_reason.is_some()
                })
                .returning(|_, _, update| {
                    let mut updated_tx = make_test_transaction(TransactionStatus::Submitted);
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    updated_tx.status_reason = update.status_reason.clone();
                    Ok(updated_tx)
                });

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.handle_status_impl(tx, None).await.unwrap();
            assert_eq!(result.status, TransactionStatus::Failed);
            assert!(result.status_reason.is_some());
            assert!(
                result
                    .status_reason
                    .as_ref()
                    .unwrap()
                    .contains("reverted on-chain"),
                "Expected on-chain revert reason, got: {:?}",
                result.status_reason
            );
        }

        #[tokio::test]
        async fn test_impl_final_expired_branch() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            // Create a transaction with status Expired.
            let tx = make_test_transaction(TransactionStatus::Expired);

            mocks
                .tx_repo
                .expect_partial_update_if_version()
                .returning(|_, _, update| {
                    let mut updated_tx = make_test_transaction(TransactionStatus::Submitted);
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    Ok(updated_tx)
                });

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.handle_status_impl(tx, None).await.unwrap();
            assert_eq!(result.status, TransactionStatus::Expired);
        }
    }

    // Tests for on-chain revert-data recovery on the transition into Failed.
    mod revert_data_recovery_tests {
        use super::super::{extract_trace_output, truncate_revert_hex, MAX_REVERT_DATA_HEX_LEN};
        use super::*;
        use crate::services::provider::ProviderError;
        use alloy::primitives::Bytes;

        const LEGACY_REASON: &str = "Transaction reverted on-chain (receipt status: failed)";

        // Trace output is RPC-controlled: only well-formed `0x`-prefixed hex is accepted. This
        // rejects malformed/non-ASCII payloads that would otherwise be recorded verbatim and
        // could panic the byte-index slicing in `truncate_revert_hex`.
        #[test]
        fn test_extract_trace_output_accepts_only_hex() {
            let hex = |out: &str| extract_trace_output(&serde_json::json!({ "output": out }));
            assert_eq!(hex("0xdeadbeef"), Some("0xdeadbeef".to_string()));
            assert_eq!(hex("0x"), None); // no payload
            assert_eq!(hex("0xZZZZ"), None); // non-hex digits
            assert_eq!(hex("0x日本語"), None); // non-ASCII
            assert_eq!(hex("deadbeef"), None); // missing 0x prefix
            assert_eq!(extract_trace_output(&serde_json::json!({})), None); // missing field
        }

        // A very long (but valid-hex) payload truncates on a byte boundary without panicking.
        #[test]
        fn test_truncate_revert_hex_bounds_long_payload() {
            let long = format!("0x{}", "ab".repeat(MAX_REVERT_DATA_HEX_LEN));
            let truncated = truncate_revert_hex(&long);
            assert!(truncated.ends_with("...(truncated)"));
            assert!(truncated.starts_with("0xabab"));
        }

        /// A Submitted tx (aged past the resubmit grace) with valid EVM fields and a hash,
        /// poised to transition to Failed from a failed receipt.
        fn failing_submitted_tx() -> TransactionRepoModel {
            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.created_at = (Utc::now() - Duration::minutes(1)).to_rfc3339();
            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.from = "0x0000000000000000000000000000000000000001".to_string();
                evm_data.to = Some("0x0000000000000000000000000000000000000002".to_string());
                evm_data.data = Some("0xabcdef".to_string());
                evm_data.hash = Some("0xFakeHash".to_string());
            }
            tx
        }

        /// Receipt with status=false (reverted) at block 100. Reused for the status check and
        /// the recovery re-fetch.
        fn expect_failed_receipt(mocks: &mut TestMocks) {
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|_| Box::pin(async { Ok(Some(make_mock_receipt(false, Some(100)))) }));
        }

        /// DB reload after the status change, plus a partial_update that captures the
        /// status_reason onto the returned transaction so assertions can inspect it.
        fn expect_reload_and_capture(mocks: &mut TestMocks) {
            mocks.tx_repo.expect_get_by_id_on_primary().returning(|_| {
                let mut reloaded = failing_submitted_tx();
                reloaded.status = TransactionStatus::Submitted;
                Ok(reloaded)
            });
            mocks
                .tx_repo
                .expect_partial_update_if_version()
                .returning(|_, _, update| {
                    let mut updated_tx = make_test_transaction(TransactionStatus::Submitted);
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    updated_tx.status_reason = update.status_reason.clone();
                    Ok(updated_tx)
                });
        }

        // eth_call surfaces revert data -> enriched status_reason.
        #[tokio::test]
        async fn test_eth_call_revert_data_enriches_status_reason() {
            let mut mocks = default_test_mocks();
            let relayer = relayer_with_revert_policy(Some(true));
            let tx = failing_submitted_tx();

            expect_failed_receipt(&mut mocks);
            // Trace path yields no output -> fall back to eth_call.
            mocks
                .provider
                .expect_raw_request_dyn_best_effort()
                .returning(|_, _| Box::pin(async { Ok(serde_json::json!({})) }));
            mocks
                .provider
                .expect_get_call_revert_data()
                .returning(|_, _| {
                    Box::pin(async { Ok(Some(Bytes::from(hex::decode("5592f1b2").unwrap()))) })
                });
            expect_reload_and_capture(&mut mocks);

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.handle_status_impl(tx, None).await.unwrap();
            assert_eq!(result.status, TransactionStatus::Failed);
            let reason = result.status_reason.unwrap();
            assert!(
                reason.contains("revert_data: 0x5592f1b2"),
                "expected enriched revert_data, got: {reason}"
            );
        }

        // No revert data recovered -> generic reason, byte-for-byte.
        #[tokio::test]
        async fn test_no_revert_data_uses_legacy_reason() {
            let mut mocks = default_test_mocks();
            let relayer = relayer_with_revert_policy(Some(true));
            let tx = failing_submitted_tx();

            expect_failed_receipt(&mut mocks);
            mocks
                .provider
                .expect_raw_request_dyn_best_effort()
                .returning(|_, _| Box::pin(async { Ok(serde_json::json!({})) }));
            mocks
                .provider
                .expect_get_call_revert_data()
                .returning(|_, _| Box::pin(async { Ok(None) }));
            expect_reload_and_capture(&mut mocks);

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.handle_status_impl(tx, None).await.unwrap();
            assert_eq!(result.status, TransactionStatus::Failed);
            assert_eq!(result.status_reason.unwrap(), LEGACY_REASON);
        }

        // Tx already Failed -> no recovery RPC, reason not rewritten.
        #[tokio::test]
        async fn test_already_failed_skips_recovery() {
            // No provider expectations set: any recovery RPC (trace/eth_call/receipt) would panic.
            let mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let tx = make_test_transaction(TransactionStatus::Failed);

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.handle_status_impl(tx, None).await.unwrap();
            assert_eq!(result.status, TransactionStatus::Failed);
            assert!(
                result.status_reason.is_none(),
                "status_reason should not be rewritten on re-poll, got: {:?}",
                result.status_reason
            );
        }

        // Trace output is used and eth_call is NOT invoked.
        #[tokio::test]
        async fn test_trace_output_used_without_eth_call() {
            let mut mocks = default_test_mocks();
            let relayer = relayer_with_revert_policy(Some(true));
            let tx = failing_submitted_tx();

            expect_failed_receipt(&mut mocks);
            mocks
                .provider
                .expect_raw_request_dyn_best_effort()
                .returning(|_, _| {
                    Box::pin(async { Ok(serde_json::json!({"output": "0xdeadbeef"})) })
                });
            // eth_call fallback must never run when the trace path produced a payload.
            mocks.provider.expect_get_call_revert_data().never();
            expect_reload_and_capture(&mut mocks);

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.handle_status_impl(tx, None).await.unwrap();
            assert_eq!(result.status, TransactionStatus::Failed);
            assert!(result
                .status_reason
                .unwrap()
                .contains("revert_data: 0xdeadbeef"));
        }

        // Trace unavailable -> eth_call fallback produces the payload.
        #[tokio::test]
        async fn test_trace_unavailable_falls_back_to_eth_call() {
            let mut mocks = default_test_mocks();
            let relayer = relayer_with_revert_policy(Some(true));
            let tx = failing_submitted_tx();

            expect_failed_receipt(&mut mocks);
            mocks
                .provider
                .expect_raw_request_dyn_best_effort()
                .returning(|_, _| {
                    Box::pin(async {
                        Err(ProviderError::Other("method not supported".to_string()))
                    })
                });
            mocks
                .provider
                .expect_get_call_revert_data()
                // The reconstructed call must carry the persisted fee context (legacy gas_price
                // here) so contracts that branch on tx.gasprice revert as they did on-chain.
                .withf(|req, _block| req.gas_price == Some(20000000000))
                .returning(|_, _| {
                    Box::pin(async { Ok(Some(Bytes::from(hex::decode("08c379a0").unwrap()))) })
                });
            expect_reload_and_capture(&mut mocks);

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.handle_status_impl(tx, None).await.unwrap();
            assert_eq!(result.status, TransactionStatus::Failed);
            assert!(result
                .status_reason
                .unwrap()
                .contains("revert_data: 0x08c379a0"));
        }

        // Both trace and eth_call fail/empty -> generic reason, status still Failed, no error.
        #[tokio::test]
        async fn test_both_paths_fail_uses_legacy_reason() {
            let mut mocks = default_test_mocks();
            let relayer = relayer_with_revert_policy(Some(true));
            let tx = failing_submitted_tx();

            expect_failed_receipt(&mut mocks);
            mocks
                .provider
                .expect_raw_request_dyn_best_effort()
                .returning(|_, _| {
                    Box::pin(async { Err(ProviderError::Other("trace down".to_string())) })
                });
            mocks
                .provider
                .expect_get_call_revert_data()
                .returning(|_, _| {
                    Box::pin(async { Err(ProviderError::Other("eth_call down".to_string())) })
                });
            expect_reload_and_capture(&mut mocks);

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.handle_status_impl(tx, None).await.unwrap();
            assert_eq!(result.status, TransactionStatus::Failed);
            assert_eq!(result.status_reason.unwrap(), LEGACY_REASON);
        }

        /// Builds a relayer whose EVM policy sets `include_revert_data` to the given value.
        fn relayer_with_revert_policy(include: Option<bool>) -> RelayerRepoModel {
            let mut relayer = create_test_relayer();
            relayer.policies = RelayerNetworkPolicy::Evm(RelayerEvmPolicy {
                include_revert_data: include,
                ..Default::default()
            });
            relayer
        }

        // None (the default) and Some(false) -> zero recovery RPCs, generic reason.
        #[tokio::test]
        async fn test_recovery_disabled_issues_no_recovery_rpcs() {
            for include in [None, Some(false)] {
                let mut mocks = default_test_mocks();
                let relayer = relayer_with_revert_policy(include);
                let tx = failing_submitted_tx();

                // Only the status-check receipt fetch is allowed; recovery must issue none of these.
                expect_failed_receipt(&mut mocks);
                mocks.provider.expect_raw_request_dyn_best_effort().never();
                mocks.provider.expect_get_call_revert_data().never();
                expect_reload_and_capture(&mut mocks);

                let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
                let result = evm_transaction.handle_status_impl(tx, None).await.unwrap();
                assert_eq!(result.status, TransactionStatus::Failed);
                assert_eq!(
                    result.status_reason.unwrap(),
                    LEGACY_REASON,
                    "recovery must not run for include_revert_data = {include:?}"
                );
            }
        }

        // Only an explicit opt-in (Some(true)) attempts recovery; None is off by default.
        #[tokio::test]
        async fn test_recovery_attempted_when_enabled() {
            let mut mocks = default_test_mocks();
            let relayer = relayer_with_revert_policy(Some(true));
            let tx = failing_submitted_tx();

            expect_failed_receipt(&mut mocks);
            // Asserting the trace RPC is issued exactly once proves recovery was attempted.
            mocks
                .provider
                .expect_raw_request_dyn_best_effort()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(serde_json::json!({"output": "0xfeed"})) }));
            expect_reload_and_capture(&mut mocks);

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.handle_status_impl(tx, None).await.unwrap();
            assert_eq!(result.status, TransactionStatus::Failed);
            assert!(
                result
                    .status_reason
                    .unwrap()
                    .contains("revert_data: 0xfeed"),
                "recovery should run for include_revert_data = Some(true)"
            );
        }
    }

    // Tests for circuit breaker functionality
    mod circuit_breaker_tests {
        use super::*;
        use crate::jobs::StatusCheckContext;

        /// Helper to create a context that should trigger the circuit breaker
        fn create_triggered_context() -> StatusCheckContext {
            StatusCheckContext::new(
                30, // consecutive_failures: exceeds EVM threshold of 25
                50, // total_failures
                60, // total_retries
                25, // max_consecutive_failures (EVM default)
                75, // max_total_failures (EVM default)
                NetworkType::Evm,
            )
        }

        /// Helper to create a context that should NOT trigger the circuit breaker
        fn create_safe_context() -> StatusCheckContext {
            StatusCheckContext::new(
                5,  // consecutive_failures: below threshold
                10, // total_failures
                15, // total_retries
                25, // max_consecutive_failures
                75, // max_total_failures
                NetworkType::Evm,
            )
        }

        /// Helper to create a context that triggers via total failures (safety net)
        fn create_total_triggered_context() -> StatusCheckContext {
            StatusCheckContext::new(
                5,   // consecutive_failures: below threshold
                80,  // total_failures: exceeds EVM threshold of 75
                100, // total_retries
                25,  // max_consecutive_failures
                75,  // max_total_failures
                NetworkType::Evm,
            )
        }

        #[tokio::test]
        async fn test_circuit_breaker_pending_marks_as_failed() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let tx = make_test_transaction(TransactionStatus::Pending);

            // Expect partial_update to be called with Failed status
            mocks
                .tx_repo
                .expect_partial_update_if_version()
                .withf(|_, _, update| update.status == Some(TransactionStatus::Failed))
                .returning(|_, _, update| {
                    let mut updated_tx = make_test_transaction(TransactionStatus::Pending);
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    updated_tx.status_reason = update.status_reason.clone();
                    Ok(updated_tx)
                });

            // Mock notification (best effort, may or may not be called)
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let ctx = create_triggered_context();

            let result = evm_transaction
                .handle_status_impl(tx, Some(ctx))
                .await
                .unwrap();

            assert_eq!(result.status, TransactionStatus::Failed);
            assert!(result.status_reason.is_some());
            assert!(result.status_reason.unwrap().contains("consecutive errors"));
        }

        #[tokio::test]
        async fn test_circuit_breaker_sent_marks_as_failed() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let tx = make_test_transaction(TransactionStatus::Sent);

            // Expect partial_update to be called with Failed status
            mocks
                .tx_repo
                .expect_partial_update_if_version()
                .withf(|_, _, update| update.status == Some(TransactionStatus::Failed))
                .returning(|_, _, update| {
                    let mut updated_tx = make_test_transaction(TransactionStatus::Sent);
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    updated_tx.status_reason = update.status_reason.clone();
                    Ok(updated_tx)
                });

            // Mock notification
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let ctx = create_triggered_context();

            let result = evm_transaction
                .handle_status_impl(tx, Some(ctx))
                .await
                .unwrap();

            assert_eq!(result.status, TransactionStatus::Failed);
        }

        #[tokio::test]
        async fn test_circuit_breaker_submitted_triggers_noop() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.sent_at = Some((Utc::now() - Duration::seconds(120)).to_rfc3339());

            // Mock network repository for NOOP processing
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            // Expect partial_update to be called with NOOP indicator
            mocks
                .tx_repo
                .expect_partial_update_if_version()
                .returning(|_, _, update| {
                    let mut updated_tx = make_test_transaction(TransactionStatus::Submitted);
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    updated_tx.status_reason = update.status_reason.clone();
                    updated_tx.noop_count = update.noop_count;
                    Ok(updated_tx)
                });

            // Mock resubmit job (NOOP triggers resubmit)
            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let ctx = create_triggered_context();

            let result = evm_transaction
                .handle_status_impl(tx, Some(ctx))
                .await
                .unwrap();

            // NOOP processing should succeed
            assert!(result.noop_count.is_some());
        }

        #[tokio::test]
        async fn test_circuit_breaker_noop_tx_excluded() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            // Create a NOOP transaction (to: self, value: 0, data: "0x")
            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.sent_at = Some((Utc::now() - Duration::seconds(120)).to_rfc3339());
            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.to = Some(evm_data.from.clone()); // to == from (NOOP indicator)
                evm_data.value = U256::from(0);
                evm_data.data = Some("0x".to_string());
                evm_data.hash = Some("0xFakeHash".to_string());
            }

            // NOOP transactions should NOT trigger circuit breaker
            // Instead, they should go through normal status checking
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|_| Box::pin(async { Ok(None) }));

            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            mocks
                .job_producer
                .expect_produce_check_transaction_status_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            // Mock resubmit job (may be triggered by normal status flow for stuck transactions)
            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            mocks
                .tx_repo
                .expect_partial_update_if_version()
                .returning(|_, _, update| {
                    let mut updated_tx = make_test_transaction(TransactionStatus::Submitted);
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    Ok(updated_tx)
                });

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let ctx = create_triggered_context();

            let result = evm_transaction
                .handle_status_impl(tx, Some(ctx))
                .await
                .unwrap();

            // NOOP tx should continue normal processing, not be force-failed
            assert_eq!(result.status, TransactionStatus::Submitted);
        }

        #[tokio::test]
        async fn test_circuit_breaker_total_failures_triggers() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let tx = make_test_transaction(TransactionStatus::Pending);

            // Expect partial_update to be called with Failed status
            mocks
                .tx_repo
                .expect_partial_update_if_version()
                .withf(|_, _, update| update.status == Some(TransactionStatus::Failed))
                .returning(|_, _, update| {
                    let mut updated_tx = make_test_transaction(TransactionStatus::Pending);
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    updated_tx.status_reason = update.status_reason.clone();
                    Ok(updated_tx)
                });

            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            // Use context that triggers via total failures (safety net)
            let ctx = create_total_triggered_context();

            let result = evm_transaction
                .handle_status_impl(tx, Some(ctx))
                .await
                .unwrap();

            assert_eq!(result.status, TransactionStatus::Failed);
            assert!(result.status_reason.is_some());
        }

        #[tokio::test]
        async fn test_circuit_breaker_below_threshold_continues_normally() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.sent_at = Some((Utc::now() - Duration::seconds(120)).to_rfc3339());

            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.hash = Some("0xFakeHash".to_string());
            }

            // Below threshold, should continue with normal status checking
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|_| Box::pin(async { Ok(None) }));

            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            mocks
                .job_producer
                .expect_produce_check_transaction_status_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            mocks
                .tx_repo
                .expect_partial_update_if_version()
                .returning(|_, _, update| {
                    let mut updated_tx = make_test_transaction(TransactionStatus::Submitted);
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    Ok(updated_tx)
                });

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let ctx = create_safe_context();

            let result = evm_transaction
                .handle_status_impl(tx, Some(ctx))
                .await
                .unwrap();

            // Should continue normal processing, not trigger circuit breaker
            assert_eq!(result.status, TransactionStatus::Submitted);
        }

        #[tokio::test]
        async fn test_circuit_breaker_no_context_continues_normally() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.sent_at = Some((Utc::now() - Duration::seconds(120)).to_rfc3339());

            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.hash = Some("0xFakeHash".to_string());
            }

            // No context means no circuit breaker, should continue normally
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|_| Box::pin(async { Ok(None) }));

            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            mocks
                .job_producer
                .expect_produce_check_transaction_status_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            mocks
                .tx_repo
                .expect_partial_update_if_version()
                .returning(|_, _, update| {
                    let mut updated_tx = make_test_transaction(TransactionStatus::Submitted);
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    Ok(updated_tx)
                });

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);

            // Pass None for context
            let result = evm_transaction.handle_status_impl(tx, None).await.unwrap();

            // Should continue normal processing
            assert_eq!(result.status, TransactionStatus::Submitted);
        }

        #[tokio::test]
        async fn test_circuit_breaker_final_state_early_return() {
            let mocks = default_test_mocks();
            let relayer = create_test_relayer();
            // Transaction is already in final state
            let tx = make_test_transaction(TransactionStatus::Confirmed);

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let ctx = create_triggered_context();

            // Even with triggered context, final states should return early
            let result = evm_transaction
                .handle_status_impl(tx, Some(ctx))
                .await
                .unwrap();

            assert_eq!(result.status, TransactionStatus::Confirmed);
        }
    }

    // Tests for hash recovery functions
    mod hash_recovery_tests {
        use super::super::HistoricalHashRecovery;
        use super::*;

        #[tokio::test]
        async fn test_should_try_hash_recovery_not_submitted() {
            let mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Pending);
            tx.hashes = vec![
                "0xHash1".to_string(),
                "0xHash2".to_string(),
                "0xHash3".to_string(),
            ];

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.should_try_hash_recovery(&tx).unwrap();

            assert!(
                !result,
                "Should not attempt recovery for non-Submitted transactions"
            );
        }

        #[tokio::test]
        async fn test_should_try_hash_recovery_not_enough_hashes() {
            let mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.hashes = vec!["0xHash1".to_string()]; // Only 1 hash
            tx.sent_at = Some((Utc::now() - Duration::minutes(3)).to_rfc3339());

            if let NetworkTransactionData::Evm(ref mut data) = tx.network_data {
                data.hash = tx.hashes.last().cloned();
            }
            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.should_try_hash_recovery(&tx).unwrap();

            assert!(
                !result,
                "Should not attempt recovery with insufficient hashes"
            );
        }

        #[tokio::test]
        async fn test_should_try_hash_recovery_too_recent() {
            let mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.hashes = vec![
                "0xHash1".to_string(),
                "0xHash2".to_string(),
                "0xHash3".to_string(),
            ];
            tx.sent_at = Some(Utc::now().to_rfc3339()); // Recent

            if let NetworkTransactionData::Evm(ref mut data) = tx.network_data {
                data.hash = tx.hashes.last().cloned();
            }
            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.should_try_hash_recovery(&tx).unwrap();

            assert!(
                !result,
                "Should not attempt recovery for recently sent transactions"
            );
        }

        #[tokio::test]
        async fn test_should_try_hash_recovery_success() {
            let mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.hashes = vec![
                "0xHash1".to_string(),
                "0xHash2".to_string(),
                "0xHash3".to_string(),
            ];
            tx.sent_at = Some((Utc::now() - Duration::minutes(3)).to_rfc3339());

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.should_try_hash_recovery(&tx).unwrap();

            assert!(
                result,
                "Should attempt recovery for stuck transactions with multiple hashes"
            );
        }

        #[tokio::test]
        async fn test_try_recover_no_historical_hash_found() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.hashes = vec![
                "0xHash1".to_string(),
                "0xHash2".to_string(),
                "0xHash3".to_string(),
            ];

            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.hash = Some("0xHash3".to_string());
            }

            // Mock provider to return None for all hash lookups
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|_| Box::pin(async { Ok(None) }));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let evm_data = tx.network_data.get_evm_transaction_data().unwrap();
            let result = evm_transaction
                .try_recover_with_historical_hashes(&tx, &evm_data)
                .await
                .unwrap();

            assert!(
                matches!(result, HistoricalHashRecovery::NotFound),
                "Should return NotFound when no historical hash is found"
            );
        }

        #[tokio::test]
        async fn test_try_recover_lookup_error_without_match_is_incomplete() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.hashes = vec![
                "0xHash1".to_string(),
                "0xHash2".to_string(),
                "0xHash3".to_string(),
            ];
            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.hash = Some("0xHash3".to_string());
            }

            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|hash| {
                    if hash == "0xHash2" {
                        Box::pin(async { Err(crate::services::provider::ProviderError::Timeout) })
                    } else {
                        Box::pin(async { Ok(None) })
                    }
                });

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let evm_data = tx.network_data.get_evm_transaction_data().unwrap();
            let result = evm_transaction
                .try_recover_with_historical_hashes(&tx, &evm_data)
                .await;

            assert!(
                matches!(result, Ok(HistoricalHashRecovery::Incomplete)),
                "Should report Incomplete when a lookup failed and no hash was found, got: {result:?}"
            );
        }

        #[tokio::test]
        async fn test_try_recover_finds_mined_historical_hash() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.hashes = vec![
                "0xHash1".to_string(),
                "0xHash2".to_string(), // This one is mined
                "0xHash3".to_string(),
            ];

            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.hash = Some("0xHash3".to_string()); // Current hash (wrong one)
            }

            // Mock provider to return None for Hash1 and Hash3, but receipt for Hash2
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|hash| {
                    if hash == "0xHash2" {
                        Box::pin(async { Ok(Some(make_mock_receipt(true, Some(100)))) })
                    } else {
                        Box::pin(async { Ok(None) })
                    }
                });

            // Mock partial_update for correcting the hash
            let tx_clone = tx.clone();
            mocks
                .tx_repo
                .expect_partial_update_if_version()
                .returning(move |_, _, update| {
                    let mut updated_tx = tx_clone.clone();
                    if let Some(status) = update.status {
                        updated_tx.status = status;
                    }
                    if let Some(NetworkTransactionData::Evm(ref evm_data)) = update.network_data {
                        if let NetworkTransactionData::Evm(ref mut updated_evm) =
                            updated_tx.network_data
                        {
                            updated_evm.hash = evm_data.hash.clone();
                        }
                    }
                    Ok(updated_tx)
                });

            // Mock notification job
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let evm_data = tx.network_data.get_evm_transaction_data().unwrap();
            let result = evm_transaction
                .try_recover_with_historical_hashes(&tx, &evm_data)
                .await
                .unwrap();

            let HistoricalHashRecovery::Recovered(recovered_tx) = result else {
                panic!("Should recover the transaction, got: {result:?}");
            };
            assert_eq!(recovered_tx.status, TransactionStatus::Mined);
        }

        #[tokio::test]
        async fn test_try_recover_network_error_continues() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.hashes = vec![
                "0xHash1".to_string(),
                "0xHash2".to_string(), // Network error
                "0xHash3".to_string(), // This one is mined
            ];

            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.hash = Some("0xHash1".to_string());
            }

            // Mock provider to return error for Hash2, receipt for Hash3
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|hash| {
                    if hash == "0xHash2" {
                        Box::pin(async { Err(crate::services::provider::ProviderError::Timeout) })
                    } else if hash == "0xHash3" {
                        Box::pin(async { Ok(Some(make_mock_receipt(true, Some(100)))) })
                    } else {
                        Box::pin(async { Ok(None) })
                    }
                });

            // Mock partial_update for correcting the hash
            let tx_clone = tx.clone();
            mocks
                .tx_repo
                .expect_partial_update_if_version()
                .returning(move |_, _, update| {
                    let mut updated_tx = tx_clone.clone();
                    if let Some(status) = update.status {
                        updated_tx.status = status;
                    }
                    Ok(updated_tx)
                });

            // Mock notification job
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let evm_data = tx.network_data.get_evm_transaction_data().unwrap();
            let result = evm_transaction
                .try_recover_with_historical_hashes(&tx, &evm_data)
                .await
                .unwrap();

            assert!(
                matches!(result, HistoricalHashRecovery::Recovered(_)),
                "Should continue checking after network error and find mined hash"
            );
        }

        #[tokio::test]
        async fn test_update_transaction_with_corrected_hash() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.hash = Some("0xWrongHash".to_string());
            }

            // Mock partial_update
            mocks
                .tx_repo
                .expect_partial_update_if_version()
                .returning(move |_, _, update| {
                    let mut updated_tx = make_test_transaction(TransactionStatus::Submitted);
                    if let Some(status) = update.status {
                        updated_tx.status = status;
                    }
                    if let Some(NetworkTransactionData::Evm(ref evm_data)) = update.network_data {
                        if let NetworkTransactionData::Evm(ref mut updated_evm) =
                            updated_tx.network_data
                        {
                            updated_evm.hash = evm_data.hash.clone();
                        }
                    }
                    Ok(updated_tx)
                });

            // Mock notification job
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let evm_data = tx.network_data.get_evm_transaction_data().unwrap();
            let result = evm_transaction
                .update_transaction_with_corrected_hash(
                    &tx,
                    &evm_data,
                    "0xCorrectHash",
                    TransactionStatus::Mined,
                )
                .await
                .unwrap();

            assert_eq!(result.status, TransactionStatus::Mined);
            if let NetworkTransactionData::Evm(ref updated_evm) = result.network_data {
                assert_eq!(updated_evm.hash.as_ref().unwrap(), "0xCorrectHash");
            }
        }
    }

    // Tests for check_transaction_status edge cases
    mod check_transaction_status_edge_cases {
        use super::*;

        #[tokio::test]
        async fn test_missing_hash_returns_error() {
            let mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let tx = make_test_transaction(TransactionStatus::Submitted);
            // Hash is None by default

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.check_transaction_status(&tx).await;

            assert!(result.is_err(), "Should return error when hash is missing");
        }

        #[tokio::test]
        async fn test_pending_status_early_return() {
            let mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let tx = make_test_transaction(TransactionStatus::Pending);

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let status = evm_transaction.check_transaction_status(&tx).await.unwrap();

            assert_eq!(
                status,
                TransactionStatus::Pending,
                "Should return Pending without querying blockchain"
            );
        }

        #[tokio::test]
        async fn test_sent_status_early_return() {
            let mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let tx = make_test_transaction(TransactionStatus::Sent);

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let status = evm_transaction.check_transaction_status(&tx).await.unwrap();

            assert_eq!(
                status,
                TransactionStatus::Sent,
                "Should return Sent without querying blockchain"
            );
        }

        #[tokio::test]
        async fn test_final_state_early_return() {
            let mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let tx = make_test_transaction(TransactionStatus::Confirmed);

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let status = evm_transaction.check_transaction_status(&tx).await.unwrap();

            assert_eq!(
                status,
                TransactionStatus::Confirmed,
                "Should return final state without querying blockchain"
            );
        }
    }

    mod nonce_recovery_tests {
        use super::*;
        use crate::domain::transaction::evm::evm_transaction::TX_NONCE_RECONCILE_TRIGGER;
        use crate::jobs::StatusCheckContext;

        /// Test reconcile_tx_nonce_state with on_chain_nonce > tx_nonce → marks Failed
        #[tokio::test]
        async fn test_nonce_recovery_nonce_consumed_externally() {
            let mut mocks = default_test_mocks_with_network();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.network_data = NetworkTransactionData::Evm(EvmTransactionData {
                nonce: Some(5),
                hash: Some("0xhash".to_string()),
                raw: Some(vec![1, 2, 3]),
                ..tx.network_data.get_evm_transaction_data().unwrap()
            });
            tx.sent_at = Some(Utc::now().to_rfc3339());

            tx.metadata = Some(crate::models::TransactionMetadata {
                nonce_used_seen_at_block: Some(76),
                nonce_used_seen_at: Some((Utc::now() - Duration::hours(2)).to_rfc3339()),
                ..Default::default()
            });
            mocks
                .provider
                .expect_get_block_number()
                .returning(|| Box::pin(async { Ok(100) }));
            // No receipt for current hash
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|_| Box::pin(async { Ok(None) }));

            // On-chain nonce is 10, tx nonce is 5 → consumed externally
            mocks
                .provider
                .expect_get_transaction_count()
                .returning(|_| Box::pin(async { Ok(10) }));

            // Should update to Failed status
            let tx_clone = tx.clone();
            mocks
                .tx_repo
                .expect_partial_update_if_version()
                .withf(|_, _, update| {
                    update.status == Some(TransactionStatus::Failed)
                        && update
                            .status_reason
                            .as_ref()
                            .map(|r| r.contains("consumed externally"))
                            .unwrap_or(false)
                })
                .returning(move |_, _, update| {
                    let mut updated_tx = tx_clone.clone();
                    updated_tx.status = update.status.unwrap();
                    updated_tx.status_reason = update.status_reason.clone();
                    Ok(updated_tx)
                });

            // Should schedule nonce health job after detecting external consumption
            mocks
                .job_producer
                .expect_produce_relayer_health_check_job()
                .withf(|job, scheduled_on| {
                    job.relayer_id == "test-relayer-id"
                        && job.metadata.as_ref().map_or(false, |m| {
                            m.get("health_check_action") == Some(&"nonce_health".to_string())
                        })
                        && scheduled_on.is_none()
                })
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction
                .reconcile_tx_nonce_state(&mut tx)
                .await
                .map(crate::domain::transaction::evm::status::NonceReconcile::handled);

            assert!(result.is_ok());
            let recovered = result.unwrap();
            assert!(recovered.is_some(), "Expected Some(tx) for consumed nonce");
            assert_eq!(recovered.unwrap().status, TransactionStatus::Failed);
        }

        /// A historical-hash lookup error must block the "consumed externally" Failed path:
        /// the mined payload may be the one we failed to check.
        #[tokio::test]
        async fn test_nonce_recovery_historical_lookup_error_prevents_force_fail() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.network_data = NetworkTransactionData::Evm(EvmTransactionData {
                nonce: Some(5),
                hash: Some("0xhash".to_string()),
                raw: Some(vec![1, 2, 3]),
                ..tx.network_data.get_evm_transaction_data().unwrap()
            });
            tx.hashes = vec!["0xresubmitted".to_string(), "0xhash".to_string()];

            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|hash| {
                    if hash == "0xresubmitted" {
                        Box::pin(async { Err(crate::services::provider::ProviderError::Timeout) })
                    } else {
                        Box::pin(async { Ok(None) })
                    }
                });
            mocks
                .provider
                .expect_get_transaction_count()
                .returning(|_| Box::pin(async { Ok(10) }));
            mocks
                .provider
                .expect_get_block_number()
                .returning(|| Box::pin(async { Ok(100) }));
            mocks.tx_repo.expect_partial_update_if_version().times(0);

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction
                .reconcile_tx_nonce_state(&mut tx)
                .await
                .map(crate::domain::transaction::evm::status::NonceReconcile::handled);

            assert!(
                matches!(result, Ok(Some(_))),
                "Expected an unchanged transaction, got: {result:?}"
            );
        }

        /// Test reconcile_tx_nonce_state with on_chain_nonce <= tx_nonce → returns None
        #[tokio::test]
        async fn test_nonce_recovery_nonce_not_consumed() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.network_data = NetworkTransactionData::Evm(EvmTransactionData {
                nonce: Some(5),
                hash: Some("0xhash".to_string()),
                raw: Some(vec![1, 2, 3]),
                ..tx.network_data.get_evm_transaction_data().unwrap()
            });

            mocks
                .provider
                .expect_get_block_number()
                .returning(|| Box::pin(async { Ok(100) }));
            // No receipt for current hash
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|_| Box::pin(async { Ok(None) }));

            // On-chain nonce is 5, same as tx nonce → not consumed yet
            mocks
                .provider
                .expect_get_transaction_count()
                .returning(|_| Box::pin(async { Ok(5) }));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction
                .reconcile_tx_nonce_state(&mut tx)
                .await
                .map(crate::domain::transaction::evm::status::NonceReconcile::handled);

            assert!(result.is_ok());
            assert!(
                result.unwrap().is_none(),
                "Expected None when nonce not consumed"
            );
        }

        /// Test reconcile_tx_nonce_state with receipt found → returns None (defer to normal flow)
        #[tokio::test]
        async fn test_nonce_recovery_receipt_found() {
            let mut mocks = default_test_mocks();
            mocks
                .provider
                .expect_get_transaction_count()
                .returning(|_| Box::pin(async { Ok(10) }));
            mocks
                .provider
                .expect_get_block_number()
                .returning(|| Box::pin(async { Ok(100) }));
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.network_data = NetworkTransactionData::Evm(EvmTransactionData {
                nonce: Some(5),
                hash: Some("0xhash".to_string()),
                raw: Some(vec![1, 2, 3]),
                ..tx.network_data.get_evm_transaction_data().unwrap()
            });

            // Receipt exists for current hash — defer to normal flow
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|_| {
                    let receipt = make_mock_receipt(true, Some(100));
                    Box::pin(async move { Ok(Some(receipt)) })
                });

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction
                .reconcile_tx_nonce_state(&mut tx)
                .await
                .map(crate::domain::transaction::evm::status::NonceReconcile::handled);

            assert!(result.is_ok());
            assert!(
                result.unwrap().is_none(),
                "Expected None when receipt found — defer to normal flow"
            );
        }

        /// A receipt lookup error keeps the transaction unchanged and blocks resubmission.
        #[tokio::test]
        async fn test_nonce_recovery_rpc_error_prevents_force_fail() {
            let mut mocks = default_test_mocks();
            mocks
                .provider
                .expect_get_transaction_count()
                .returning(|_| Box::pin(async { Ok(10) }));
            mocks
                .provider
                .expect_get_block_number()
                .returning(|| Box::pin(async { Ok(100) }));
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.network_data = NetworkTransactionData::Evm(EvmTransactionData {
                nonce: Some(5),
                hash: Some("0xhash".to_string()),
                raw: Some(vec![1, 2, 3]),
                ..tx.network_data.get_evm_transaction_data().unwrap()
            });

            // Receipt check FAILS with RPC error
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|_| {
                    Box::pin(async {
                        Err(crate::services::provider::ProviderError::Other(
                            "RPC timeout".to_string(),
                        ))
                    })
                });

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction
                .reconcile_tx_nonce_state(&mut tx)
                .await
                .map(crate::domain::transaction::evm::status::NonceReconcile::handled);

            assert!(result.is_ok());
            assert!(
                result.unwrap().is_some(),
                "Expected an unchanged transaction when RPC errors occurred — must not force-fail on incomplete data"
            );
        }

        /// Test handle_status_impl with nonce_error_hint metadata triggers recovery
        #[tokio::test]
        async fn test_handle_status_impl_nonce_recovery_hint() {
            let mut mocks = default_test_mocks_with_network();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.network_data = NetworkTransactionData::Evm(EvmTransactionData {
                nonce: Some(5),
                hash: Some("0xhash".to_string()),
                raw: Some(vec![1, 2, 3]),
                ..tx.network_data.get_evm_transaction_data().unwrap()
            });
            tx.sent_at = Some(Utc::now().to_rfc3339());

            tx.metadata = Some(crate::models::TransactionMetadata {
                nonce_used_seen_at_block: Some(76),
                nonce_used_seen_at: Some((Utc::now() - Duration::hours(2)).to_rfc3339()),
                ..Default::default()
            });
            mocks
                .provider
                .expect_get_block_number()
                .returning(|| Box::pin(async { Ok(100) }));
            // No receipt for current hash
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|_| Box::pin(async { Ok(None) }));

            // On-chain nonce > tx nonce → consumed externally
            mocks
                .provider
                .expect_get_transaction_count()
                .returning(|_| Box::pin(async { Ok(10) }));

            // Should update to Failed
            let tx_clone = tx.clone();
            mocks
                .tx_repo
                .expect_partial_update_if_version()
                .returning(move |_, _, update| {
                    let mut updated_tx = tx_clone.clone();
                    if let Some(status) = update.status {
                        updated_tx.status = status;
                    }
                    updated_tx.status_reason = update.status_reason.clone();
                    Ok(updated_tx)
                });

            // Should schedule nonce health job after detecting external consumption
            mocks
                .job_producer
                .expect_produce_relayer_health_check_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);

            // Build context with nonce_error_hint metadata
            let mut metadata = std::collections::HashMap::new();
            metadata.insert(
                TX_NONCE_RECONCILE_TRIGGER.to_string(),
                "NonceTooLow".to_string(),
            );
            let context = StatusCheckContext::default().with_job_metadata(Some(metadata));

            let result = evm_transaction.handle_status_impl(tx, Some(context)).await;
            assert!(result.is_ok());
            assert_eq!(result.unwrap().status, TransactionStatus::Failed);
        }
    }

    mod circuit_breaker_sent_state_tests {
        use super::*;
        use crate::jobs::StatusCheckContext;

        /// Test circuit breaker on Sent tx with nonce → issues NOOP + submit job
        #[tokio::test]
        async fn test_circuit_breaker_sent_with_nonce_issues_noop() {
            let mut mocks = default_test_mocks_with_network();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Sent);
            tx.network_data = NetworkTransactionData::Evm(EvmTransactionData {
                nonce: Some(5),
                hash: Some("0xhash".to_string()),
                raw: Some(vec![1, 2, 3]),
                ..tx.network_data.get_evm_transaction_data().unwrap()
            });
            tx.sent_at = Some(Utc::now().to_rfc3339());

            // process_noop_transaction calls prepare_noop_update_request → partial_update
            let tx_clone = tx.clone();
            mocks
                .tx_repo
                .expect_partial_update_if_version()
                .returning(move |_, _, update| {
                    let mut updated_tx = tx_clone.clone();
                    if let Some(status) = update.status {
                        updated_tx.status = status;
                    }
                    updated_tx.status_reason = update.status_reason.clone();
                    Ok(updated_tx)
                });

            // Non-Arbitrum by name: the NOOP resubmit is never deferred.
            mocks
                .network_repo
                .expect_get_by_name()
                .returning(|_, _| Ok(Some(create_test_network_model())));
            mocks.provider.expect_get_transaction_count().never();

            // Should produce a submit job (for the NOOP)
            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);

            // Circuit breaker context that should trigger
            let ctx = StatusCheckContext::new(100, 200, 250, 15, 45, NetworkType::Evm);

            let result = evm_transaction
                .handle_circuit_breaker_safely(tx, &ctx)
                .await;
            assert!(result.is_ok());
        }

        /// Arbitrum: the circuit breaker persists a Sent NOOP but defers its resubmit
        /// while the nonce is above the chain head; at the head it is resubmitted.
        #[tokio::test]
        async fn arbitrum_circuit_breaker_sent_noop_resubmits_only_at_head() {
            for (on_chain_nonce, expect_job) in [(5u64, false), (7u64, true)] {
                let mut mocks = default_test_mocks_with_network();
                let relayer = create_test_relayer();

                let mut tx = make_test_transaction(TransactionStatus::Sent);
                tx.network_data = NetworkTransactionData::Evm(EvmTransactionData {
                    nonce: Some(7),
                    hash: Some("0xhash".to_string()),
                    raw: Some(vec![1, 2, 3]),
                    ..tx.network_data.get_evm_transaction_data().unwrap()
                });
                tx.sent_at = Some(Utc::now().to_rfc3339());

                mocks
                    .network_repo
                    .expect_get_by_name()
                    .with(
                        mockall::predicate::eq(NetworkType::Evm),
                        mockall::predicate::eq(relayer.network.clone()),
                    )
                    .returning(|_, _| Ok(Some(create_test_arbitrum_network_model())));
                mocks
                    .provider
                    .expect_get_transaction_count()
                    .times(1)
                    .returning(move |_| Box::pin(async move { Ok(on_chain_nonce) }));

                let tx_clone = tx.clone();
                mocks
                    .tx_repo
                    .expect_partial_update_if_version()
                    .times(1)
                    .returning(move |_, _, update| {
                        let mut updated_tx = tx_clone.clone();
                        updated_tx.noop_count = update.noop_count;
                        Ok(updated_tx)
                    });
                mocks
                    .job_producer
                    .expect_produce_submit_transaction_job()
                    .times(usize::from(expect_job))
                    .returning(|_, _| Box::pin(async { Ok(()) }));

                let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
                let ctx = StatusCheckContext::new(100, 200, 250, 15, 45, NetworkType::Evm);
                let result = evm_transaction
                    .handle_circuit_breaker_safely(tx, &ctx)
                    .await
                    .unwrap();
                assert!(result.noop_count.unwrap_or(0) > 0, "NOOP must be persisted");
            }
        }

        /// Test circuit breaker on Sent tx without nonce → marks Failed
        #[tokio::test]
        async fn test_circuit_breaker_sent_without_nonce_marks_failed() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Sent);
            // No nonce assigned
            tx.network_data = NetworkTransactionData::Evm(EvmTransactionData {
                nonce: None,
                hash: None,
                raw: None,
                ..tx.network_data.get_evm_transaction_data().unwrap()
            });
            tx.sent_at = Some(Utc::now().to_rfc3339());

            // Should mark as Failed
            let tx_clone = tx.clone();
            mocks
                .tx_repo
                .expect_partial_update_if_version()
                .withf(|_, _, update| update.status == Some(TransactionStatus::Failed))
                .returning(move |_, _, update| {
                    let mut updated_tx = tx_clone.clone();
                    updated_tx.status = update.status.unwrap();
                    Ok(updated_tx)
                });

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);

            let ctx = StatusCheckContext::new(100, 200, 250, 15, 45, NetworkType::Evm);

            let result = evm_transaction
                .handle_circuit_breaker_safely(tx, &ctx)
                .await;
            assert!(result.is_ok());
            assert_eq!(result.unwrap().status, TransactionStatus::Failed);
        }

        /// Test circuit breaker on Pending tx → marks Failed (unchanged behavior)
        #[tokio::test]
        async fn test_circuit_breaker_pending_marks_failed() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let tx = make_test_transaction(TransactionStatus::Pending);

            // Should mark as Failed
            let tx_clone = tx.clone();
            mocks
                .tx_repo
                .expect_partial_update_if_version()
                .withf(|_, _, update| update.status == Some(TransactionStatus::Failed))
                .returning(move |_, _, update| {
                    let mut updated_tx = tx_clone.clone();
                    updated_tx.status = update.status.unwrap();
                    Ok(updated_tx)
                });

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);

            let ctx = StatusCheckContext::new(100, 200, 250, 15, 45, NetworkType::Evm);

            let result = evm_transaction
                .handle_circuit_breaker_safely(tx, &ctx)
                .await;
            assert!(result.is_ok());
            assert_eq!(result.unwrap().status, TransactionStatus::Failed);
        }
    }
    /// Issue #817 regression tests, ported from the characterization suite.
    mod issue_817_tests {
        use super::*;
        use crate::constants::EVM_MIN_HASHES_FOR_RECOVERY;
        use crate::domain::is_final_state;
        use crate::domain::transaction::evm::evm_transaction::TX_NONCE_RECONCILE_TRIGGER;
        use crate::domain::transaction::evm::PriceParams;
        use crate::domain::transaction::Transaction;
        use crate::jobs::StatusCheckContext;
        use crate::models::TransactionUpdateRequest;
        use crate::services::provider::ProviderError;
        use futures::future::ready;
        use std::collections::HashMap;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Mutex;

        const TX_NONCE: u64 = 5;

        type TestEvm = EvmRelayerTransaction<
            MockEvmProviderTrait,
            MockRelayerRepository,
            MockNetworkRepository,
            MockTransactionRepository,
            MockJobProducerTrait,
            MockSigner,
            MockTransactionCounterTrait,
            MockPriceCalculatorTrait,
        >;

        fn hinted_context() -> StatusCheckContext {
            let mut m = HashMap::new();
            m.insert(
                TX_NONCE_RECONCILE_TRIGGER.to_string(),
                "NonceTooLow".to_string(),
            );
            StatusCheckContext::default().with_job_metadata(Some(m))
        }

        /// Submitted tx with nonce 5. The last entry of `hashes` is the current hash.
        fn stuck_tx(hashes: &[&str], sent_ago: Duration) -> TransactionRepoModel {
            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.network_data = NetworkTransactionData::Evm(EvmTransactionData {
                nonce: Some(TX_NONCE),
                hash: hashes.last().map(|h| h.to_string()),
                raw: Some(vec![1, 2, 3]),
                ..tx.network_data.get_evm_transaction_data().unwrap()
            });
            tx.hashes = hashes.iter().map(|h| h.to_string()).collect();
            tx.created_at = (Utc::now() - Duration::minutes(30)).to_rfc3339();
            tx.sent_at = Some((Utc::now() - sent_ago).to_rfc3339());
            tx
        }

        fn apply_update(tx: &mut TransactionRepoModel, u: &TransactionUpdateRequest) {
            if let Some(v) = &u.status {
                tx.status = v.clone();
            }
            if let Some(v) = &u.status_reason {
                tx.status_reason = Some(v.clone());
            }
            if let Some(v) = &u.sent_at {
                tx.sent_at = Some(v.clone());
            }
            if let Some(v) = &u.network_data {
                tx.network_data = v.clone();
            }
            if let Some(v) = &u.hashes {
                tx.hashes = v.clone();
            }
            if let Some(v) = &u.metadata {
                tx.metadata = Some(v.clone());
            }
            if let Some(v) = &u.noop_count {
                tx.noop_count = Some(*v);
            }
            if let Some(v) = &u.priced_at {
                tx.priced_at = Some(v.clone());
            }
        }

        /// Counters observed by the simulated queue.
        #[derive(Clone, Default)]
        struct Counters {
            hinted_status_jobs: Arc<AtomicUsize>,
            plain_status_jobs: Arc<AtomicUsize>,
            resubmit_jobs: Arc<AtomicUsize>,
            sends: Arc<AtomicUsize>,
            failed_writes: Arc<AtomicUsize>,
        }

        /// Builds a relayer transaction against a "lagging pool": every receipt lookup
        /// returns Ok(None), every broadcast returns "nonce too low", and the nonce count
        /// is `on_chain_nonce`. The repository is a shared in-memory record.
        fn lagging_pool(
            tx: TransactionRepoModel,
            on_chain_nonce: u64,
        ) -> (TestEvm, Arc<Mutex<TransactionRepoModel>>, Counters) {
            let mut mocks = default_test_mocks_with_network();
            let state = Arc::new(Mutex::new(tx));
            let c = Counters::default();

            mocks
                .network_repo
                .expect_get_by_name()
                .returning(|_, _| Ok(None));
            mocks
                .provider
                .expect_get_block_number()
                .returning(|| Box::pin(async { Ok(100) }));
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|_| Box::pin(async { Ok(None) }));
            mocks
                .provider
                .expect_get_transaction_count()
                .returning(move |_| Box::pin(async move { Ok(on_chain_nonce) }));
            mocks.provider.expect_get_block_by_number().returning(|| {
                Box::pin(async { Err(ProviderError::Other("not mocked".to_string())) })
            });
            mocks
                .provider
                .expect_get_balance()
                .returning(|_| Box::pin(async { Ok(U256::from(10u128.pow(20))) }));
            let sends = c.sends.clone();
            mocks
                .provider
                .expect_send_raw_transaction()
                .returning(move |_| {
                    sends.fetch_add(1, Ordering::SeqCst);
                    Box::pin(async { Err(ProviderError::Other("nonce too low".to_string())) })
                });
            mocks
                .price_calc
                .expect_calculate_bumped_gas_price()
                .returning(|_, _, _| {
                    Box::pin(async {
                        Ok(PriceParams {
                            gas_price: Some(30_000_000_000),
                            max_fee_per_gas: None,
                            max_priority_fee_per_gas: None,
                            is_min_bumped: Some(true),
                            extra_fee: None,
                            total_cost: U256::from(1u64),
                        })
                    })
                });
            let sign_n = Arc::new(AtomicUsize::new(0));
            mocks.signer.expect_sign_transaction().returning(move |_| {
                let n = sign_n.fetch_add(1, Ordering::SeqCst);
                Box::pin(ready(Ok(
                    crate::domain::relayer::SignTransactionResponse::Evm(
                        crate::domain::relayer::SignTransactionResponseEvm {
                            hash: format!("0xresigned{n}"),
                            signature: crate::models::EvmTransactionDataSignature {
                                r: "r".to_string(),
                                s: "s".to_string(),
                                v: 1,
                                sig: "0xsig".to_string(),
                            },
                            raw: vec![4, 5, 6],
                        },
                    ),
                )))
            });

            let st = state.clone();
            let failed = c.failed_writes.clone();
            mocks
                .tx_repo
                .expect_partial_update_if_version()
                .returning(move |_, _, update| {
                    if update.status == Some(TransactionStatus::Failed) {
                        failed.fetch_add(1, Ordering::SeqCst);
                    }
                    let mut tx = st.lock().unwrap();
                    apply_update(&mut tx, &update);
                    Ok(tx.clone())
                });
            let st = state.clone();
            mocks
                .tx_repo
                .expect_get_by_id()
                .returning(move |_| Ok(st.lock().unwrap().clone()));

            let (hinted, plain) = (c.hinted_status_jobs.clone(), c.plain_status_jobs.clone());
            mocks
                .job_producer
                .expect_produce_check_transaction_status_job()
                .returning(move |job, _| {
                    let is_hinted = job
                        .metadata
                        .as_ref()
                        .is_some_and(|m| m.contains_key(TX_NONCE_RECONCILE_TRIGGER));
                    if is_hinted {
                        hinted.fetch_add(1, Ordering::SeqCst);
                    } else {
                        plain.fetch_add(1, Ordering::SeqCst);
                    }
                    Box::pin(async { Ok(()) })
                });
            let resubmits = c.resubmit_jobs.clone();
            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .returning(move |_, _| {
                    resubmits.fetch_add(1, Ordering::SeqCst);
                    Box::pin(async { Ok(()) })
                });
            mocks
                .job_producer
                .expect_produce_relayer_health_check_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let evm = make_test_evm_relayer_transaction(create_test_relayer(), mocks);
            (evm, state, c)
        }

        /// Runs `cycles` rounds of a simulated queue. Each round runs every live status
        /// job once (status jobs retry forever with NotYetFinal and keep their payload),
        /// then runs every resubmit job produced in that round. New hinted status jobs
        /// join the pool. Returns the number of live status jobs after each round.
        async fn simulate(
            evm: &TestEvm,
            state: &Arc<Mutex<TransactionRepoModel>>,
            c: &Counters,
            cycles: usize,
        ) -> Vec<usize> {
            // live status jobs: false = plain, true = hinted
            let mut live: Vec<bool> = vec![false];
            let mut sizes = Vec::new();
            for _ in 0..cycles {
                let resub_before = c.resubmit_jobs.load(Ordering::SeqCst);
                for hinted in live.clone() {
                    let cur = state.lock().unwrap().clone();
                    if is_final_state(&cur.status) {
                        break;
                    }
                    let ctx = if hinted {
                        hinted_context()
                    } else {
                        StatusCheckContext::default()
                    };
                    let _ = evm.handle_status_impl(cur, Some(ctx)).await;
                }
                let new_resubs = c.resubmit_jobs.load(Ordering::SeqCst) - resub_before;
                let hinted_before = c.hinted_status_jobs.load(Ordering::SeqCst);
                for _ in 0..new_resubs {
                    let cur = state.lock().unwrap().clone();
                    let _ = evm.resubmit_transaction(cur).await;
                }
                let new_hinted = c.hinted_status_jobs.load(Ordering::SeqCst) - hinted_before;
                live.extend(std::iter::repeat(true).take(new_hinted));
                if is_final_state(&state.lock().unwrap().status) {
                    live.clear();
                }
                sizes.push(live.len());
            }
            sizes
        }

        // ---------------------------------------------------------------- C1

        /// C1: the regular (unhinted) status check never marks Failed, even when the
        /// nonce is consumed and no receipt is visible. It resubmits instead.
        #[tokio::test]
        async fn c1_unhinted_check_never_fails_on_consumed_nonce() {
            let tx = stuck_tx(&["0xh1"], Duration::minutes(4));
            let (evm, state, c) = lagging_pool(tx, 10);

            for _ in 0..3 {
                let cur = state.lock().unwrap().clone();
                let out = evm
                    .handle_status_impl(cur, Some(StatusCheckContext::default()))
                    .await
                    .unwrap();
                assert_eq!(out.status, TransactionStatus::Submitted);
            }
            assert_eq!(c.failed_writes.load(Ordering::SeqCst), 0);
            assert_eq!(c.resubmit_jobs.load(Ordering::SeqCst), 3);
        }

        /// The first hinted check starts the block window without a resubmit.
        #[tokio::test]
        async fn c1_hinted_check_waits_on_first_observation() {
            let tx = stuck_tx(&["0xh1"], Duration::seconds(5));
            let (evm, _state, c) = lagging_pool(tx.clone(), 10);

            let out = evm
                .handle_status_impl(tx, Some(hinted_context()))
                .await
                .unwrap();
            assert_eq!(out.status, TransactionStatus::Submitted);
            assert_eq!(out.metadata.unwrap().nonce_used_seen_at_block, Some(100));
            assert_eq!(c.resubmit_jobs.load(Ordering::SeqCst), 0);
            assert_eq!(c.failed_writes.load(Ordering::SeqCst), 0);
        }

        /// C1 end to end: plain check -> resubmit -> "nonce too low" -> hinted check
        /// -> observation window, without further resubmits once the marker is stored.
        #[tokio::test]
        async fn c1_sequence_resubmit_nonce_too_low_then_wait() {
            let tx = stuck_tx(&["0xh1"], Duration::minutes(4));
            let (evm, state, c) = lagging_pool(tx, 10);

            let sizes = simulate(&evm, &state, &c, 3).await;
            assert_eq!(state.lock().unwrap().status, TransactionStatus::Submitted);
            assert_eq!(c.failed_writes.load(Ordering::SeqCst), 0);
            assert_eq!(sizes, vec![2, 3, 3]);
        }

        // ---------------------------------------------------------------- C2

        /// C2: the NonceTooLow branch of resubmit does not refresh sent_at and does
        /// not append a hash, so should_resubmit stays true right after it.
        #[tokio::test]
        async fn c2_nonce_too_low_resubmit_keeps_should_resubmit_true() {
            let tx = stuck_tx(&["0xh1"], Duration::minutes(4));
            let sent_before = tx.sent_at.clone();
            let (evm, state, c) = lagging_pool(tx.clone(), 10);

            evm.resubmit_transaction(tx).await.unwrap();

            let after = state.lock().unwrap().clone();
            assert_eq!(after.sent_at, sent_before, "sent_at not refreshed");
            assert_eq!(after.hashes, vec!["0xh1".to_string()], "no hash appended");
            assert_eq!(c.hinted_status_jobs.load(Ordering::SeqCst), 1);
            assert!(evm.should_resubmit(&after).await.unwrap());
        }

        /// C2 (fan-out): when the hinted reconcile defers (here: the pool's nonce count
        /// lags, so it reports the nonce as not consumed while send says "nonce too
        /// low"), every live status job resubmits and every resubmit adds one hinted
        /// status job. The pool doubles every cycle.
        ///
        /// KNOWN DEFECT, older than this change: the doubling is not intended. This
        /// test pins today's behaviour so a change to it is noticed; when the fan-out
        /// is fixed, replace the expected sizes with a bounded pool.
        #[tokio::test]
        async fn c2_status_job_fanout_when_reconcile_defers() {
            let tx = stuck_tx(&["0xh1"], Duration::minutes(4));
            let (evm, state, c) = lagging_pool(tx, TX_NONCE);

            let sizes = simulate(&evm, &state, &c, 5).await;
            assert_eq!(sizes, vec![2, 4, 8, 16, 32], "known fan-out defect");
            assert_eq!(c.failed_writes.load(Ordering::SeqCst), 0);
            let after = state.lock().unwrap().clone();
            assert_eq!(after.hashes.len(), 1, "attempt count never grows");
        }

        // ---------------------------------------------------------------- C3

        /// Reconciliation checks two hashes before the normal recovery age threshold.
        #[tokio::test]
        async fn c3_reconcile_ignores_recovery_age_threshold() {
            assert_eq!(EVM_MIN_HASHES_FOR_RECOVERY, 2);
            let mut mocks = default_test_mocks();
            mocks
                .provider
                .expect_get_transaction_count()
                .returning(|_| Box::pin(async { Ok(10) }));
            let mut tx = stuck_tx(&["0xold", "0xcur"], Duration::seconds(30));
            mocks
                .provider
                .expect_get_block_number()
                .returning(|| Box::pin(async { Ok(100) }));
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|h| {
                    let found = h == "0xold";
                    Box::pin(async move {
                        Ok(if found {
                            Some(make_mock_receipt(true, Some(100)))
                        } else {
                            None
                        })
                    })
                });
            let base = tx.clone();
            mocks
                .tx_repo
                .expect_partial_update_if_version()
                .withf(|_, _, u| u.status == Some(TransactionStatus::Mined))
                .times(1)
                .returning(move |_, _, u| {
                    let mut t = base.clone();
                    apply_update(&mut t, &u);
                    Ok(t)
                });
            let evm = make_test_evm_relayer_transaction(create_test_relayer(), mocks);
            let out = evm
                .reconcile_tx_nonce_state(&mut tx)
                .await
                .map(crate::domain::transaction::evm::status::NonceReconcile::handled)
                .unwrap()
                .unwrap();
            assert_eq!(out.status, TransactionStatus::Mined);
        }

        /// A historical receipt lookup error blocks the Failed verdict.
        #[tokio::test]
        async fn c3_historical_lookup_error_blocks_failed() {
            let mut mocks = default_test_mocks();
            let mut tx = stuck_tx(&["0xold", "0xcur"], Duration::seconds(30));
            tx.metadata = Some(crate::models::TransactionMetadata {
                nonce_used_seen_at_block: Some(1),
                nonce_used_seen_at: Some((Utc::now() - Duration::hours(2)).to_rfc3339()),
                ..Default::default()
            });
            mocks
                .provider
                .expect_get_block_number()
                .returning(|| Box::pin(async { Ok(100) }));
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|h| {
                    let err = h == "0xold";
                    Box::pin(async move {
                        if err {
                            Err(ProviderError::Other("timeout".to_string()))
                        } else {
                            Ok(None)
                        }
                    })
                });
            mocks
                .provider
                .expect_get_transaction_count()
                .returning(|_| Box::pin(async { Ok(10) }));
            mocks.tx_repo.expect_partial_update_if_version().times(0);
            let evm = make_test_evm_relayer_transaction(create_test_relayer(), mocks);
            let out = evm
                .reconcile_tx_nonce_state(&mut tx)
                .await
                .map(crate::domain::transaction::evm::status::NonceReconcile::handled)
                .unwrap()
                .unwrap();
            assert_eq!(out.status, TransactionStatus::Submitted);
        }

        /// A replacement must check the original even with only one saved hash.
        #[tokio::test]
        async fn replaced_tx_with_one_recorded_hash_recovers_mined_original() {
            let mut mocks = default_test_mocks();
            let mut tx = stuck_tx(&["0xoriginal"], Duration::seconds(30));
            // replace_transaction wrote a new signed payload; hashes is unchanged.
            let mut data = tx.network_data.get_evm_transaction_data().unwrap();
            data.hash = Some("0xreplacement".to_string());
            tx.network_data = NetworkTransactionData::Evm(data);

            let checked = Arc::new(Mutex::new(Vec::<String>::new()));
            let checked_c = checked.clone();
            mocks
                .provider
                .expect_get_block_number()
                .returning(|| Box::pin(async { Ok(100) }));
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(move |h| {
                    checked_c.lock().unwrap().push(h.to_string());
                    let mined = h == "0xoriginal";
                    Box::pin(async move {
                        Ok(if mined {
                            Some(make_mock_receipt(true, Some(100)))
                        } else {
                            None
                        })
                    })
                });
            mocks
                .provider
                .expect_get_transaction_count()
                .returning(|_| Box::pin(async { Ok(TX_NONCE + 1) }));
            let base = tx.clone();
            mocks
                .tx_repo
                .expect_partial_update_if_version()
                .returning(move |_, _, u| {
                    let mut t = base.clone();
                    apply_update(&mut t, &u);
                    Ok(t)
                });
            mocks
                .job_producer
                .expect_produce_relayer_health_check_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));
            let evm = make_test_evm_relayer_transaction(create_test_relayer(), mocks);
            let out = evm
                .reconcile_tx_nonce_state(&mut tx)
                .await
                .map(crate::domain::transaction::evm::status::NonceReconcile::handled)
                .unwrap()
                .unwrap();
            assert_eq!(
                *checked.lock().unwrap(),
                vec!["0xreplacement".to_string(), "0xoriginal".to_string()],
                "the saved original hash must also be checked"
            );
            assert_eq!(out.status, TransactionStatus::Mined);
        }
    }
    mod nonce_used_span_tests {
        use super::*;
        use crate::models::TransactionMetadata;
        use mockall::Sequence;
        use std::sync::{
            atomic::{AtomicBool, AtomicU64, Ordering},
            Mutex,
        };

        #[test]
        fn span_covers_confirmation_depth_floor_and_one_minute() {
            use super::super::nonce_used_failure_span;
            for (confirmations, blocktime, expected) in [
                (12, 12_000, 24),
                (1, 12_000, 12),
                (1, 260, 231),
                (1, 2_000, 30),
                (20, 0, 40),
                (1, 0, 12),
                (u64::MAX, 1, u64::MAX),
            ] {
                assert_eq!(nonce_used_failure_span(confirmations, blocktime), expected);
            }
        }

        fn transaction() -> TransactionRepoModel {
            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            if let NetworkTransactionData::Evm(ref mut data) = tx.network_data {
                data.nonce = Some(5);
                data.hash = Some("0xcurrent".to_string());
            }
            tx.hashes = vec!["0xcurrent".to_string()];
            tx.sent_at = Some((Utc::now() - Duration::minutes(4)).to_rfc3339());
            tx.metadata = Some(TransactionMetadata {
                total_failures: 7,
                ..Default::default()
            });
            tx
        }

        fn repository(
            mocks: &mut TestMocks,
            tx: &TransactionRepoModel,
        ) -> Arc<Mutex<TransactionRepoModel>> {
            let state = Arc::new(Mutex::new(tx.clone()));
            let updated = state.clone();
            mocks.tx_repo.expect_partial_update_if_version().returning(
                move |_, expected, update| {
                    let mut tx = updated.lock().unwrap();
                    if tx.version != expected {
                        return Err(crate::models::RepositoryError::ConcurrentUpdateConflict(
                            tx.id.clone(),
                        ));
                    }
                    if let Some(status) = update.status {
                        tx.status = status;
                    }
                    if let Some(metadata) = update.metadata {
                        tx.metadata = Some(metadata);
                    }
                    if let Some(data) = update.network_data {
                        tx.network_data = data;
                    }
                    tx.version += 1;
                    Ok(tx.clone())
                },
            );
            mocks.tx_repo.expect_get_by_id_on_primary().returning({
                let state = state.clone();
                move |_| Ok(state.lock().unwrap().clone())
            });
            state
        }

        fn seen_at(state: &Arc<Mutex<TransactionRepoModel>>) -> Option<u64> {
            state
                .lock()
                .unwrap()
                .metadata
                .as_ref()
                .unwrap()
                .nonce_used_seen_at_block
        }

        fn reads(mocks: &mut TestMocks, block: Arc<AtomicU64>, receipt_visible: Arc<AtomicBool>) {
            mocks
                .provider
                .expect_get_transaction_count()
                .returning(|_| Box::pin(async { Ok(6) }));
            mocks.provider.expect_get_block_number().returning(move || {
                let head = block.load(Ordering::SeqCst);
                Box::pin(async move { Ok(head) })
            });
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(move |_| {
                    let receipt = receipt_visible
                        .load(Ordering::SeqCst)
                        .then(|| make_mock_receipt(true, Some(100)));
                    Box::pin(async move { Ok(receipt) })
                });
        }

        #[tokio::test]
        async fn used_nonce_waits_until_span_without_resubmitting() {
            let mut tx = transaction();
            let mut mocks = default_test_mocks_with_network();
            let block = Arc::new(AtomicU64::new(100));
            reads(&mut mocks, block.clone(), Arc::new(AtomicBool::new(false)));
            let state = repository(&mut mocks, &tx);
            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .times(0);
            mocks
                .job_producer
                .expect_produce_relayer_health_check_job()
                .times(2)
                .returning(|_, _| Box::pin(async { Ok(()) }));
            let evm = make_test_evm_relayer_transaction(create_test_relayer(), mocks);
            let mut first = evm
                .reconcile_tx_nonce_state(&mut tx)
                .await
                .map(crate::domain::transaction::evm::status::NonceReconcile::handled)
                .unwrap()
                .unwrap();
            assert_eq!(first.status, TransactionStatus::Submitted);
            assert_eq!(
                first.metadata.as_ref().unwrap().nonce_used_seen_at_block,
                Some(100)
            );
            assert!(first
                .metadata
                .as_ref()
                .unwrap()
                .nonce_used_seen_at
                .is_some());
            assert_eq!(first.metadata.as_ref().unwrap().total_failures, 7);
            // Pass the wall-clock minimum so only the block span decides below.
            let long_ago = Some((Utc::now() - Duration::hours(2)).to_rfc3339());
            first.metadata.as_mut().unwrap().nonce_used_seen_at = long_ago.clone();
            state
                .lock()
                .unwrap()
                .metadata
                .as_mut()
                .unwrap()
                .nonce_used_seen_at = long_ago;
            block.store(123, Ordering::SeqCst);
            let before_span = evm.handle_status_impl(first, None).await.unwrap();
            assert_eq!(before_span.status, TransactionStatus::Submitted);
            block.store(124, Ordering::SeqCst);
            let at_span = evm.handle_status_impl(before_span, None).await.unwrap();
            assert_eq!(at_span.status, TransactionStatus::Failed);
        }

        #[tokio::test]
        async fn receipt_between_sightings_confirms() {
            let mut tx = transaction();
            let mut mocks = default_test_mocks_with_network();
            let block = Arc::new(AtomicU64::new(100));
            let visible = Arc::new(AtomicBool::new(false));
            reads(&mut mocks, block.clone(), visible.clone());
            repository(&mut mocks, &tx);
            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .times(0);
            mocks
                .job_producer
                .expect_produce_relayer_health_check_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));
            let evm = make_test_evm_relayer_transaction(create_test_relayer(), mocks);
            let first = evm
                .reconcile_tx_nonce_state(&mut tx)
                .await
                .map(crate::domain::transaction::evm::status::NonceReconcile::handled)
                .unwrap()
                .unwrap();
            block.store(124, Ordering::SeqCst);
            visible.store(true, Ordering::SeqCst);
            let result = evm.handle_status_impl(first, None).await.unwrap();
            assert_eq!(result.status, TransactionStatus::Confirmed);
        }

        #[tokio::test]
        async fn reads_nonce_then_block_then_every_saved_hash() {
            let mut tx = transaction();
            tx.hashes = vec!["0xoriginal".to_string(), "0xcurrent".to_string()];
            let mut mocks = default_test_mocks();
            let mut sequence = Sequence::new();
            mocks
                .provider
                .expect_get_transaction_count()
                .times(1)
                .in_sequence(&mut sequence)
                .returning(|_| Box::pin(async { Ok(6) }));
            mocks
                .provider
                .expect_get_block_number()
                .times(1)
                .in_sequence(&mut sequence)
                .returning(|| Box::pin(async { Ok(100) }));
            for hash in ["0xcurrent", "0xoriginal"] {
                mocks
                    .provider
                    .expect_get_transaction_receipt()
                    .withf(move |h| h == hash)
                    .times(1)
                    .in_sequence(&mut sequence)
                    .returning(|_| Box::pin(async { Ok(None) }));
            }
            mocks
                .job_producer
                .expect_produce_relayer_health_check_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));
            repository(&mut mocks, &tx);
            let evm = make_test_evm_relayer_transaction(create_test_relayer(), mocks);
            let result = evm
                .reconcile_tx_nonce_state(&mut tx)
                .await
                .map(crate::domain::transaction::evm::status::NonceReconcile::handled)
                .unwrap()
                .unwrap();
            assert_eq!(result.status, TransactionStatus::Submitted);
        }

        #[tokio::test]
        async fn current_lookup_error_after_span_never_fails() {
            let mut tx = transaction();
            tx.metadata.as_mut().unwrap().nonce_used_seen_at_block = Some(1);
            tx.metadata.as_mut().unwrap().nonce_used_seen_at =
                Some((Utc::now() - Duration::hours(2)).to_rfc3339());
            let mut mocks = default_test_mocks();
            mocks
                .provider
                .expect_get_transaction_count()
                .returning(|_| Box::pin(async { Ok(6) }));
            mocks
                .provider
                .expect_get_block_number()
                .returning(|| Box::pin(async { Ok(100) }));
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|_| {
                    Box::pin(async { Err(crate::services::provider::ProviderError::Timeout) })
                });
            mocks.tx_repo.expect_partial_update_if_version().times(0);
            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .times(0);
            let evm = make_test_evm_relayer_transaction(create_test_relayer(), mocks);
            assert_eq!(
                evm.handle_status_impl(tx, None).await.unwrap().status,
                TransactionStatus::Submitted
            );
        }

        #[tokio::test]
        async fn recovered_hash_write_error_stops_status_handling() {
            let mut tx = transaction();
            tx.hashes = vec!["0xoriginal".to_string()];
            let mut mocks = default_test_mocks();
            mocks
                .provider
                .expect_get_transaction_count()
                .returning(|_| Box::pin(async { Ok(6) }));
            mocks
                .provider
                .expect_get_block_number()
                .returning(|| Box::pin(async { Ok(100) }));
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|hash| {
                    let receipt =
                        (hash == "0xoriginal").then(|| make_mock_receipt(true, Some(100)));
                    Box::pin(async move { Ok(receipt) })
                });
            mocks
                .tx_repo
                .expect_partial_update_if_version()
                .times(1)
                .returning(|_, _, _| {
                    Err(crate::models::RepositoryError::ConnectionError(
                        "redis down".to_string(),
                    ))
                });
            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .times(0);
            let evm = make_test_evm_relayer_transaction(create_test_relayer(), mocks);
            let mut metadata = std::collections::HashMap::new();
            metadata.insert(
                super::super::TX_NONCE_RECONCILE_TRIGGER.to_string(),
                "NonceTooLow".to_string(),
            );
            let context =
                crate::jobs::StatusCheckContext::default().with_job_metadata(Some(metadata));
            assert!(evm.handle_status_impl(tx, Some(context)).await.is_err());
        }

        #[tokio::test]
        async fn mined_sent_transaction_is_found() {
            let mut tx = transaction();
            tx.status = TransactionStatus::Sent;
            let mut mocks = default_test_mocks_with_network();
            reads(
                &mut mocks,
                Arc::new(AtomicU64::new(124)),
                Arc::new(AtomicBool::new(true)),
            );
            repository(&mut mocks, &tx);
            let evm = make_test_evm_relayer_transaction(create_test_relayer(), mocks);
            assert_eq!(
                evm.handle_status_impl(tx, None).await.unwrap().status,
                TransactionStatus::Confirmed
            );
        }

        #[tokio::test]
        async fn regular_check_recovers_one_saved_original_hash() {
            let mut tx = transaction();
            tx.hashes = vec!["0xoriginal".to_string()];
            tx.sent_at = Some(Utc::now().to_rfc3339());
            let mut mocks = default_test_mocks();
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|hash| {
                    let receipt =
                        (hash == "0xoriginal").then(|| make_mock_receipt(true, Some(100)));
                    Box::pin(async move { Ok(receipt) })
                });
            repository(&mut mocks, &tx);
            let evm = make_test_evm_relayer_transaction(create_test_relayer(), mocks);
            assert_eq!(
                evm.check_transaction_status(&tx).await.unwrap(),
                TransactionStatus::Mined
            );
        }

        #[tokio::test]
        async fn sent_checks_all_saved_hashes() {
            let mut tx = transaction();
            tx.status = TransactionStatus::Sent;
            tx.hashes = vec!["0xoriginal".to_string(), "0xcurrent".to_string()];
            let mut mocks = default_test_mocks();
            mocks
                .provider
                .expect_get_transaction_receipt()
                .times(2)
                .returning(|hash| {
                    let receipt =
                        (hash == "0xoriginal").then(|| make_mock_receipt(true, Some(100)));
                    Box::pin(async move { Ok(receipt) })
                });
            repository(&mut mocks, &tx);
            let evm = make_test_evm_relayer_transaction(create_test_relayer(), mocks);
            assert_eq!(
                evm.check_transaction_status(&tx).await.unwrap(),
                TransactionStatus::Mined
            );
        }

        #[tokio::test]
        async fn missing_blocktime_holds_without_verdict() {
            let mut tx = transaction();
            tx.metadata.as_mut().unwrap().nonce_used_seen_at_block = Some(100);
            tx.metadata.as_mut().unwrap().nonce_used_seen_at =
                Some((Utc::now() - Duration::hours(2)).to_rfc3339());
            let mut mocks = default_test_mocks();
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| {
                    let mut model = create_test_network_model();
                    if let NetworkConfigData::Evm(ref mut config) = model.config {
                        config.common.average_blocktime_ms = None;
                    }
                    Ok(Some(model))
                });
            reads(
                &mut mocks,
                Arc::new(AtomicU64::new(10_000)),
                Arc::new(AtomicBool::new(false)),
            );
            mocks.tx_repo.expect_partial_update_if_version().times(0);
            let evm = make_test_evm_relayer_transaction(create_test_relayer(), mocks);
            let held = evm
                .reconcile_tx_nonce_state(&mut tx)
                .await
                .map(crate::domain::transaction::evm::status::NonceReconcile::handled)
                .unwrap()
                .unwrap();
            assert_eq!(held.status, TransactionStatus::Submitted);
        }

        /// #991 review: the block span alone is about 60 s on Base; #817 saw a
        /// 45-minute receipt lag. Failed also needs the wall-clock minimum.
        #[tokio::test]
        async fn block_span_without_wall_clock_minimum_keeps_waiting() {
            let mut tx = transaction();
            tx.metadata.as_mut().unwrap().nonce_used_seen_at_block = Some(100);
            tx.metadata.as_mut().unwrap().nonce_used_seen_at = Some(
                (Utc::now() - crate::constants::get_evm_nonce_used_failure_min_age()
                    + Duration::minutes(1))
                .to_rfc3339(),
            );
            let mut mocks = default_test_mocks_with_network();
            reads(
                &mut mocks,
                Arc::new(AtomicU64::new(10_000)),
                Arc::new(AtomicBool::new(false)),
            );
            mocks.tx_repo.expect_partial_update_if_version().times(0);
            let evm = make_test_evm_relayer_transaction(create_test_relayer(), mocks);
            let result = evm
                .reconcile_tx_nonce_state(&mut tx)
                .await
                .map(crate::domain::transaction::evm::status::NonceReconcile::handled)
                .unwrap()
                .unwrap();
            assert_eq!(result.status, TransactionStatus::Submitted);
        }

        #[tokio::test]
        async fn wall_clock_minimum_without_block_span_keeps_waiting() {
            let mut tx = transaction();
            tx.metadata.as_mut().unwrap().nonce_used_seen_at_block = Some(100);
            tx.metadata.as_mut().unwrap().nonce_used_seen_at =
                Some((Utc::now() - Duration::hours(2)).to_rfc3339());
            let mut mocks = default_test_mocks_with_network();
            reads(
                &mut mocks,
                Arc::new(AtomicU64::new(123)),
                Arc::new(AtomicBool::new(false)),
            );
            mocks.tx_repo.expect_partial_update_if_version().times(0);
            let evm = make_test_evm_relayer_transaction(create_test_relayer(), mocks);
            let result = evm
                .reconcile_tx_nonce_state(&mut tx)
                .await
                .map(crate::domain::transaction::evm::status::NonceReconcile::handled)
                .unwrap()
                .unwrap();
            assert_eq!(result.status, TransactionStatus::Submitted);
        }

        /// A sighting saved without its time restarts the wait instead of failing.
        #[tokio::test]
        async fn sighting_without_time_restarts_wait() {
            let mut tx = transaction();
            tx.metadata.as_mut().unwrap().nonce_used_seen_at_block = Some(1);
            let mut mocks = default_test_mocks_with_network();
            reads(
                &mut mocks,
                Arc::new(AtomicU64::new(10_000)),
                Arc::new(AtomicBool::new(false)),
            );
            mocks
                .job_producer
                .expect_produce_relayer_health_check_job()
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(()) }));
            let state = repository(&mut mocks, &tx);
            let evm = make_test_evm_relayer_transaction(create_test_relayer(), mocks);
            let result = evm
                .reconcile_tx_nonce_state(&mut tx)
                .await
                .map(crate::domain::transaction::evm::status::NonceReconcile::handled)
                .unwrap()
                .unwrap();
            assert_eq!(result.status, TransactionStatus::Submitted);
            assert_eq!(seen_at(&state), Some(10_000));
            assert!(state
                .lock()
                .unwrap()
                .metadata
                .as_ref()
                .unwrap()
                .nonce_used_seen_at
                .is_some());
        }

        /// A head that stops advancing ends the wait after the maximum age.
        #[tokio::test]
        async fn stalled_head_fails_after_maximum_age() {
            let mut tx = transaction();
            tx.metadata.as_mut().unwrap().nonce_used_seen_at_block = Some(100);
            tx.metadata.as_mut().unwrap().nonce_used_seen_at = Some(
                (Utc::now()
                    - crate::constants::get_evm_nonce_used_failure_max_age()
                    - Duration::minutes(1))
                .to_rfc3339(),
            );
            let mut mocks = default_test_mocks_with_network();
            reads(
                &mut mocks,
                Arc::new(AtomicU64::new(100)),
                Arc::new(AtomicBool::new(false)),
            );
            repository(&mut mocks, &tx);
            mocks
                .job_producer
                .expect_produce_relayer_health_check_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));
            let evm = make_test_evm_relayer_transaction(create_test_relayer(), mocks);
            let result = evm
                .reconcile_tx_nonce_state(&mut tx)
                .await
                .map(crate::domain::transaction::evm::status::NonceReconcile::handled)
                .unwrap()
                .unwrap();
            assert_eq!(result.status, TransactionStatus::Failed);
        }

        /// Records that another job already wrote `status`, one version ahead of `tx`.
        fn written_by_other_job(state: &Mutex<TransactionRepoModel>, status: TransactionStatus) {
            let mut stored = state.lock().unwrap();
            stored.status = status;
            stored.version += 1;
        }

        /// A stale job's Failed verdict must not overwrite another job's Mined.
        #[tokio::test]
        async fn stale_failed_verdict_does_not_overwrite_mined() {
            let mut tx = transaction();
            let metadata = tx.metadata.as_mut().unwrap();
            metadata.nonce_used_seen_at_block = Some(100);
            metadata.nonce_used_seen_at = Some(
                (Utc::now()
                    - crate::constants::get_evm_nonce_used_failure_max_age()
                    - Duration::minutes(1))
                .to_rfc3339(),
            );
            let mut mocks = default_test_mocks_with_network();
            reads(
                &mut mocks,
                Arc::new(AtomicU64::new(100)),
                Arc::new(AtomicBool::new(false)),
            );
            let state = repository(&mut mocks, &tx);
            written_by_other_job(&state, TransactionStatus::Mined);
            mocks
                .job_producer
                .expect_produce_relayer_health_check_job()
                .times(0);
            let evm = make_test_evm_relayer_transaction(create_test_relayer(), mocks);
            let err = evm.reconcile_tx_nonce_state(&mut tx).await.unwrap_err();
            assert!(err.is_concurrent_update_conflict());
            let stored = state.lock().unwrap();
            assert_eq!(stored.status, TransactionStatus::Mined);
            assert_eq!(stored.status_reason, None);
        }

        /// A stale first sighting must not overwrite another job's Mined record.
        #[tokio::test]
        async fn stale_first_sighting_does_not_overwrite_mined() {
            let mut tx = transaction();
            let mut mocks = default_test_mocks_with_network();
            reads(
                &mut mocks,
                Arc::new(AtomicU64::new(100)),
                Arc::new(AtomicBool::new(false)),
            );
            let state = repository(&mut mocks, &tx);
            written_by_other_job(&state, TransactionStatus::Mined);
            mocks
                .job_producer
                .expect_produce_relayer_health_check_job()
                .times(0);
            let evm = make_test_evm_relayer_transaction(create_test_relayer(), mocks);
            let err = evm.reconcile_tx_nonce_state(&mut tx).await.unwrap_err();
            assert!(err.is_concurrent_update_conflict());
            let stored = state.lock().unwrap();
            assert_eq!(stored.status, TransactionStatus::Mined);
            assert!(!super::super::has_nonce_used_seen(&stored));
        }

        /// A stale job that finds the receipt clears its sighting first; once another
        /// job wrote Failed, that write is refused, so no Mined overwrites it either.
        #[tokio::test]
        async fn stale_receipt_does_not_overwrite_failed() {
            let mut tx = transaction();
            let metadata = tx.metadata.as_mut().unwrap();
            metadata.nonce_used_seen_at_block = Some(100);
            metadata.nonce_used_seen_at = Some(Utc::now().to_rfc3339());
            let mut mocks = default_test_mocks_with_network();
            reads(
                &mut mocks,
                Arc::new(AtomicU64::new(124)),
                Arc::new(AtomicBool::new(true)),
            );
            let state = repository(&mut mocks, &tx);
            written_by_other_job(&state, TransactionStatus::Failed);
            mocks
                .job_producer
                .expect_produce_relayer_health_check_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));
            let evm = make_test_evm_relayer_transaction(create_test_relayer(), mocks);
            let err = evm.handle_status_impl(tx, None).await.unwrap_err();
            assert!(err.is_concurrent_update_conflict());
            assert_eq!(state.lock().unwrap().status, TransactionStatus::Failed);
        }

        /// Clearing the sighting must not make the same job's Mined write conflict.
        #[tokio::test]
        async fn receipt_after_sighting_confirms_in_one_check() {
            let mut tx = transaction();
            let metadata = tx.metadata.as_mut().unwrap();
            metadata.nonce_used_seen_at_block = Some(100);
            metadata.nonce_used_seen_at = Some(Utc::now().to_rfc3339());
            let mut mocks = default_test_mocks_with_network();
            reads(
                &mut mocks,
                Arc::new(AtomicU64::new(124)),
                Arc::new(AtomicBool::new(true)),
            );
            let state = repository(&mut mocks, &tx);
            let evm = make_test_evm_relayer_transaction(create_test_relayer(), mocks);
            let result = evm.handle_status_impl(tx, None).await.unwrap();
            assert_eq!(result.status, TransactionStatus::Confirmed);
            let stored = state.lock().unwrap();
            assert_eq!(stored.status, TransactionStatus::Confirmed);
            assert!(!super::super::has_nonce_used_seen(&stored));
        }

        fn timeout() -> crate::services::provider::ProviderError {
            crate::services::provider::ProviderError::Timeout
        }

        /// #991 review: without a recorded sighting, a failed nonce or block read
        /// returns to the normal flow, which still reads receipts.
        #[tokio::test]
        async fn nonce_read_error_without_sighting_uses_normal_flow() {
            let mut tx = transaction();
            let mut mocks = default_test_mocks();
            mocks
                .provider
                .expect_get_transaction_count()
                .returning(|_| Box::pin(async { Err(timeout()) }));
            mocks.provider.expect_get_block_number().times(0);
            mocks.provider.expect_get_transaction_receipt().times(0);
            mocks.tx_repo.expect_partial_update_if_version().times(0);
            let evm = make_test_evm_relayer_transaction(create_test_relayer(), mocks);
            assert!(evm
                .reconcile_tx_nonce_state(&mut tx)
                .await
                .map(crate::domain::transaction::evm::status::NonceReconcile::handled)
                .unwrap()
                .is_none());
        }

        #[tokio::test]
        async fn block_read_error_without_sighting_uses_normal_flow() {
            let mut tx = transaction();
            let mut mocks = default_test_mocks();
            mocks
                .provider
                .expect_get_transaction_count()
                .returning(|_| Box::pin(async { Ok(6) }));
            mocks
                .provider
                .expect_get_block_number()
                .returning(|| Box::pin(async { Err(timeout()) }));
            mocks.provider.expect_get_transaction_receipt().times(0);
            mocks.tx_repo.expect_partial_update_if_version().times(0);
            let evm = make_test_evm_relayer_transaction(create_test_relayer(), mocks);
            assert!(evm
                .reconcile_tx_nonce_state(&mut tx)
                .await
                .map(crate::domain::transaction::evm::status::NonceReconcile::handled)
                .unwrap()
                .is_none());
        }

        /// During the wait the nonce is known to be used: a failed read still checks
        /// receipts but gives no verdict and keeps the normal flow (resubmit, NOOP) out.
        #[tokio::test]
        async fn read_errors_during_wait_hold_after_checking_receipts() {
            for nonce_fails in [true, false] {
                let mut tx = transaction();
                tx.metadata.as_mut().unwrap().nonce_used_seen_at_block = Some(1);
                tx.metadata.as_mut().unwrap().nonce_used_seen_at =
                    Some((Utc::now() - Duration::days(2)).to_rfc3339());
                let mut mocks = default_test_mocks();
                mocks
                    .provider
                    .expect_get_transaction_count()
                    .returning(move |_| {
                        Box::pin(async move {
                            if nonce_fails {
                                Err(timeout())
                            } else {
                                Ok(6)
                            }
                        })
                    });
                mocks
                    .provider
                    .expect_get_block_number()
                    .times(usize::from(!nonce_fails))
                    .returning(|| Box::pin(async { Err(timeout()) }));
                mocks
                    .provider
                    .expect_get_transaction_receipt()
                    .times(1)
                    .returning(|_| Box::pin(async { Ok(None) }));
                mocks.tx_repo.expect_partial_update_if_version().times(0);
                let evm = make_test_evm_relayer_transaction(create_test_relayer(), mocks);
                let held = evm
                    .reconcile_tx_nonce_state(&mut tx)
                    .await
                    .map(crate::domain::transaction::evm::status::NonceReconcile::handled)
                    .unwrap()
                    .unwrap();
                assert_eq!(held.status, TransactionStatus::Submitted);
            }
        }

        #[tokio::test]
        async fn receipt_during_wait_recovers_despite_nonce_read_error() {
            let mut tx = transaction();
            tx.metadata.as_mut().unwrap().nonce_used_seen_at_block = Some(1);
            tx.metadata.as_mut().unwrap().nonce_used_seen_at =
                Some((Utc::now() - Duration::days(2)).to_rfc3339());
            let mut mocks = default_test_mocks();
            mocks
                .provider
                .expect_get_transaction_count()
                .returning(|_| Box::pin(async { Err(timeout()) }));
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|_| Box::pin(async { Ok(Some(make_mock_receipt(true, Some(100)))) }));
            let state = repository(&mut mocks, &tx);
            let evm = make_test_evm_relayer_transaction(create_test_relayer(), mocks);
            assert!(evm
                .reconcile_tx_nonce_state(&mut tx)
                .await
                .map(crate::domain::transaction::evm::status::NonceReconcile::handled)
                .unwrap()
                .is_none());
            assert_eq!(seen_at(&state), None);
        }

        /// The head is past the sighting but a second nonce read shows the nonce used:
        /// the first answer was stale, so hold and keep the sighting.
        #[tokio::test]
        async fn second_nonce_read_must_confirm_unused_nonce() {
            let mut tx = transaction();
            tx.metadata.as_mut().unwrap().nonce_used_seen_at_block = Some(100);
            tx.metadata.as_mut().unwrap().nonce_used_seen_at =
                Some((Utc::now() - Duration::minutes(5)).to_rfc3339());
            let mut mocks = default_test_mocks();
            let mut sequence = mockall::Sequence::new();
            mocks
                .provider
                .expect_get_transaction_count()
                .times(1)
                .in_sequence(&mut sequence)
                .returning(|_| Box::pin(async { Ok(5) }));
            mocks
                .provider
                .expect_get_block_number()
                .times(1)
                .in_sequence(&mut sequence)
                .returning(|| Box::pin(async { Ok(101) }));
            mocks
                .provider
                .expect_get_transaction_count()
                .times(1)
                .in_sequence(&mut sequence)
                .returning(|_| Box::pin(async { Ok(6) }));
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|_| Box::pin(async { Ok(None) }));
            let state = repository(&mut mocks, &tx);
            let evm = make_test_evm_relayer_transaction(create_test_relayer(), mocks);
            let held = evm
                .reconcile_tx_nonce_state(&mut tx)
                .await
                .map(crate::domain::transaction::evm::status::NonceReconcile::handled)
                .unwrap()
                .unwrap();
            assert_eq!(held.status, TransactionStatus::Submitted);
            assert_eq!(seen_at(&state), Some(100));
        }

        /// A failed head read cannot confirm an unused nonce: hold. After the maximum
        /// age the unused nonce is trusted, so the hold cannot last forever.
        #[tokio::test]
        async fn unconfirmed_unused_nonce_holds_until_maximum_age() {
            for (age, clears) in [(Duration::minutes(5), false), (Duration::days(2), true)] {
                let mut tx = transaction();
                tx.metadata.as_mut().unwrap().nonce_used_seen_at_block = Some(100);
                tx.metadata.as_mut().unwrap().nonce_used_seen_at =
                    Some((Utc::now() - age).to_rfc3339());
                let mut mocks = default_test_mocks();
                mocks
                    .provider
                    .expect_get_transaction_count()
                    .returning(|_| Box::pin(async { Ok(5) }));
                mocks
                    .provider
                    .expect_get_block_number()
                    .returning(|| Box::pin(async { Err(timeout()) }));
                mocks
                    .provider
                    .expect_get_transaction_receipt()
                    .returning(|_| Box::pin(async { Ok(None) }));
                let state = repository(&mut mocks, &tx);
                let evm = make_test_evm_relayer_transaction(create_test_relayer(), mocks);
                let result = evm
                    .reconcile_tx_nonce_state(&mut tx)
                    .await
                    .map(crate::domain::transaction::evm::status::NonceReconcile::handled)
                    .unwrap();
                assert_eq!(result.is_none(), clears);
                assert_eq!(seen_at(&state).is_none(), clears);
            }
        }

        /// An unused-nonce answer from a node behind the sighting's head is lag, not a
        /// reorg: hold (after checking receipts) instead of clearing the sighting.
        #[tokio::test]
        async fn lagging_unused_nonce_during_wait_holds() {
            for (head, clears) in [(99_u64, false), (100, true)] {
                let mut tx = transaction();
                tx.metadata.as_mut().unwrap().nonce_used_seen_at_block = Some(100);
                tx.metadata.as_mut().unwrap().nonce_used_seen_at =
                    Some((Utc::now() - Duration::minutes(5)).to_rfc3339());
                let mut mocks = default_test_mocks();
                mocks
                    .provider
                    .expect_get_transaction_count()
                    .returning(|_| Box::pin(async { Ok(5) }));
                mocks
                    .provider
                    .expect_get_block_number()
                    .times(1)
                    .returning(move || Box::pin(async move { Ok(head) }));
                mocks
                    .provider
                    .expect_get_transaction_receipt()
                    .times(usize::from(!clears))
                    .returning(|_| Box::pin(async { Ok(None) }));
                let state = repository(&mut mocks, &tx);
                let evm = make_test_evm_relayer_transaction(create_test_relayer(), mocks);
                let result = evm
                    .reconcile_tx_nonce_state(&mut tx)
                    .await
                    .map(crate::domain::transaction::evm::status::NonceReconcile::handled)
                    .unwrap();
                if clears {
                    assert!(result.is_none());
                    assert_eq!(seen_at(&state), None);
                } else {
                    assert_eq!(result.unwrap().status, TransactionStatus::Submitted);
                    assert_eq!(seen_at(&state), Some(100));
                }
            }
        }

        /// #991 review: a Sent check must not count an RPC outage as a failure.
        #[tokio::test]
        async fn sent_receipt_lookup_error_keeps_sent() {
            let mut tx = transaction();
            tx.status = TransactionStatus::Sent;
            let mut mocks = default_test_mocks();
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|_| {
                    Box::pin(async { Err(crate::services::provider::ProviderError::Timeout) })
                });
            let evm = make_test_evm_relayer_transaction(create_test_relayer(), mocks);
            assert_eq!(
                evm.check_transaction_status(&tx).await.unwrap(),
                TransactionStatus::Sent
            );
        }

        /// A receipt proves the transaction mined: a failed head read gives Mined, not
        /// an error that counts toward the circuit breaker.
        #[tokio::test]
        async fn block_read_error_after_receipt_gives_mined() {
            for status in [TransactionStatus::Sent, TransactionStatus::Submitted] {
                let mut tx = transaction();
                tx.status = status;
                let mut mocks = default_test_mocks();
                mocks
                    .provider
                    .expect_get_transaction_receipt()
                    .returning(|_| {
                        Box::pin(async { Ok(Some(make_mock_receipt(true, Some(100)))) })
                    });
                mocks
                    .provider
                    .expect_get_block_number()
                    .returning(|| Box::pin(async { Err(timeout()) }));
                let evm = make_test_evm_relayer_transaction(create_test_relayer(), mocks);
                assert_eq!(
                    evm.check_transaction_status(&tx).await.unwrap(),
                    TransactionStatus::Mined
                );
            }
        }

        /// #991 review (5): the receipt reconciliation found is evaluated directly. A
        /// second read could hit a lagging node, return null and resubmit a mined tx.
        #[tokio::test]
        async fn receipt_found_by_reconcile_is_not_read_again() {
            let mut tx = transaction();
            tx.metadata.as_mut().unwrap().nonce_used_seen_at_block = Some(100);
            tx.metadata.as_mut().unwrap().nonce_used_seen_at =
                Some((Utc::now() - Duration::minutes(5)).to_rfc3339());
            let mut mocks = default_test_mocks_with_network();
            mocks
                .provider
                .expect_get_transaction_count()
                .returning(|_| Box::pin(async { Ok(6) }));
            mocks
                .provider
                .expect_get_block_number()
                .returning(|| Box::pin(async { Ok(110) }));
            mocks
                .provider
                .expect_get_transaction_receipt()
                .times(1)
                .returning(|_| Box::pin(async { Ok(Some(make_mock_receipt(true, Some(100)))) }));
            let state = repository(&mut mocks, &tx);
            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .times(0);
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));
            mocks
                .job_producer
                .expect_produce_check_transaction_status_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));
            let evm = make_test_evm_relayer_transaction(create_test_relayer(), mocks);
            let result = evm.handle_status_impl(tx, None).await.unwrap();
            assert!(matches!(
                result.status,
                TransactionStatus::Mined | TransactionStatus::Confirmed
            ));
            assert_eq!(seen_at(&state), None);
        }

        /// #991 review (4): when the original wins over a replacement, publish the
        /// original's payload with its hash, not the replacement's.
        #[tokio::test]
        async fn recovered_original_publishes_its_own_payload() {
            let mut tx = transaction();
            if let NetworkTransactionData::Evm(ref mut data) = tx.network_data {
                data.hash = Some("0xreplacement".to_string());
                data.to = Some("0xReplacementRecipient".to_string());
                data.value = U256::from(2);
                data.data = Some("0xbeef".to_string());
                data.signature = Some(crate::models::EvmTransactionDataSignature {
                    r: "r".to_string(),
                    s: "s".to_string(),
                    v: 27,
                    sig: "sig".to_string(),
                });
            }
            tx.hashes = vec!["0xoriginal".to_string()];
            let mut mocks = default_test_mocks();
            mocks.provider = crate::services::provider::MockEvmProviderTrait::new();
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|hash| {
                    let receipt =
                        (hash == "0xoriginal").then(|| make_mock_receipt(true, Some(100)));
                    Box::pin(async move { Ok(receipt) })
                });
            mocks
                .provider
                .expect_raw_request_dyn()
                .withf(|method, params| {
                    method == "eth_getTransactionByHash" && params[0] == "0xoriginal"
                })
                .times(1)
                .returning(|_, _| {
                    Box::pin(async {
                        Ok(serde_json::json!({
                            "to": "0xoriginalrecipient",
                            "value": "0x1",
                            "input": "0xcafe",
                            "gas": "0x5208",
                            "maxFeePerGas": "0x64",
                            "maxPriorityFeePerGas": "0x2",
                            "gasPrice": "0x50"
                        }))
                    })
                });
            let state = repository(&mut mocks, &tx);
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));
            let evm = make_test_evm_relayer_transaction(create_test_relayer(), mocks);
            let data = tx.network_data.get_evm_transaction_data().unwrap();
            let super::super::HistoricalHashRecovery::Recovered(recovered) = evm
                .try_recover_with_historical_hashes(&tx, &data)
                .await
                .unwrap()
            else {
                panic!("expected recovery");
            };
            assert_eq!(recovered.status, TransactionStatus::Mined);
            let saved = state
                .lock()
                .unwrap()
                .network_data
                .get_evm_transaction_data()
                .unwrap();
            assert_eq!(saved.hash.as_deref(), Some("0xoriginal"));
            assert_eq!(saved.to.as_deref(), Some("0xoriginalrecipient"));
            assert_eq!(saved.value, U256::from(1));
            assert_eq!(saved.data.as_deref(), Some("0xcafe"));
            assert_eq!(saved.max_fee_per_gas, Some(100));
            assert_eq!(saved.max_priority_fee_per_gas, Some(2));
            assert_eq!(saved.gas_price, None);
            assert!(saved.signature.is_none());
        }

        /// Without the mined payload, recovery still records Mined (stored payload,
        /// corrected hash): a dropped receipt would let the normal flow resubmit.
        #[tokio::test]
        async fn unreadable_mined_payload_still_records_mined() {
            let mut tx = transaction();
            if let NetworkTransactionData::Evm(ref mut data) = tx.network_data {
                data.hash = Some("0xreplacement".to_string());
                data.to = Some("0xStoredRecipient".to_string());
                data.value = U256::from(7);
                data.raw = Some(vec![1, 2, 3]);
            }
            tx.hashes = vec!["0xoriginal".to_string()];
            let mut mocks = default_test_mocks();
            mocks.provider = crate::services::provider::MockEvmProviderTrait::new();
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|hash| {
                    let receipt =
                        (hash == "0xoriginal").then(|| make_mock_receipt(true, Some(100)));
                    Box::pin(async move { Ok(receipt) })
                });
            mocks
                .provider
                .expect_raw_request_dyn()
                .times(2)
                .returning(|_, _| Box::pin(async { Ok(serde_json::Value::Null) }));
            let state = repository(&mut mocks, &tx);
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));
            let evm = make_test_evm_relayer_transaction(create_test_relayer(), mocks);
            assert_eq!(
                evm.check_transaction_status(&tx).await.unwrap(),
                TransactionStatus::Mined
            );
            let saved = state
                .lock()
                .unwrap()
                .network_data
                .get_evm_transaction_data()
                .unwrap();
            assert_eq!(saved.hash.as_deref(), Some("0xoriginal"));
            assert_eq!(saved.to.as_deref(), Some("0xStoredRecipient"));
            assert_eq!(saved.value, U256::from(7));
            assert!(saved.raw.is_none());
        }

        /// A recovered NOOP keeps the stored `from` spelling, so `to == from` holds.
        #[test]
        fn rpc_payload_keeps_stored_address_spelling() {
            use super::super::payload_from_rpc;
            let mut current = transaction()
                .network_data
                .get_evm_transaction_data()
                .unwrap();
            current.from = "0xAbCd".to_string();
            current.to = Some("0xReplacement".to_string());
            let rpc = serde_json::json!({
                "to": "0xabcd", "value": "0x0", "input": "0x", "gas": "0x5208", "gasPrice": "0x1"
            });
            let data = payload_from_rpc(&current, &rpc).unwrap();
            assert_eq!(data.to.as_deref(), Some("0xAbCd"));
            assert_eq!(data.gas_price, Some(1));
            assert_eq!(data.max_fee_per_gas, None);
        }

        #[tokio::test]
        async fn submitted_receipt_lookup_error_still_errors() {
            let tx = transaction();
            let mut mocks = default_test_mocks();
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|_| {
                    Box::pin(async { Err(crate::services::provider::ProviderError::Timeout) })
                });
            let evm = make_test_evm_relayer_transaction(create_test_relayer(), mocks);
            assert!(evm.check_transaction_status(&tx).await.is_err());
        }

        /// #991 review: no hash recovery (and no WARN log) when the only saved
        /// hash is the current one.
        #[test]
        fn no_hash_recovery_when_only_current_hash_is_saved() {
            let mut tx = transaction();
            tx.status = TransactionStatus::Sent;
            let evm =
                make_test_evm_relayer_transaction(create_test_relayer(), default_test_mocks());
            assert!(!evm.should_try_hash_recovery(&tx).unwrap());
            tx.hashes = vec!["0xoriginal".to_string(), "0xcurrent".to_string()];
            assert!(evm.should_try_hash_recovery(&tx).unwrap());
        }

        /// #991 review: a recovered receipt ends the wait, so one later empty
        /// receipt answer after the span must not mark the transaction `Failed`.
        #[tokio::test]
        async fn recovered_receipt_clears_seen_block_and_later_empty_receipt_never_fails() {
            let mut tx = transaction();
            tx.hashes = vec!["0xoriginal".to_string(), "0xcurrent".to_string()];
            tx.metadata.as_mut().unwrap().nonce_used_seen_at_block = Some(100);
            tx.metadata.as_mut().unwrap().nonce_used_seen_at =
                Some((Utc::now() - Duration::hours(2)).to_rfc3339());
            let mut mocks = default_test_mocks_with_network();
            let block = Arc::new(AtomicU64::new(110));
            let original_visible = Arc::new(AtomicBool::new(true));
            mocks
                .provider
                .expect_get_transaction_count()
                .returning(|_| Box::pin(async { Ok(6) }));
            mocks.provider.expect_get_block_number().returning({
                let block = block.clone();
                move || {
                    let head = block.load(Ordering::SeqCst);
                    Box::pin(async move { Ok(head) })
                }
            });
            mocks.provider.expect_get_transaction_receipt().returning({
                let original_visible = original_visible.clone();
                move |hash| {
                    let receipt = (hash == "0xoriginal" && original_visible.load(Ordering::SeqCst))
                        .then(|| make_mock_receipt(true, Some(100)));
                    Box::pin(async move { Ok(receipt) })
                }
            });
            let state = repository(&mut mocks, &tx);
            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));
            mocks
                .job_producer
                .expect_produce_relayer_health_check_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));
            let evm = make_test_evm_relayer_transaction(create_test_relayer(), mocks);

            let recovered = evm.handle_status_impl(tx, None).await.unwrap();
            assert_eq!(recovered.status, TransactionStatus::Mined);
            assert_eq!(seen_at(&state), None);

            block.store(124, Ordering::SeqCst);
            original_visible.store(false, Ordering::SeqCst);
            let later = evm.handle_status_impl(recovered, None).await.unwrap();
            assert_ne!(later.status, TransactionStatus::Failed);
        }

        #[tokio::test]
        async fn current_receipt_clears_seen_block() {
            let mut tx = transaction();
            tx.metadata.as_mut().unwrap().nonce_used_seen_at_block = Some(100);
            tx.metadata.as_mut().unwrap().nonce_used_seen_at =
                Some((Utc::now() - Duration::hours(2)).to_rfc3339());
            let mut mocks = default_test_mocks_with_network();
            reads(
                &mut mocks,
                Arc::new(AtomicU64::new(110)),
                Arc::new(AtomicBool::new(true)),
            );
            let state = repository(&mut mocks, &tx);
            let evm = make_test_evm_relayer_transaction(create_test_relayer(), mocks);
            assert!(evm
                .reconcile_tx_nonce_state(&mut tx)
                .await
                .map(crate::domain::transaction::evm::status::NonceReconcile::handled)
                .unwrap()
                .is_none());
            assert_eq!(seen_at(&state), None);
            assert_eq!(
                state
                    .lock()
                    .unwrap()
                    .metadata
                    .as_ref()
                    .unwrap()
                    .total_failures,
                7
            );
        }

        #[tokio::test]
        async fn unused_nonce_clears_seen_block() {
            let mut tx = transaction();
            tx.metadata.as_mut().unwrap().nonce_used_seen_at_block = Some(100);
            tx.metadata.as_mut().unwrap().nonce_used_seen_at =
                Some((Utc::now() - Duration::hours(2)).to_rfc3339());
            let mut mocks = default_test_mocks();
            mocks
                .provider
                .expect_get_transaction_count()
                .returning(|_| Box::pin(async { Ok(5) }));
            mocks
                .provider
                .expect_get_block_number()
                .returning(|| Box::pin(async { Ok(100) }));
            let state = repository(&mut mocks, &tx);
            let evm = make_test_evm_relayer_transaction(create_test_relayer(), mocks);
            assert!(evm
                .reconcile_tx_nonce_state(&mut tx)
                .await
                .map(crate::domain::transaction::evm::status::NonceReconcile::handled)
                .unwrap()
                .is_none());
            assert_eq!(seen_at(&state), None);
        }

        #[test]
        fn old_transaction_without_observation_field_loads() {
            let mut json = serde_json::to_value(transaction()).unwrap();
            json["metadata"]
                .as_object_mut()
                .unwrap()
                .remove("nonce_used_seen_at_block");
            let loaded: TransactionRepoModel = serde_json::from_value(json).unwrap();
            assert_eq!(loaded.metadata.unwrap().nonce_used_seen_at_block, None);
        }
    }
}

#[cfg(test)]
#[path = "reprepare_tests.rs"]
mod reprepare_tests;

#[cfg(test)]
#[path = "save_before_send_tests.rs"]
mod save_before_send_tests;
