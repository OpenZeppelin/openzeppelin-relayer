//! Repository layer models and data persistence for notifications.
//!
//! This module provides the data layer representation of notifications, including:
//!
//! - **Repository Models**: Data structures optimized for storage and retrieval
//! - **Data Conversions**: Mapping between domain objects and repository representations
//! - **Persistence Logic**: Storage-specific validation and constraints
//!
//! Acts as the bridge between the domain layer and actual data storage implementations
//! (in-memory, Redis, etc.), ensuring consistent data representation across repositories.

use crate::models::{
    notification::Notification, NotificationType, NotificationValidationError, SecretString,
};
use crate::utils::{deserialize_option_secret_string, serialize_option_secret_string};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
pub struct NotificationRepoModel {
    pub id: String,
    pub notification_type: NotificationType,
    pub url: String,
    #[serde(
        serialize_with = "serialize_option_secret_string",
        deserialize_with = "deserialize_option_secret_string"
    )]
    pub signing_key: Option<SecretString>,
}

impl From<Notification> for NotificationRepoModel {
    fn from(notification: Notification) -> Self {
        Self {
            id: notification.id,
            notification_type: notification.notification_type,
            url: notification.url,
            signing_key: notification.signing_key,
        }
    }
}

impl From<NotificationRepoModel> for Notification {
    fn from(repo_model: NotificationRepoModel) -> Self {
        Self {
            id: repo_model.id,
            notification_type: repo_model.notification_type,
            url: repo_model.url,
            signing_key: repo_model.signing_key,
        }
    }
}

impl NotificationRepoModel {
    /// Validates the repository model using core validation logic
    pub fn validate(&self) -> Result<(), NotificationValidationError> {
        let core_notification = Notification::from(self.clone());
        core_notification.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::SecretString;

    #[test]
    fn test_from_core_notification() {
        let core = Notification::new(
            "test-id".to_string(),
            NotificationType::Webhook,
            "https://example.com/webhook".to_string(),
            Some(SecretString::new("test-key")),
        );

        let repo_model = NotificationRepoModel::from(core);
        assert_eq!(repo_model.id, "test-id");
        assert_eq!(repo_model.notification_type, NotificationType::Webhook);
        assert_eq!(repo_model.url, "https://example.com/webhook");
        assert!(repo_model.signing_key.is_some());
    }

    #[test]
    fn test_to_core_notification() {
        let repo_model = NotificationRepoModel {
            id: "test-id".to_string(),
            notification_type: NotificationType::Webhook,
            url: "https://example.com/webhook".to_string(),
            signing_key: Some(SecretString::new("test-key")),
        };

        let core = Notification::from(repo_model);
        assert_eq!(core.id, "test-id");
        assert_eq!(core.notification_type, NotificationType::Webhook);
        assert_eq!(core.url, "https://example.com/webhook");
        assert!(core.signing_key.is_some());
    }

    #[test]
    fn test_validation() {
        let repo_model = NotificationRepoModel {
            id: "test-id".to_string(),
            notification_type: NotificationType::Webhook,
            url: "https://example.com/webhook".to_string(),
            signing_key: Some(SecretString::new(&"a".repeat(32))),
        };

        assert!(repo_model.validate().is_ok());
    }
}
