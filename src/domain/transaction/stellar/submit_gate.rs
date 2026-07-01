//! Per-account ordered submission gate for concurrent-mode Stellar relayers.
//!
//! Sequence *allocation* is already race-safe (atomic Redis `INCR`). The failure
//! this gate prevents is *submission ordering*: prepare allocates a sequence then
//! enqueues a separate `TransactionSubmission` job, so under concurrency seq `N+1`
//! can reach the network before `N` → Stellar rejects `N+1` with `TxBadSeq`.
//!
//! Constitution III (NON-NEGOTIABLE) requires authoritative, ordered per-account
//! sequencing under concurrent submission. This gate enforces it with a process-local
//! watermark (`next_expected`) per `(relayer, source_account)`, mirroring the storage
//! model of [`super::lane_gate`] but as a *watermark* rather than a single-slot flag:
//!
//! - A submit attempt for `seq = N` proceeds only when `N <= next_expected`.
//! - `N > next_expected` is deferred (the caller re-enqueues the submit job with a
//!   bounded delay) — never submitted out of order.
//! - The watermark advances on submit-success and on terminal Failed/Expired, so a
//!   dead `N` can never block `N+1` forever.
//!
//! The gate only guards the *ordering decision* — its operations are lock-free and
//! synchronous, so no guard is ever held across the submission RPC. Cross-process
//! correctness still relies on the monotonic Redis counter ([`sync_floor`]) plus
//! chain-floor seeding; this gate reduces churn within a single process.
//!
//! [`sync_floor`]: crate::repositories::TransactionCounterTrait::sync_floor

use dashmap::{DashMap, Entry};
use once_cell::sync::Lazy;

/// `{relayer_id}:{source_account}` → `next_expected` (lowest sequence not yet
/// successfully submitted for that account).
static WATERMARK: Lazy<DashMap<String, i64>> = Lazy::new(DashMap::new);

fn key(relayer_id: &str, source_account: &str) -> String {
    format!("{relayer_id}:{source_account}")
}

/// Outcome of an ordered-submission admission check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    /// `seq` is at or below the watermark — submit now.
    Ready,
    /// `seq` is ahead of the watermark — defer (re-enqueue with a bounded delay).
    TooEarly,
}

/// Current watermark for an account, if it has been seeded. Lets the caller skip
/// the chain-floor fetch when a watermark already exists.
pub fn peek(relayer_id: &str, source_account: &str) -> Option<i64> {
    WATERMARK.get(&key(relayer_id, source_account)).map(|v| *v)
}

/// Decide whether `seq` may be submitted now for `(relayer, source_account)`.
///
/// Seeds the watermark from `chain_floor` on first sight (or after [`reset`]).
/// `Ready` iff `seq <= next_expected`: `seq == next_expected` is the in-order case,
/// and `seq < next_expected` is a stale/redelivered submit that the RPC will dedup
/// (admitting it avoids deferring forever). `seq > next_expected` is `TooEarly`.
pub fn try_admit(relayer_id: &str, source_account: &str, seq: i64, chain_floor: i64) -> Admission {
    let next_expected = match WATERMARK.entry(key(relayer_id, source_account)) {
        Entry::Occupied(e) => *e.get(),
        Entry::Vacant(e) => {
            e.insert(chain_floor);
            chain_floor
        }
    };
    if seq <= next_expected {
        Admission::Ready
    } else {
        Admission::TooEarly
    }
}

/// Advance the watermark past `seq` after a successful submission, so the next
/// sequence becomes admissible. Idempotent and monotonic (never rewinds).
pub fn record_submitted(relayer_id: &str, source_account: &str, seq: i64) {
    advance_past(relayer_id, source_account, seq);
}

/// Advance the watermark past `seq` when a transaction for `seq` reaches a terminal
/// state (Failed/Expired). CRITICAL for no-deadlock: a dead `N` must not block `N+1`.
pub fn advance_on_terminal(relayer_id: &str, source_account: &str, seq: i64) {
    advance_past(relayer_id, source_account, seq);
}

