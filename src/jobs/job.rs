//! Job processing module for handling asynchronous tasks.
//!
//! Provides generic job structure for different types of operations:
//! - Transaction processing
//! - Status monitoring
//! - Notifications
use crate::models::WebhookNotification;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use strum::Display;
use uuid::Uuid;

// Common message structure
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Job<T> {
    pub message_id: String,
    pub version: String,
    pub timestamp: String,
    pub job_type: JobType,
    pub data: T,
}

impl<T> Job<T> {
    pub fn new(job_type: JobType, data: T) -> Self {
        Self {
            message_id: Uuid::new_v4().to_string(),
            version: "1.0".to_string(),
            timestamp: Utc::now().timestamp().to_string(),
            job_type,
            data,
        }
    }
}

// Enum to represent different message types
#[derive(Debug, Serialize, Deserialize, Display, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JobType {
    TransactionRequest,
    TransactionSend,
    TransactionStatusCheck,
    NotificationSend,
}

// Example message data for transaction request
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TransactionRequest {
    pub transaction_id: String,
    pub relayer_id: String,
    pub metadata: Option<HashMap<String, String>>,
}

impl TransactionRequest {
    pub fn new(transaction_id: impl Into<String>, relayer_id: impl Into<String>) -> Self {
        Self {
            transaction_id: transaction_id.into(),
            relayer_id: relayer_id.into(),
            metadata: None,
        }
    }

    pub fn with_metadata(mut self, metadata: HashMap<String, String>) -> Self {
        self.metadata = Some(metadata);
        self
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum TransactionCommand {
    Submit,
    Cancel { reason: String },
    Resubmit,
    Resend,
}

// Example message data for order creation
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TransactionSend {
    pub transaction_id: String,
    pub relayer_id: String,
    pub command: TransactionCommand,
    pub metadata: Option<HashMap<String, String>>,
}

impl TransactionSend {
    pub fn submit(transaction_id: impl Into<String>, relayer_id: impl Into<String>) -> Self {
        Self {
            transaction_id: transaction_id.into(),
            relayer_id: relayer_id.into(),
            command: TransactionCommand::Submit,
            metadata: None,
        }
    }

    pub fn cancel(
        transaction_id: impl Into<String>,
        relayer_id: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            transaction_id: transaction_id.into(),
            relayer_id: relayer_id.into(),
            command: TransactionCommand::Cancel {
                reason: reason.into(),
            },
            metadata: None,
        }
    }

    pub fn resubmit(transaction_id: impl Into<String>, relayer_id: impl Into<String>) -> Self {
        Self {
            transaction_id: transaction_id.into(),
            relayer_id: relayer_id.into(),
            command: TransactionCommand::Resubmit,
            metadata: None,
        }
    }

    pub fn resend(transaction_id: impl Into<String>, relayer_id: impl Into<String>) -> Self {
        Self {
            transaction_id: transaction_id.into(),
            relayer_id: relayer_id.into(),
            command: TransactionCommand::Resend,
            metadata: None,
        }
    }

    pub fn with_metadata(mut self, metadata: HashMap<String, String>) -> Self {
        self.metadata = Some(metadata);
        self
    }
}

// Struct for individual order item
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TransactionStatusCheck {
    pub transaction_id: String,
    pub relayer_id: String,
    pub metadata: Option<HashMap<String, String>>,
}

impl TransactionStatusCheck {
    pub fn new(transaction_id: impl Into<String>, relayer_id: impl Into<String>) -> Self {
        Self {
            transaction_id: transaction_id.into(),
            relayer_id: relayer_id.into(),
            metadata: None,
        }
    }

    pub fn with_metadata(mut self, metadata: HashMap<String, String>) -> Self {
        self.metadata = Some(metadata);
        self
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct NotificationSend {
    pub notification_id: String,
    pub notification: WebhookNotification,
}

impl NotificationSend {
    pub fn new(notification_id: String, notification: WebhookNotification) -> Self {
        Self {
            notification_id,
            notification,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::models::{
        EvmTransactionResponse, TransactionResponse, TransactionStatus, WebhookPayload,
    };

    use super::*;

    #[test]
    fn test_notification_send_serialization() {
        let payload =
            WebhookPayload::Transaction(TransactionResponse::Evm(EvmTransactionResponse {
                id: "tx123".to_string(),
                hash: Some("0x123".to_string()),
                status: TransactionStatus::Confirmed,
                created_at: "2025-01-27T15:31:10.777083+00:00".to_string(),
                sent_at: "2025-01-27T15:31:10.777083+00:00".to_string(),
                confirmed_at: "2025-01-27T15:31:10.777083+00:00".to_string(),
                gas_price: 1000000000,
                gas_limit: 21000,
                nonce: 1,
                value: 1000000000000000000,
                from: "0xabc".to_string(),
                to: "0xdef".to_string(),
                relayer_id: "relayer-1".to_string(),
            }));

        let notification = WebhookNotification::new("transaction".to_string(), payload);
        let notification_send =
            NotificationSend::new("notification-test".to_string(), notification);

        let serialized = serde_json::to_string(&notification_send).unwrap();
        match serde_json::from_str::<NotificationSend>(&serialized) {
            Ok(deserialized) => {
                assert_eq!(notification_send, deserialized);
            }
            Err(e) => {
                eprintln!("Failed to deserialize NotificationSend: {}", e);
                panic!("Deserialization error: {}", e);
            }
        }
    }
}
