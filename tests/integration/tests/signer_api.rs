//! Signer API integration tests
//!
//! Tests for the signer REST API endpoints including creating, listing,
//! retrieving, updating, and deleting signers.

use crate::integration::common::client::RelayerClient;
use serial_test::serial;

// =============================================================================
// Helper
// =============================================================================

/// Creates a unique signer ID safe for the API (alphanumeric + dashes, max 36 chars).
fn unique_id(prefix: &str) -> String {
    let uuid = uuid::Uuid::new_v4().to_string().replace('-', "");
    // "prefix-<uuid chars>" keeps it under 36 chars
    let max_uuid_len = 36 - prefix.len() - 1;
    format!("{}-{}", prefix, &uuid[..max_uuid_len])
}

/// Helper to build a local signer create request as JSON.
/// Uses a valid 64-hex-char key (32 bytes).
fn local_signer_request(id: Option<&str>) -> serde_json::Value {
    let mut req = serde_json::json!({
        "type": "plain",
        "config": {
            "key": "0000000000000000000000000000000000000000000000000000000000000001"
        }
    });
    if let Some(id) = id {
        req.as_object_mut()
            .unwrap()
            .insert("id".to_string(), serde_json::json!(id));
    }
    req
}

/// Helper to build an AWS KMS signer create request as JSON.
fn aws_kms_signer_request(id: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "type": "aws_kms",
        "config": {
            "region": "us-east-1",
            "key_id": "arn:aws:kms:us-east-1:123456789012:key/test-key-id"
        }
    })
}

/// Search for signer IDs across multiple paginated list responses.
async fn find_signers_in_pages(client: &RelayerClient, signer_ids: &[&str]) -> eyre::Result<bool> {
    let mut remaining: std::collections::HashSet<&str> = signer_ids.iter().copied().collect();

    // Scan multiple pages to avoid assumptions about default ordering/size.
    for page in 1..=10 {
        let signers = client.list_signers(Some(page), Some(100)).await?;
        if signers.is_empty() {
            break;
        }

        for signer in &signers {
            remaining.remove(signer.id.as_str());
        }

        if remaining.is_empty() {
            return Ok(true);
        }
    }

    Ok(false)
}

// =============================================================================
// Create Signer Tests
// =============================================================================

/// Tests creating a local signer with an explicit ID
#[tokio::test]
#[serial]
async fn test_create_signer_local() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };
    let id = unique_id("sig");

    let created = client
        .create_signer(local_signer_request(Some(&id)))
        .await
        .expect("Failed to create signer");

    assert_eq!(created.id, id);
    assert_eq!(
        created.r#type,
        openzeppelin_relayer::models::signer::SignerType::Local
    );

    // Cleanup
    let _ = client.delete_signer(&id).await;
}

/// Tests creating a signer without an explicit ID (auto-generated UUID)
#[tokio::test]
#[serial]
async fn test_create_signer_auto_id() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };

    let created = client
        .create_signer(local_signer_request(None))
        .await
        .expect("Failed to create signer");

    assert!(
        !created.id.is_empty(),
        "Auto-generated ID should not be empty"
    );
    assert!(
        created.id.len() <= 36,
        "Auto-generated ID should be at most 36 characters"
    );

    // Cleanup
    let _ = client.delete_signer(&created.id).await;
}

/// Tests creating an AWS KMS signer
#[tokio::test]
#[serial]
async fn test_create_signer_aws_kms() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };
    let id = unique_id("awssig");

    let created = client
        .create_signer(aws_kms_signer_request(&id))
        .await
        .expect("Failed to create AWS KMS signer");

    assert_eq!(created.id, id);
    assert_eq!(
        created.r#type,
        openzeppelin_relayer::models::signer::SignerType::AwsKms
    );

    // Cleanup
    let _ = client.delete_signer(&id).await;
}

// =============================================================================
// List Signers Tests
// =============================================================================

