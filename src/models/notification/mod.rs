//! Notification domain model and business logic.
//!
//! This module provides the central `Notification` type that represents notifications
//! throughout the relayer system, including:
//!
//! - **Domain Model**: Core `Notification` struct with validation
//! - **Business Logic**: Update operations and validation rules  
//! - **Error Handling**: Comprehensive validation error types
//! - **Interoperability**: Conversions between API, config, and repository representations
//!
//! The notification model supports webhook-based notifications with optional message signing.

mod config;
pub use config::*;

mod request;
pub use request::*;

mod response;
pub use response::*;

mod repository;
pub use repository::NotificationRepoModel;

mod webhook_notification;
pub use webhook_notification::*;

use crate::{
    constants::{ID_REGEX, MINIMUM_SECRET_VALUE_LENGTH},
    models::SecretString,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::{Validate, ValidationError};

/// Notification type enum used by both config file and API
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum NotificationType {
    Webhook,
}

/// Notification model used by both config file and API
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct Notification {
    #[validate(
        length(min = 1, max = 36, message = "ID must be between 1 and 36 characters"),
        regex(
            path = "*ID_REGEX",
            message = "ID must contain only letters, numbers, dashes and underscores"
        )
    )]
    pub id: String,
    pub notification_type: NotificationType,
    #[validate(url(message = "Invalid URL format"))]
    pub url: String,
    #[validate(custom(function = "validate_signing_key"))]
    pub signing_key: Option<SecretString>,
}

/// Custom validator for signing key - validator handles Option automatically
fn validate_signing_key(signing_key: &SecretString) -> Result<(), ValidationError> {
    let is_valid = signing_key.as_str(|key_str| key_str.len() >= MINIMUM_SECRET_VALUE_LENGTH);
    if !is_valid {
        return Err(ValidationError::new("signing_key_too_short"));
    }
    Ok(())
}

impl Notification {
    /// Creates a new notification
    pub fn new(
        id: String,
        notification_type: NotificationType,
        url: String,
        signing_key: Option<SecretString>,
    ) -> Self {
        Self {
            id,
            notification_type,
            url,
            signing_key,
        }
    }

    /// Validates the notification using the validator crate
    pub fn validate(&self) -> Result<(), NotificationValidationError> {
        Validate::validate(self).map_err(|validation_errors| {
            // Convert validator errors to our custom error type
            // Return the first error for simplicity
            for (field, errors) in validation_errors.field_errors() {
                if let Some(error) = errors.first() {
                    let field_str = field.as_ref();
                    return match (field_str, error.code.as_ref()) {
                        ("id", "length") => NotificationValidationError::InvalidIdFormat,
                        ("id", "regex") => NotificationValidationError::InvalidIdFormat,
                        ("url", _) => NotificationValidationError::InvalidUrl,
                        ("signing_key", "signing_key_too_short") => {
                            NotificationValidationError::signing_key_too_short()
                        }
                        _ => NotificationValidationError::InvalidIdFormat, // fallback
                    };
                }
            }
            // Fallback error
            NotificationValidationError::InvalidIdFormat
        })
    }

    /// Applies an update request to create a new validated notification
    ///
    /// This method provides a domain-first approach where the core model handles
    /// its own business rules and validation rather than having update logic
    /// scattered across request models.
    ///
    /// # Arguments
    /// * `request` - The update request containing partial data to apply
    ///
    /// # Returns
    /// * `Ok(Notification)` - A new validated notification with updates applied
    /// * `Err(NotificationValidationError)` - If the resulting notification would be invalid
    pub fn apply_update(
        &self,
        request: &NotificationUpdateRequest,
    ) -> Result<Self, NotificationValidationError> {
        let mut updated = self.clone();

        // Apply updates from request
        if let Some(notification_type) = &request.r#type {
            updated.notification_type = notification_type.clone();
        }

        if let Some(url) = &request.url {
            updated.url = url.clone();
        }

        if let Some(signing_key) = &request.signing_key {
            updated.signing_key = if signing_key.is_empty() {
                // Empty string means remove the signing key
                None
            } else {
                // Non-empty string means update the signing key
                Some(SecretString::new(signing_key))
            };
        }

        // Validate the complete updated model
        updated.validate()?;

        Ok(updated)
    }
}

/// Common validation errors for notifications
#[derive(Debug, thiserror::Error)]
pub enum NotificationValidationError {
    #[error("Notification ID cannot be empty")]
    EmptyId,
    #[error("Notification ID must contain only letters, numbers, dashes and underscores and must be at most 36 characters long")]
    InvalidIdFormat,
    #[error("Notification URL cannot be empty")]
    EmptyUrl,
    #[error("Invalid notification URL format")]
    InvalidUrl,
    #[error("Signing key must be at least {0} characters long")]
    SigningKeyTooShort(usize),
}

impl NotificationValidationError {
    pub fn signing_key_too_short() -> Self {
        Self::SigningKeyTooShort(MINIMUM_SECRET_VALUE_LENGTH)
    }
}

