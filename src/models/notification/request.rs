//! API request models and validation for notification endpoints.
//!
//! This module handles incoming HTTP requests for notification operations, providing:
//!
//! - **Request Models**: Structures for creating and updating notifications via API
//! - **Input Validation**: Sanitization and validation of user-provided data
//! - **Domain Conversion**: Transformation from API requests to domain objects
//!
//! Serves as the entry point for notification data from external clients, ensuring
//! all input is properly validated before reaching the core business logic.

use crate::models::{ ApiError, notification::Notification, NotificationType, SecretString};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Request structure for creating a new notification
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, ToSchema)]
pub struct NotificationCreateRequest {
    pub id: String,
    pub r#type: NotificationType,
    pub url: String,
    /// Optional signing key for securing webhook notifications
    pub signing_key: Option<String>,
}

/// Request structure for updating an existing notification
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, ToSchema)]
pub struct NotificationUpdateRequest {
    pub r#type: Option<NotificationType>,
    pub url: Option<String>,
    /// Optional signing key for securing webhook notifications.
    /// - None: don't change the existing signing key
    /// - Some(""): remove the signing key
    /// - Some("key"): set the signing key to the provided value
    pub signing_key: Option<String>,
}


impl TryFrom<NotificationCreateRequest> for Notification {
    type Error = ApiError;

    fn try_from(request: NotificationCreateRequest) -> Result<Self, Self::Error> {
        let signing_key = request.signing_key.map(|s| SecretString::new(&s));

        let notification = Notification::new(request.id, request.r#type, request.url, signing_key);

        // Validate using core validation logic
        notification.validate().map_err(ApiError::from)?;

        Ok(notification)
    }
}

#[cfg(test)]
mod tests {
    use crate::models::NotificationRepoModel;

    use super::*;

    #[test]
    fn test_valid_create_request_conversion() {
        let request = NotificationCreateRequest {
            id: "test-notification".to_string(),
            r#type: NotificationType::Webhook,
            url: "https://example.com/webhook".to_string(),
            signing_key: Some("a".repeat(32)), // Minimum length
        };

        let result = Notification::try_from(request);
        assert!(result.is_ok());

        let notification = result.unwrap();
        assert_eq!(notification.id, "test-notification");
        assert_eq!(notification.notification_type, NotificationType::Webhook);
        assert_eq!(notification.url, "https://example.com/webhook");
        assert!(notification.signing_key.is_some());
    }

    #[test]
    fn test_invalid_create_request_conversion() {
        let request = NotificationCreateRequest {
            id: "invalid@id".to_string(), // Invalid characters
            r#type: NotificationType::Webhook,
            url: "https://example.com/webhook".to_string(),
            signing_key: None,
        };

        let result = Notification::try_from(request);
        assert!(result.is_err());

        if let Err(ApiError::BadRequest(msg)) = result {
            assert!(msg.contains("ID must contain only letters, numbers, dashes and underscores"));
        } else {
            panic!("Expected BadRequest error");
        }
    }

    #[test]
    fn test_signing_key_too_short() {
        let request = NotificationCreateRequest {
            id: "test-notification".to_string(),
            r#type: NotificationType::Webhook,
            url: "https://example.com/webhook".to_string(),
            signing_key: Some("short".to_string()), // Too short
        };

        let result = Notification::try_from(request);
        assert!(result.is_err());

        if let Err(ApiError::BadRequest(msg)) = result {
            assert!(msg.contains("Signing key must be at least"));
        } else {
            panic!("Expected BadRequest error");
        }
    }

    #[test]
    fn test_invalid_url() {
        let request = NotificationCreateRequest {
            id: "test-notification".to_string(),
            r#type: NotificationType::Webhook,
            url: "not-a-url".to_string(),
            signing_key: None,
        };

        let result = Notification::try_from(request);
        assert!(result.is_err());

        if let Err(ApiError::BadRequest(msg)) = result {
            assert!(msg.contains("Invalid URL format"));
        } else {
            panic!("Expected BadRequest error");
        }
    }

    #[test]
    fn test_update_request_validation_domain_first() {
        // Create existing core notification
        let existing_core = Notification::new(
            "test-id".to_string(),
            NotificationType::Webhook,
            "https://example.com/webhook".to_string(),
            Some(SecretString::new("existing-key")),
        );

        let update_request = NotificationUpdateRequest {
            r#type: None,
            url: Some("https://new-example.com/webhook".to_string()),
            signing_key: Some("a".repeat(32)), // Valid length
        };

        let result = existing_core.apply_update(&update_request);
        assert!(result.is_ok());

        let updated = result.unwrap();
        assert_eq!(updated.id, "test-id"); // ID should remain unchanged
        assert_eq!(updated.url, "https://new-example.com/webhook"); // URL should be updated
        assert!(updated.signing_key.is_some()); // Signing key should be updated
    }

    #[test]
    fn test_update_request_invalid_url_domain_first() {
        // Create existing core notification
        let existing_core = Notification::new(
            "test-id".to_string(),
            NotificationType::Webhook,
            "https://example.com/webhook".to_string(),
            None,
        );

        let update_request = NotificationUpdateRequest {
            r#type: None,
            url: Some("not-a-url".to_string()), // Invalid URL
            signing_key: None,
        };

        let result = existing_core.apply_update(&update_request);
        assert!(result.is_err());

        // Test the From conversion to ApiError
        let api_error: ApiError = result.unwrap_err().into();
        if let ApiError::BadRequest(msg) = api_error {
            assert!(msg.contains("Invalid URL format"));
        } else {
            panic!("Expected BadRequest error");
        }
    }

    #[test]
    fn test_notification_to_repo_model() {
        let notification = Notification::new(
            "test-id".to_string(),
            NotificationType::Webhook,
            "https://example.com/webhook".to_string(),
            Some(SecretString::new("test-key")),
        );

        let repo_model = NotificationRepoModel::from(notification);
        assert_eq!(repo_model.id, "test-id");
        assert_eq!(repo_model.notification_type, NotificationType::Webhook);
        assert_eq!(repo_model.url, "https://example.com/webhook");
        assert!(repo_model.signing_key.is_some());
    }
}
