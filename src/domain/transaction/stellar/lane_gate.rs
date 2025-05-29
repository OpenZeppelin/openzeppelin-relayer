//! Lane gating mechanism for transaction relayers.
//!
//! This module provides a lock-free, atomic gating system that ensures only one transaction
//! can hold a "lane" for a given relayer at any time. This prevents race conditions and
//! conflicting operations when multiple transactions attempt to use the same relayer concurrently.
//!
//! ## Key Features
//!
//! - **Lock-free operations**: Uses DashMap for high-performance concurrent access
//! - **Per-relayer lanes**: Different relayers can operate concurrently without blocking each other
//! - **Atomic ownership transfer**: Supports atomic handoff of lane ownership between transactions
//! - **Idempotent operations**: Safe to call multiple times with the same parameters
use dashmap::{DashMap, Entry};
use once_cell::sync::Lazy;

type RelayerId = String;
type TxId = String;

static BUSY: Lazy<DashMap<RelayerId, TxId>> = Lazy::new(DashMap::new);

/// Try to claim the lane for relayer_id:tx_id.
/// Returns true if it becomes owner.
/// Returns true if it already owns the lane.
/// Returns false if another tx owns it.
pub fn claim(relayer_id: &str, tx_id: &str) -> bool {
    match BUSY.entry(relayer_id.to_owned()) {
        Entry::Vacant(entry) => {
            entry.insert(tx_id.to_owned());
            true
        }
        Entry::Occupied(entry) => {
            // Already owns the lane if same tx_id
            entry.get() == tx_id
        }
    }
}

/// Pass the lane from current_tx_id to next_tx_id
///
/// This operation is atomic and lock-free per relayer.
pub fn pass_to(relayer_id: &str, current_tx_id: &str, next_tx_id: &str) {
    if let Entry::Occupied(mut entry) = BUSY.entry(relayer_id.to_owned()) {
        if entry.get() == current_tx_id {
            entry.insert(next_tx_id.to_owned());
        }
    }
}

/// Free the lane if we still own it.
///
/// This operation is atomic and lock-free per relayer.
pub fn free(relayer_id: &str, tx_id: &str) {
    if let Entry::Occupied(entry) = BUSY.entry(relayer_id.to_owned()) {
        if entry.get() == tx_id {
            entry.remove();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Barrier,
    };
    use std::thread;
    use std::time::Duration;

    /// Helper to restore a clean state for every test.
    fn reset() {
        BUSY.clear();
    }

    #[test]
    fn claim_is_idempotent() {
        reset();
        assert!(claim("r", "tx1"));
        assert!(claim("r", "tx1")); // same owner
    }

    #[test]
    fn claim_is_exclusive() {
        reset();
        assert!(claim("r", "tx1"));
        assert!(!claim("r", "tx2")); // different tx blocked
    }

    #[test]
    fn free_releases_lane() {
        reset();
        assert!(claim("r", "tx1"));
        free("r", "tx1");
        assert!(claim("r", "tx2")); // now succeeds
    }

    #[test]
    fn free_by_non_owner_is_noop() {
        reset();
        assert!(claim("r", "tx1"));
        free("r", "tx2"); // wrong tx
        assert!(!claim("r", "tx2")); // still owned by tx1
        assert!(claim("r", "tx1")); // owner unchanged
    }

    #[test]
    fn pass_to_transfers_ownership() {
        reset();
        assert!(claim("r", "tx1"));
        pass_to("r", "tx1", "tx2");
        assert!(!claim("r", "tx1")); // old owner lost
        assert!(claim("r", "tx2")); // new owner
    }

    #[test]
    fn pass_to_by_non_owner_is_noop() {
        reset();
        assert!(claim("r", "tx1"));
        pass_to("r", "txX", "tx2"); // wrong current owner
        assert!(!claim("r", "tx2")); // transfer failed
        assert!(claim("r", "tx1"));
    }

    #[test]
    fn exclusivity_holds_under_contention() {
        reset();
        const THREADS: usize = 8;
        const ATTEMPTS: usize = 200;
        let active = Arc::new(AtomicUsize::new(0));
        let max_seen = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(THREADS));

        let handles: Vec<_> = (0..THREADS)
            .map(|idx| {
                let active = Arc::clone(&active);
                let max_seen = Arc::clone(&max_seen);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait(); // start together
                    for a in 0..ATTEMPTS {
                        let tx = format!("t{}-{}", idx, a);
                        if claim("relayer", &tx) {
                            let cur = active.fetch_add(1, Ordering::SeqCst) + 1;
                            // record maximum concurrent owners
                            max_seen.fetch_max(cur, Ordering::SeqCst);
                            thread::sleep(Duration::from_micros(10));
                            active.fetch_sub(1, Ordering::SeqCst);
                            free("relayer", &tx);
                        }
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(max_seen.load(Ordering::SeqCst), 1); // never more than one owner
    }

    #[test]
    fn different_relayers_do_not_interfere() {
        reset();
        let barrier = Arc::new(Barrier::new(2));

        let h1 = {
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                assert!(claim("r1", "tx1"));
                thread::sleep(Duration::from_millis(5));
                free("r1", "tx1");
            })
        };
        let h2 = {
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                assert!(claim("r2", "tx1"));
                thread::sleep(Duration::from_millis(5));
                free("r2", "tx1");
            })
        };

        h1.join().unwrap();
        h2.join().unwrap();
    }
}
