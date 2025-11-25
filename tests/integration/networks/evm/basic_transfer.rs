//! Basic EVM transfer integration tests

use crate::integration::common::{
    client::RelayerClient,
    confirmation::{wait_for_receipt, ReceiptConfig},
    network_selection::get_test_networks,
    registry::TestRegistry,
};
use serial_test::serial;

use super::helpers::{setup_test_relayer, verify_network_ready};

/// Burn address for test transfers
const BURN_ADDRESS: &str = "0x000000000000000000000000000000000000dEaD";

/// Test value for transfers (0.0001 ETH in wei)
const TRANSFER_VALUE: &str = "100000000000000";

async fn run_basic_transfer_test(network: &str) -> eyre::Result<()> {
    println!("\n{}", "=".repeat(60));
    println!("Testing basic transfer on: {}", network);
    println!("{}\n", "=".repeat(60));

    let registry = TestRegistry::load()?;
    verify_network_ready(&registry, network)?;

    let client = RelayerClient::from_env()?;
    let relayer = setup_test_relayer(&client, &registry, network).await?;

    let tx_request = serde_json::json!({
        "to": BURN_ADDRESS,
        "value": TRANSFER_VALUE,
        "data": "0x",
        "gas_limit": 21000,
        "speed": "fast"
    });

    let tx_response = client.send_transaction(&relayer.id, tx_request).await?;
    println!(
        "Sent transaction {} with status {}",
        tx_response.id, tx_response.status
    );

    let receipt_config = ReceiptConfig::from_network(network)?;
    println!(
        "Waiting for confirmation (poll: {}ms, max wait: {}ms)",
        receipt_config.poll_interval_ms, receipt_config.max_wait_ms
    );

    wait_for_receipt(&client, &relayer.id, &tx_response.id, &receipt_config).await?;

    let final_tx = client.get_transaction(&relayer.id, &tx_response.id).await?;
    println!(
        "Transaction confirmed! Hash: {:?}, Status: {}",
        final_tx.hash, final_tx.status
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

    println!("Test completed successfully for {}\n", network);
    Ok(())
}

/// Test basic ETH transfer on all selected EVM networks
#[tokio::test]
#[ignore = "Requires running relayer and funded signer"]
#[serial]
async fn test_evm_basic_transfer() {
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
        println!("No EVM networks in selection, skipping test");
        return;
    }

    println!(
        "Testing {} EVM networks: {:?}",
        evm_networks.len(),
        evm_networks
    );

    let mut failures = Vec::new();

    // Run test for each EVM network
    for network in &evm_networks {
        match run_basic_transfer_test(network).await {
            Ok(()) => {
                println!("PASS: {}", network);
            }
            Err(e) => {
                eprintln!("FAIL: {} - {}", network, e);
                failures.push((network.clone(), e.to_string()));
            }
        }
    }

    // Report results
    println!("\n{}", "=".repeat(60));
    println!("Test Summary");
    println!("{}", "=".repeat(60));
    println!(
        "Passed: {}/{}",
        evm_networks.len() - failures.len(),
        evm_networks.len()
    );

    if !failures.is_empty() {
        println!("\nFailures:");
        for (network, error) in &failures {
            println!("  - {}: {}", network, error);
        }
        panic!(
            "{} of {} EVM network tests failed",
            failures.len(),
            evm_networks.len()
        );
    }

    println!("\nAll EVM basic transfer tests passed!");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constants() {
        // Verify burn address is valid
        assert!(BURN_ADDRESS.starts_with("0x"));
        assert_eq!(BURN_ADDRESS.len(), 42);

        // Verify transfer value is reasonable (0.0001 ETH)
        let value: u128 = TRANSFER_VALUE.parse().unwrap();
        assert_eq!(value, 100_000_000_000_000); // 0.0001 ETH in wei
    }
}
