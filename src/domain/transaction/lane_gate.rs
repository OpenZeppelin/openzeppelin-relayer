//! Controls the single-lane submission flow for each Stellar relayer.
//!
//! At any moment a relayer may have one transaction “on the lane” (being
//! prepared, submitted, or awaiting confirmation).
//! This helper keeps track of which transaction currently owns that lane and
//! provides atomic operations to:
//! * `claim`   – grab the lane (idempotent for the same tx)
//! * `pass_to` – hand ownership directly to the next tx without releasing
//! * `free`    – release the lane so another tx can claim it.
//!

use once_cell::sync::Lazy;
use std::collections::HashMap;
use tokio::sync::Mutex;

type RelayerId = String;
type TxId = String;

static BUSY: Lazy<Mutex<HashMap<RelayerId, TxId>>> = Lazy::new(|| Mutex::new(HashMap::new()));

/// Try to claim the lane for `relayer_id:tx_id`.
/// Returns `true` if it becomes owner.
/// Returns `true` if it already owns the lane.
/// Returns `false` if another tx owns it.
pub async fn claim(relayer_id: &str, tx_id: &str) -> bool {
    let mut map = BUSY.lock().await;
    match map.get(relayer_id) {
        None => {
            map.insert(relayer_id.to_owned(), tx_id.to_owned());
            true
        }
        Some(cur) if cur == tx_id => true,
        Some(_) => false,
    }
}

/// Pass the lane from `current_tx_id` to `next_tx_id`
pub async fn pass_to(relayer_id: &str, current_tx_id: &str, next_tx_id: &str) {
    let mut map = BUSY.lock().await;
    if matches!(map.get(relayer_id), Some(cur) if cur == current_tx_id) {
        map.insert(relayer_id.to_owned(), next_tx_id.to_owned());
    }
}

/// Free the lane if we still own it.
pub async fn free(relayer_id: &str, tx_id: &str) {
    let mut map = BUSY.lock().await;
    if matches!(map.get(relayer_id), Some(cur) if cur == tx_id) {
        map.remove(relayer_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::runtime::Runtime;

    fn rt() -> Runtime {
        Runtime::new().expect("failed to create tokio runtime")
    }

    #[test]
    fn claim_is_idempotent_and_exclusive() {
        rt().block_on(async {
            // first claim succeeds
            assert!(claim("relayer-a", "tx1").await);
            // same tx can claim again (idempotent)
            assert!(claim("relayer-a", "tx1").await);
            // different tx is blocked
            assert!(!claim("relayer-a", "tx2").await);
            free("relayer-a", "tx1").await;
        });
    }

    #[test]
    fn pass_to_transfers_ownership() {
        rt().block_on(async {
            claim("relayer-b", "tx1").await;
            pass_to("relayer-b", "tx1", "tx2").await;
            // old owner no longer succeeds
            assert!(!claim("relayer-b", "tx1").await);
            // new owner is recognised as owner
            assert!(claim("relayer-b", "tx2").await);
            free("relayer-b", "tx2").await;
        });
    }

    #[test]
    fn free_releases_lane() {
        rt().block_on(async {
            claim("relayer-c", "tx1").await;
            free("relayer-c", "tx1").await;
            // now another tx can claim
            assert!(claim("relayer-c", "tx2").await);
            free("relayer-c", "tx2").await;
        });
    }
}
