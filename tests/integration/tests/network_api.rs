//! Network API integration tests
//!
//! Tests for the network REST API endpoints including listing, retrieving,
//! and updating network configurations.

use crate::integration::common::client::RelayerClient;
use openzeppelin_relayer::models::{RpcConfig, UpdateNetworkRequest};
use serial_test::serial;
use std::time::Duration;
use tokio::time::sleep;

// =============================================================================
// List Networks Tests
// =============================================================================

/// Tests that listing networks returns a successful response with networks
#[tokio::test]
#[serial]
async fn test_list_networks() {
    let client = RelayerClient::from_env().expect("Failed to create client");
    let networks = client
        .list_networks(None, None)
        .await
        .expect("Failed to list networks");

    assert!(!networks.is_empty(), "Should have at least one network");

    // Verify network structure
    let network = &networks[0];
    assert!(!network.id.is_empty(), "Network ID should not be empty");
    assert!(!network.name.is_empty(), "Network name should not be empty");
}

/// Tests pagination for listing networks
#[tokio::test]
#[serial]
async fn test_list_networks_pagination() {
    let client = RelayerClient::from_env().expect("Failed to create client");

    // Get first page
    let page1 = client
        .list_networks(Some(1), Some(2))
        .await
        .expect("Failed to list networks page 1");

    // Get second page
    let page2 = client
        .list_networks(Some(2), Some(2))
        .await
        .expect("Failed to list networks page 2");

    // Pages should not overlap (if we have enough networks)
    if !page1.is_empty() && !page2.is_empty() {
        let page1_ids: std::collections::HashSet<_> = page1.iter().map(|n| &n.id).collect();
        let page2_ids: std::collections::HashSet<_> = page2.iter().map(|n| &n.id).collect();
        assert!(
            page1_ids.is_disjoint(&page2_ids),
            "Page 1 and Page 2 should not have overlapping networks"
        );
    }
}

// =============================================================================
// Get Network Tests
// =============================================================================

/// Tests that getting a network by ID returns the correct network
#[tokio::test]
#[serial]
async fn test_get_network_by_id() {
    let client = RelayerClient::from_env().expect("Failed to create client");

    // First, list networks to get a valid network ID
    let networks = client
        .list_networks(None, None)
        .await
        .expect("Failed to list networks");
    assert!(!networks.is_empty(), "Should have at least one network");

    let network_id = &networks[0].id;

    // Get the network by ID
    let network = client
        .get_network(network_id)
        .await
        .expect("Failed to get network");

    assert_eq!(network.id, *network_id, "Network ID should match");
    assert_eq!(network.name, networks[0].name, "Network name should match");
    assert_eq!(
        network.network_type, networks[0].network_type,
        "Network type should match"
    );
}

/// Tests that getting a non-existent network returns 404
#[tokio::test]
#[serial]
async fn test_get_network_not_found() {
    let client = RelayerClient::from_env().expect("Failed to create client");

    // Try to get a network that doesn't exist
    let result = client.get_network("nonexistent:network").await;

    assert!(
        result.is_err(),
        "Should return an error for non-existent network"
    );
    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("404") || error_msg.contains("not found"),
        "Error should indicate network not found: {}",
        error_msg
    );
}

// =============================================================================
// Update Network Tests
// =============================================================================

/// Helper function to find a network that is not actively being used by relayers
/// Returns None if no suitable network is found
///
/// This helps prevent network API tests from interfering with relayer tests by
/// avoiding modification of networks that relayers depend on.
async fn find_unused_network(
    client: &RelayerClient,
) -> eyre::Result<Option<openzeppelin_relayer::models::NetworkResponse>> {
    let networks = client.list_networks(None, None).await?;

    // If no networks exist, return None
    if networks.is_empty() {
        return Ok(None);
    }

    let relayers = client.list_relayers().await.unwrap_or_default();

    // Extract network names from relayers (format: "network_name" from relayer.network)
    let used_networks: std::collections::HashSet<String> =
        relayers.iter().map(|r| r.network.clone()).collect();

    // Find a network that's not being used by any relayers
    for network in &networks {
        // Extract network name from ID (format: "evm:sepolia" -> "sepolia")
        let network_name = network.id.split(':').nth(1).unwrap_or(&network.id);
        if !used_networks.contains(network_name) {
            return Ok(Some(network.clone()));
        }
    }

    // If all networks are in use, return None to skip the test rather than risk breaking relayers
    Ok(None)
}

