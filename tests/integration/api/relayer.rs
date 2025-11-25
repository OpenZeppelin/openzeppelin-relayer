//! Relayer API integration tests

use crate::integration::common::{
    client::{CreateRelayerRequest, RelayerClient},
    network_selection::get_test_networks,
    registry::TestRegistry,
};
use openzeppelin_relayer::models::relayer::RelayerNetworkType;
use serial_test::serial;

/// Test creating and getting relayer details
#[tokio::test]
#[ignore = "Requires running relayer and funded signer"]
#[serial]
async fn test_relayer_crud() {
    let networks = get_test_networks().expect("Failed to get test networks");

    if networks.is_empty() {
        panic!("No networks selected for testing");
    }

    let registry = TestRegistry::load().expect("Failed to load test registry");

    let network = networks
        .iter()
        .find(|n| {
            registry
                .get_network(n)
                .map(|c| c.network_type == "evm")
                .unwrap_or(false)
        })
        .expect("No EVM network found in selection");

    let network_config = registry
        .get_network(network)
        .expect("Network not found in registry");

    println!("Testing relayer CRUD on: {}", network);

    let client = RelayerClient::from_env().expect("Failed to create RelayerClient");

    let cleanup_count = client
        .delete_all_relayers_by_network(network)
        .await
        .expect("Failed to clean up existing relayers");

    if cleanup_count > 0 {
        println!(
            "Cleaned up {} existing relayers for {}",
            cleanup_count, network
        );
    }

    let relayer_id = uuid::Uuid::new_v4().to_string();
    let create_request = CreateRelayerRequest {
        id: Some(relayer_id.clone()),
        name: format!("Test CRUD Relayer - {}", network),
        network: network.to_string(),
        paused: false,
        network_type: RelayerNetworkType::Evm,
        policies: None,
        signer_id: network_config.signer.id.clone(),
        notification_id: None,
        custom_rpc_urls: None,
    };

    let created = client
        .create_relayer(create_request)
        .await
        .expect("Failed to create relayer");

    println!("Created relayer: {}", created.id);
    assert_eq!(created.id, relayer_id);
    assert_eq!(created.network, network.to_string());
    assert_eq!(created.network_type, RelayerNetworkType::Evm);
    assert!(!created.paused);

    let fetched = client
        .get_relayer(&relayer_id)
        .await
        .expect("Failed to get relayer");

    println!("Fetched relayer: {}", fetched.id);
    println!("  Name: {}", fetched.name);
    println!("  Network: {}", fetched.network);
    println!("  Address: {:?}", fetched.address);
    println!("  System disabled: {:?}", fetched.system_disabled);
    println!("  Disabled reason: {:?}", fetched.disabled_reason);

    assert_eq!(fetched.id, relayer_id);
    assert_eq!(fetched.network, network.to_string());
    assert_eq!(fetched.signer_id, network_config.signer.id);
    assert!(fetched.address.is_some(), "Relayer should have an address");

    client
        .delete_relayer(&relayer_id)
        .await
        .expect("Failed to delete relayer");
    println!("Deleted relayer: {}", relayer_id);

    let result = client.get_relayer(&relayer_id).await;
    assert!(result.is_err(), "Relayer should not exist after deletion");

    println!("Relayer CRUD test passed!");
}

/// Test that getting a non-existent relayer returns an error
#[tokio::test]
#[ignore = "Requires running relayer"]
#[serial]
async fn test_get_nonexistent_relayer() {
    let client = RelayerClient::from_env().expect("Failed to create RelayerClient");

    let fake_id = uuid::Uuid::new_v4().to_string();
    let result = client.get_relayer(&fake_id).await;

    assert!(result.is_err(), "Should fail for non-existent relayer");

    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("404") || err.contains("not found"),
        "Error should indicate not found: {}",
        err
    );

    println!("Non-existent relayer test passed!");
}

/// Test deleting all relayers for a specific network
#[tokio::test]
#[ignore = "Requires running relayer and funded signer"]
#[serial]
async fn test_delete_all_relayers_by_network() {
    let networks = get_test_networks().expect("Failed to get test networks");

    if networks.is_empty() {
        panic!("No networks selected for testing");
    }

    let registry = TestRegistry::load().expect("Failed to load test registry");

    let network = networks
        .iter()
        .find(|n| {
            registry
                .get_network(n)
                .map(|c| c.network_type == "evm")
                .unwrap_or(false)
        })
        .expect("No EVM network found in selection");

    let network_config = registry
        .get_network(network)
        .expect("Network not found in registry");

    println!("Testing cleanup on: {}", network);

    let client = RelayerClient::from_env().expect("Failed to create RelayerClient");

    let initial_cleanup = client
        .delete_all_relayers_by_network(network)
        .await
        .expect("Failed to clean up existing relayers");

    if initial_cleanup > 0 {
        println!(
            "Cleaned up {} existing relayers for {}",
            initial_cleanup, network
        );
    }

    let relayer_id = uuid::Uuid::new_v4().to_string();
    let create_request = CreateRelayerRequest {
        id: Some(relayer_id.clone()),
        name: format!("Test Cleanup Relayer - {}", network),
        network: network.to_string(),
        paused: false,
        network_type: RelayerNetworkType::Evm,
        policies: None,
        signer_id: network_config.signer.id.clone(),
        notification_id: None,
        custom_rpc_urls: None,
    };

    let created = client
        .create_relayer(create_request)
        .await
        .expect("Failed to create relayer");

    println!("Created relayer: {}", created.id);

    let all_relayers = client
        .list_relayers()
        .await
        .expect("Failed to list relayers");

    let network_relayers_count = all_relayers
        .iter()
        .filter(|r| r.network == *network)
        .count();
    println!("Total relayers for {}: {}", network, network_relayers_count);
    assert!(
        network_relayers_count >= 1,
        "Should have at least 1 relayer for the network"
    );

    let deleted_count = client
        .delete_all_relayers_by_network(network)
        .await
        .expect("Failed to delete relayers");

    println!("Deleted {} relayers for {}", deleted_count, network);
    assert!(deleted_count >= 1, "Should have deleted at least 1 relayer");

    let remaining_relayers = client
        .list_relayers()
        .await
        .expect("Failed to list relayers");

    let remaining_network_relayers: Vec<_> = remaining_relayers
        .iter()
        .filter(|r| r.network == *network)
        .collect();

    assert!(
        remaining_network_relayers.is_empty(),
        "Should have no remaining relayers for {}, but found: {:?}",
        network,
        remaining_network_relayers
            .iter()
            .map(|r| &r.id)
            .collect::<Vec<_>>()
    );

    println!("Delete all relayers by network test passed!");
}
