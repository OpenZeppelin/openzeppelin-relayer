//! Health endpoint integration tests

use crate::integration::common::client::RelayerClient;

/// Test that the health endpoint returns OK
#[tokio::test]
#[ignore = "Requires running relayer"]
async fn test_health_endpoint() {
    let client = RelayerClient::from_env().expect("Failed to create RelayerClient");

    let health = client.health().await.expect("Health check failed");

    assert_eq!(health.status, "ok", "Health status should be 'ok'");

    println!("Health check passed: status = {}", health.status);
}
