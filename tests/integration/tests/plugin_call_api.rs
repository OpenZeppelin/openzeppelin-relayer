//! Plugin call integration tests
//!
//! Covers POST/GET plugin call routes.

use crate::integration::common::client::RelayerClient;
use reqwest::StatusCode;
use serde_json::Value;
use serial_test::serial;
use std::env;

const E2E_PLUGIN_ID: &str = "e2e-echo";

fn payload_data(body: &str) -> Value {
    let parsed: Value = serde_json::from_str(body).expect("Response body should be valid JSON");
    parsed.get("data").cloned().unwrap_or(parsed)
}

async fn has_e2e_plugin(client: &RelayerClient) -> bool {
    client.get_plugin(E2E_PLUGIN_ID).await.is_ok()
}

fn metadata_field(body: &str, field: &str) -> Option<Value> {
    let parsed: Value = serde_json::from_str(body).expect("Response body should be valid JSON");
    parsed.get("metadata").and_then(|m| m.get(field)).cloned()
}

fn test_base_url() -> String {
    env::var("RELAYER_BASE_URL").unwrap_or_else(|_| "http://localhost:8080".to_string())
}

#[tokio::test]
#[serial]
async fn test_plugin_call_post_not_found() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };

    let (status, body) = client
        .call_plugin_post_raw("nonexistent-plugin-id", "", serde_json::json!({"x": 1}))
        .await
        .expect("Failed to call plugin POST route");

    assert!(
        status == 404 || body.to_lowercase().contains("not found"),
        "Expected plugin POST call to report not found, got status={} body={}",
        status,
        body
    );
}

#[tokio::test]
#[serial]
async fn test_plugin_call_get_not_found() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };

    let (status, body) = client
        .call_plugin_get_raw("nonexistent-plugin-id", "")
        .await
        .expect("Failed to call plugin GET route");

    assert!(
        status == 404 || body.to_lowercase().contains("not found"),
        "Expected plugin GET call to report not found, got status={} body={}",
        status,
        body
    );
}

#[tokio::test]
#[serial]
async fn test_plugin_call_post_body_as_params() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };
    if !has_e2e_plugin(&client).await {
        return;
    }

    let (status, body) = client
        .call_plugin_post_raw(
            E2E_PLUGIN_ID,
            "/payments/quote",
            serde_json::json!({
                "alpha": 1,
                "nested": { "ok": true }
            }),
        )
        .await
        .expect("Failed to call plugin POST route");

    assert_eq!(status, 200, "Expected HTTP 200, got body={}", body);
    let data = payload_data(&body);

    assert_eq!(
        data.get("plugin").and_then(Value::as_str),
        Some(E2E_PLUGIN_ID),
        "plugin id should be forwarded"
    );
    assert_eq!(
        data.get("method").and_then(Value::as_str),
        Some("POST"),
        "HTTP method should be forwarded"
    );
    assert_eq!(
        data.get("route").and_then(Value::as_str),
        Some("/payments/quote"),
        "route should be forwarded"
    );
    assert_eq!(
        data.pointer("/params/alpha"),
        Some(&serde_json::json!(1)),
        "POST body should map to plugin params"
    );
    assert_eq!(
        data.pointer("/params/nested/ok"),
        Some(&serde_json::json!(true)),
        "Nested payload should be preserved"
    );
    assert_eq!(
        data.pointer("/config/suite"),
        Some(&serde_json::json!("integration")),
        "Plugin config from config.json should be injected"
    );
    assert_eq!(
        data.get("has_authorization_header")
            .and_then(Value::as_bool),
        Some(true),
        "Authorization header should be visible in plugin context"
    );
}

#[tokio::test]
#[serial]
async fn test_plugin_call_post_params_wrapper() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };
    if !has_e2e_plugin(&client).await {
        return;
    }

    let (status, body) = client
        .call_plugin_post_raw(
            E2E_PLUGIN_ID,
            "/wrapped",
            serde_json::json!({
                "params": {
                    "beta": "two"
                }
            }),
        )
        .await
        .expect("Failed to call plugin POST route");

    assert_eq!(status, 200, "Expected HTTP 200, got body={}", body);
    let data = payload_data(&body);
    assert_eq!(
        data.pointer("/params/beta"),
        Some(&serde_json::json!("two")),
        "Explicit params wrapper should be unwrapped by API layer"
    );
}

#[tokio::test]
#[serial]
async fn test_plugin_call_get_with_query_and_route() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };
    if !has_e2e_plugin(&client).await {
        return;
    }

    let (status, body) = client
        .call_plugin_get_raw(E2E_PLUGIN_ID, "/status/check?region=us&verbose=true")
        .await
        .expect("Failed to call plugin GET route");

    assert_eq!(status, 200, "Expected HTTP 200, got body={}", body);
    let data = payload_data(&body);

    assert_eq!(
        data.get("method").and_then(Value::as_str),
        Some("GET"),
        "GET should be visible in plugin context"
    );
    assert_eq!(
        data.get("route").and_then(Value::as_str),
        Some("/status/check"),
        "route should not include query string"
    );
    assert_eq!(
        data.pointer("/query/region"),
        Some(&serde_json::json!("us")),
        "query.region should be forwarded"
    );
    assert_eq!(
        data.pointer("/query/verbose"),
        Some(&serde_json::json!("true")),
        "query.verbose should be forwarded"
    );
}