/// Helper function to wait for relayers using a network to recover after RPC URL restore
async fn wait_for_relayer_recovery(client: &RelayerClient, network_name: &str) {
    // Wait a bit for health checks to potentially re-enable relayers
    sleep(Duration::from_secs(2)).await;

    // Check if any relayers using this network are disabled
    if let Ok(relayers) = client.list_relayers().await {
        let disabled_relayers: Vec<_> = relayers
            .iter()
            .filter(|r| r.network == network_name && r.system_disabled == Some(true))
            .collect();

        if !disabled_relayers.is_empty() {
            // Wait a bit more for health checks to complete
            sleep(Duration::from_secs(3)).await;
        }
    }
}

/// Tests updating a network's RPC URLs with simple string format
#[tokio::test]
#[serial]
async fn test_update_network_rpc_urls_simple_format() {
    let client = RelayerClient::from_env().expect("Failed to create client");

    // Find a network that's not actively being used by relayers
    // Skip this test if all networks are in use to avoid interfering with relayer tests
    let network = match find_unused_network(&client).await {
        Ok(Some(network)) => network,
        Ok(None) => {
            return; // Skip test if all networks are in use
        }
        Err(e) => {
            panic!("Failed to find network: {}", e);
        }
    };

    let network_id = &network.id;
    let network_name = network.id.split(':').nth(1).unwrap_or(&network.id);
    let original_network = client
        .get_network(network_id)
        .await
        .expect("Failed to get original network");

    // Update with simple string format
    let new_rpc_urls = vec![
        "https://rpc1.example.com".to_string(),
        "https://rpc2.example.com".to_string(),
    ];

    // Create update request with simple string format
    let update_request = UpdateNetworkRequest {
        rpc_urls: Some(
            new_rpc_urls
                .iter()
                .map(|url| RpcConfig::new(url.clone()))
                .collect(),
        ),
    };

    let updated_network = client
        .update_network(network_id, update_request)
        .await
        .expect("Failed to update network");

    // Verify the update
    assert_eq!(
        updated_network.id, *network_id,
        "Network ID should not change"
    );
    assert!(
        updated_network.rpc_urls.is_some(),
        "RPC URLs should be present"
    );
    let updated_urls: Vec<String> = updated_network
        .rpc_urls
        .unwrap()
        .iter()
        .map(|config| config.url.clone())
        .collect();
    assert_eq!(updated_urls.len(), 2, "Should have 2 RPC URLs");
    assert!(
        updated_urls.contains(&"https://rpc1.example.com".to_string()),
        "Should contain rpc1.example.com"
    );
    assert!(
        updated_urls.contains(&"https://rpc2.example.com".to_string()),
        "Should contain rpc2.example.com"
    );

    // Restore original RPC URLs if they existed
    if let Some(original_urls) = original_network.rpc_urls {
        let restore_request = UpdateNetworkRequest {
            rpc_urls: Some(original_urls),
        };
        let _ = client
            .update_network(network_id, restore_request)
            .await
            .expect("Failed to restore original RPC URLs");

        // Wait for relayers using this network to recover if any were disabled
        wait_for_relayer_recovery(&client, network_name).await;
    }
}

/// Tests updating a network's RPC URLs with extended format (with weights)
#[tokio::test]
#[serial]
async fn test_update_network_rpc_urls_extended_format() {
    let client = RelayerClient::from_env().expect("Failed to create client");

    // Find a network that's not actively being used by relayers
    // Skip this test if all networks are in use to avoid interfering with relayer tests
    let network = match find_unused_network(&client).await {
        Ok(Some(network)) => network,
        Ok(None) => {
            return; // Skip test if all networks are in use
        }
        Err(e) => {
            panic!("Failed to find network: {}", e);
        }
    };

    let network_id = &network.id;
    let network_name = network.id.split(':').nth(1).unwrap_or(&network.id);
    let original_network = client
        .get_network(network_id)
        .await
        .expect("Failed to get original network");

    // Update with extended format (with weights)
    let update_request = UpdateNetworkRequest {
        rpc_urls: Some(vec![
            RpcConfig {
                url: "https://rpc-weighted1.example.com".to_string(),
                weight: 80,
            },
            RpcConfig {
                url: "https://rpc-weighted2.example.com".to_string(),
                weight: 20,
            },
        ]),
    };

    let updated_network = client
        .update_network(network_id, update_request)
        .await
        .expect("Failed to update network");

    // Verify the update
    assert_eq!(
        updated_network.id, *network_id,
        "Network ID should not change"
    );
    assert!(
        updated_network.rpc_urls.is_some(),
        "RPC URLs should be present"
    );
    let updated_configs = updated_network.rpc_urls.unwrap();
    assert_eq!(updated_configs.len(), 2, "Should have 2 RPC configs");

    // Verify weights
    let config1 = updated_configs
        .iter()
        .find(|c| c.url == "https://rpc-weighted1.example.com")
        .expect("Should find rpc-weighted1");
    assert_eq!(config1.weight, 80, "First RPC should have weight 80");

    let config2 = updated_configs
        .iter()
        .find(|c| c.url == "https://rpc-weighted2.example.com")
        .expect("Should find rpc-weighted2");
    assert_eq!(config2.weight, 20, "Second RPC should have weight 20");

    // Restore original RPC URLs if they existed
    if let Some(original_urls) = original_network.rpc_urls {
        let restore_request = UpdateNetworkRequest {
            rpc_urls: Some(original_urls),
        };
        let _ = client
            .update_network(network_id, restore_request)
            .await
            .expect("Failed to restore original RPC URLs");

        // Wait for relayers using this network to recover if any were disabled
        wait_for_relayer_recovery(&client, network_name).await;
    }
}

