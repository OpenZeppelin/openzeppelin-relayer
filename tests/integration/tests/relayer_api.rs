//! API integration tests
//!
//! Tests for the relayer REST API endpoints including health checks,
//! relayer CRUD operations, and related functionality.

use crate::integration::common::{
    client::RelayerClient, logging::init_test_logging, registry::TestRegistry,
};
use openzeppelin_relayer::models::relayer::{
    CreateRelayerPolicyRequest, CreateRelayerRequest, RelayerEvmPolicy, RelayerNetworkType,
    RelayerSolanaPolicy, RelayerStellarPolicy,
};
use serial_test::serial;
use tracing::{info, info_span};

// =============================================================================
// Health Endpoint Tests
// =============================================================================

/// Tests that the health endpoint returns a successful response
#[tokio::test]
#[serial]
async fn test_health_endpoint() {
    init_test_logging();
    let _span = info_span!("test_health_endpoint").entered();
    info!("Starting health endpoint test");

    let client = RelayerClient::from_env().expect("Failed to create client");
    let health = client.health().await.expect("Health check failed");

    info!(status = %health.status, "Health check response");
    assert_eq!(health.status, "ok", "Health status should be 'ok'");

    info!("Health endpoint test completed successfully");
}

// =============================================================================
// Relayer CRUD Tests
// =============================================================================

/// Helper to create a test relayer for CRUD operations
///
/// This function:
/// 1. Uses the `sepolia` network for CRUD testing
/// 2. Creates a new signer with a unique ID
/// 3. Creates a new test relayer with a unique ID using that signer
/// 4. Returns info that can be used to clean up the relayer and signer
struct CrudTestRelayer {
    relayer_id: String,
    network_name: String,
    signer_id: String,
}

impl CrudTestRelayer {
    /// Creates a new test relayer for CRUD operations
    async fn create(client: &RelayerClient) -> eyre::Result<Self> {
        const CRUD_TEST_NETWORK: &str = "sepolia";
        const REGISTRY_KEY: &str = "sepolia";

        let registry = TestRegistry::load()?;
        let network_config = registry.get_network(REGISTRY_KEY)?;

        let network_type = match network_config.network_type.as_str() {
            "evm" => RelayerNetworkType::Evm,
            "solana" => RelayerNetworkType::Solana,
            "stellar" => RelayerNetworkType::Stellar,
            _ => {
                return Err(eyre::eyre!(
                    "Unknown network type: {}",
                    network_config.network_type
                ))
            }
        };

        // Create a unique test signer ID
        // Using short prefix + UUID without dashes to stay under 36 char limit
        let signer_id = format!("s-{}", uuid::Uuid::new_v4().to_string().replace("-", ""));

        // Create a local signer with a test private key
        // Using a test key: 64 hex characters (32 bytes) for secp256k1
        // Note: API expects "plain" as the type for local signers
        let signer_request = serde_json::json!({
            "id": signer_id.clone(),
            "type": "plain",
            "config": {
                "key": "0000000000000000000000000000000000000000000000000000000000000001"
            }
        });

        info!(id = %signer_id, "Creating test signer for CRUD operations");
        client.create_signer(signer_request).await?;

        // Create a unique test relayer ID
        // Using short prefix + UUID without dashes to stay under 36 char limit
        let relayer_id = format!("r-{}", uuid::Uuid::new_v4().to_string().replace("-", ""));

        // Create policies with min_balance = 0 to avoid relayer being disabled
        let policies = match network_type {
            RelayerNetworkType::Evm => Some(CreateRelayerPolicyRequest::Evm(RelayerEvmPolicy {
                min_balance: Some(0),
                gas_limit_estimation: None,
                gas_price_cap: None,
                whitelist_receivers: None,
                eip1559_pricing: None,
                private_transactions: None,
            })),
            RelayerNetworkType::Solana => {
                Some(CreateRelayerPolicyRequest::Solana(RelayerSolanaPolicy {
                    min_balance: Some(0),
                    fee_payment_strategy: None,
                    max_signatures: None,
                    allowed_tokens: None,
                    allowed_programs: None,
                    allowed_accounts: None,
                    disallowed_accounts: None,
                    max_tx_data_size: None,
                    max_allowed_fee_lamports: None,
                    swap_config: None,
                    fee_margin_percentage: None,
                }))
            }
            RelayerNetworkType::Stellar => {
                Some(CreateRelayerPolicyRequest::Stellar(RelayerStellarPolicy {
                    min_balance: Some(0),
                    max_fee: None,
                    timeout_seconds: None,
                    concurrent_transactions: None,
                    allowed_tokens: None,
                    fee_payment_strategy: None,
                    slippage_percentage: None,
                    fee_margin_percentage: None,
                    swap_config: None,
                }))
            }
        };

        let request = CreateRelayerRequest {
            id: Some(relayer_id.clone()),
            name: "CRUD Test Relayer".to_string(),
            network: CRUD_TEST_NETWORK.to_string(),
            paused: false,
            network_type,
            policies,
            signer_id: signer_id.clone(),
            notification_id: None,
            custom_rpc_urls: None,
        };

        info!(id = %relayer_id, network = %CRUD_TEST_NETWORK, signer_id = %signer_id, "Creating test relayer for CRUD operations");
        client.create_relayer(request).await?;

        Ok(Self {
            relayer_id,
            network_name: CRUD_TEST_NETWORK.to_string(),
            signer_id,
        })
    }

