//! This module handles outgoing HTTP responses for notification operations, providing:
//!
//! - **Response Models**: Structures for representing notification data in API responses
//! - **Security Handling**: Obfuscation of sensitive data (e.g., signing keys)
//! - **Serialization**: Conversion to JSON format for HTTP responses
//!
//! Serves as the output format for notification data to external clients, ensuring
//! all sensitive information is properly masked and formatted correctly.
use crate::models::{NotificationRepoModel, NotificationType};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Response structure for notification API endpoints
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, ToSchema)]
pub struct NotificationResponse {
    pub id: String,
    pub r#type: NotificationType,
    pub url: String,
    /// Signing key is hidden in responses for security
    pub has_signing_key: bool,
}

impl From<NotificationRepoModel> for NotificationResponse {
    fn from(model: NotificationRepoModel) -> Self {
        Self {
            id: model.id,
            r#type: model.notification_type,
            url: model.url,
            has_signing_key: model.signing_key.is_some(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::SecretString;

    #[test]
    fn test_from_notification_repo_model() {
        let model = NotificationRepoModel {
            id: "test-id".to_string(),
            notification_type: NotificationType::Webhook,
            url: "https://example.com/webhook".to_string(),
            signing_key: Some(SecretString::new("secret-key")),
        };

        let response = NotificationResponse::from(model);

        assert_eq!(response.id, "test-id");
        assert_eq!(response.r#type, NotificationType::Webhook);
        assert_eq!(response.url, "https://example.com/webhook");
        assert!(response.has_signing_key);
    }

    #[test]
    fn test_from_notification_repo_model_without_signing_key() {
        let model = NotificationRepoModel {
            id: "test-id".to_string(),
            notification_type: NotificationType::Webhook,
            url: "https://example.com/webhook".to_string(),
            signing_key: None,
        };

        let response = NotificationResponse::from(model);

        assert_eq!(response.id, "test-id");
        assert_eq!(response.r#type, NotificationType::Webhook);
        assert_eq!(response.url, "https://example.com/webhook");
        assert!(!response.has_signing_key);
    }
}