/// Tests updating a network with empty RPC URLs (should fail validation)
#[tokio::test]
#[serial]
async fn test_update_network_empty_rpc_urls() {
    let client = RelayerClient::from_env().expect("Failed to create client");

    // Get an existing network
    let networks = client
        .list_networks(None, None)
        .await
        .expect("Failed to list networks");
    assert!(!networks.is_empty(), "Should have at least one network");

    let network_id = &networks[0].id;

    // Try to update with empty RPC URLs
    let update_request = UpdateNetworkRequest {
        rpc_urls: Some(vec![]),
    };

    let result = client.update_network(network_id, update_request).await;

    assert!(result.is_err(), "Should fail validation for empty RPC URLs");
    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("400")
            || error_msg.contains("validation")
            || error_msg.contains("empty"),
        "Error should indicate validation failure: {}",
        error_msg
    );
}

/// Tests updating a network with invalid RPC URL format
#[tokio::test]
#[serial]
async fn test_update_network_invalid_rpc_url() {
    let client = RelayerClient::from_env().expect("Failed to create client");

    // Get an existing network
    let networks = client
        .list_networks(None, None)
        .await
        .expect("Failed to list networks");
    assert!(!networks.is_empty(), "Should have at least one network");

    let network_id = &networks[0].id;

    // Try to update with invalid RPC URL (not HTTP/HTTPS)
    let update_request = UpdateNetworkRequest {
        rpc_urls: Some(vec![RpcConfig::new("invalid-url".to_string())]),
    };

    let result = client.update_network(network_id, update_request).await;

    assert!(
        result.is_err(),
        "Should fail validation for invalid RPC URL"
    );
    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("400")
            || error_msg.contains("validation")
            || error_msg.contains("invalid"),
        "Error should indicate validation failure: {}",
        error_msg
    );
}

/// Tests updating a non-existent network returns 404
#[tokio::test]
#[serial]
async fn test_update_network_not_found() {
    let client = RelayerClient::from_env().expect("Failed to create client");

    // Try to update a network that doesn't exist
    let update_request = UpdateNetworkRequest {
        rpc_urls: Some(vec![RpcConfig::new("https://rpc.example.com".to_string())]),
    };

    let result = client
        .update_network("nonexistent:network", update_request)
        .await;

    assert!(
        result.is_err(),
        "Should return an error for non-existent network"
    );
    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("404") || error_msg.contains("not found"),
        "Error should indicate network not found: {}",
        error_msg
    );
}

/// Tests updating a network with empty request (should fail)
#[tokio::test]
#[serial]
async fn test_update_network_empty_request() {
    let client = RelayerClient::from_env().expect("Failed to create client");

    // Get an existing network
    let networks = client
        .list_networks(None, None)
        .await
        .expect("Failed to list networks");
    assert!(!networks.is_empty(), "Should have at least one network");

    let network_id = &networks[0].id;

    // Try to update with empty request (no fields provided)
    let update_request = UpdateNetworkRequest { rpc_urls: None };

    let result = client.update_network(network_id, update_request).await;

    assert!(result.is_err(), "Should fail when no fields are provided");
    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("400") || error_msg.contains("field") || error_msg.contains("required"),
        "Error should indicate that at least one field is required: {}",
        error_msg
    );
}
