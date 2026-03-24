//! EVM transaction cancel/replace integration tests
//!
//! On Anvil with auto-mining, transactions confirm instantly, so these tests
//! focus on the API behavior for already-confirmed transactions and the
//! delete-pending-transactions cleanup endpoint.

use crate::integration::common::{
    client::RelayerClient,
    confirmation::{wait_for_receipt, ReceiptConfig},
    context::{is_evm_network, run_multi_network_test},
    evm_helpers::{setup_test_relayer, verify_network_ready},
    registry::TestRegistry,
};
use openzeppelin_relayer::models::relayer::RelayerResponse;
use serial_test::serial;
use tracing::{info, info_span};

const BURN_ADDRESS: &str = "0x000000000000000000000000000000000000dEaD";

/// Tests that cancelling an already-confirmed transaction fails gracefully.
async fn run_cancel_confirmed_tx_test(
    network: String,
    relayer_info: RelayerResponse,
) -> eyre::Result<()> {
    let _span =
        info_span!("cancel_confirmed", network = %network, relayer = %relayer_info.id).entered();
    info!("Starting cancel confirmed tx test");

    let registry = TestRegistry::load()?;
    verify_network_ready(&registry, &network, &relayer_info)?;

    let client = RelayerClient::from_env()?;
    let relayer = setup_test_relayer(&client, &relayer_info).await?;

    // Submit and wait for confirmation
    let tx = client
        .send_transaction(
            &relayer.id,
            serde_json::json!({
                "to": BURN_ADDRESS,
                "value": "1000000000",
                "data": "0x",
                "gas_limit": 21000,
                "speed": "fast"
            }),
        )
        .await?;

    let receipt_config = ReceiptConfig::from_network(&network)?;
    wait_for_receipt(&client, &relayer.id, &tx.id, &receipt_config).await?;

    // Try to cancel — should fail since tx is confirmed
    let cancel_result = client.cancel_transaction(&relayer.id, &tx.id).await;

    assert!(
        cancel_result.is_err(),
        "Cancelling a confirmed transaction should fail"
    );
    let err_msg = cancel_result.unwrap_err().to_string().to_lowercase();
    assert!(
        err_msg.contains("400")
            || err_msg.contains("409")
            || err_msg.contains("not")
            || err_msg.contains("cannot")
            || err_msg.contains("already"),
        "Error should indicate tx can't be cancelled, got: {}",
        err_msg
    );

    info!("Cancel confirmed tx correctly rejected");
    Ok(())
}

/// Tests that replacing an already-confirmed transaction fails gracefully.
async fn run_replace_confirmed_tx_test(
    network: String,
    relayer_info: RelayerResponse,
) -> eyre::Result<()> {
    let _span =
        info_span!("replace_confirmed", network = %network, relayer = %relayer_info.id).entered();
    info!("Starting replace confirmed tx test");

    let registry = TestRegistry::load()?;
    verify_network_ready(&registry, &network, &relayer_info)?;

    let client = RelayerClient::from_env()?;
    let relayer = setup_test_relayer(&client, &relayer_info).await?;

    // Submit and wait for confirmation
    let tx = client
        .send_transaction(
            &relayer.id,
            serde_json::json!({
                "to": BURN_ADDRESS,
                "value": "1000000000",
                "data": "0x",
                "gas_limit": 21000,
                "speed": "fast"
            }),
        )
        .await?;

    let receipt_config = ReceiptConfig::from_network(&network)?;
    wait_for_receipt(&client, &relayer.id, &tx.id, &receipt_config).await?;

    // Try to replace — should fail since tx is confirmed
    let replace_result = client
        .replace_transaction(&relayer.id, &tx.id, serde_json::json!({ "speed": "fast" }))
        .await;

    assert!(
        replace_result.is_err(),
        "Replacing a confirmed transaction should fail"
    );
    let err_msg = replace_result.unwrap_err().to_string().to_lowercase();
    assert!(
        err_msg.contains("400")
            || err_msg.contains("409")
            || err_msg.contains("not")
            || err_msg.contains("cannot")
            || err_msg.contains("already"),
        "Error should indicate tx can't be replaced, got: {}",
        err_msg
    );

    info!("Replace confirmed tx correctly rejected");
    Ok(())
}

/// Tests the delete-pending-transactions cleanup endpoint.
async fn run_delete_pending_test(
    network: String,
    relayer_info: RelayerResponse,
) -> eyre::Result<()> {
    let _span =
        info_span!("delete_pending", network = %network, relayer = %relayer_info.id).entered();
    info!("Starting delete pending transactions test");

    let registry = TestRegistry::load()?;
    verify_network_ready(&registry, &network, &relayer_info)?;

    let client = RelayerClient::from_env()?;
    let relayer = setup_test_relayer(&client, &relayer_info).await?;

    // Calling delete pending should succeed even if there are no pending txs
    let result = client.delete_pending_transactions(&relayer.id).await;

    assert!(
        result.is_ok(),
        "Delete pending transactions should succeed: {:?}",
        result.err()
    );

    info!("Delete pending transactions succeeded");
    Ok(())
}

#[tokio::test]
#[serial]
async fn test_evm_cancel_confirmed_transaction() {
    run_multi_network_test(
        "cancel_confirmed_tx",
        is_evm_network,
        run_cancel_confirmed_tx_test,
    )
    .await;
}

#[tokio::test]
#[serial]
async fn test_evm_replace_confirmed_transaction() {
    run_multi_network_test(
        "replace_confirmed_tx",
        is_evm_network,
        run_replace_confirmed_tx_test,
    )
    .await;
}

#[tokio::test]
#[serial]
async fn test_evm_delete_pending_transactions() {
    run_multi_network_test("delete_pending", is_evm_network, run_delete_pending_test).await;
}
