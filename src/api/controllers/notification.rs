//! # Notifications Controller
//!
//! Handles HTTP endpoints for notification operations including:
//! - Listing notifications
//! - Getting notification details
//! - Creating notifications
//! - Updating notifications
//! - Deleting notifications

use crate::{
    jobs::JobProducerTrait,
    models::{
        ApiError, ApiResponse, NetworkRepoModel, Notification, NotificationCreateRequest,
        NotificationRepoModel, NotificationResponse, NotificationUpdateRequest, PaginationMeta,
        PaginationQuery, RelayerRepoModel, SignerRepoModel, ThinDataAppState, TransactionRepoModel,
    },
    repositories::{
        NetworkRepository, PluginRepositoryTrait, RelayerRepository, Repository,
        TransactionCounterTrait, TransactionRepository,
    },
};

use actix_web::HttpResponse;
use eyre::Result;

/// Lists all notifications with pagination support.
///
/// # Arguments
///
/// * `query` - The pagination query parameters.
/// * `state` - The application state containing the notification repository.
///
/// # Returns
///
/// A paginated list of notifications.
pub async fn list_notifications<J, RR, TR, NR, NFR, SR, TCR, PR>(
    query: PaginationQuery,
    state: ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR>,
) -> Result<HttpResponse, ApiError>
where
    J: JobProducerTrait + Send + Sync + 'static,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
    SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PR: PluginRepositoryTrait + Send + Sync + 'static,
{
    let notifications = state.notification_repository.list_paginated(query).await?;

    let mapped_notifications: Vec<NotificationResponse> =
        notifications.items.into_iter().map(|n| n.into()).collect();

    Ok(HttpResponse::Ok().json(ApiResponse::paginated(
        mapped_notifications,
        PaginationMeta {
            total_items: notifications.total,
            current_page: notifications.page,
            per_page: notifications.per_page,
        },
    )))
}

/// Retrieves details of a specific notification by ID.
///
/// # Arguments
///
/// * `notification_id` - The ID of the notification to retrieve.
/// * `state` - The application state containing the notification repository.
///
/// # Returns
///
/// The notification details or an error if not found.
pub async fn get_notification<J, RR, TR, NR, NFR, SR, TCR, PR>(
    notification_id: String,
    state: ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR>,
) -> Result<HttpResponse, ApiError>
where
    J: JobProducerTrait + Send + Sync + 'static,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
    SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PR: PluginRepositoryTrait + Send + Sync + 'static,
{
    let notification = state
        .notification_repository
        .get_by_id(notification_id)
        .await?;

    let response = NotificationResponse::from(notification);
    Ok(HttpResponse::Ok().json(ApiResponse::success(response)))
}

/// Creates a new notification.
///
/// # Arguments
///
/// * `request` - The notification creation request.
/// * `state` - The application state containing the notification repository.
///
/// # Returns
///
/// The created notification or an error if creation fails.
pub async fn create_notification<J, RR, TR, NR, NFR, SR, TCR, PR>(
    request: NotificationCreateRequest,
    state: ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR>,
) -> Result<HttpResponse, ApiError>
where
    J: JobProducerTrait + Send + Sync + 'static,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
    SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PR: PluginRepositoryTrait + Send + Sync + 'static,
{
    // Convert request to core notification (validates automatically)
    let notification = Notification::try_from(request)?;

    // Convert to repository model
    let notification_model = NotificationRepoModel::from(notification);
    let created_notification = state
        .notification_repository
        .create(notification_model)
        .await?;

    let response = NotificationResponse::from(created_notification);
    Ok(HttpResponse::Created().json(ApiResponse::success(response)))
}

/// Updates an existing notification.
///
/// # Arguments
///
/// * `notification_id` - The ID of the notification to update.
/// * `request` - The notification update request.
/// * `state` - The application state containing the notification repository.
///
/// # Returns
///
/// The updated notification or an error if update fails.
pub async fn update_notification<J, RR, TR, NR, NFR, SR, TCR, PR>(
    notification_id: String,
    request: NotificationUpdateRequest,
    state: ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR>,
) -> Result<HttpResponse, ApiError>
where
    J: JobProducerTrait + Send + Sync + 'static,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
    SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PR: PluginRepositoryTrait + Send + Sync + 'static,
{
    // Get the existing notification from repository
    let existing_repo_model = state
        .notification_repository
        .get_by_id(notification_id.clone())
        .await?;

    // Apply update (with validation)
    let updated = Notification::from(existing_repo_model).apply_update(&request)?;

    let saved_notification = state
        .notification_repository
        .update(notification_id, NotificationRepoModel::from(updated))
        .await?;

    let response = NotificationResponse::from(saved_notification);
    Ok(HttpResponse::Ok().json(ApiResponse::success(response)))
}

