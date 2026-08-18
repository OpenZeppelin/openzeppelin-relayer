//! EVM nonce drift recovery integration test (issue #818)
//!
//! Reproduces the incident shape from #818 against a live Redis + Anvil stack:
//! the Redis nonce counter drifts above the on-chain nonce (a failure between
//! the counter increment and the tx-record persist "burns" nonces), leaving new
//! transactions stuck ahead of the chain. Verifies the relayer self-heals via
//! its nonce-health machinery — rewinding the counter over the verified-empty
//! region above the stuck tx and gap-filling the burned nonces below it with
//! NOOPs — without any storage reset.

use crate::integration::common::{
    client::RelayerClient,
    confirmation::{wait_for_receipt, ReceiptConfig},
    context::{is_evm_network, run_multi_network_test},
    evm_helpers::{setup_test_relayer, verify_network_ready},
    registry::TestRegistry,
};
use eyre::WrapErr;
use openzeppelin_relayer::models::relayer::RelayerResponse;
use redis::AsyncCommands;
use serial_test::serial;
use std::time::{Duration, Instant};
use tracing::{info, info_span, warn};

/// Burn address for test transfers
const BURN_ADDRESS: &str = "0x000000000000000000000000000000000000dEaD";

/// Test value for transfers (0.000001 ETH in wei)
const TRANSFER_VALUE: &str = "1000000000000";

/// Nonces "burned" below the stuck tx — the health run must gap-fill these with NOOPs.
const BURNED_BELOW: u64 = 3;

/// Nonces "burned" above the stuck tx — the health run must rewind the counter over these.
const BURNED_ABOVE: u64 = 4;

/// Max wait for the stuck tx to recover. Dominated by the resubmit timeout that
/// gates gap detection (~3 min for "fast"), then NOOP fills + mining.
const RECOVERY_MAX_WAIT_MS: u64 = 420_000;

/// Max wait for the counter rewind to be visible in Redis after recovery.
const REWIND_VISIBLE_TIMEOUT: Duration = Duration::from_secs(60);

/// Max wait for the stuck tx's async nonce allocation to appear in the counter.
const ALLOCATION_TIMEOUT: Duration = Duration::from_secs(30);

async fn redis_connection() -> eyre::Result<redis::aio::MultiplexedConnection> {
    let url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
    let client = redis::Client::open(url.as_str())
        .wrap_err_with(|| format!("Failed to open Redis client for {url}"))?;
    client
        .get_multiplexed_async_connection()
        .await
        .wrap_err_with(|| format!("Failed to connect to Redis at {url}"))
}

/// Finds the relayer's nonce counter key without assuming address formatting.
///
/// Returns `None` when no key exists — the relayer is running with in-memory
/// storage (`REPOSITORY_STORAGE_TYPE` != `redis`), where this test cannot
/// manipulate the counter.
async fn find_counter_key(
    con: &mut redis::aio::MultiplexedConnection,
    relayer_id: &str,
) -> eyre::Result<Option<String>> {
    let prefix = std::env::var("REDIS_KEY_PREFIX").unwrap_or_else(|_| "oz-relayer".to_string());
    let pattern = format!("{prefix}:transaction_counter:{relayer_id}:*");
    let keys: Vec<String> = con.keys(&pattern).await?;
    match keys.as_slice() {
        [key] => Ok(Some(key.clone())),
        [] => Ok(None),
        _ => Err(eyre::eyre!(
            "Expected exactly one counter key for {pattern}, found {keys:?}"
        )),
    }
}

async fn send_transfer(client: &RelayerClient, relayer_id: &str) -> eyre::Result<String> {
    let tx_request = serde_json::json!({
        "to": BURN_ADDRESS,
        "value": TRANSFER_VALUE,
        "data": "0x",
        "gas_limit": 21000,
        "speed": "fast"
    });
    let tx_response = client.send_transaction(relayer_id, tx_request).await?;
    Ok(tx_response.id)
}

