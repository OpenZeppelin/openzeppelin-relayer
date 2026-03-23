//! Pagination boundary integration tests.

use crate::integration::common::client::RelayerClient;
use reqwest::StatusCode;
use serde_json::Value;
use serial_test::serial;
use std::env;

fn base_url() -> String {
    env::var("RELAYER_BASE_URL").unwrap_or_else(|_| "http://localhost:8080".to_string())
}

fn api_key() -> String {
    env::var("API_KEY").expect("API_KEY should be set for integration tests")
}

async fn get_paginated(path: &str, page: u32, per_page: u32) -> (StatusCode, String) {
    let url = format!("{}{}?page={}&per_page={}", base_url(), path, page, per_page);
    let response = reqwest::Client::new()
        .get(&url)
        .header("Authorization", format!("Bearer {}", api_key()))
        .send()
        .await
        .unwrap_or_else(|e| panic!("Failed to request {}: {}", path, e));
    let status = response.status();
    let body = response
        .text()
        .await
        .unwrap_or_else(|e| panic!("Failed reading {} response body: {}", path, e));
    (status, body)
}

fn assert_pagination_success_shape(body: &str) {
    let parsed: Value = serde_json::from_str(body).expect("Response should be valid JSON");
    let data = parsed
        .get("data")
        .and_then(Value::as_array)
        .expect("data should be an array in paginated response");
    let pagination = parsed
        .get("pagination")
        .expect("pagination should be present in paginated response");
    let current_page = pagination
        .get("current_page")
        .and_then(Value::as_u64)
        .expect("pagination.current_page should be numeric");
    let per_page = pagination
        .get("per_page")
        .and_then(Value::as_u64)
        .expect("pagination.per_page should be numeric");

    assert!(current_page >= 1, "current_page should be >= 1");
    assert!(per_page >= 1, "per_page should be >= 1");
    assert!(
        data.len() as u64 <= per_page,
        "Returned item count should be <= per_page"
    );
}

#[tokio::test]
#[serial]
async fn test_pagination_boundary_page_zero() {
    let Some(_client) = RelayerClient::from_env_or_skip().await else {
        return;
    };
    let endpoints = [
        "/api/v1/networks",
        "/api/v1/signers",
        "/api/v1/plugins",
        "/api/v1/api-keys",
    ];

    for endpoint in endpoints {
        let (status, body) = get_paginated(endpoint, 0, 2).await;
        assert!(
            status == StatusCode::OK || status == StatusCode::BAD_REQUEST,
            "{} page=0 should return 200 or 400, got {} body={}",
            endpoint,
            status,
            body
        );
        if status == StatusCode::OK {
            assert_pagination_success_shape(&body);
        }
    }
}

#[tokio::test]
#[serial]
async fn test_pagination_boundary_huge_per_page() {
    let Some(_client) = RelayerClient::from_env_or_skip().await else {
        return;
    };
    let endpoints = [
        "/api/v1/networks",
        "/api/v1/signers",
        "/api/v1/plugins",
        "/api/v1/api-keys",
    ];

    for endpoint in endpoints {
        let (status, body) = get_paginated(endpoint, 1, 10_000).await;
        assert!(
            status == StatusCode::OK || status == StatusCode::BAD_REQUEST,
            "{} per_page=10000 should return 200 or 400, got {} body={}",
            endpoint,
            status,
            body
        );
        if status == StatusCode::OK {
            assert_pagination_success_shape(&body);
        }
    }
}

#[tokio::test]
#[serial]
async fn test_pagination_boundary_out_of_range_page() {
    let Some(_client) = RelayerClient::from_env_or_skip().await else {
        return;
    };
    let endpoints = [
        "/api/v1/networks",
        "/api/v1/signers",
        "/api/v1/plugins",
        "/api/v1/api-keys",
    ];

    for endpoint in endpoints {
        let (status, body) = get_paginated(endpoint, 999_999, 2).await;
        assert!(
            status == StatusCode::OK || status == StatusCode::BAD_REQUEST,
            "{} huge page should return 200 or 400, got {} body={}",
            endpoint,
            status,
            body
        );
        if status == StatusCode::OK {
            assert_pagination_success_shape(&body);
        }
    }
}
