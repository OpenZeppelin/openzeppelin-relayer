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
/// 1. Dynamically selects an enabled network from the registry (prefers "sepolia" if available, otherwise uses the first enabled network)
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
        let registry = TestRegistry::load()?;

        // Get all enabled networks from the registry
        let enabled_networks = registry.enabled_networks();

        if enabled_networks.is_empty() {
            return Err(eyre::eyre!(
                "No enabled networks found in registry. At least one network must be enabled for CRUD tests."
            ));
        }

        // Prefer "sepolia" if available (for backward compatibility with testnet mode),
        // otherwise use the first enabled network (works with local Anvil which uses "localhost")
        let selected_network = enabled_networks
            .iter()
            .find(|n| *n == "sepolia")
            .or_else(|| enabled_networks.first())
            .ok_or_else(|| eyre::eyre!("Failed to select a network"))?;

        info!(
            selected = %selected_network,
            available = ?enabled_networks,
            "Selected network for CRUD test"
        );

        let network_config = registry.get_network(selected_network)?;

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

        info!(id = %signer_id, network = %selected_network, "Creating test signer for CRUD operations");
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
            network: selected_network.clone(),
            paused: false,
            network_type,
            policies,
            signer_id: signer_id.clone(),
            notification_id: None,
            custom_rpc_urls: None,
        };

        info!(id = %relayer_id, network = %selected_network, signer_id = %signer_id, "Creating test relayer for CRUD operations");
        client.create_relayer(request).await?;

        Ok(Self {
            relayer_id,
            network_name: selected_network.clone(),
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

/// Tests deleting a relayer via the API
///
/// Creates a test relayer on the fly and tests deleting it.
/// This test is isolated and does not affect pre-configured relayers.
#[tokio::test]
#[serial]
async fn test_delete_relayer() {
    init_test_logging();
    let _span = info_span!("test_delete_relayer").entered();
    info!("Starting delete relayer test");

    let client = RelayerClient::from_env().expect("Failed to create client");

    // Create a test relayer on the fly
    let test_relayer = CrudTestRelayer::create(&client)
        .await
        .expect("Failed to create test relayer");

    let relayer_id = test_relayer.id();

    // Verify the relayer exists before deletion
    let relayer = client
        .get_relayer(relayer_id)
        .await
        .expect("Failed to get relayer before deletion");

    assert_eq!(relayer.id, relayer_id);
    info!(id = %relayer.id, "Relayer exists before deletion");

    // Delete the relayer
    client
        .delete_relayer(relayer_id)
        .await
        .expect("Failed to delete relayer");

    info!(id = %relayer_id, "Relayer deleted successfully");

    // Verify the relayer no longer exists
    let result = client.get_relayer(relayer_id).await;
    assert!(
        result.is_err(),
        "Relayer {} should have been deleted",
        relayer_id
    );

    // Cleanup: also delete the signer (cleanup method handles this, but we already deleted the relayer)
    test_relayer
        .cleanup(&client)
        .await
        .expect("Failed to cleanup test signer");

    info!("Delete relayer test completed successfully");
}