#[tokio::test]
#[serial]
async fn test_plugin_call_post_forced_error() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };
    if !has_e2e_plugin(&client).await {
        return;
    }

    let (status, body) = client
        .call_plugin_post_raw(
            E2E_PLUGIN_ID,
            "/error",
            serde_json::json!({
                "force_error": true
            }),
        )
        .await
        .expect("Failed to call plugin POST route");

    assert!(
        status >= 400,
        "Expected error status when plugin throws, got status={} body={}",
        status,
        body
    );
    assert!(
        body.to_lowercase().contains("forced error"),
        "Expected plugin error message to surface, got body={}",
        body
    );
}

#[tokio::test]
#[serial]
async fn test_plugin_call_metadata_logs_and_traces_enabled() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };
    if !has_e2e_plugin(&client).await {
        return;
    }

    let original = client
        .get_plugin(E2E_PLUGIN_ID)
        .await
        .expect("Failed to get e2e plugin");

    client
        .update_plugin(
            E2E_PLUGIN_ID,
            serde_json::json!({
                "emit_logs": true,
                "emit_traces": true
            }),
        )
        .await
        .expect("Failed to enable plugin metadata flags");

    let call_result = client
        .call_plugin_post_raw(
            E2E_PLUGIN_ID,
            "/meta",
            serde_json::json!({
                "hello": "world"
            }),
        )
        .await;

    let _ = client
        .update_plugin(
            E2E_PLUGIN_ID,
            serde_json::json!({
                "emit_logs": original.emit_logs,
                "emit_traces": original.emit_traces
            }),
        )
        .await;

    let (status, body) = call_result.expect("Failed to call plugin route");
    assert_eq!(status, 200, "Expected HTTP 200, got body={}", body);

    let logs = metadata_field(&body, "logs");
    assert!(
        logs.as_ref().is_some_and(Value::is_array),
        "Expected metadata.logs array when emit_logs=true, got body={}",
        body
    );
    assert!(
        logs.as_ref()
            .and_then(Value::as_array)
            .is_some_and(|arr| !arr.is_empty()),
        "Expected metadata.logs to be non-empty, got body={}",
        body
    );

    let traces = metadata_field(&body, "traces");
    assert!(
        traces.as_ref().is_some_and(Value::is_array),
        "Expected metadata.traces array when emit_traces=true, got body={}",
        body
    );
}

#[tokio::test]
#[serial]
async fn test_plugin_call_metadata_omitted_when_disabled() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };
    if !has_e2e_plugin(&client).await {
        return;
    }

    let original = client
        .get_plugin(E2E_PLUGIN_ID)
        .await
        .expect("Failed to get e2e plugin");

    client
        .update_plugin(
            E2E_PLUGIN_ID,
            serde_json::json!({
                "emit_logs": false,
                "emit_traces": false
            }),
        )
        .await
        .expect("Failed to disable plugin metadata flags");

    let call_result = client
        .call_plugin_post_raw(
            E2E_PLUGIN_ID,
            "/meta-disabled",
            serde_json::json!({
                "x": 1
            }),
        )
        .await;

    let _ = client
        .update_plugin(
            E2E_PLUGIN_ID,
            serde_json::json!({
                "emit_logs": original.emit_logs,
                "emit_traces": original.emit_traces
            }),
        )
        .await;

    let (status, body) = call_result.expect("Failed to call plugin route");
    assert_eq!(status, 200, "Expected HTTP 200, got body={}", body);

    let parsed: Value = serde_json::from_str(&body).expect("Response body should be valid JSON");
    assert!(
        parsed.get("metadata").is_none(),
        "Expected metadata to be omitted when flags are disabled, got body={}",
        body
    );
}

#[tokio::test]
#[serial]
async fn test_plugin_call_post_invalid_json_payload() {
    let Some(_client) = RelayerClient::from_env_or_skip().await else {
        return;
    };
    let api_key = env::var("API_KEY").expect("API_KEY should be set for integration tests");
    let url = format!("{}/api/v1/plugins/{}/call", test_base_url(), E2E_PLUGIN_ID);

    let response = reqwest::Client::new()
        .post(&url)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .body("{invalid-json")
        .send()
        .await
        .expect("Failed to send malformed JSON plugin call");

    let status = response.status();
    let body = response
        .text()
        .await
        .expect("Failed to read plugin call response");

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Expected 400 for malformed JSON payload, got status={} body={}",
        status,
        body
    );
    assert!(
        body.to_lowercase().contains("invalid json"),
        "Expected invalid json message, got body={}",
        body
    );
}

#[tokio::test]
#[serial]
async fn test_plugin_call_post_invalid_request_shape() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };

    let (status, body) = client
        .call_plugin_post_raw(
            E2E_PLUGIN_ID,
            "/invalid-shape",
            serde_json::json!({
                "params": {},
                "headers": "not-an-object"
            }),
        )
        .await
        .expect("Failed to call plugin POST route");

    assert_eq!(
        status, 200,
        "Expected 200, got status={} body={}",
        status, body
    );
    let data = payload_data(&body);
    assert!(
        data.pointer("/params").is_some(),
        "Expected request to be accepted with params forwarded, got body={}",
        body
    );
}
