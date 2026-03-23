//! API keys integration tests
//!
//! Covers API key create/list/permissions/delete endpoint behavior.

use crate::integration::common::client::RelayerClient;
use serial_test::serial;

fn unique_name(prefix: &str) -> String {
    format!("{}-{}", prefix, uuid::Uuid::new_v4())
}

#[tokio::test]
#[serial]
async fn test_create_and_list_api_keys() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };

    let name = unique_name("e2e-key");
    let created = client
        .create_api_key(serde_json::json!({
            "name": name,
            "permissions": ["relayer:all:execute"],
            "allowed_origins": ["*"]
        }))
        .await
        .expect("Failed to create api key");

    let created_id = created
        .get("id")
        .and_then(serde_json::Value::as_str)
        .expect("Created API key should include id")
        .to_string();
    assert!(
        !created_id.is_empty(),
        "Created API key id should not be empty"
    );

    let listed = client
        .list_api_keys_paginated(1, 100)
        .await
        .expect("Failed to list api keys");

    if let Some(meta) = listed.pagination {
        assert_eq!(meta.current_page, 1);
        assert_eq!(meta.per_page, 100);
        assert!(meta.total_items >= listed.items.len() as u64);
    }

    let has_created = listed.items.iter().any(|item| {
        item.get("id")
            .and_then(serde_json::Value::as_str)
            .map(|id| id == created_id)
            .unwrap_or(false)
    });
    assert!(has_created, "List should include created API key");
}

#[tokio::test]
#[serial]
async fn test_get_api_key_permissions() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };

    let name = unique_name("e2e-perms");
    let created = client
        .create_api_key(serde_json::json!({
            "name": name,
            "permissions": ["relayer:all:execute"],
            "allowed_origins": ["*"]
        }))
        .await
        .expect("Failed to create api key");
    let created_id = created
        .get("id")
        .and_then(serde_json::Value::as_str)
        .expect("Created API key should include id")
        .to_string();

    let perms = client
        .get_api_key_permissions(&created_id)
        .await
        .expect("Failed to get API key permissions");
    assert!(
        perms.iter().any(|p| p == "relayer:all:execute"),
        "Expected relayer:all:execute in permissions"
    );
}

#[tokio::test]
#[serial]
async fn test_delete_api_key_current_behavior() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };

    let (status, body) = client
        .delete_api_key_raw("some-api-key-id")
        .await
        .expect("Failed to call delete API key");

    // Current controller behavior is 200 with "Not implemented" error payload.
    assert_eq!(status, 200, "Delete API key should currently return 200");
    assert!(
        body.contains("Not implemented"),
        "Delete response should indicate current not-implemented behavior"
    );
}
