//! Relayer API integration tests

use crate::integration::common::{
    client::{CreateRelayerRequest, RelayerClient},
    network_selection::get_test_networks,
    registry::TestRegistry,
};
use openzeppelin_relayer::models::relayer::RelayerNetworkType;

/// Test creating and getting relayer details
#[tokio::test]
#[ignore = "Requires running relayer and funded signer"]
async fn test_relayer_crud() {
    // Get networks to test
    let networks = get_test_networks().expect("Failed to get test networks");

    if networks.is_empty() {
        panic!("No networks selected for testing");
    }

    // Load registry to filter for EVM networks
    let registry = TestRegistry::load().expect("Failed to load test registry");

    // Get first EVM network
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

    // Create relayer
    let relayer_id = uuid::Uuid::new_v4().to_string();
    let create_request = CreateRelayerRequest {
        id: Some(relayer_id.clone()),
        name: format!("Test Relayer - {}", network),
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

    // Get relayer details
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

    // Delete relayer
    client
        .delete_relayer(&relayer_id)
        .await
        .expect("Failed to delete relayer");

    println!("Deleted relayer: {}", relayer_id);

    // Verify relayer is deleted
    let result = client.get_relayer(&relayer_id).await;
    assert!(result.is_err(), "Relayer should not exist after deletion");

    println!("Relayer CRUD test passed!");
}

/// Test that getting a non-existent relayer returns an error
#[tokio::test]
#[ignore = "Requires running relayer"]
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