/// Centralized conversion from NotificationValidationError to ApiError
impl From<NotificationValidationError> for crate::models::ApiError {
    fn from(error: NotificationValidationError) -> Self {
        use crate::models::ApiError;

        ApiError::BadRequest(match error {
          NotificationValidationError::EmptyId => "ID cannot be empty".to_string(),
          NotificationValidationError::InvalidIdFormat => {
              "ID must contain only letters, numbers, dashes and underscores and must be at most 36 characters long".to_string()
          }
          NotificationValidationError::EmptyUrl => "URL cannot be empty".to_string(),
          NotificationValidationError::InvalidUrl => "Invalid URL format".to_string(),
          NotificationValidationError::SigningKeyTooShort(min_len) => {
              format!("Signing key must be at least {} characters long", min_len)
          }
      })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_notification() {
        let notification = Notification::new(
            "valid-id".to_string(),
            NotificationType::Webhook,
            "https://example.com/webhook".to_string(),
            Some(SecretString::new(&"a".repeat(32))),
        );

        assert!(notification.validate().is_ok());
    }

    #[test]
    fn test_empty_id() {
        let notification = Notification::new(
            "".to_string(),
            NotificationType::Webhook,
            "https://example.com/webhook".to_string(),
            None,
        );

        assert!(matches!(
            notification.validate(),
            Err(NotificationValidationError::InvalidIdFormat)
        ));
    }

    #[test]
    fn test_id_too_long() {
        let notification = Notification::new(
            "a".repeat(37),
            NotificationType::Webhook,
            "https://example.com/webhook".to_string(),
            None,
        );

        assert!(matches!(
            notification.validate(),
            Err(NotificationValidationError::InvalidIdFormat)
        ));
    }

    #[test]
    fn test_invalid_id_format() {
        let notification = Notification::new(
            "invalid@id".to_string(),
            NotificationType::Webhook,
            "https://example.com/webhook".to_string(),
            None,
        );

        assert!(matches!(
            notification.validate(),
            Err(NotificationValidationError::InvalidIdFormat)
        ));
    }

    #[test]
    fn test_invalid_url() {
        let notification = Notification::new(
            "valid-id".to_string(),
            NotificationType::Webhook,
            "not-a-url".to_string(),
            None,
        );

        assert!(matches!(
            notification.validate(),
            Err(NotificationValidationError::InvalidUrl)
        ));
    }

    #[test]
    fn test_signing_key_too_short() {
        let notification = Notification::new(
            "valid-id".to_string(),
            NotificationType::Webhook,
            "https://example.com/webhook".to_string(),
            Some(SecretString::new("short")),
        );

        assert!(matches!(
            notification.validate(),
            Err(NotificationValidationError::SigningKeyTooShort(_))
        ));
    }

    #[test]
    fn test_apply_update_success() {
        let original = Notification::new(
            "test-id".to_string(),
            NotificationType::Webhook,
            "https://example.com/webhook".to_string(),
            Some(SecretString::new(&"a".repeat(32))),
        );

        let update_request = NotificationUpdateRequest {
            r#type: None, // Keep existing type
            url: Some("https://updated.example.com/webhook".to_string()),
            signing_key: Some("b".repeat(32)), // Update signing key
        };

        let result = original.apply_update(&update_request);
        assert!(result.is_ok());

        let updated = result.unwrap();
        assert_eq!(updated.id, "test-id"); // ID should remain unchanged
        assert_eq!(updated.notification_type, NotificationType::Webhook); // Type unchanged
        assert_eq!(updated.url, "https://updated.example.com/webhook"); // URL updated
        assert!(updated.signing_key.is_some()); // Signing key updated
    }

    #[test]
    fn test_apply_update_remove_signing_key() {
        let original = Notification::new(
            "test-id".to_string(),
            NotificationType::Webhook,
            "https://example.com/webhook".to_string(),
            Some(SecretString::new(&"a".repeat(32))),
        );

        let update_request = NotificationUpdateRequest {
            r#type: None,
            url: None,
            signing_key: Some("".to_string()), // Empty string removes signing key
        };

        let result = original.apply_update(&update_request);
        assert!(result.is_ok());

        let updated = result.unwrap();
        assert_eq!(updated.id, "test-id");
        assert_eq!(updated.url, "https://example.com/webhook"); // URL unchanged
        assert!(updated.signing_key.is_none()); // Signing key removed
    }

    #[test]
    fn test_apply_update_validation_failure() {
        let original = Notification::new(
            "test-id".to_string(),
            NotificationType::Webhook,
            "https://example.com/webhook".to_string(),
            None,
        );

        let update_request = NotificationUpdateRequest {
            r#type: None,
            url: Some("not-a-valid-url".to_string()), // Invalid URL
            signing_key: None,
        };

        let result = original.apply_update(&update_request);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            NotificationValidationError::InvalidUrl
        ));
    }

    #[test]
    fn test_error_conversion_to_api_error() {
        let error = NotificationValidationError::InvalidUrl;
        let api_error: crate::models::ApiError = error.into();

        if let crate::models::ApiError::BadRequest(msg) = api_error {
            assert_eq!(msg, "Invalid URL format");
        } else {
            panic!("Expected BadRequest error");
        }
    }
}
