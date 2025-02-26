//! This module defines the configuration structures and validation logic for notifications.
//!
//! It includes:
//! - `NotificationFileConfigType`: An enum representing the type of notification configuration.
//! - `SigningKeyConfig`: An enum for specifying signing key configurations, either from an
//!   environment variable or a plain value.
//! - `NotificationFileConfig`: A struct representing a single notification configuration, with
//!   methods for validation and signing key retrieval.
//! - `NotificationsFileConfig`: A struct for managing a collection of notification configurations,
//!   with validation to ensure uniqueness and completeness.
use super::ConfigFileError;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum NotificationFileConfigType {
    Webhook,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SigningKeyConfig {
    Env { name: String },
    Plain { value: String },
}

/// Represents the type of notification configuration.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct NotificationFileConfig {
    pub id: String,
    pub r#type: NotificationFileConfigType,
    pub url: String,
    pub signing_key: Option<SigningKeyConfig>,
}

impl NotificationFileConfig {
    fn validate_signing_key(&self) -> Result<(), ConfigFileError> {
        match &self.signing_key {
            Some(signing_key) => match signing_key {
                SigningKeyConfig::Env { name } => {
                    if name.is_empty() {
                        return Err(ConfigFileError::MissingField(
                            "Signing key environment variable name cannot be empty".into(),
                        ));
                    }
                    if std::env::var(name).is_err() {
                        return Err(ConfigFileError::MissingEnvVar(format!(
                            "Environment variable {} not found",
                            name
                        )));
                    }
                }
                SigningKeyConfig::Plain { value } => {
                    if value.is_empty() {
                        return Err(ConfigFileError::InvalidFormat(
                            "Signing key value cannot be empty".into(),
                        ));
                    }
                }
            },
            None => return Ok(()),
        }

        Ok(())
    }

    pub fn get_signing_key(&self) -> Option<String> {
        match &self.signing_key {
            Some(signing_key) => match signing_key {
                SigningKeyConfig::Env { name } => {
                    let signing_key = std::env::var(name);
                    signing_key.ok()
                }
                SigningKeyConfig::Plain { value } => Some(value.clone()),
            },
            None => None,
        }
    }

    pub fn validate(&self) -> Result<(), ConfigFileError> {
        if self.id.is_empty() {
            return Err(ConfigFileError::MissingField("notification id".into()));
        }

        match &self.r#type {
            NotificationFileConfigType::Webhook => {
                if self.url.is_empty() {
                    return Err(ConfigFileError::MissingField(
                        "Webhook URL is required".into(),
                    ));
                }
                Url::parse(&self.url)
                    .map_err(|_| ConfigFileError::InvalidFormat("Invalid Webhook URL".into()))?;
            }
        }

        self.validate_signing_key()?;

        Ok(())
    }
}

/// Manages a collection of notification configurations.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct NotificationsFileConfig {
    pub notifications: Vec<NotificationFileConfig>,
}

impl NotificationsFileConfig {
    /// Creates a new `NotificationsFileConfig` with the given notifications.
    pub fn new(notifications: Vec<NotificationFileConfig>) -> Self {
        Self { notifications }
    }

    /// Validates the collection of notification configurations.
    ///
    /// Ensures that each notification is valid and that there are no duplicate IDs.
    pub fn validate(&self) -> Result<(), ConfigFileError> {
        if self.notifications.is_empty() {
            return Err(ConfigFileError::MissingField("notifications".into()));
        }

        let mut ids = HashSet::new();
        for notification in &self.notifications {
            notification.validate()?;
            if !ids.insert(notification.id.clone()) {
                return Err(ConfigFileError::DuplicateId(notification.id.clone()));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_valid_webhook_notification() {
        let config = json!({
            "id": "notification-test",
            "type": "webhook",
            "url": "https://api.example.com/notifications"
        });

        let notification: NotificationFileConfig = serde_json::from_value(config).unwrap();
        assert!(notification.validate().is_ok());
        assert_eq!(notification.id, "notification-test");
        assert_eq!(notification.r#type, NotificationFileConfigType::Webhook);
    }

    #[test]
    #[should_panic(expected = "missing field `url`")]
    fn test_missing_webhook_url() {
        let config = json!({
            "id": "notification-test",
            "type": "webhook"
        });

        let _notification: NotificationFileConfig = serde_json::from_value(config).unwrap();
    }

    #[test]
    fn test_invalid_webhook_url() {
        let config = json!({
            "id": "notification-test",
            "type": "webhook",
            "url": "invalid-url"
        });

        let notification: NotificationFileConfig = serde_json::from_value(config).unwrap();

        assert!(matches!(
            notification.validate(),
            Err(ConfigFileError::InvalidFormat(_))
        ));
    }

    #[test]
    fn test_duplicate_notification_ids() {
        let config = json!({
            "notifications": [
                {
                    "id": "notification-test",
                    "type": "webhook",
                    "url": "https://api.example.com/notifications"
                },
                {
                    "id": "notification-test",
                    "type": "webhook",
                    "url": "https://api.example.com/notifications"
                }
            ]
        });

        let notifications_config: NotificationsFileConfig = serde_json::from_value(config).unwrap();
        assert!(matches!(
            notifications_config.validate(),
            Err(ConfigFileError::DuplicateId(_))
        ));
    }

    #[test]
    fn test_empty_notification_id() {
        let config = json!({
            "notifications": [
                {
                    "id": "",
                    "type": "webhook",
                    "url": "https://api.example.com/notifications"
                }
            ]
        });

        let notifications_config: NotificationsFileConfig = serde_json::from_value(config).unwrap();
        assert!(matches!(
            notifications_config.validate(),
            Err(ConfigFileError::MissingField(_))
        ));
    }
}
