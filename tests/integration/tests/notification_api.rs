//! Notification API integration tests
//!
//! Tests for the notification REST API endpoints including creating, listing,
//! retrieving, updating, and deleting notifications.

use crate::integration::common::client::RelayerClient;
use openzeppelin_relayer::models::{
    NotificationCreateRequest, NotificationType, NotificationUpdateRequest,
};
use serial_test::serial;

// =============================================================================
// Helper
// =============================================================================

/// Creates a unique notification ID safe for the API (alphanumeric + dashes, max 36 chars).
fn unique_id(prefix: &str) -> String {
    let uuid = uuid::Uuid::new_v4().to_string();
    // "prefix-<first 24 chars of uuid>" keeps it under 36 chars
    let max_uuid_len = 36 - prefix.len() - 1;
    format!("{}-{}", prefix, &uuid[..max_uuid_len])
}

// =============================================================================
// CRUD Happy Path Tests
// =============================================================================

/// Tests creating a notification with all fields
#[tokio::test]
#[serial]
async fn test_create_notification() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };
    let id = unique_id("notif");

    let created = client
        .create_notification(NotificationCreateRequest {
            id: Some(id.clone()),
            r#type: Some(NotificationType::Webhook),
            url: "https://example.com/webhook".to_string(),
            signing_key: Some("a".repeat(32)),
        })
        .await
        .expect("Failed to create notification");

    assert_eq!(created.id, id);
    assert_eq!(created.r#type, NotificationType::Webhook);
    assert_eq!(created.url, "https://example.com/webhook");
    assert!(created.has_signing_key);

    // Cleanup
    let _ = client.delete_notification(&id).await;
}

/// Tests creating a notification without a signing key
#[tokio::test]
#[serial]
async fn test_create_notification_without_signing_key() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };
    let id = unique_id("notif");

    let created = client
        .create_notification(NotificationCreateRequest {
            id: Some(id.clone()),
            r#type: Some(NotificationType::Webhook),
            url: "https://example.com/webhook".to_string(),
            signing_key: None,
        })
        .await
        .expect("Failed to create notification");

    assert_eq!(created.id, id);
    assert!(!created.has_signing_key);

    // Cleanup
    let _ = client.delete_notification(&id).await;
}

/// Tests creating a notification without an explicit id (auto-generated UUID)
#[tokio::test]
#[serial]
async fn test_create_notification_auto_id() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };

    let created = client
        .create_notification(NotificationCreateRequest {
            id: None,
            r#type: Some(NotificationType::Webhook),
            url: "https://example.com/webhook".to_string(),
            signing_key: None,
        })
        .await
        .expect("Failed to create notification");

    assert!(
        !created.id.is_empty(),
        "Auto-generated ID should not be empty"
    );
    assert!(
        created.id.len() <= 36,
        "Auto-generated ID should be at most 36 characters"
    );

    // Cleanup
    let _ = client.delete_notification(&created.id).await;
}

/// Tests listing notifications returns created resources
#[tokio::test]
#[serial]
async fn test_list_notifications() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };
    let id1 = unique_id("list1");
    let id2 = unique_id("list2");

    // Create two notifications
    client
        .create_notification(NotificationCreateRequest {
            id: Some(id1.clone()),
            r#type: Some(NotificationType::Webhook),
            url: "https://example.com/hook1".to_string(),
            signing_key: None,
        })
        .await
        .expect("Failed to create notification 1");

    client
        .create_notification(NotificationCreateRequest {
            id: Some(id2.clone()),
            r#type: Some(NotificationType::Webhook),
            url: "https://example.com/hook2".to_string(),
            signing_key: None,
        })
        .await
        .expect("Failed to create notification 2");

    let notifications = client
        .list_notifications(None, None)
        .await
        .expect("Failed to list notifications");

    let ids: Vec<&str> = notifications.iter().map(|n| n.id.as_str()).collect();
    assert!(ids.contains(&id1.as_str()), "Should contain notification 1");
    assert!(ids.contains(&id2.as_str()), "Should contain notification 2");

    // Cleanup
    let _ = client.delete_notification(&id1).await;
    let _ = client.delete_notification(&id2).await;
}

