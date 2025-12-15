//! Basic EVM transfer integration tests

use crate::integration::common::{
    client::RelayerClient,
    confirmation::{wait_for_receipt, ReceiptConfig},
    context::{is_evm_network, run_multi_network_test},
    evm_helpers::{setup_test_relayer, verify_network_ready},
    registry::{RelayerInfo, TestRegistry},
};
use serial_test::serial;
use tracing::{debug, info, info_span};

/// Burn address for test transfers
const BURN_ADDRESS: &str = "0x000000000000000000000000000000000000dEaD";

/// Test value for transfers (0.000001 ETH in wei)
const TRANSFER_VALUE: &str = "1000000000000";

async fn run_basic_transfer_test(network: String, relayer_info: RelayerInfo) -> eyre::Result<()> {
    let _span =
        info_span!("basic_transfer", network = %network, relayer = %relayer_info.id).entered();
    info!("Starting basic transfer test");

    let registry = TestRegistry::load()?;
    verify_network_ready(&registry, &network, &relayer_info)?;

    let client = RelayerClient::from_env()?;
    let relayer = setup_test_relayer(&client, &relayer_info).await?;

    // INFO: Condensed relayer info
    info!(relayer_id = %relayer.id, address = ?relayer.address, "Relayer ready");

    // DEBUG: Detailed test parameters (only shown with RUST_LOG=debug)
    debug!(relayer = ?relayer, "Full relayer details");
    debug!(
        burn_address = BURN_ADDRESS,
        transfer_value = TRANSFER_VALUE,
        "Test parameters"
    );

    let tx_request = serde_json::json!({
        "to": BURN_ADDRESS,
        "value": TRANSFER_VALUE,
        "data": "0x",
        "gas_limit": 21000,
        "speed": "fast"
    });

    let tx_response = client.send_transaction(&relayer.id, tx_request).await?;
    info!(
        tx_id = %tx_response.id,
        status = %tx_response.status,
        "Transaction submitted"
    );

    let receipt_config = ReceiptConfig::from_network(&network)?;
    debug!(
        poll_interval_ms = receipt_config.poll_interval_ms,
        max_wait_ms = receipt_config.max_wait_ms,
        "Waiting for confirmation"
    );

    wait_for_receipt(&client, &relayer.id, &tx_response.id, &receipt_config).await?;

    let final_tx = client.get_transaction(&relayer.id, &tx_response.id).await?;
    info!(
        tx_id = %final_tx.id,
        hash = ?final_tx.hash,
        status = %final_tx.status,
        "Transaction confirmed"
    );

    if final_tx.status != "confirmed" && final_tx.status != "mined" {
        return Err(eyre::eyre!(
            "Transaction status should be confirmed or mined, got: {}",
            final_tx.status
        ));
    }

    if final_tx.hash.is_none() {
        return Err(eyre::eyre!("Confirmed transaction should have a hash"));
    }

    info!("Test completed successfully");
    Ok(())
}

/// Test basic ETH transfer on all selected EVM networks
#[tokio::test]
#[serial]
async fn test_evm_basic_transfer() {
    run_multi_network_test("basic_transfer", is_evm_network, run_basic_transfer_test).await;
}
