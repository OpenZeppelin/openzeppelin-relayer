//! Configuration file representation and parsing for notifications.
//!
//! This module handles the configuration file format for notifications, providing:
//!
//! - **Config Models**: Structures that match the configuration file schema
//! - **Validation**: Config-specific validation rules and constraints
//! - **Conversions**: Bidirectional mapping between config and domain models
//! - **Collections**: Container types for managing multiple notification configurations
//!
//! Used primarily during application startup to parse notification settings from config files.
use crate::{
    config::ConfigFileError,
    models::{
        notification::Notification, NotificationType, NotificationValidationError, PlainOrEnvValue,
        SecretString,
    },
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Configuration file representation of a notification
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct NotificationConfig {
    pub id: String,
    pub r#type: NotificationType,
    pub url: String,
    pub signing_key: Option<PlainOrEnvValue>,
}

impl TryFrom<NotificationConfig> for Notification {
    type Error = ConfigFileError;

    fn try_from(config: NotificationConfig) -> Result<Self, Self::Error> {
        let signing_key = config.get_signing_key()?;

        // Create core notification
        let notification = Notification::new(config.id, config.r#type, config.url, signing_key);

        // Validate using core validation logic
        notification.validate().map_err(|e| match e {
            NotificationValidationError::EmptyId => {
                ConfigFileError::MissingField("notification id".into())
            }
            NotificationValidationError::InvalidIdFormat => {
                ConfigFileError::InvalidFormat("Invalid notification ID format".into())
            }
            NotificationValidationError::EmptyUrl => {
                ConfigFileError::MissingField("Webhook URL is required".into())
            }
            NotificationValidationError::InvalidUrl => {
                ConfigFileError::InvalidFormat("Invalid Webhook URL".into())
            }
            NotificationValidationError::SigningKeyTooShort(min_len) => {
                ConfigFileError::InvalidFormat(format!(
                    "Signing key must be at least {} characters long",
                    min_len
                ))
            }
        })?;

        Ok(notification)
    }
}

impl NotificationConfig {
    /// Validates the notification configuration by converting to core model
    pub fn validate(&self) -> Result<(), ConfigFileError> {
        let _notification = Notification::try_from(self.clone())?;
        Ok(())
    }

    /// Converts to core notification model
    pub fn to_core_notification(&self) -> Result<Notification, ConfigFileError> {
        Notification::try_from(self.clone())
    }

    /// Gets the resolved signing key with config-specific error handling
    pub fn get_signing_key(&self) -> Result<Option<SecretString>, ConfigFileError> {
        match &self.signing_key {
            Some(signing_key) => match signing_key {
                PlainOrEnvValue::Env { value } => {
                    if value.is_empty() {
                        return Err(ConfigFileError::MissingField(
                            "Signing key environment variable name cannot be empty".into(),
                        ));
                    }

                    match std::env::var(value) {
                        Ok(key_value) => {
                            let secret = SecretString::new(&key_value);
                            Ok(Some(secret))
                        }
                        Err(e) => Err(ConfigFileError::MissingEnvVar(format!(
                            "Environment variable '{}' not found: {}",
                            value, e
                        ))),
                    }
                }
                PlainOrEnvValue::Plain { value } => {
                    let is_empty = value.as_str(|s| s.is_empty());
                    if is_empty {
                        return Err(ConfigFileError::InvalidFormat(
                            "Signing key value cannot be empty".into(),
                        ));
                    }
                    Ok(Some(value.clone()))
                }
            },
            None => Ok(None),
        }
    }
}

/// Collection of notification configurations
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct NotificationConfigs {
    pub notifications: Vec<NotificationConfig>,
}

impl NotificationConfigs {
    /// Creates a new collection of notification configurations
    pub fn new(notifications: Vec<NotificationConfig>) -> Self {
        Self { notifications }
    }

    /// Validates all notification configurations
    pub fn validate(&self) -> Result<(), ConfigFileError> {
        if self.notifications.is_empty() {
            return Ok(());
        }

        let mut ids = HashSet::new();
        for notification in &self.notifications {
            // Validate each notification using core validation
            notification.validate()?;

            // Check for duplicate IDs
            if !ids.insert(notification.id.clone()) {
                return Err(ConfigFileError::DuplicateId(notification.id.clone()));
            }
        }
        Ok(())
    }

    /// Converts all configurations to core notification models
    pub fn to_core_notifications(&self) -> Result<Vec<Notification>, ConfigFileError> {
        self.notifications
            .iter()
            .map(|config| Notification::try_from(config.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_notification_config_conversion() {
        let config = NotificationConfig {
            id: "test-webhook".to_string(),
            r#type: NotificationType::Webhook,
            url: "https://example.com/webhook".to_string(),
            signing_key: Some(PlainOrEnvValue::Plain {
                value: SecretString::new(&"a".repeat(32)),
            }),
        };

        let result = Notification::try_from(config);
        assert!(result.is_ok());

        let notification = result.unwrap();
        assert_eq!(notification.id, "test-webhook");
        assert_eq!(notification.notification_type, NotificationType::Webhook);
        assert_eq!(notification.url, "https://example.com/webhook");
        assert!(notification.signing_key.is_some());
    }

    #[test]
    fn test_invalid_notification_config_conversion() {
        let config = NotificationConfig {
            id: "invalid@id".to_string(), // Invalid ID format
            r#type: NotificationType::Webhook,
            url: "https://example.com/webhook".to_string(),
            signing_key: None,
        };

        let result = Notification::try_from(config);
        assert!(result.is_err());

        if let Err(ConfigFileError::InvalidFormat(msg)) = result {
            assert!(msg.contains("Invalid notification ID format"));
        } else {
            panic!("Expected InvalidFormat error");
        }
    }

    #[test]
    fn test_to_core_notification() {
        let config = NotificationConfig {
            id: "test-webhook".to_string(),
            r#type: NotificationType::Webhook,
            url: "https://example.com/webhook".to_string(),
            signing_key: Some(PlainOrEnvValue::Plain {
                value: SecretString::new(&"a".repeat(32)),
            }),
        };

        let core = config.to_core_notification().unwrap();
        assert_eq!(core.id, "test-webhook");
        assert_eq!(core.notification_type, NotificationType::Webhook);
        assert_eq!(core.url, "https://example.com/webhook");
        assert!(core.signing_key.is_some());
    }

    #[test]
    fn test_notification_configs_validation() {
        let configs = NotificationConfigs::new(vec![
            NotificationConfig {
                id: "webhook1".to_string(),
                r#type: NotificationType::Webhook,
                url: "https://example.com/webhook1".to_string(),
                signing_key: None,
            },
            NotificationConfig {
                id: "webhook2".to_string(),
                r#type: NotificationType::Webhook,
                url: "https://example.com/webhook2".to_string(),
                signing_key: None,
            },
        ]);

        assert!(configs.validate().is_ok());
    }

    #[test]
    fn test_duplicate_ids() {
        let configs = NotificationConfigs::new(vec![
            NotificationConfig {
                id: "webhook1".to_string(),
                r#type: NotificationType::Webhook,
                url: "https://example.com/webhook1".to_string(),
                signing_key: None,
            },
            NotificationConfig {
                id: "webhook1".to_string(), // Duplicate ID
                r#type: NotificationType::Webhook,
                url: "https://example.com/webhook2".to_string(),
                signing_key: None,
            },
        ]);

        assert!(matches!(
            configs.validate(),
            Err(ConfigFileError::DuplicateId(_))
        ));
    }

    #[test]
    fn test_config_with_short_signing_key() {
        let config = NotificationConfig {
            id: "test-webhook".to_string(),
            r#type: NotificationType::Webhook,
            url: "https://example.com/webhook".to_string(),
            signing_key: Some(PlainOrEnvValue::Plain {
                value: SecretString::new("short"), // Too short
            }),
        };

        let result = Notification::try_from(config);
        assert!(result.is_err());

        if let Err(ConfigFileError::InvalidFormat(msg)) = result {
            assert!(msg.contains("Signing key must be at least"));
        } else {
            panic!("Expected InvalidFormat error for short key");
        }
    }

    // Additional tests for JSON deserialization and environment handling
    #[test]
    fn test_valid_webhook_notification_json() {
        use serde_json::json;

        let config = json!({
            "id": "notification-test",
            "type": "webhook",
            "url": "https://api.example.com/notifications"
        });

        let notification: NotificationConfig = serde_json::from_value(config).unwrap();
        assert!(notification.validate().is_ok());
        assert_eq!(notification.id, "notification-test");
        assert_eq!(notification.r#type, NotificationType::Webhook);
    }

    #[test]
    fn test_invalid_webhook_url_json() {
        use serde_json::json;

        let config = json!({
            "id": "notification-test",
            "type": "webhook",
            "url": "invalid-url"
        });

        let notification: NotificationConfig = serde_json::from_value(config).unwrap();
        assert!(notification.validate().is_err());
    }

    #[test]
    fn test_webhook_notification_with_signing_key_json() {
        use serde_json::json;

        let config = json!({
            "id": "notification-test",
            "type": "webhook",
            "url": "https://api.example.com/notifications",
            "signing_key": {
                "type": "plain",
                "value": "a".repeat(32)
            }
        });

        let notification: NotificationConfig = serde_json::from_value(config).unwrap();
        assert!(notification.validate().is_ok());
        assert!(notification.get_signing_key().unwrap().is_some());
    }

    #[test]
    fn test_webhook_notification_with_env_signing_key_json() {
        use serde_json::json;
        use std::sync::Mutex;

        static ENV_MUTEX: Mutex<()> = Mutex::new(());
        let _lock = ENV_MUTEX.lock().unwrap();

        // Set environment variable
        std::env::set_var("TEST_SIGNING_KEY", "a".repeat(32));

        let config = json!({
            "id": "notification-test",
            "type": "webhook",
            "url": "https://api.example.com/notifications",
            "signing_key": {
                "type": "env",
                "value": "TEST_SIGNING_KEY"
            }
        });

        let notification: NotificationConfig = serde_json::from_value(config).unwrap();
        assert!(notification.validate().is_ok());
        assert!(notification.get_signing_key().unwrap().is_some());

        // Clean up
        std::env::remove_var("TEST_SIGNING_KEY");
    }
}