/// Tests pagination for listing notifications
#[tokio::test]
#[serial]
async fn test_list_notifications_pagination() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };
    let ids: Vec<String> = (0..3).map(|i| unique_id(&format!("pg{}", i))).collect();

    for id in &ids {
        client
            .create_notification(NotificationCreateRequest {
                id: Some(id.clone()),
                r#type: Some(NotificationType::Webhook),
                url: "https://example.com/hook".to_string(),
                signing_key: None,
            })
            .await
            .unwrap_or_else(|_| panic!("Failed to create notification {}", id));
    }

    let page1 = client
        .list_notifications(Some(1), Some(2))
        .await
        .expect("Failed to list page 1");

    let page2 = client
        .list_notifications(Some(2), Some(2))
        .await
        .expect("Failed to list page 2");

    // Pages should not overlap
    if !page1.is_empty() && !page2.is_empty() {
        let page1_ids: std::collections::HashSet<_> = page1.iter().map(|n| &n.id).collect();
        let page2_ids: std::collections::HashSet<_> = page2.iter().map(|n| &n.id).collect();
        assert!(
            page1_ids.is_disjoint(&page2_ids),
            "Page 1 and Page 2 should not have overlapping notifications"
        );
    }

    // Cleanup
    for id in &ids {
        let _ = client.delete_notification(id).await;
    }
}

/// Tests getting a notification by ID
#[tokio::test]
#[serial]
async fn test_get_notification() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };
    let id = unique_id("get");

    let created = client
        .create_notification(NotificationCreateRequest {
            id: Some(id.clone()),
            r#type: Some(NotificationType::Webhook),
            url: "https://example.com/webhook".to_string(),
            signing_key: None,
        })
        .await
        .expect("Failed to create notification");

    let fetched = client
        .get_notification(&id)
        .await
        .expect("Failed to get notification");

    assert_eq!(fetched.id, created.id);
    assert_eq!(fetched.r#type, created.r#type);
    assert_eq!(fetched.url, created.url);
    assert_eq!(fetched.has_signing_key, created.has_signing_key);

    // Cleanup
    let _ = client.delete_notification(&id).await;
}

/// Tests updating a notification's URL
#[tokio::test]
#[serial]
async fn test_update_notification_url() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };
    let id = unique_id("upd");

    client
        .create_notification(NotificationCreateRequest {
            id: Some(id.clone()),
            r#type: Some(NotificationType::Webhook),
            url: "https://example.com/old".to_string(),
            signing_key: None,
        })
        .await
        .expect("Failed to create notification");

    let updated = client
        .update_notification(
            &id,
            NotificationUpdateRequest {
                r#type: None,
                url: Some("https://example.com/new".to_string()),
                signing_key: None,
            },
        )
        .await
        .expect("Failed to update notification");

    assert_eq!(updated.id, id);
    assert_eq!(updated.url, "https://example.com/new");

    // Cleanup
    let _ = client.delete_notification(&id).await;
}

/// Tests updating a notification to add a signing key
#[tokio::test]
#[serial]
async fn test_update_notification_signing_key() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };
    let id = unique_id("updkey");

    let created = client
        .create_notification(NotificationCreateRequest {
            id: Some(id.clone()),
            r#type: Some(NotificationType::Webhook),
            url: "https://example.com/webhook".to_string(),
            signing_key: None,
        })
        .await
        .expect("Failed to create notification");

    assert!(!created.has_signing_key);

    let updated = client
        .update_notification(
            &id,
            NotificationUpdateRequest {
                r#type: None,
                url: None,
                signing_key: Some("b".repeat(32)),
            },
        )
        .await
        .expect("Failed to update notification");

    assert!(updated.has_signing_key);

    // Cleanup
    let _ = client.delete_notification(&id).await;
}