/// Deletes a notification by ID.
///
/// # Arguments
///
/// * `notification_id` - The ID of the notification to delete.
/// * `state` - The application state containing the notification repository.
///
/// # Returns
///
/// A success response or an error if deletion fails.
///
/// # Security
///
/// This endpoint ensures that notifications cannot be deleted if they are still being
/// used by any relayers. This prevents breaking existing relayer configurations
/// and maintains system integrity.
pub async fn delete_notification<J, RR, TR, NR, NFR, SR, TCR, PR>(
    notification_id: String,
    state: ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR>,
) -> Result<HttpResponse, ApiError>
where
    J: JobProducerTrait + Send + Sync + 'static,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
    SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PR: PluginRepositoryTrait + Send + Sync + 'static,
{
    // First check if the notification exists
    let _notification = state
        .notification_repository
        .get_by_id(notification_id.clone())
        .await?;

    // Check if any relayers are using this notification
    let connected_relayers = state
        .relayer_repository
        .list_by_notification_id(&notification_id)
        .await?;

    if !connected_relayers.is_empty() {
        let relayer_names: Vec<String> =
            connected_relayers.iter().map(|r| r.name.clone()).collect();
        return Err(ApiError::BadRequest(format!(
            "Cannot delete notification '{}' because it is being used by {} relayer(s): {}. Please remove or reconfigure these relayers before deleting the notification.",
            notification_id,
            connected_relayers.len(),
            relayer_names.join(", ")
        )));
    }

    // Safe to delete - no relayers are using this notification
    state
        .notification_repository
        .delete_by_id(notification_id)
        .await?;

    Ok(HttpResponse::Ok().json(ApiResponse::success("Notification deleted successfully")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        models::{ApiError, NotificationType, SecretString},
        utils::mocks::mockutils::create_mock_app_state,
    };
    use actix_web::web::ThinData;

    /// Helper function to create a test notification model
    fn create_test_notification_model(id: &str) -> NotificationRepoModel {
        NotificationRepoModel {
            id: id.to_string(),
            notification_type: NotificationType::Webhook,
            url: "https://example.com/webhook".to_string(),
            signing_key: Some(SecretString::new("a".repeat(32).as_str())), // 32 chars minimum
        }
    }

    /// Helper function to create a test notification create request
    fn create_test_notification_create_request(id: &str) -> NotificationCreateRequest {
        NotificationCreateRequest {
            id: Some(id.to_string()),
            r#type: Some(NotificationType::Webhook),
            url: "https://example.com/webhook".to_string(),
            signing_key: Some("a".repeat(32)), // 32 chars minimum
        }
    }

    /// Helper function to create a test notification update request
    fn create_test_notification_update_request() -> NotificationUpdateRequest {
        NotificationUpdateRequest {
            r#type: Some(NotificationType::Webhook),
            url: Some("https://updated.example.com/webhook".to_string()),
            signing_key: Some("b".repeat(32)), // 32 chars minimum
        }
    }

    #[actix_web::test]
    async fn test_list_notifications_empty() {
        let app_state = create_mock_app_state(None, None, None, None, None).await;
        let query = PaginationQuery {
            page: 1,
            per_page: 10,
        };

        let result = list_notifications(query, ThinData(app_state)).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 200);

        let body = actix_web::body::to_bytes(response.into_body())
            .await
            .unwrap();
        let api_response: ApiResponse<Vec<NotificationResponse>> =
            serde_json::from_slice(&body).unwrap();

        assert!(api_response.success);
        let data = api_response.data.unwrap();
        assert_eq!(data.len(), 0);
    }

    #[actix_web::test]
    async fn test_list_notifications_with_data() {
        let app_state = create_mock_app_state(None, None, None, None, None).await;

        // Create test notifications
        let notification1 = create_test_notification_model("test-1");
        let notification2 = create_test_notification_model("test-2");

        app_state
            .notification_repository
            .create(notification1)
            .await
            .unwrap();
        app_state
            .notification_repository
            .create(notification2)
            .await
            .unwrap();

        let query = PaginationQuery {
            page: 1,
            per_page: 10,
        };

        let result = list_notifications(query, ThinData(app_state)).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 200);

        let body = actix_web::body::to_bytes(response.into_body())
            .await
            .unwrap();
        let api_response: ApiResponse<Vec<NotificationResponse>> =
            serde_json::from_slice(&body).unwrap();

        assert!(api_response.success);
        let data = api_response.data.unwrap();
        assert_eq!(data.len(), 2);

        // Check that both notifications are present (order not guaranteed)
        let ids: Vec<&String> = data.iter().map(|n| &n.id).collect();
        assert!(ids.contains(&&"test-1".to_string()));
        assert!(ids.contains(&&"test-2".to_string()));
    }

    #[actix_web::test]
    async fn test_list_notifications_pagination() {
        let app_state = create_mock_app_state(None, None, None, None, None).await;

        // Create multiple test notifications
        for i in 1..=5 {
            let notification = create_test_notification_model(&format!("test-{}", i));
            app_state
                .notification_repository
                .create(notification)
                .await
                .unwrap();
        }

        let query = PaginationQuery {
            page: 2,
            per_page: 2,
        };

        let result = list_notifications(query, ThinData(app_state)).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 200);

        let body = actix_web::body::to_bytes(response.into_body())
            .await
            .unwrap();
        let api_response: ApiResponse<Vec<NotificationResponse>> =
            serde_json::from_slice(&body).unwrap();

        assert!(api_response.success);
        let data = api_response.data.unwrap();
        assert_eq!(data.len(), 2);
    }

    #[actix_web::test]
    async fn test_get_notification_success() {
        let app_state = create_mock_app_state(None, None, None, None, None).await;

        // Create a test notification
        let notification = create_test_notification_model("test-notification");
        app_state
            .notification_repository
            .create(notification.clone())
            .await
            .unwrap();

        let result = get_notification("test-notification".to_string(), ThinData(app_state)).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 200);

        let body = actix_web::body::to_bytes(response.into_body())
            .await
            .unwrap();
        let api_response: ApiResponse<NotificationResponse> =
            serde_json::from_slice(&body).unwrap();

        assert!(api_response.success);
        let data = api_response.data.unwrap();
        assert_eq!(data.id, "test-notification");
        assert_eq!(data.r#type, NotificationType::Webhook);
        assert_eq!(data.url, "https://example.com/webhook");
        assert!(data.has_signing_key); // Should have signing key (32 chars)
    }

    #[actix_web::test]
    async fn test_get_notification_not_found() {
        let app_state = create_mock_app_state(None, None, None, None, None).await;

        let result = get_notification("non-existent".to_string(), ThinData(app_state)).await;

        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(matches!(error, ApiError::NotFound(_)));
    }

    #[actix_web::test]
    async fn test_create_notification_success() {
        let app_state = create_mock_app_state(None, None, None, None, None).await;

        let request = create_test_notification_create_request("new-notification");

        let result = create_notification(request, ThinData(app_state)).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 201);

        let body = actix_web::body::to_bytes(response.into_body())
            .await
            .unwrap();
        let api_response: ApiResponse<NotificationResponse> =
            serde_json::from_slice(&body).unwrap();

        assert!(api_response.success);
        let data = api_response.data.unwrap();
        assert_eq!(data.id, "new-notification");
        assert_eq!(data.r#type, NotificationType::Webhook);
        assert_eq!(data.url, "https://example.com/webhook");
        assert!(data.has_signing_key); // Should have signing key (32 chars)
    }

    #[actix_web::test]
    async fn test_create_notification_without_signing_key() {
        let app_state = create_mock_app_state(None, None, None, None, None).await;

        let request = NotificationCreateRequest {
            id: Some("new-notification".to_string()),
            r#type: Some(NotificationType::Webhook),
            url: "https://example.com/webhook".to_string(),
            signing_key: None,
        };

        let result = create_notification(request, ThinData(app_state)).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 201);

        let body = actix_web::body::to_bytes(response.into_body())
            .await
            .unwrap();
        let api_response: ApiResponse<NotificationResponse> =
            serde_json::from_slice(&body).unwrap();

        assert!(api_response.success);
        let data = api_response.data.unwrap();
        assert_eq!(data.id, "new-notification");
        assert_eq!(data.r#type, NotificationType::Webhook);
        assert_eq!(data.url, "https://example.com/webhook");
        assert!(!data.has_signing_key); // Should not have signing key
    }

    #[actix_web::test]
    async fn test_update_notification_success() {
        let app_state = create_mock_app_state(None, None, None, None, None).await;

        // Create a test notification
        let notification = create_test_notification_model("test-notification");
        app_state
            .notification_repository
            .create(notification)
            .await
            .unwrap();

        let update_request = create_test_notification_update_request();

        let result = update_notification(
            "test-notification".to_string(),
            update_request,
            ThinData(app_state),
        )
        .await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 200);

        let body = actix_web::body::to_bytes(response.into_body())
            .await
            .unwrap();
        let api_response: ApiResponse<NotificationResponse> =
            serde_json::from_slice(&body).unwrap();

        assert!(api_response.success);
        let data = api_response.data.unwrap();
        assert_eq!(data.id, "test-notification");
        assert_eq!(data.url, "https://updated.example.com/webhook");
        assert!(data.has_signing_key); // Should have updated signing key
    }

    #[actix_web::test]
    async fn test_update_notification_not_found() {
        let app_state = create_mock_app_state(None, None, None, None, None).await;

        let update_request = create_test_notification_update_request();

        let result = update_notification(
            "non-existent".to_string(),
            update_request,
            ThinData(app_state),
        )
        .await;

        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(matches!(error, ApiError::NotFound(_)));
    }

    #[actix_web::test]
    async fn test_delete_notification_success() {
        let app_state = create_mock_app_state(None, None, None, None, None).await;

        // Create a test notification
        let notification = create_test_notification_model("test-notification");
        app_state
            .notification_repository
            .create(notification)
            .await
            .unwrap();

        let result =
            delete_notification("test-notification".to_string(), ThinData(app_state)).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 200);

        let body = actix_web::body::to_bytes(response.into_body())
            .await
            .unwrap();
        let api_response: ApiResponse<&str> = serde_json::from_slice(&body).unwrap();

        assert!(api_response.success);
        assert_eq!(
            api_response.data.unwrap(),
            "Notification deleted successfully"
        );
    }

    #[actix_web::test]
    async fn test_delete_notification_not_found() {
        let app_state = create_mock_app_state(None, None, None, None, None).await;

        let result = delete_notification("non-existent".to_string(), ThinData(app_state)).await;

        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(matches!(error, ApiError::NotFound(_)));
    }

    #[actix_web::test]
    async fn test_notification_response_conversion() {
        let notification_model = NotificationRepoModel {
            id: "test-id".to_string(),
            notification_type: NotificationType::Webhook,
            url: "https://example.com/webhook".to_string(),
            signing_key: Some(SecretString::new("secret-key")),
        };

        let response = NotificationResponse::from(notification_model);

        assert_eq!(response.id, "test-id");
        assert_eq!(response.r#type, NotificationType::Webhook);
        assert_eq!(response.url, "https://example.com/webhook");
        assert!(response.has_signing_key);
    }

    #[actix_web::test]
    async fn test_notification_response_conversion_without_signing_key() {
        let notification_model = NotificationRepoModel {
            id: "test-id".to_string(),
            notification_type: NotificationType::Webhook,
            url: "https://example.com/webhook".to_string(),
            signing_key: None,
        };

        let response = NotificationResponse::from(notification_model);

        assert_eq!(response.id, "test-id");
        assert_eq!(response.r#type, NotificationType::Webhook);
        assert_eq!(response.url, "https://example.com/webhook");
        assert!(!response.has_signing_key);
    }

    #[actix_web::test]
    async fn test_create_notification_validates_repository_creation() {
        let app_state = create_mock_app_state(None, None, None, None, None).await;
        let app_state_2 = create_mock_app_state(None, None, None, None, None).await;

        let request = create_test_notification_create_request("new-notification");
        let result = create_notification(request, ThinData(app_state)).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 201);

        let body = actix_web::body::to_bytes(response.into_body())
            .await
            .unwrap();
        let api_response: ApiResponse<NotificationResponse> =
            serde_json::from_slice(&body).unwrap();

        assert!(api_response.success);
        let data = api_response.data.unwrap();
        assert_eq!(data.id, "new-notification");
        assert_eq!(data.r#type, NotificationType::Webhook);
        assert_eq!(data.url, "https://example.com/webhook");
        assert!(data.has_signing_key);

        let request_2 = create_test_notification_create_request("new-notification");
        let result_2 = create_notification(request_2, ThinData(app_state_2)).await;

        assert!(result_2.is_ok());
        let response_2 = result_2.unwrap();
        assert_eq!(response_2.status(), 201);
    }

    #[actix_web::test]
    async fn test_create_notification_validation_error() {
        let app_state = create_mock_app_state(None, None, None, None, None).await;

        // Create a request with only invalid ID to make test deterministic
        let request = NotificationCreateRequest {
            id: Some("invalid@id".to_string()), // Invalid characters
            r#type: Some(NotificationType::Webhook),
            url: "https://valid.example.com/webhook".to_string(), // Valid URL
            signing_key: Some("a".repeat(32)),                    // Valid signing key
        };

        let result = create_notification(request, ThinData(app_state)).await;

        // Should fail with validation error
        assert!(result.is_err());
        if let Err(ApiError::BadRequest(msg)) = result {
            // The validator returns the first validation error it encounters
            // In this case, ID validation fails first
            assert!(msg.contains("ID must contain only letters, numbers, dashes and underscores"));
        } else {
            panic!("Expected BadRequest error with validation messages");
        }
    }

    #[actix_web::test]
    async fn test_update_notification_validation_error() {
        let app_state = create_mock_app_state(None, None, None, None, None).await;

        // Create a test notification
        let notification = create_test_notification_model("test-notification");
        app_state
            .notification_repository
            .create(notification)
            .await
            .unwrap();

        // Create an update request with invalid signing key but valid URL
        let update_request = NotificationUpdateRequest {
            r#type: Some(NotificationType::Webhook),
            url: Some("https://valid.example.com/webhook".to_string()), // Valid URL
            signing_key: Some("short".to_string()),                     // Too short
        };

        let result = update_notification(
            "test-notification".to_string(),
            update_request,
            ThinData(app_state),
        )
        .await;

        // Should fail with validation error
        assert!(result.is_err());
        if let Err(ApiError::BadRequest(msg)) = result {
            // The validator returns the first error it encounters
            // In this case, signing key validation fails first
            assert!(
                msg.contains("Signing key must be at least") && msg.contains("characters long")
            );
        } else {
            panic!("Expected BadRequest error with validation messages");
        }
    }

    #[actix_web::test]
    async fn test_delete_notification_blocked_by_connected_relayers() {
        let app_state = create_mock_app_state(None, None, None, None, None).await;

        // Create a test notification
        let notification = create_test_notification_model("connected-notification");
        app_state
            .notification_repository
            .create(notification)
            .await
            .unwrap();

        // Create a relayer that uses this notification
        let relayer = crate::models::RelayerRepoModel {
            id: "test-relayer".to_string(),
            name: "Test Relayer".to_string(),
            network: "ethereum".to_string(),
            paused: false,
            network_type: crate::models::NetworkType::Evm,
            signer_id: "test-signer".to_string(),
            policies: crate::models::RelayerNetworkPolicy::Evm(
                crate::models::RelayerEvmPolicy::default(),
            ),
            address: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            notification_id: Some("connected-notification".to_string()), // References our notification
            system_disabled: false,
            custom_rpc_urls: None,
        };
        app_state.relayer_repository.create(relayer).await.unwrap();

        // Try to delete the notification - should fail
        let result =
            delete_notification("connected-notification".to_string(), ThinData(app_state)).await;

        assert!(result.is_err());
        let error = result.unwrap_err();
        if let ApiError::BadRequest(msg) = error {
            assert!(msg.contains("Cannot delete notification"));
            assert!(msg.contains("being used by"));
            assert!(msg.contains("Test Relayer"));
            assert!(msg.contains("remove or reconfigure"));
        } else {
            panic!("Expected BadRequest error");
        }
    }

    #[actix_web::test]
    async fn test_delete_notification_after_relayer_removed() {
        let app_state = create_mock_app_state(None, None, None, None, None).await;

        // Create a test notification
        let notification = create_test_notification_model("cleanup-notification");
        app_state
            .notification_repository
            .create(notification)
            .await
            .unwrap();

        // Create a relayer that uses this notification
        let relayer = crate::models::RelayerRepoModel {
            id: "temp-relayer".to_string(),
            name: "Temporary Relayer".to_string(),
            network: "ethereum".to_string(),
            paused: false,
            network_type: crate::models::NetworkType::Evm,
            signer_id: "test-signer".to_string(),
            policies: crate::models::RelayerNetworkPolicy::Evm(
                crate::models::RelayerEvmPolicy::default(),
            ),
            address: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            notification_id: Some("cleanup-notification".to_string()),
            system_disabled: false,
            custom_rpc_urls: None,
        };
        app_state.relayer_repository.create(relayer).await.unwrap();

        // First deletion attempt should fail
        let result =
            delete_notification("cleanup-notification".to_string(), ThinData(app_state)).await;
        assert!(result.is_err());

        // Create new app state for second test (since app_state was consumed)
        let app_state2 = create_mock_app_state(None, None, None, None, None).await;

        // Re-create the notification in the new state
        let notification2 = create_test_notification_model("cleanup-notification");
        app_state2
            .notification_repository
            .create(notification2)
            .await
            .unwrap();

        // Now notification deletion should succeed (no relayers in new state)
        let result =
            delete_notification("cleanup-notification".to_string(), ThinData(app_state2)).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 200);
    }

    #[actix_web::test]
    async fn test_delete_notification_with_multiple_relayers() {
        let app_state = create_mock_app_state(None, None, None, None, None).await;

        // Create a test notification
        let notification = create_test_notification_model("multi-relayer-notification");
        app_state
            .notification_repository
            .create(notification)
            .await
            .unwrap();

        // Create multiple relayers that use this notification
        let relayers = vec![
            crate::models::RelayerRepoModel {
                id: "relayer-1".to_string(),
                name: "EVM Relayer".to_string(),
                network: "ethereum".to_string(),
                paused: false,
                network_type: crate::models::NetworkType::Evm,
                signer_id: "test-signer".to_string(),
                policies: crate::models::RelayerNetworkPolicy::Evm(
                    crate::models::RelayerEvmPolicy::default(),
                ),
                address: "0x1111111111111111111111111111111111111111".to_string(),
                notification_id: Some("multi-relayer-notification".to_string()),
                system_disabled: false,
                custom_rpc_urls: None,
            },
            crate::models::RelayerRepoModel {
                id: "relayer-2".to_string(),
                name: "Solana Relayer".to_string(),
                network: "solana".to_string(),
                paused: true, // Even paused relayers should block deletion
                network_type: crate::models::NetworkType::Solana,
                signer_id: "test-signer".to_string(),
                policies: crate::models::RelayerNetworkPolicy::Solana(
                    crate::models::RelayerSolanaPolicy::default(),
                ),
                address: "solana-address".to_string(),
                notification_id: Some("multi-relayer-notification".to_string()),
                system_disabled: false,
                custom_rpc_urls: None,
            },
            crate::models::RelayerRepoModel {
                id: "relayer-3".to_string(),
                name: "Stellar Relayer".to_string(),
                network: "stellar".to_string(),
                paused: false,
                network_type: crate::models::NetworkType::Stellar,
                signer_id: "test-signer".to_string(),
                policies: crate::models::RelayerNetworkPolicy::Stellar(
                    crate::models::RelayerStellarPolicy::default(),
                ),
                address: "stellar-address".to_string(),
                notification_id: Some("multi-relayer-notification".to_string()),
                system_disabled: true, // Even disabled relayers should block deletion
                custom_rpc_urls: None,
            },
        ];

        // Create all relayers
        for relayer in relayers {
            app_state.relayer_repository.create(relayer).await.unwrap();
        }

        // Try to delete the notification - should fail with detailed error
        let result = delete_notification(
            "multi-relayer-notification".to_string(),
            ThinData(app_state),
        )
        .await;

        assert!(result.is_err());
        let error = result.unwrap_err();
        if let ApiError::BadRequest(msg) = error {
            assert!(msg.contains("Cannot delete notification 'multi-relayer-notification'"));
            assert!(msg.contains("being used by 3 relayer(s)"));
            assert!(msg.contains("EVM Relayer"));
            assert!(msg.contains("Solana Relayer"));
            assert!(msg.contains("Stellar Relayer"));
            assert!(msg.contains("remove or reconfigure"));
        } else {
            panic!("Expected BadRequest error, got: {:?}", error);
        }
    }

    #[actix_web::test]
    async fn test_delete_notification_with_some_relayers_using_different_notification() {
        let app_state = create_mock_app_state(None, None, None, None, None).await;

        // Create two test notifications
        let notification1 = create_test_notification_model("notification-to-delete");
        let notification2 = create_test_notification_model("other-notification");
        app_state
            .notification_repository
            .create(notification1)
            .await
            .unwrap();
        app_state
            .notification_repository
            .create(notification2)
            .await
            .unwrap();

        // Create relayers - only one uses the notification we want to delete
        let relayer1 = crate::models::RelayerRepoModel {
            id: "blocking-relayer".to_string(),
            name: "Blocking Relayer".to_string(),
            network: "ethereum".to_string(),
            paused: false,
            network_type: crate::models::NetworkType::Evm,
            signer_id: "test-signer".to_string(),
            policies: crate::models::RelayerNetworkPolicy::Evm(
                crate::models::RelayerEvmPolicy::default(),
            ),
            address: "0x1111111111111111111111111111111111111111".to_string(),
            notification_id: Some("notification-to-delete".to_string()), // This one blocks deletion
            system_disabled: false,
            custom_rpc_urls: None,
        };

        let relayer2 = crate::models::RelayerRepoModel {
            id: "non-blocking-relayer".to_string(),
            name: "Non-blocking Relayer".to_string(),
            network: "polygon".to_string(),
            paused: false,
            network_type: crate::models::NetworkType::Evm,
            signer_id: "test-signer".to_string(),
            policies: crate::models::RelayerNetworkPolicy::Evm(
                crate::models::RelayerEvmPolicy::default(),
            ),
            address: "0x2222222222222222222222222222222222222222".to_string(),
            notification_id: Some("other-notification".to_string()), // This one uses different notification
            system_disabled: false,
            custom_rpc_urls: None,
        };

        let relayer3 = crate::models::RelayerRepoModel {
            id: "no-notification-relayer".to_string(),
            name: "No Notification Relayer".to_string(),
            network: "bsc".to_string(),
            paused: false,
            network_type: crate::models::NetworkType::Evm,
            signer_id: "test-signer".to_string(),
            policies: crate::models::RelayerNetworkPolicy::Evm(
                crate::models::RelayerEvmPolicy::default(),
            ),
            address: "0x3333333333333333333333333333333333333333".to_string(),
            notification_id: None, // This one has no notification
            system_disabled: false,
            custom_rpc_urls: None,
        };

        app_state.relayer_repository.create(relayer1).await.unwrap();
        app_state.relayer_repository.create(relayer2).await.unwrap();
        app_state.relayer_repository.create(relayer3).await.unwrap();

        // Try to delete the first notification - should fail because of one relayer
        let result =
            delete_notification("notification-to-delete".to_string(), ThinData(app_state)).await;

        assert!(result.is_err());
        let error = result.unwrap_err();
        if let ApiError::BadRequest(msg) = error {
            assert!(msg.contains("being used by 1 relayer(s)"));
            assert!(msg.contains("Blocking Relayer"));
            assert!(!msg.contains("Non-blocking Relayer")); // Should not mention the other relayer
            assert!(!msg.contains("No Notification Relayer")); // Should not mention relayer with no notification
        } else {
            panic!("Expected BadRequest error");
        }

        // Try to delete the second notification - should succeed (no relayers using it in our test)
        let app_state2 = create_mock_app_state(None, None, None, None, None).await;
        let notification2_recreated = create_test_notification_model("other-notification");
        app_state2
            .notification_repository
            .create(notification2_recreated)
            .await
            .unwrap();

        let result =
            delete_notification("other-notification".to_string(), ThinData(app_state2)).await;

        assert!(result.is_ok());
    }
}
