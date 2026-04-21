//! Live-network smoke test for SharedDustSyncTask against preview testnet.
//!
//! Exercises:
//! - WS handshake + subscription to `dustLedgerEvents`
//! - Batch event replay on every subscribed wallet
//! - Ready signal transition (applied_id catches up to chain_max_id)
//! - Post-sync DUST state: wallet should have ≥1 UTXO and non-zero balance
//!
//! Marked `#[ignore]` so CI doesn't hit the live testnet; run manually with:
//!
//!     cargo test --test midnight_shared_sync_smoke \
//!         --features midnight -- --ignored --nocapture
//!
//! Requires the unit-test keystore at
//! tests/utils/test_keys/unit-test-local-signer.json (passphrase "test"),
//! which is the same wallet that carries ~5 DUST on preview testnet.

#![cfg(feature = "midnight")]

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use midnight_node_ledger_helpers::{LedgerContext, Timestamp, WalletSeed};

use openzeppelin_relayer::config::network::midnight::IndexerUrls;
use openzeppelin_relayer::services::sync::midnight::{
    get_network_sync, init_network_sync, shutdown_network_sync, MidnightIndexerClient,
    SharedDustSyncTask,
};

#[tokio::test]
#[ignore = "hits live preview testnet"]
async fn shared_task_reaches_ready_and_populates_balance() {
    // Decrypt the unit-test keystore via oz_keystore (same crate the
    // production signer uses). Keystore's 32 raw bytes ARE the wallet seed.
    let keystore_path = std::path::Path::new("tests/utils/test_keys/unit-test-local-signer.json");
    assert!(
        keystore_path.exists(),
        "test keystore missing at {keystore_path:?} — run from repo root"
    );
    let loaded = oz_keystore::LocalClient::load(keystore_path.to_path_buf(), "test".to_string());
    let seed_bytes: [u8; 32] = loaded[..].try_into().expect("32-byte seed");
    let wallet_seed = WalletSeed::Medium(seed_bytes);

    let context = Arc::new(LedgerContext::new_from_wallet_seeds(
        "preview".to_string(),
        &[wallet_seed.clone()],
    ));

    let indexer = MidnightIndexerClient::new(IndexerUrls {
        http: "https://indexer.preview.midnight.network/api/v4/graphql".into(),
        ws: "wss://indexer.preview.midnight.network/api/v4/graphql/ws".into(),
    });

    let task = SharedDustSyncTask::new("preview".into(), indexer, context.clone());
    let handle = task.subscribe_wallet(wallet_seed.clone());
    task.start();

    // Initial 34K-event catch-up runs ~30s. Cap at 120s so a stuck handshake
    // or network hang fails loud rather than hanging the test harness.
    let ready_timeout = tokio::time::Duration::from_secs(120);
    match tokio::time::timeout(ready_timeout, handle.await_ready()).await {
        Ok(Ok(())) => {}
        Ok(Err(reason)) => panic!("sync failed: {reason}"),
        Err(_) => panic!("timed out waiting for Ready after {ready_timeout:?}"),
    }

    // Assert wallet state post-sync. Uses wall-clock now for value
    // computation, same as LedgerContextManager::dust_balance.
    let now = Timestamp::from_secs(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    );
    let (utxos, balance) = context.with_wallet_from_seed(wallet_seed, |wallet| {
        let state = wallet
            .dust
            .dust_local_state
            .as_ref()
            .expect("DustLocalState present after sync");
        (state.utxos().count(), state.wallet_balance(now))
    });

    println!("smoke: utxos={utxos} balance={balance} SPECK");
    assert!(
        utxos > 0,
        "wallet should have at least one DUST UTXO after sync"
    );
    assert!(balance > 0, "DUST balance should be non-zero after sync");

    task.stop().await;
}

/// Same end-to-end as above but goes through the process-wide registry
/// (`init_network_sync` / `get_network_sync`). Verifies that a second lookup
/// returns the same task instance (idempotent init) and that a wallet handle
/// obtained via the slot sees Ready after sync.
#[tokio::test]
#[ignore = "hits live preview testnet"]
async fn registry_init_is_idempotent_and_reaches_ready() {
    let keystore_path = std::path::Path::new("tests/utils/test_keys/unit-test-local-signer.json");
    let loaded = oz_keystore::LocalClient::load(keystore_path.to_path_buf(), "test".to_string());
    let seed_bytes: [u8; 32] = loaded[..].try_into().expect("32-byte seed");
    let wallet_seed = WalletSeed::Medium(seed_bytes);

    let indexer = MidnightIndexerClient::new(IndexerUrls {
        http: "https://indexer.preview.midnight.network/api/v4/graphql".into(),
        ws: "wss://indexer.preview.midnight.network/api/v4/graphql/ws".into(),
    });

    // Use a unique network id so the test doesn't collide with the other
    // smoke test's global registry entry.
    let network_id = "preview-registry-smoke";

    let slot = init_network_sync(network_id, vec![wallet_seed.clone()], indexer.clone());
    let slot_again = init_network_sync(
        network_id,
        vec![wallet_seed.clone()],
        indexer, // ignored on existing slot
    );
    assert!(
        Arc::ptr_eq(&slot, &slot_again),
        "registry should return same slot"
    );
    assert!(
        Arc::ptr_eq(&slot, &get_network_sync(network_id).unwrap()),
        "get_network_sync should return the same Arc"
    );

    let handle = slot.task.subscribe_wallet(wallet_seed.clone());

    let ready_timeout = tokio::time::Duration::from_secs(120);
    tokio::time::timeout(ready_timeout, handle.await_ready())
        .await
        .expect("timed out")
        .expect("sync failed");

    let now = Timestamp::from_secs(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    );
    let balance = slot.context.with_wallet_from_seed(wallet_seed, |wallet| {
        wallet
            .dust
            .dust_local_state
            .as_ref()
            .expect("DustLocalState present")
            .wallet_balance(now)
    });
    println!("registry-smoke: balance={balance} SPECK");
    assert!(balance > 0);

    shutdown_network_sync(network_id).await;
    assert!(
        get_network_sync(network_id).is_none(),
        "slot should be removed"
    );
}

// Keep type imports consistent: SharedDustSyncTask is used implicitly via
// the slot; mark as used.
#[allow(dead_code)]
fn _touch_task(_t: Arc<SharedDustSyncTask>) {}