    /// Gets the relayer ID
    fn id(&self) -> &str {
        &self.relayer_id
    }

    /// Gets the network name
    fn network(&self) -> &str {
        &self.network_name
    }

    /// Explicitly clean up the test relayer and signer
    async fn cleanup(&self, client: &RelayerClient) -> eyre::Result<()> {
        info!(id = %self.relayer_id, network = %self.network_name, "Cleaning up test relayer");
        let _ = client.delete_relayer(&self.relayer_id).await;

        info!(id = %self.signer_id, "Cleaning up test signer");
        let _ = client.delete_signer(&self.signer_id).await;

        Ok(())
    }
}

/// Tests creating a new relayer via the API
///
/// Creates a test relayer on the fly for CRUD operations testing.
#[tokio::test]
#[serial]
async fn test_create_relayer() {
    init_test_logging();
    let _span = info_span!("test_create_relayer").entered();
    info!("Starting create relayer test");

    let client = RelayerClient::from_env().expect("Failed to create client");

    // Create a test relayer on the fly
    let test_relayer = CrudTestRelayer::create(&client)
        .await
        .expect("Failed to create test relayer");

    // Verify the relayer was created successfully
    let relayer = client
        .get_relayer(test_relayer.id())
        .await
        .expect("Failed to get created relayer");

    info!(
        id = %relayer.id,
        name = %relayer.name,
        network = %relayer.network,
        "Relayer created successfully"
    );

    assert_eq!(relayer.id, test_relayer.id());
    assert_eq!(relayer.network, test_relayer.network());
    assert!(!relayer.paused);

    // Cleanup: delete the test relayer
    test_relayer
        .cleanup(&client)
        .await
        .expect("Failed to cleanup test relayer");

    info!("Create relayer test completed successfully");
}

/// Tests listing all relayers
#[tokio::test]
#[serial]
async fn test_list_relayers() {
    init_test_logging();
    let _span = info_span!("test_list_relayers").entered();
    info!("Starting list relayers test");

    let client = RelayerClient::from_env().expect("Failed to create client");

    let relayers = client
        .list_relayers()
        .await
        .expect("Failed to list relayers");

    info!(count = relayers.len(), "Retrieved relayers");

    // List should return successfully (may be empty or have relayers)
    for relayer in &relayers {
        info!(
            id = %relayer.id,
            name = %relayer.name,
            network = %relayer.network,
            "Found relayer"
        );
    }

    info!("List relayers test completed successfully");
}