/// Tests listing signers returns created resources
#[tokio::test]
#[serial]
async fn test_list_signers() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };
    let id1 = unique_id("ls1");
    let id2 = unique_id("ls2");

    client
        .create_signer(local_signer_request(Some(&id1)))
        .await
        .expect("Failed to create signer 1");
    client
        .create_signer(aws_kms_signer_request(&id2))
        .await
        .expect("Failed to create signer 2");

    let signers = client
        .list_signers(Some(1), Some(100))
        .await
        .expect("Failed to list signers");

    let ids: Vec<&str> = signers.iter().map(|s| s.id.as_str()).collect();
    let has_id1 = ids.contains(&id1.as_str());
    let has_id2 = ids.contains(&id2.as_str());
    if !has_id1 || !has_id2 {
        let found = find_signers_in_pages(&client, &[id1.as_str(), id2.as_str()])
            .await
            .expect("Failed to search signers in paginated list");
        assert!(
            found,
            "Created signers should appear in paginated signer list"
        );
    }

    // Cleanup
    let _ = client.delete_signer(&id1).await;
    let _ = client.delete_signer(&id2).await;
}

/// Tests pagination for listing signers
#[tokio::test]
#[serial]
async fn test_list_signers_pagination() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };
    let ids: Vec<String> = (0..3).map(|i| unique_id(&format!("pg{}", i))).collect();

    for id in &ids {
        client
            .create_signer(local_signer_request(Some(id)))
            .await
            .unwrap_or_else(|_| panic!("Failed to create signer {}", id));
    }

    let page1 = client
        .list_signers_paginated(1, 2)
        .await
        .expect("Failed to list page 1");

    let page2 = client
        .list_signers_paginated(2, 2)
        .await
        .expect("Failed to list page 2");

    if let Some(meta) = page1.pagination {
        assert_eq!(meta.current_page, 1);
        assert_eq!(meta.per_page, 2);
        assert!(meta.total_items >= page1.items.len() as u64);
    }
    if let Some(meta) = page2.pagination {
        assert_eq!(meta.current_page, 2);
        assert_eq!(meta.per_page, 2);
        assert!(meta.total_items >= page2.items.len() as u64);
    }

    // Pages should not overlap
    if !page1.items.is_empty() && !page2.items.is_empty() {
        let page1_ids: std::collections::HashSet<_> = page1.items.iter().map(|s| &s.id).collect();
        let page2_ids: std::collections::HashSet<_> = page2.items.iter().map(|s| &s.id).collect();
        assert!(
            page1_ids.is_disjoint(&page2_ids),
            "Page 1 and Page 2 should not have overlapping signers"
        );
    }

    // Cleanup
    for id in &ids {
        let _ = client.delete_signer(id).await;
    }
}

// =============================================================================
// Get Signer Tests
// =============================================================================

/// Tests getting a signer by ID
#[tokio::test]
#[serial]
async fn test_get_signer() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };
    let id = unique_id("get");

    let created = client
        .create_signer(local_signer_request(Some(&id)))
        .await
        .expect("Failed to create signer");

    let fetched = client.get_signer(&id).await.expect("Failed to get signer");

    assert_eq!(fetched.id, created.id);
    assert_eq!(fetched.r#type, created.r#type);

    // Cleanup
    let _ = client.delete_signer(&id).await;
}

/// Tests that getting a non-existent signer returns 404
#[tokio::test]
#[serial]
async fn test_get_signer_not_found() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };

    let result = client.get_signer("nonexistent-signer-id").await;

    assert!(
        result.is_err(),
        "Should return an error for non-existent signer"
    );
    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("404") || error_msg.contains("not found"),
        "Error should indicate not found: {}",
        error_msg
    );
}

// =============================================================================
// Update Signer Tests
// =============================================================================