/// Clear the watermark so it re-seeds from chain on the next [`try_admit`]. Called
/// alongside the counter's `sync_floor` after a `TxBadSeq` so gate and counter
/// re-seed consistently.
pub fn reset(relayer_id: &str, source_account: &str) {
    WATERMARK.remove(&key(relayer_id, source_account));
}

/// Set `next_expected = max(next_expected, seq + 1)`, seeding to `seq + 1` if absent.
fn advance_past(relayer_id: &str, source_account: &str, seq: i64) {
    let target = seq.saturating_add(1);
    match WATERMARK.entry(key(relayer_id, source_account)) {
        Entry::Occupied(mut e) => {
            if *e.get() < target {
                e.insert(target);
            }
        }
        Entry::Vacant(e) => {
            e.insert(target);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Each test uses a unique (relayer, account) pair so the process-global map
    // does not bleed across tests running in parallel.

    #[test]
    fn seeds_from_chain_floor_and_admits_in_order() {
        let (r, a) = ("r-seed", "acct");
        assert_eq!(peek(r, a), None);
        // First sight: seed to chain floor 5; seq 5 is in order.
        assert_eq!(try_admit(r, a, 5, 5), Admission::Ready);
        assert_eq!(peek(r, a), Some(5));
    }

    #[test]
    fn defers_sequence_ahead_of_watermark() {
        let (r, a) = ("r-ahead", "acct");
        // Watermark seeds to 10; seq 12 is ahead → TooEarly.
        assert_eq!(try_admit(r, a, 12, 10), Admission::TooEarly);
        // seq 10 (the expected one) is admitted.
        assert_eq!(try_admit(r, a, 10, 10), Admission::Ready);
        // After recording 10 submitted, 11 becomes admissible but 12 still ahead.
        record_submitted(r, a, 10);
        assert_eq!(try_admit(r, a, 11, 10), Admission::Ready);
        record_submitted(r, a, 11);
        assert_eq!(try_admit(r, a, 12, 10), Admission::Ready);
    }

    #[test]
    fn stale_sequence_below_watermark_is_admitted_not_deferred() {
        let (r, a) = ("r-stale", "acct");
        assert_eq!(try_admit(r, a, 20, 20), Admission::Ready);
        record_submitted(r, a, 20); // watermark -> 21
                                    // A redelivered submit for the already-submitted seq 20 must not deadlock.
        assert_eq!(try_admit(r, a, 20, 21), Admission::Ready);
    }

    #[test]
    fn terminal_advance_unblocks_next_sequence() {
        let (r, a) = ("r-terminal", "acct");
        assert_eq!(try_admit(r, a, 30, 30), Admission::Ready);
        // seq 30 dies (Failed). Without advance, 31 would be stuck.
        assert_eq!(try_admit(r, a, 31, 30), Admission::TooEarly);
        advance_on_terminal(r, a, 30);
        assert_eq!(try_admit(r, a, 31, 30), Admission::Ready);
    }

    #[test]
    fn record_submitted_is_monotonic() {
        let (r, a) = ("r-mono", "acct");
        record_submitted(r, a, 100); // -> 101
        record_submitted(r, a, 50); // must NOT rewind
        assert_eq!(peek(r, a), Some(101));
    }

    #[test]
    fn reset_clears_watermark_for_reseed() {
        let (r, a) = ("r-reset", "acct");
        assert_eq!(try_admit(r, a, 5, 5), Admission::Ready);
        record_submitted(r, a, 5); // -> 6
        reset(r, a);
        assert_eq!(peek(r, a), None);
        // Re-seeds from the fresh chain floor after reset.
        assert_eq!(try_admit(r, a, 8, 8), Admission::Ready);
        assert_eq!(peek(r, a), Some(8));
    }
}