/// Tests updating a relayer via the API
///
/// Creates a test relayer on the fly and tests updating it.
#[tokio::test]
#[serial]
async fn test_update_relayer() {
    init_test_logging();
    let _span = info_span!("test_update_relayer").entered();
    info!("Starting update relayer test");

    let client = RelayerClient::from_env().expect("Failed to create client");

    // Create a test relayer on the fly
    let test_relayer = CrudTestRelayer::create(&client)
        .await
        .expect("Failed to create test relayer");

    let relayer_id = test_relayer.id();

    // Test updating name only
    let update_name = serde_json::json!({
        "name": "Updated Name"
    });

    let updated = client
        .update_relayer(relayer_id, update_name)
        .await
        .expect("Failed to update relayer name");

    assert_eq!(updated.name, "Updated Name");
    info!(name = %updated.name, "Name updated");

    // Test updating paused status only
    let update_paused = serde_json::json!({
        "paused": true
    });

    let updated = client
        .update_relayer(relayer_id, update_paused)
        .await
        .expect("Failed to update relayer paused status");

    assert_eq!(updated.name, "Updated Name", "Name should remain unchanged");
    assert!(updated.paused, "Paused should be true");
    info!(paused = %updated.paused, "Paused status updated");

    // Test updating multiple fields at once
    let update_multiple = serde_json::json!({
        "name": "Final Name",
        "paused": false
    });

    let updated = client
        .update_relayer(relayer_id, update_multiple)
        .await
        .expect("Failed to update multiple fields");

    assert_eq!(updated.name, "Final Name");
    assert!(!updated.paused);
    info!(name = %updated.name, paused = %updated.paused, "Multiple fields updated");

    // Cleanup: delete the test relayer
    test_relayer
        .cleanup(&client)
        .await
        .expect("Failed to cleanup test relayer");

    info!("Update relayer test completed successfully");
}

/// Tests deleting all relayers by network
///
/// Creates multiple test relayers on the fly and tests deleting all relayers by network.
#[tokio::test]
#[serial]
async fn test_delete_all_relayers_by_network() {
    init_test_logging();
    let _span = info_span!("test_delete_all_relayers_by_network").entered();
    info!("Starting delete all relayers by network test");

    let client = RelayerClient::from_env().expect("Failed to create client");

    // Create multiple test relayers to test bulk deletion
    info!("Creating test relayers for bulk delete test");
    let test_relayer1 = CrudTestRelayer::create(&client)
        .await
        .expect("Failed to create test relayer 1");
    let test_relayer2 = CrudTestRelayer::create(&client)
        .await
        .expect("Failed to create test relayer 2");
    let test_relayer3 = CrudTestRelayer::create(&client)
        .await
        .expect("Failed to create test relayer 3");

    let network_name = test_relayer1.network();

    // Store created relayer IDs for verification
    let created_ids = vec![
        test_relayer1.id().to_string(),
        test_relayer2.id().to_string(),
        test_relayer3.id().to_string(),
    ];

    info!(
        network = %network_name,
        count = created_ids.len(),
        "Created test relayers for bulk delete"
    );

    // Delete all relayers for the network
    let deleted_count = client
        .delete_all_relayers_by_network(network_name)
        .await
        .expect("Failed to delete relayers by network");

    info!(deleted = deleted_count, network = %network_name, "Deleted relayers by network");

    // Verify we deleted at least the ones we created (there may have been others)
    assert!(
        deleted_count >= created_ids.len(),
        "Should have deleted at least {} relayers, deleted {}",
        created_ids.len(),
        deleted_count
    );

    // Verify each of our test relayers is actually gone
    for relayer_id in &created_ids {
        let result = client.get_relayer(relayer_id).await;
        assert!(
            result.is_err(),
            "Relayer {} should have been deleted",
            relayer_id
        );
    }

    info!("Delete all relayers by network test completed successfully");
}
