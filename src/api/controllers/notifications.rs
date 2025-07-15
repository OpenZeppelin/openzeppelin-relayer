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
        ApiError, ApiResponse, NetworkRepoModel, Notification, NotificationCreateRequest, NotificationUpdateRequest, NotificationResponse,
        NotificationRepoModel, PaginationMeta, PaginationQuery, RelayerRepoModel, SignerRepoModel, ThinDataAppState, TransactionRepoModel,
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
            id: id.to_string(),
            r#type: NotificationType::Webhook,
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
            id: "new-notification".to_string(),
            r#type: NotificationType::Webhook,
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
            id: "invalid@id".to_string(), // Invalid characters
            r#type: NotificationType::Webhook,
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
}
