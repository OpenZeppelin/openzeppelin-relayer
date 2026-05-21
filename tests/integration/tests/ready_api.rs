//! Ready API integration tests
//!
//! Tests for the readiness endpoint (`GET /api/v1/ready`) that validate
//! response shape and status/ready consistency.

use crate::integration::common::client::RelayerClient;
use serial_test::serial;

/// Tests that the ready endpoint returns expected HTTP status codes.
#[tokio::test]
#[serial]
async fn test_ready_endpoint_status_code() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };
    let response = client
        .readiness()
        .await
        .expect("Ready endpoint request failed");

    assert!(
        response.http_status == 200 || response.http_status == 503,
        "Ready endpoint should return 200 or 503, got {}",
        response.http_status
    );
}

/// Tests that ready endpoint returns required top-level fields.
#[tokio::test]
#[serial]
async fn test_ready_endpoint_response_fields() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };
    let response = client
        .readiness()
        .await
        .expect("Ready endpoint request failed");

    let readiness = response.readiness;

    assert!(
        matches!(
            readiness.status.as_str(),
            "healthy" | "degraded" | "unhealthy"
        ),
        "Unexpected readiness status: {}",
        readiness.status
    );
    assert!(
        !readiness.timestamp.is_empty(),
        "Timestamp should not be empty"
    );
    assert!(
        readiness.components.is_object(),
        "Components should be a JSON object"
    );
}

/// Tests that the HTTP status code matches the `ready` field.
#[tokio::test]
#[serial]
async fn test_ready_endpoint_status_matches_ready_field() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };
    let response = client
        .readiness()
        .await
        .expect("Ready endpoint request failed");

    if response.readiness.ready {
        assert_eq!(
            response.http_status, 200,
            "ready=true should return HTTP 200"
        );
        assert!(
            response.readiness.status == "healthy" || response.readiness.status == "degraded",
            "ready=true should have healthy or degraded status, got {}",
            response.readiness.status
        );
    } else {
        assert_eq!(
            response.http_status, 503,
            "ready=false should return HTTP 503"
        );
        assert_eq!(
            response.readiness.status, "unhealthy",
            "ready=false should have unhealthy status"
        );
    }
}

/// Tests that required component sections are present in the readiness response.
#[tokio::test]
#[serial]
async fn test_ready_endpoint_components_shape() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };
    let response = client
        .readiness()
        .await
        .expect("Ready endpoint request failed");

    let components = &response.readiness.components;
    let system = components
        .get("system")
        .and_then(serde_json::Value::as_object)
        .expect("components.system should be an object");
    let redis = components
        .get("redis")
        .and_then(serde_json::Value::as_object)
        .expect("components.redis should be an object");
    let queue = components
        .get("queue")
        .and_then(serde_json::Value::as_object)
        .expect("components.queue should be an object");

    assert!(
        system.contains_key("status"),
        "components.system should include status"
    );
    assert!(
        redis.contains_key("status"),
        "components.redis should include status"
    );
    assert!(
        queue.contains_key("status"),
        "components.queue should include status"
    );
}
