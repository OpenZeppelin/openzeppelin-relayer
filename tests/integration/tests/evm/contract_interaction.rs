//! Contract interaction integration tests

use crate::integration::common::{
    client::RelayerClient,
    confirmation::{wait_for_receipt, ReceiptConfig},
    context::{evm_with_contract, run_multi_network_test},
    evm_helpers::{setup_test_relayer, verify_network_ready},
    registry::{RelayerInfo, TestRegistry},
};
use serial_test::serial;
use tracing::{debug, error, info, info_span};

/// SimpleStorage contract function selector for setNumber(uint256)
/// keccak256("setNumber(uint256)")[0:4] = 0x3fb5c1cb
const SET_NUMBER_SELECTOR: &str = "3fb5c1cb";

/// Encode a call to SimpleStorage.setNumber(uint256)
fn encode_set_number_call(value: u64) -> String {
    // Function selector + uint256 value (32 bytes, zero-padded)
    format!("0x{}{:064x}", SET_NUMBER_SELECTOR, value)
}

async fn run_contract_interaction_test(
    network: String,
    relayer_info: RelayerInfo,
) -> eyre::Result<()> {
    let _span = info_span!("contract_interaction", network = %network, relayer = %relayer_info.id)
        .entered();
    info!("Starting contract interaction test");

    let registry = TestRegistry::load()?;
    verify_network_ready(&registry, &network, &relayer_info)?;

    let contract_address = match registry.get_contract(&network, "simple_storage") {
        Ok(addr) => {
            if addr.starts_with("0x0000000000000000") {
                return Err(eyre::eyre!(
                    "SimpleStorage contract not deployed on {} (placeholder address)",
                    network
                ));
            }
            addr.clone()
        }
        Err(_) => {
            return Err(eyre::eyre!(
                "SimpleStorage contract not found in registry for {}",
                network
            ));
        }
    };

    info!(contract = %contract_address, "SimpleStorage contract");

    let client = RelayerClient::from_env()?;
    let relayer = setup_test_relayer(&client, &relayer_info).await?;

    let test_value = rand::random::<u64>() % 1_000_000;
    let call_data = encode_set_number_call(test_value);

    info!(value = test_value, "Calling SimpleStorage.setNumber");
    debug!(call_data = %call_data, "Call data");

    let tx_request = serde_json::json!({
        "to": contract_address,
        "value": "0",
        "data": call_data,
        "gas_limit": 200000,
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

    if let Err(e) = wait_for_receipt(&client, &relayer.id, &tx_response.id, &receipt_config).await {
        if let Ok(failed_tx) = client.get_transaction(&relayer.id, &tx_response.id).await {
            error!(
                status = %failed_tx.status,
                hash = ?failed_tx.hash,
                error = ?failed_tx.error,
                "Transaction failed"
            );
        }
        return Err(e);
    }

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

    info!("Contract interaction test completed successfully");
    Ok(())
}

/// Test SimpleStorage contract interaction on all selected EVM networks
#[tokio::test]
#[serial]
async fn test_evm_contract_interaction() {
    run_multi_network_test(
        "contract_interaction",
        evm_with_contract("simple_storage"),
        run_contract_interaction_test,
    )
    .await;
}
