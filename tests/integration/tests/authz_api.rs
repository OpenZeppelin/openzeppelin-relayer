//! Authorization and permission integration tests for mutable endpoints.

use crate::integration::common::client::RelayerClient;
use reqwest::{Method, StatusCode};
use serial_test::serial;
use std::env;

struct MutableEndpoint {
    name: &'static str,
    method: Method,
    path: &'static str,
    body: Option<serde_json::Value>,
}

fn base_url() -> String {
    env::var("RELAYER_BASE_URL").unwrap_or_else(|_| "http://localhost:8080".to_string())
}

fn mutable_endpoints() -> Vec<MutableEndpoint> {
    vec![
        MutableEndpoint {
            name: "create_relayer",
            method: Method::POST,
            path: "/api/v1/relayers",
            body: Some(serde_json::json!({})),
        },
        MutableEndpoint {
            name: "update_relayer",
            method: Method::PATCH,
            path: "/api/v1/relayers/nonexistent",
            body: Some(serde_json::json!({"name": "x"})),
        },
        MutableEndpoint {
            name: "delete_relayer",
            method: Method::DELETE,
            path: "/api/v1/relayers/nonexistent",
            body: None,
        },
        MutableEndpoint {
            name: "create_signer",
            method: Method::POST,
            path: "/api/v1/signers",
            body: Some(serde_json::json!({})),
        },
        MutableEndpoint {
            name: "delete_signer",
            method: Method::DELETE,
            path: "/api/v1/signers/nonexistent",
            body: None,
        },
        MutableEndpoint {
            name: "update_network",
            method: Method::PATCH,
            path: "/api/v1/networks/nonexistent",
            body: Some(serde_json::json!({"rpc_urls": ["https://example.com"]})),
        },
        MutableEndpoint {
            name: "create_notification",
            method: Method::POST,
            path: "/api/v1/notifications",
            body: Some(serde_json::json!({})),
        },
        MutableEndpoint {
            name: "update_notification",
            method: Method::PATCH,
            path: "/api/v1/notifications/nonexistent",
            body: Some(serde_json::json!({"url": "https://example.com/new"})),
        },
        MutableEndpoint {
            name: "delete_notification",
            method: Method::DELETE,
            path: "/api/v1/notifications/nonexistent",
            body: None,
        },
        MutableEndpoint {
            name: "update_plugin",
            method: Method::PATCH,
            path: "/api/v1/plugins/e2e-echo",
            body: Some(serde_json::json!({"emit_logs": true})),
        },
        MutableEndpoint {
            name: "call_plugin",
            method: Method::POST,
            path: "/api/v1/plugins/e2e-echo/call",
            body: Some(serde_json::json!({"x": 1})),
        },
        MutableEndpoint {
            name: "create_api_key",
            method: Method::POST,
            path: "/api/v1/api-keys",
            body: Some(serde_json::json!({})),
        },
        MutableEndpoint {
            name: "delete_api_key",
            method: Method::DELETE,
            path: "/api/v1/api-keys/nonexistent",
            body: None,
        },
    ]
}

async fn send_mutation(
    http: &reqwest::Client,
    endpoint: &MutableEndpoint,
    authorization: Option<&str>,
) -> (StatusCode, String) {
    let url = format!("{}{}", base_url(), endpoint.path);
    let mut request = http.request(endpoint.method.clone(), &url);
    if let Some(header) = authorization {
        request = request.header("Authorization", header);
    }
    if let Some(body) = &endpoint.body {
        request = request.json(body);
    }
    let response = request
        .send()
        .await
        .unwrap_or_else(|e| panic!("Request {} failed: {}", endpoint.name, e));
    let status = response.status();
    let body = response
        .text()
        .await
        .unwrap_or_else(|e| panic!("Failed reading body for {}: {}", endpoint.name, e));
    (status, body)
}

#[tokio::test]
#[serial]
async fn test_mutable_endpoints_require_authorization_header() {
    let Some(_client) = RelayerClient::from_env_or_skip().await else {
        return;
    };
    let http = reqwest::Client::new();
    let mut failures = Vec::new();

    for endpoint in mutable_endpoints() {
        let (status, body) = send_mutation(&http, &endpoint, None).await;
        if status != StatusCode::UNAUTHORIZED && status != StatusCode::FORBIDDEN {
            failures.push(format!(
                "{} returned {} without auth (body={})",
                endpoint.name, status, body
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "Mutable endpoints should reject missing auth:\n{}",
        failures.join("\n")
    );
}

#[tokio::test]
#[serial]
async fn test_mutable_endpoints_reject_invalid_token() {
    let Some(_client) = RelayerClient::from_env_or_skip().await else {
        return;
    };
    let http = reqwest::Client::new();
    let mut failures = Vec::new();

    for endpoint in mutable_endpoints() {
        let (status, body) = send_mutation(&http, &endpoint, Some("Bearer invalid-token")).await;
        if status != StatusCode::UNAUTHORIZED && status != StatusCode::FORBIDDEN {
            failures.push(format!(
                "{} returned {} with invalid token (body={})",
                endpoint.name, status, body
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "Mutable endpoints should reject invalid token:\n{}",
        failures.join("\n")
    );
}
