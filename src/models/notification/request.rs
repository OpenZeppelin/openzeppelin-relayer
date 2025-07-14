use crate::models::{NotificationRepoModel, NotificationType, SecretString};
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

impl NotificationUpdateRequest {
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

#[cfg(test)]
mod tests {
    use super::*;

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