async fn run_nonce_drift_recovery_test(
    network: String,
    relayer_info: RelayerResponse,
) -> eyre::Result<()> {
    let _span = info_span!("nonce_drift_recovery", network = %network, relayer = %relayer_info.id)
        .entered();

    // This test writes the relayer's counter key directly in Redis and relies on
    // cheap, fast NOOP fills — local Anvil only.
    if std::env::var("MODE").is_ok_and(|m| m.eq_ignore_ascii_case("testnet")) {
        info!("Skipping nonce drift recovery test in testnet mode");
        return Ok(());
    }

    let registry = TestRegistry::load()?;
    verify_network_ready(&registry, &network, &relayer_info)?;

    let client = RelayerClient::from_env()?;
    let relayer = setup_test_relayer(&client, &relayer_info).await?;

    // Baseline transfer: proves the relayer works and guarantees the counter key
    // exists in Redis and matches the on-chain nonce.
    let baseline_id = send_transfer(&client, &relayer.id).await?;
    let receipt_config = ReceiptConfig::from_network(&network)?;
    wait_for_receipt(&client, &relayer.id, &baseline_id, &receipt_config).await?;
    info!(tx_id = %baseline_id, "Baseline transfer confirmed");

    let mut con = redis_connection().await?;
    let Some(counter_key) = find_counter_key(&mut con, &relayer.id).await? else {
        warn!(
            relayer_id = %relayer.id,
            "SKIPPED: no Redis counter key — relayer is not using Redis storage. \
             Set REPOSITORY_STORAGE_TYPE=redis (and STORAGE_ENCRYPTION_KEY) in \
             .env.integration to run this test."
        );
        return Ok(());
    };
    let c0: u64 = con.get(&counter_key).await?;
    info!(counter_key = %counter_key, counter = c0, "Counter in sync with chain");

    // Simulate burned nonces below: the next tx will allocate c0 + BURNED_BELOW,
    // landing ahead of the chain and getting stuck in the node's queue.
    let _: () = con.set(&counter_key, c0 + BURNED_BELOW).await?;
    let stuck_id = send_transfer(&client, &relayer.id).await?;
    info!(tx_id = %stuck_id, nonce = c0 + BURNED_BELOW, "Sent tx stuck ahead of chain");

    // Nonce allocation happens asynchronously in the prepare job — wait until the
    // stuck tx's allocation moves the counter to c0 + BURNED_BELOW + 1.
    let after_alloc = c0 + BURNED_BELOW + 1;
    let deadline = Instant::now() + ALLOCATION_TIMEOUT;
    loop {
        let counter: u64 = con.get(&counter_key).await?;
        if counter == after_alloc {
            break;
        }
        if counter != c0 + BURNED_BELOW || Instant::now() > deadline {
            return Err(eyre::eyre!(
                "Counter is {counter}, expected {after_alloc} after stuck-tx allocation; \
                 a health run may have interfered — rerun the test"
            ));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // Simulate burned nonces above the stuck tx's record. The health run must
    // rewind the counter over this verified-empty region via CAS.
    let inflated = after_alloc + BURNED_ABOVE;
    let _: () = con.set(&counter_key, inflated).await?;
    info!(counter = inflated, "Inflated counter above stuck tx");

    // Recovery: a status check on the stuck tx detects the gap ahead and enqueues
    // a nonce-health job, which rewinds the counter and gap-fills the burned
    // nonces below with NOOPs; the NOOPs mine, then the stuck tx mines.
    let recovery_config = ReceiptConfig::custom(&network, 2_000, RECOVERY_MAX_WAIT_MS);
    wait_for_receipt(&client, &relayer.id, &stuck_id, &recovery_config).await?;
    info!(tx_id = %stuck_id, "Stuck tx recovered and confirmed");

    // The rewind runs before the gap fill in the same health run, so by now the
    // counter must have been CAS'd from `inflated` down to after_alloc
    // (= highest tx record + 1). Poll briefly to absorb read lag.
    let deadline = Instant::now() + REWIND_VISIBLE_TIMEOUT;
    loop {
        let counter: u64 = con.get(&counter_key).await?;
        if counter == after_alloc {
            info!(counter = counter, "Counter rewound over burned region");
            break;
        }
        if Instant::now() > deadline {
            return Err(eyre::eyre!(
                "Counter is {counter}, expected rewind to {after_alloc} within {:?}",
                REWIND_VISIBLE_TIMEOUT
            ));
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    // A normal transfer must now allocate the rewound nonce and confirm — the
    // relayer is fully healed with no storage reset.
    let final_id = send_transfer(&client, &relayer.id).await?;
    wait_for_receipt(&client, &relayer.id, &final_id, &receipt_config).await?;
    let final_counter: u64 = con.get(&counter_key).await?;
    if final_counter != after_alloc + 1 {
        return Err(eyre::eyre!(
            "Counter is {final_counter}, expected {} after post-recovery transfer",
            after_alloc + 1
        ));
    }
    info!(tx_id = %final_id, counter = final_counter, "Post-recovery transfer confirmed");

    Ok(())
}

/// Test nonce drift recovery (issue #818) on all selected EVM networks
#[tokio::test]
#[serial]
async fn test_evm_nonce_drift_recovery() {
    run_multi_network_test(
        "nonce_drift_recovery",
        is_evm_network,
        run_nonce_drift_recovery_test,
    )
    .await;
}
