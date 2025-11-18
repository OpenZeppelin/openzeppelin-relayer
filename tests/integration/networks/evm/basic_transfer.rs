//! Basic EVM transfer integration tests
//!
//! Tests basic ETH transfers on EVM networks.

use crate::integration::common::{
    client::{CreateRelayerRequest, RelayerClient},
    confirmation::{wait_for_receipt, ReceiptConfig},
    network_selection::get_test_networks,
    registry::TestRegistry,
};
use openzeppelin_relayer::models::relayer::RelayerNetworkType;

/// Burn address for test transfers
const BURN_ADDRESS: &str = "0x000000000000000000000000000000000000dEaD";

/// Test value for transfers (0.0001 ETH in wei)
const TRANSFER_VALUE: &str = "100000000000000";

/// Run a basic transfer test for a single EVM network
async fn run_basic_transfer_test(network: &str) -> eyre::Result<()> {
    println!("\n{}", "=".repeat(60));
    println!("Testing basic transfer on: {}", network);
    println!("{}\n", "=".repeat(60));

    // Load registry and get network config
    let registry = TestRegistry::load()?;
    let network_config = registry.get_network(network)?;

    // Verify it's an EVM network
    if network_config.network_type != "evm" {
        println!("Skipping {} - not an EVM network", network);
        return Ok(());
    }

    // Verify network is ready for testing
    let readiness = registry.validate_readiness(network)?;
    if !readiness.ready {
        return Err(eyre::eyre!(
            "Network {} is not ready: enabled={}, has_signer={}, has_contracts={}",
            network,
            readiness.enabled,
            readiness.has_signer,
            readiness.has_contracts
        ));
    }

    // Create client from environment
    let client = RelayerClient::from_env()?;

    // Create a unique relayer ID for this test (max 36 chars)
    let relayer_id = uuid::Uuid::new_v4().to_string();

    // Create relayer request
    let create_request = CreateRelayerRequest {
        id: Some(relayer_id.clone()),
        name: format!("Integration Test - {} Basic Transfer", network),
        network: network.to_string(),
        paused: false,
        network_type: RelayerNetworkType::Evm,
        policies: None,
        signer_id: network_config.signer.id.clone(),
        notification_id: None,
        custom_rpc_urls: None,
    };

    // Create the relayer
    let relayer = client.create_relayer(create_request).await?;

    println!(
        "Created relayer {} with address {:?}, system_disabled: {:?}, disabled_reason: {:?}",
        relayer.id, relayer.address, relayer.system_disabled, relayer.disabled_reason
    );

    // Check if relayer is disabled
    if relayer.system_disabled == Some(true) {
        let reason = relayer
            .disabled_reason
            .map(|r| format!("{:?}", r))
            .unwrap_or_else(|| "unknown".to_string());

        // Cleanup before returning error
        let _ = client.delete_relayer(&relayer.id).await;

        return Err(eyre::eyre!(
            "Relayer was created but is disabled: {}. Check signer balance and configuration.",
            reason
        ));
    }

    // Wait for health check to run and update balance, then verify relayer is still enabled
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    let relayer_status = client.get_relayer(&relayer.id).await?;
    if relayer_status.system_disabled == Some(true) {
        let reason = relayer_status
            .disabled_reason
            .map(|r| format!("{:?}", r))
            .unwrap_or_else(|| "unknown".to_string());

        let _ = client.delete_relayer(&relayer.id).await;

        return Err(eyre::eyre!(
            "Relayer was disabled after creation: {}. The signer likely has insufficient balance (needs {} for {}).",
            reason,
            network_config.min_balance,
            network
        ));
    }

    // Prepare transaction request
    let tx_request = serde_json::json!({
        "to": BURN_ADDRESS,
        "value": TRANSFER_VALUE,
        "data": "0x",
        "gas_limit": 21000,
        "speed": "fast"
    });

    // Send the transaction
    let tx_response = match client.send_transaction(&relayer.id, tx_request).await {
        Ok(response) => response,
        Err(e) => {
            // Cleanup on error
            let _ = client.delete_relayer(&relayer.id).await;
            return Err(e);
        }
    };

    println!(
        "Sent transaction {} with status {}",
        tx_response.id, tx_response.status
    );

    // Wait for confirmation with network-aware timing
    let receipt_config = ReceiptConfig::from_network(network)?;

    println!(
        "Waiting for confirmation (poll: {}ms, max wait: {}ms)",
        receipt_config.poll_interval_ms, receipt_config.max_wait_ms
    );

    if let Err(e) = wait_for_receipt(&client, &relayer.id, &tx_response.id, &receipt_config).await {
        // Cleanup on error
        let _ = client.delete_relayer(&relayer.id).await;
        return Err(e);
    }

    // Get final transaction status
    let final_tx = match client.get_transaction(&relayer.id, &tx_response.id).await {
        Ok(tx) => tx,
        Err(e) => {
            let _ = client.delete_relayer(&relayer.id).await;
            return Err(e);
        }
    };

    println!(
        "Transaction confirmed! Hash: {:?}, Status: {}",
        final_tx.hash, final_tx.status
    );

    // Assert transaction was successful
    if final_tx.status != "confirmed" && final_tx.status != "mined" {
        let _ = client.delete_relayer(&relayer.id).await;
        return Err(eyre::eyre!(
            "Transaction status should be confirmed or mined, got: {}",
            final_tx.status
        ));
    }

    if final_tx.hash.is_none() {
        let _ = client.delete_relayer(&relayer.id).await;
        return Err(eyre::eyre!("Confirmed transaction should have a hash"));
    }

    // Cleanup: delete the relayer
    client.delete_relayer(&relayer.id).await?;
    println!("Deleted relayer {}", relayer.id);

    println!("Test completed successfully for {}\n", network);

    Ok(())
}

/// Test basic ETH transfer on all selected EVM networks
///
/// Networks are selected via:
/// - TEST_NETWORKS env var (comma-separated list)
/// - TEST_TAGS env var (e.g., "evm,quick")
/// - TEST_MODE env var (quick, ci, full)
///
/// This test:
/// 1. Gets the list of networks to test
/// 2. Filters for EVM networks
/// 3. For each network:
///    - Creates a relayer
///    - Sends a small ETH transfer to the burn address
///    - Waits for confirmation
///    - Cleans up
#[tokio::test]
#[ignore = "Requires running relayer and funded signer"]
async fn test_evm_basic_transfer() {
    // Get networks to test based on environment configuration
    let networks = get_test_networks().expect("Failed to get test networks");

    if networks.is_empty() {
        panic!("No networks selected for testing");
    }

    // Load registry to filter for EVM networks
    let registry = TestRegistry::load().expect("Failed to load test registry");

    // Filter for EVM networks only
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
