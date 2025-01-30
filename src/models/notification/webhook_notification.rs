use crate::{
    jobs::NotificationSend,
    models::{TransactionRepoModel, TransactionResponse},
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct WebhookNotification {
    pub id: String,
    pub event: String,
    pub payload: WebhookPayload,
    pub timestamp: String,
}

impl WebhookNotification {
    pub fn new(event: String, payload: WebhookPayload) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            event,
            payload,
            timestamp: Utc::now().to_rfc3339(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct TransactionFailurePayload {
    pub transaction: TransactionResponse,
    pub failure_reason: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum WebhookPayload {
    Transaction(TransactionResponse),
    TransactionFailure(TransactionFailurePayload),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WebhookResponse {
    pub status: String,
    pub message: Option<String>,
}

pub fn produce_transaction_update_notification_payload(
    notification_id: &str,
    transaction: &TransactionRepoModel,
) -> NotificationSend {
    let tx_payload: TransactionResponse = transaction.clone().into();
    NotificationSend::new(
        notification_id.to_string(),
        WebhookNotification::new(
            "transaction_update".to_string(),
            WebhookPayload::Transaction(tx_payload),
        ),
    )
}
