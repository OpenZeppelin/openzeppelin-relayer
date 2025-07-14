use crate::{
    constants::{ID_REGEX, MINIMUM_SECRET_VALUE_LENGTH},
    models::{ApiError, NotificationRepoModel, NotificationType, SecretString},
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::{Validate, ValidationError};
/// Request structure for creating a new notification
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, ToSchema, Validate)]
pub struct NotificationCreateRequest {
    #[validate(
        length(min = 1, max = 36, message = "ID must be between 1 and 36 characters"),
        regex(
            path = "*ID_REGEX",
            message = "ID must contain only letters, numbers, dashes and underscores"
        )
    )]
    pub id: String,
    pub r#type: NotificationType,
    #[validate(url(message = "Invalid URL format"))]
    pub url: String,
    /// Optional signing key for securing webhook notifications
    #[validate(custom(function = "validate_signing_key"))]
    pub signing_key: Option<String>,
}

/// Request structure for updating an existing notification
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, ToSchema, Validate)]
pub struct NotificationUpdateRequest {
    pub r#type: Option<NotificationType>,
    #[validate(url(message = "Invalid URL format"))]
    pub url: Option<String>,
    /// Optional signing key for securing webhook notifications.
    /// - None: don't change the existing signing key
    /// - Some(""): remove the signing key
    /// - Some("key"): set the signing key to the provided value
    #[validate(custom(function = "validate_optional_signing_key"))]
    pub signing_key: Option<String>,
}
/// Custom validator for signing key in create requests
fn validate_signing_key(signing_key: &String) -> Result<(), ValidationError> {
    if !signing_key.is_empty() && signing_key.len() < MINIMUM_SECRET_VALUE_LENGTH {
        return Err(ValidationError::new("signing_key_too_short"));
    }
    Ok(())
}

/// Custom validator for optional signing key in update requests
fn validate_optional_signing_key(signing_key: &String) -> Result<(), ValidationError> {
    // Allow empty string (means remove the key)
    if !signing_key.is_empty() && signing_key.len() < MINIMUM_SECRET_VALUE_LENGTH {
        return Err(ValidationError::new("signing_key_too_short"));
    }
    Ok(())
}

impl NotificationCreateRequest {
    /// Validates the create request
    pub fn validate(&self) -> Result<(), ApiError> {
        Validate::validate(self).map_err(|e| {
            let error_messages: Vec<String> = e
                .field_errors()
                .iter()
                .flat_map(|(field, errors)| {
                    errors.iter().map(move |error| {
                        format!("{}: {}", field, error.message.as_ref().unwrap_or(&"Invalid value".into()))
                    })
                })
                .collect();
            ApiError::BadRequest(error_messages.join(", "))
        })
    }
}

impl NotificationUpdateRequest {
    /// Validates the update request
    pub fn validate(&self) -> Result<(), ApiError> {
        Validate::validate(self).map_err(|e| {
            let error_messages: Vec<String> = e
                .field_errors()
                .iter()
                .flat_map(|(field, errors)| {
                    errors.iter().map(move |error| {
                        format!("{}: {}", field, error.message.as_ref().unwrap_or(&"Invalid value".into()))
                    })
                })
                .collect();
            ApiError::BadRequest(error_messages.join(", "))
        })
    }

     /// Applies the update request to an existing notification model
     pub fn apply_to(&self, mut model: NotificationRepoModel) -> NotificationRepoModel {
        if let Some(notification_type) = &self.r#type {
            model.notification_type = notification_type.clone();
        }
        if let Some(url) = &self.url {
            model.url = url.clone();
        }
        if let Some(signing_key) = &self.signing_key {
            if signing_key.is_empty() {
                // Empty string means remove the signing key
                model.signing_key = None;
            } else {
                // Non-empty string means update the signing key
                model.signing_key = Some(SecretString::new(signing_key));
            }
        }
        model
    }
}

