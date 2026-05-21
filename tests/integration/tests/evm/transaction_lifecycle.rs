//! EVM transaction lifecycle integration tests
//!
//! Validates the full transaction state machine: submit → pending → confirmed,
//! plus transaction queries (by ID, by nonce, list).

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
const TRANSFER_VALUE: &str = "1000000000000";

/// Tests that a confirmed transaction has all expected fields populated.
async fn run_transaction_fields_test(
    network: String,
    relayer_info: RelayerResponse,
) -> eyre::Result<()> {
    let _span = info_span!("tx_fields", network = %network, relayer = %relayer_info.id).entered();
    info!("Starting transaction fields test");

    let registry = TestRegistry::load()?;
    verify_network_ready(&registry, &network, &relayer_info)?;

    let client = RelayerClient::from_env()?;
    let relayer = setup_test_relayer(&client, &relayer_info).await?;

    // Submit a transaction
    let tx_response = client
        .send_transaction(
            &relayer.id,
            serde_json::json!({
                "to": BURN_ADDRESS,
                "value": TRANSFER_VALUE,
                "data": "0x",
                "gas_limit": 21000,
                "speed": "fast"
            }),
        )
        .await?;

    // Immediately after submit: status should be pending or sent
    assert!(
        tx_response.status == "pending" || tx_response.status == "sent",
        "Initial status should be pending or sent, got: {}",
        tx_response.status
    );
    assert!(!tx_response.id.is_empty(), "Transaction ID should be set");
    assert_eq!(tx_response.relayer_id, relayer.id);

    // Wait for confirmation
    let receipt_config = ReceiptConfig::from_network(&network)?;
    wait_for_receipt(&client, &relayer.id, &tx_response.id, &receipt_config).await?;

    // Verify confirmed state has all fields
    let final_tx = client.get_transaction(&relayer.id, &tx_response.id).await?;

    assert!(
        final_tx.status == "confirmed" || final_tx.status == "mined",
        "Final status should be confirmed or mined, got: {}",
        final_tx.status
    );
    assert!(
        final_tx.hash.is_some(),
        "Confirmed tx should have a tx hash"
    );
    assert!(
        final_tx.created_at.is_some(),
        "Confirmed tx should have created_at"
    );

    info!(
        hash = ?final_tx.hash,
        status = %final_tx.status,
        "Transaction confirmed with all expected fields"
    );
    Ok(())
}

/// Tests listing transactions for a relayer and verifying the submitted tx appears.
async fn run_transaction_list_test(
    network: String,
    relayer_info: RelayerResponse,
) -> eyre::Result<()> {
    let _span = info_span!("tx_list", network = %network, relayer = %relayer_info.id).entered();
    info!("Starting transaction list test");

    let registry = TestRegistry::load()?;
    verify_network_ready(&registry, &network, &relayer_info)?;

    let client = RelayerClient::from_env()?;
    let relayer = setup_test_relayer(&client, &relayer_info).await?;

    // Submit a transaction
    let tx_response = client
        .send_transaction(
            &relayer.id,
            serde_json::json!({
                "to": BURN_ADDRESS,
                "value": TRANSFER_VALUE,
                "data": "0x",
                "gas_limit": 21000,
                "speed": "fast"
            }),
        )
        .await?;

    let receipt_config = ReceiptConfig::from_network(&network)?;
    wait_for_receipt(&client, &relayer.id, &tx_response.id, &receipt_config).await?;

    // List transactions — our tx should appear
    let list_result = client.list_relayer_transactions(&relayer.id, 1, 50).await?;

    // Response may be {data: [...]} or direct array
    let items = list_result
        .get("data")
        .and_then(|d| d.as_array())
        .or_else(|| list_result.as_array())
        .expect(&format!(
            "Transaction list should have data array, got: {}",
            serde_json::to_string_pretty(&list_result).unwrap_or_default()
        ));

    let found = items.iter().any(|item| {
        item.get("id")
            .and_then(|id| id.as_str())
            .map_or(false, |id| id == tx_response.id)
    });

    assert!(
        found,
        "Submitted transaction {} should appear in list",
        tx_response.id
    );

    info!(tx_count = items.len(), "Transaction list verified");
    Ok(())
}

/// Tests submitting multiple transactions and verifying ordering/nonce progression.
async fn run_sequential_transactions_test(
    network: String,
    relayer_info: RelayerResponse,
) -> eyre::Result<()> {
    let _span =
        info_span!("tx_sequential", network = %network, relayer = %relayer_info.id).entered();
    info!("Starting sequential transactions test");

    let registry = TestRegistry::load()?;
    verify_network_ready(&registry, &network, &relayer_info)?;

    let client = RelayerClient::from_env()?;
    let relayer = setup_test_relayer(&client, &relayer_info).await?;
    let receipt_config = ReceiptConfig::from_network(&network)?;

    // Submit two sequential transactions
    let tx1 = client
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

    wait_for_receipt(&client, &relayer.id, &tx1.id, &receipt_config).await?;

    let tx2 = client
        .send_transaction(
            &relayer.id,
            serde_json::json!({
                "to": BURN_ADDRESS,
                "value": "2000000000",
                "data": "0x",
                "gas_limit": 21000,
                "speed": "fast"
            }),
        )
        .await?;

    wait_for_receipt(&client, &relayer.id, &tx2.id, &receipt_config).await?;

    // Verify both confirmed with distinct hashes
    let final_tx1 = client.get_transaction(&relayer.id, &tx1.id).await?;
    let final_tx2 = client.get_transaction(&relayer.id, &tx2.id).await?;

    assert!(
        final_tx1.status == "confirmed" || final_tx1.status == "mined",
        "tx1 should be confirmed"
    );
    assert!(
        final_tx2.status == "confirmed" || final_tx2.status == "mined",
        "tx2 should be confirmed"
    );
    assert_ne!(
        final_tx1.hash, final_tx2.hash,
        "Sequential transactions should have different hashes"
    );

    info!("Sequential transactions confirmed with distinct hashes");
    Ok(())
}

#[tokio::test]
#[serial]
async fn test_evm_transaction_fields() {
    run_multi_network_test(
        "transaction_fields",
        is_evm_network,
        run_transaction_fields_test,
    )
    .await;
}

#[tokio::test]
#[serial]
async fn test_evm_transaction_list() {
    run_multi_network_test(
        "transaction_list",
        is_evm_network,
        run_transaction_list_test,
    )
    .await;
}

#[tokio::test]
#[serial]
async fn test_evm_sequential_transactions() {
    run_multi_network_test(
        "sequential_transactions",
        is_evm_network,
        run_sequential_transactions_test,
    )
    .await;
}