/// Tests deleting a notification and verifying it's gone
#[tokio::test]
#[serial]
async fn test_delete_notification() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };
    let id = unique_id("del");

    client
        .create_notification(NotificationCreateRequest {
            id: Some(id.clone()),
            r#type: Some(NotificationType::Webhook),
            url: "https://example.com/webhook".to_string(),
            signing_key: None,
        })
        .await
        .expect("Failed to create notification");

    client
        .delete_notification(&id)
        .await
        .expect("Failed to delete notification");

    let result = client.get_notification(&id).await;
    assert!(
        result.is_err(),
        "Should return an error for deleted notification"
    );
    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("404") || error_msg.contains("not found"),
        "Error should indicate not found: {}",
        error_msg
    );
}

// =============================================================================
// Error / Validation Tests
// =============================================================================

/// Tests that getting a non-existent notification returns 404
#[tokio::test]
#[serial]
async fn test_get_notification_not_found() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };

    let result = client.get_notification("nonexistent-id").await;

    assert!(
        result.is_err(),
        "Should return an error for non-existent notification"
    );
    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("404") || error_msg.contains("not found"),
        "Error should indicate not found: {}",
        error_msg
    );
}

/// Tests that creating a notification with an invalid URL returns 400
#[tokio::test]
#[serial]
async fn test_create_notification_invalid_url() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };

    let result = client
        .create_notification(NotificationCreateRequest {
            id: Some(unique_id("badurl")),
            r#type: Some(NotificationType::Webhook),
            url: "not-a-url".to_string(),
            signing_key: None,
        })
        .await;

    assert!(result.is_err(), "Should fail for invalid URL");
    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("400") || error_msg.contains("URL"),
        "Error should indicate bad request: {}",
        error_msg
    );
}

/// Tests that creating a notification with an invalid ID returns 400
#[tokio::test]
#[serial]
async fn test_create_notification_invalid_id() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };

    let result = client
        .create_notification(NotificationCreateRequest {
            id: Some("invalid@id".to_string()),
            r#type: Some(NotificationType::Webhook),
            url: "https://example.com/webhook".to_string(),
            signing_key: None,
        })
        .await;

    assert!(result.is_err(), "Should fail for invalid ID");
    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("400") || error_msg.contains("ID"),
        "Error should indicate bad request: {}",
        error_msg
    );
}

/// Tests that creating a notification with a short signing key returns 400
#[tokio::test]
#[serial]
async fn test_create_notification_short_signing_key() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };

    let result = client
        .create_notification(NotificationCreateRequest {
            id: Some(unique_id("shortkey")),
            r#type: Some(NotificationType::Webhook),
            url: "https://example.com/webhook".to_string(),
            signing_key: Some("short".to_string()),
        })
        .await;

    assert!(result.is_err(), "Should fail for short signing key");
    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("400") || error_msg.contains("signing") || error_msg.contains("Signing"),
        "Error should indicate bad request: {}",
        error_msg
    );
}

/// Tests that updating a non-existent notification returns 404
#[tokio::test]
#[serial]
async fn test_update_notification_not_found() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };

    let result = client
        .update_notification(
            "nonexistent-id",
            NotificationUpdateRequest {
                r#type: None,
                url: Some("https://example.com/new".to_string()),
                signing_key: None,
            },
        )
        .await;

    assert!(
        result.is_err(),
        "Should return an error for non-existent notification"
    );
    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("404") || error_msg.contains("not found"),
        "Error should indicate not found: {}",
        error_msg
    );
}

/// Tests that deleting a non-existent notification returns 404
#[tokio::test]
#[serial]
async fn test_delete_notification_not_found() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };

    let result = client.delete_notification("nonexistent-id").await;

    assert!(
        result.is_err(),
        "Should return an error for non-existent notification"
    );
    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("404") || error_msg.contains("not found"),
        "Error should indicate not found: {}",
        error_msg
    );
}