impl From<NotificationCreateRequest> for NotificationRepoModel {
    fn from(request: NotificationCreateRequest) -> Self {
        Self {
            id: request.id,
            notification_type: request.r#type,
            url: request.url,
            signing_key: request.signing_key.map(|s| SecretString::new(&s)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_create_request() {
        let request = NotificationCreateRequest {
            id: "test-notification".to_string(),
            r#type: NotificationType::Webhook,
            url: "https://example.com/webhook".to_string(),
            signing_key: Some("a".repeat(32)), // Minimum length
        };

        assert!(request.validate().is_ok());
    }

    #[test]
    fn test_invalid_id_too_long() {
        let request = NotificationCreateRequest {
            id: "a".repeat(37), // Too long
            r#type: NotificationType::Webhook,
            url: "https://example.com/webhook".to_string(),
            signing_key: None,
        };

        assert!(request.validate().is_err());
    }

    #[test]
    fn test_invalid_id_format() {
        let request = NotificationCreateRequest {
            id: "invalid@id".to_string(), // Invalid characters
            r#type: NotificationType::Webhook,
            url: "https://example.com/webhook".to_string(),
            signing_key: None,
        };

        assert!(request.validate().is_err());
    }

    #[test]
    fn test_invalid_url_format() {
        let request = NotificationCreateRequest {
            id: "test-notification".to_string(),
            r#type: NotificationType::Webhook,
            url: "not-a-url".to_string(),
            signing_key: None,
        };

        assert!(request.validate().is_err());
    }

    #[test]
    fn test_signing_key_too_short() {
        let request = NotificationCreateRequest {
            id: "test-notification".to_string(),
            r#type: NotificationType::Webhook,
            url: "https://example.com/webhook".to_string(),
            signing_key: Some("short".to_string()), // Too short
        };

        assert!(request.validate().is_err());
    }

    #[test]
    fn test_valid_update_request() {
        let request = NotificationUpdateRequest {
            r#type: Some(NotificationType::Webhook),
            url: Some("https://updated.example.com/webhook".to_string()),
            signing_key: Some("a".repeat(32)), // Minimum length
        };

        assert!(request.validate().is_ok());
    }

    #[test]
    fn test_update_request_empty_signing_key() {
        let request = NotificationUpdateRequest {
            r#type: None,
            url: None,
            signing_key: Some("".to_string()), // Empty string to remove key
        };

        assert!(request.validate().is_ok());
    }

    #[test]
    fn test_from_notification_create_request() {
        let request = NotificationCreateRequest {
            id: "test-id".to_string(),
            r#type: NotificationType::Webhook,
            url: "https://example.com/webhook".to_string(),
            signing_key: Some("secret-key".to_string()),
        };

        let model = NotificationRepoModel::from(request);

        assert_eq!(model.id, "test-id");
        assert_eq!(model.notification_type, NotificationType::Webhook);
        assert_eq!(model.url, "https://example.com/webhook");
        assert!(model.signing_key.is_some());
    }

    #[test]
    fn test_from_notification_create_request_without_signing_key() {
        let request = NotificationCreateRequest {
            id: "test-id".to_string(),
            r#type: NotificationType::Webhook,
            url: "https://example.com/webhook".to_string(),
            signing_key: None,
        };

        let model = NotificationRepoModel::from(request);

        assert_eq!(model.id, "test-id");
        assert_eq!(model.notification_type, NotificationType::Webhook);
        assert_eq!(model.url, "https://example.com/webhook");
        assert!(model.signing_key.is_none());
    }

    #[test]
    fn test_notification_update_request_apply_to() {
        let original_model = NotificationRepoModel {
            id: "test-id".to_string(),
            notification_type: NotificationType::Webhook,
            url: "https://example.com/webhook".to_string(),
            signing_key: Some(SecretString::new("old-key")),
        };

        let update_request = NotificationUpdateRequest {
            r#type: None,
            url: Some("https://new-example.com/webhook".to_string()),
            signing_key: Some("new-key".to_string()),
        };

        let updated_model = update_request.apply_to(original_model);

        assert_eq!(updated_model.id, "test-id");
        assert_eq!(updated_model.notification_type, NotificationType::Webhook);
        assert_eq!(updated_model.url, "https://new-example.com/webhook");
        assert!(updated_model.signing_key.is_some());
    }

    #[test]
    fn test_notification_update_request_partial_update() {
        let original_model = NotificationRepoModel {
            id: "test-id".to_string(),
            notification_type: NotificationType::Webhook,
            url: "https://example.com/webhook".to_string(),
            signing_key: Some(SecretString::new("old-key")),
        };

        let update_request = NotificationUpdateRequest {
            r#type: None,
            url: Some("https://new-example.com/webhook".to_string()),
            signing_key: None,
        };

        let updated_model = update_request.apply_to(original_model);

        assert_eq!(updated_model.id, "test-id");
        assert_eq!(updated_model.notification_type, NotificationType::Webhook);
        assert_eq!(updated_model.url, "https://new-example.com/webhook");
        assert!(updated_model.signing_key.is_some()); // Should remain unchanged
    }

    #[test]
    fn test_notification_update_request_remove_signing_key() {
        let mut model = NotificationRepoModel {
            id: "test-id".to_string(),
            notification_type: NotificationType::Webhook,
            url: "https://example.com/webhook".to_string(),
            signing_key: Some(SecretString::new("existing-key")),
        };

        let update_request = NotificationUpdateRequest {
            r#type: None,
            url: None,
            signing_key: Some("".to_string()), // Empty string to remove
        };

        model = update_request.apply_to(model);

        assert_eq!(model.signing_key, None);
    }
}
