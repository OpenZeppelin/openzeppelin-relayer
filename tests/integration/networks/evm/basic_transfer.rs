//! Basic EVM transfer integration tests

use crate::integration::common::{
    client::RelayerClient,
    confirmation::{wait_for_receipt, ReceiptConfig},
    logging::init_test_logging,
    network_selection::get_test_networks,
    registry::TestRegistry,
};
use serial_test::serial;
use tracing::{debug, error, info, info_span};

use super::helpers::{setup_test_relayer, verify_network_ready};

/// Burn address for test transfers
const BURN_ADDRESS: &str = "0x000000000000000000000000000000000000dEaD";

/// Test value for transfers (0.000001 ETH in wei)
const TRANSFER_VALUE: &str = "1000000000000";

async fn run_basic_transfer_test(network: &str) -> eyre::Result<()> {
    let _span = info_span!("basic_transfer", network = %network).entered();
    info!("Starting basic transfer test");

    let registry = TestRegistry::load()?;
    verify_network_ready(&registry, network)?;

    let client = RelayerClient::from_env()?;
    let relayer = setup_test_relayer(&client, &registry, network).await?;

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

    let receipt_config = ReceiptConfig::from_network(network)?;
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
    init_test_logging();

    let _span = info_span!("test_evm_basic_transfer").entered();

    let networks = get_test_networks().expect("Failed to get test networks");

    if networks.is_empty() {
        panic!("No networks selected for testing");
    }

    let registry = TestRegistry::load().expect("Failed to load test registry");

    let evm_networks: Vec<String> = networks
        .into_iter()
        .filter(|network| {
            registry
                .get_network(network)
                .map(|config| config.network_type == "evm")
                .unwrap_or(false)
        })
        .collect();

    if evm_networks.is_empty() {
        info!("No EVM networks in selection, skipping test");
        return;
    }

    info!(
        count = evm_networks.len(),
        networks = ?evm_networks,
        "Testing EVM networks"
    );

    let mut failures = Vec::new();

    // Run test for each EVM network
    for network in &evm_networks {
        match run_basic_transfer_test(network).await {
            Ok(()) => {
                info!(network = %network, "PASS");
            }
            Err(e) => {
                error!(network = %network, error = %e, "FAIL");
                failures.push((network.clone(), e.to_string()));
            }
        }
    }

    // Report results
    info!("Test Summary");
    info!(
        passed = evm_networks.len() - failures.len(),
        total = evm_networks.len(),
        "Results"
    );

    if !failures.is_empty() {
        error!("Failures:");
        for (network, error) in &failures {
            error!(network = %network, error = %error, "Test failed");
        }
        panic!(
            "{} of {} EVM network tests failed",
            failures.len(),
            evm_networks.len()
        );
    }

    info!("All EVM basic transfer tests passed!");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constants() {
        // Verify burn address is valid
        assert!(BURN_ADDRESS.starts_with("0x"));
        assert_eq!(BURN_ADDRESS.len(), 42);

        // Verify transfer value is reasonable (0.000001 ETH)
        let value: u128 = TRANSFER_VALUE.parse().unwrap();
        assert_eq!(value, 1_000_000_000_000); // 0.000001 ETH in wei
    }
}