/// Tests that updating a signer is rejected (not allowed for security reasons)
#[tokio::test]
#[serial]
async fn test_update_signer_not_allowed() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };
    let id = unique_id("upd");

    client
        .create_signer(local_signer_request(Some(&id)))
        .await
        .expect("Failed to create signer");

    let result = client.update_signer(&id, serde_json::json!({})).await;

    assert!(result.is_err(), "Update should be rejected");
    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("400") || error_msg.contains("not allowed"),
        "Error should indicate updates not allowed: {}",
        error_msg
    );

    // Cleanup
    let _ = client.delete_signer(&id).await;
}

// =============================================================================
// Delete Signer Tests
// =============================================================================

/// Tests deleting a signer and verifying it's gone
#[tokio::test]
#[serial]
async fn test_delete_signer() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };
    let id = unique_id("del");

    client
        .create_signer(local_signer_request(Some(&id)))
        .await
        .expect("Failed to create signer");

    client
        .delete_signer(&id)
        .await
        .expect("Failed to delete signer");

    let result = client.get_signer(&id).await;
    assert!(result.is_err(), "Should return an error for deleted signer");
    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("404") || error_msg.contains("not found"),
        "Error should indicate not found: {}",
        error_msg
    );
}

/// Tests that deleting a non-existent signer returns 404
#[tokio::test]
#[serial]
async fn test_delete_signer_not_found() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };

    let result = client.delete_signer("nonexistent-signer-id").await;

    assert!(
        result.is_err(),
        "Should return an error for non-existent signer"
    );
    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("404") || error_msg.contains("not found"),
        "Error should indicate not found: {}",
        error_msg
    );
}

// =============================================================================
// Validation / Error Tests
// =============================================================================

/// Tests that creating a signer with an invalid ID returns 400
#[tokio::test]
#[serial]
async fn test_create_signer_invalid_id() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };

    let result = client
        .create_signer(local_signer_request(Some("invalid@id")))
        .await;

    assert!(result.is_err(), "Should fail for invalid ID");
    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("400") || error_msg.contains("ID"),
        "Error should indicate bad request: {}",
        error_msg
    );
}

/// Tests that creating a local signer with an invalid hex key returns 400
#[tokio::test]
#[serial]
async fn test_create_signer_invalid_hex_key() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };
    let id = unique_id("badhex");

    let result = client
        .create_signer(serde_json::json!({
            "id": id,
            "type": "plain",
            "config": {
                "key": "not-valid-hex"
            }
        }))
        .await;

    assert!(result.is_err(), "Should fail for invalid hex key");
    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("400") || error_msg.contains("hex") || error_msg.contains("Invalid"),
        "Error should indicate bad request: {}",
        error_msg
    );
}

/// Tests that creating an AWS KMS signer with an empty key_id returns 400
#[tokio::test]
#[serial]
async fn test_create_signer_empty_aws_key_id() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };
    let id = unique_id("emptykey");

    let result = client
        .create_signer(serde_json::json!({
            "id": id,
            "type": "aws_kms",
            "config": {
                "region": "us-east-1",
                "key_id": ""
            }
        }))
        .await;

    assert!(result.is_err(), "Should fail for empty key_id");
    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("400") || error_msg.contains("Key ID") || error_msg.contains("empty"),
        "Error should indicate bad request: {}",
        error_msg
    );
}

/// Tests that creating a signer with mismatched type and config returns 400
#[tokio::test]
#[serial]
async fn test_create_signer_type_config_mismatch() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };
    let id = unique_id("mismatch");

    // Type says aws_kms but config has a "key" field (local signer config)
    let result = client
        .create_signer(serde_json::json!({
            "id": id,
            "type": "aws_kms",
            "config": {
                "key": "0000000000000000000000000000000000000000000000000000000000000001"
            }
        }))
        .await;

    assert!(result.is_err(), "Should fail for type/config mismatch");
    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("400") || error_msg.contains("does not match"),
        "Error should indicate bad request: {}",
        error_msg
    );
}
