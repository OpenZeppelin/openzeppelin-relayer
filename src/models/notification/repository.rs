use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::models::SecretString;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum NotificationType {
    Webhook,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationRepoModel {
    pub id: String,
    pub notification_type: NotificationType,
    pub url: String,
    pub signing_key: Option<SecretString>,
}
