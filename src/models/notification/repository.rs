use serde::{Deserialize, Serialize};

use crate::models::SecretString;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum NotificationType {
    Webhook,
}

#[derive(Debug, Clone, Serialize)]
pub struct NotificationRepoModel {
    pub id: String,
    pub notification_type: NotificationType,
    pub url: String,
    pub signing_key: Option<SecretString>,
}
