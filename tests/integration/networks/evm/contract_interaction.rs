//! Contract interaction integration tests

use crate::integration::common::{
    client::RelayerClient,
    confirmation::{wait_for_receipt, ReceiptConfig},
    network_selection::get_test_networks,
    registry::TestRegistry,
};
use serial_test::serial;

use super::helpers::{setup_test_relayer, verify_network_ready};

/// SimpleStorage contract function selector for setNumber(uint256)
/// keccak256("setNumber(uint256)")[0:4] = 0x3fb5c1cb
const SET_NUMBER_SELECTOR: &str = "3fb5c1cb";

/// Encode a call to SimpleStorage.setNumber(uint256)
fn encode_set_number_call(value: u64) -> String {
    // Function selector + uint256 value (32 bytes, zero-padded)
    format!("0x{}{:064x}", SET_NUMBER_SELECTOR, value)
}

async fn run_contract_interaction_test(network: &str) -> eyre::Result<()> {
    println!("\n{}", "=".repeat(60));
    println!("Testing contract interaction on: {}", network);
    println!("{}\n", "=".repeat(60));

    let registry = TestRegistry::load()?;
    verify_network_ready(&registry, network)?;

    let contract_address = match registry.get_contract(network, "simple_storage") {
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

    println!("SimpleStorage contract: {}", contract_address);

    let client = RelayerClient::from_env()?;
    let relayer = setup_test_relayer(&client, &registry, network).await?;

    let test_value = rand::random::<u64>() % 1_000_000;
    let call_data = encode_set_number_call(test_value);

    println!("Calling SimpleStorage.setNumber({})", test_value);
    println!("Call data: {}", call_data);

    let tx_request = serde_json::json!({
        "to": contract_address,
        "value": "0",
        "data": call_data,
        "gas_limit": 200000,
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

    if let Err(e) = wait_for_receipt(&client, &relayer.id, &tx_response.id, &receipt_config).await {
        if let Ok(failed_tx) = client.get_transaction(&relayer.id, &tx_response.id).await {
            eprintln!("Transaction failed details:");
            eprintln!("  Status: {}", failed_tx.status);
            eprintln!("  Hash: {:?}", failed_tx.hash);
            eprintln!("  Error: {:?}", failed_tx.error);
        }
        return Err(e);
    }

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

    println!(
        "Contract interaction test completed successfully for {}\n",
        network
    );
    Ok(())
}

/// Test SimpleStorage contract interaction on all selected EVM networks
#[tokio::test]
#[ignore = "Requires running relayer, funded signer, and deployed contract"]
#[serial]
async fn test_evm_contract_interaction() {
    let networks = get_test_networks().expect("Failed to get test networks");

    if networks.is_empty() {
        panic!("No networks selected for testing");
    }

    let registry = TestRegistry::load().expect("Failed to load test registry");

    let eligible_networks: Vec<String> = networks
        .into_iter()
        .filter(|network| {
            let is_evm = registry
                .get_network(network)
                .map(|config| config.network_type == "evm")
                .unwrap_or(false);

            let has_contract = registry
                .has_real_contract(network, "simple_storage")
                .unwrap_or(false);

            is_evm && has_contract
        })
        .collect();

    if eligible_networks.is_empty() {
        println!("No EVM networks with deployed SimpleStorage contract, skipping test");
        return;
    }

    println!(
        "Testing {} EVM networks with SimpleStorage: {:?}",
        eligible_networks.len(),
        eligible_networks
    );

    let mut failures = Vec::new();

    // Run test for each eligible network
    for network in &eligible_networks {
        match run_contract_interaction_test(network).await {
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
    println!("Contract Interaction Test Summary");
    println!("{}", "=".repeat(60));
    println!(
        "Passed: {}/{}",
        eligible_networks.len() - failures.len(),
        eligible_networks.len()
    );

    if !failures.is_empty() {
        println!("\nFailures:");
        for (network, error) in &failures {
            println!("  - {}: {}", network, error);
        }
        panic!(
            "{} of {} contract interaction tests failed",
            failures.len(),
            eligible_networks.len()
        );
    }

    println!("\nAll contract interaction tests passed!");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_set_number_call() {
        // Test encoding setNumber(42)
        let encoded = encode_set_number_call(42);
        assert!(encoded.starts_with("0x3fb5c1cb"));
        assert_eq!(encoded.len(), 2 + 8 + 64); // 0x + selector + 32 bytes

        // Value should be at the end, zero-padded
        assert!(encoded.ends_with("2a")); // 42 in hex

        // Test encoding setNumber(0)
        let encoded_zero = encode_set_number_call(0);
        assert!(encoded_zero.ends_with(&"0".repeat(64)));

        // Test encoding setNumber(255)
        let encoded_ff = encode_set_number_call(255);
        assert!(encoded_ff.ends_with("ff"));
    }

    #[test]
    fn test_function_selector() {
        // Verify the function selector is correct for setNumber(uint256)
        assert_eq!(SET_NUMBER_SELECTOR, "3fb5c1cb");
    }
}
