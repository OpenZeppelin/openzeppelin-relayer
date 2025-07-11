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
    SolanaTokenSwapRequest,
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

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct SolanaTokenSwapRequest {
    pub relayer_id: String,
}

impl SolanaTokenSwapRequest {
    pub fn new(relayer_id: String) -> Self {
        Self { relayer_id }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::str::FromStr;

    use crate::models::{
        evm::Speed, EvmTransactionDataSignature, EvmTransactionResponse, TransactionResponse,
        TransactionStatus, WebhookNotification, WebhookPayload, U256,
    };

    use super::*;

    #[test]
    fn test_job_creation() {
        let job_data = TransactionRequest::new("tx123", "relayer-1");
        let job = Job::new(JobType::TransactionRequest, job_data.clone());

        assert_eq!(job.job_type.to_string(), "TransactionRequest");
        assert_eq!(job.version, "1.0");
        assert_eq!(job.data.transaction_id, "tx123");
        assert_eq!(job.data.relayer_id, "relayer-1");
        assert!(job.data.metadata.is_none());
    }

    #[test]
    fn test_transaction_request_with_metadata() {
        let mut metadata = HashMap::new();
        metadata.insert("chain_id".to_string(), "1".to_string());
        metadata.insert("gas_price".to_string(), "20000000000".to_string());

        let tx_request =
            TransactionRequest::new("tx123", "relayer-1").with_metadata(metadata.clone());

        assert_eq!(tx_request.transaction_id, "tx123");
        assert_eq!(tx_request.relayer_id, "relayer-1");
        assert!(tx_request.metadata.is_some());
        assert_eq!(tx_request.metadata.unwrap(), metadata);
    }

    #[test]
    fn test_transaction_send_methods() {
        // Test submit
        let tx_submit = TransactionSend::submit("tx123", "relayer-1");
        assert_eq!(tx_submit.transaction_id, "tx123");
        assert_eq!(tx_submit.relayer_id, "relayer-1");
        matches!(tx_submit.command, TransactionCommand::Submit);

        // Test cancel
        let tx_cancel = TransactionSend::cancel("tx123", "relayer-1", "user requested");
        matches!(tx_cancel.command, TransactionCommand::Cancel { reason } if reason == "user requested");

        // Test resubmit
        let tx_resubmit = TransactionSend::resubmit("tx123", "relayer-1");
        matches!(tx_resubmit.command, TransactionCommand::Resubmit);

        // Test resend
        let tx_resend = TransactionSend::resend("tx123", "relayer-1");
        matches!(tx_resend.command, TransactionCommand::Resend);

        // Test with_metadata
        let mut metadata = HashMap::new();
        metadata.insert("nonce".to_string(), "5".to_string());

        let tx_with_metadata =
            TransactionSend::submit("tx123", "relayer-1").with_metadata(metadata.clone());

        assert!(tx_with_metadata.metadata.is_some());
        assert_eq!(tx_with_metadata.metadata.unwrap(), metadata);
    }

    #[test]
    fn test_transaction_status_check() {
        let tx_status = TransactionStatusCheck::new("tx123", "relayer-1");
        assert_eq!(tx_status.transaction_id, "tx123");
        assert_eq!(tx_status.relayer_id, "relayer-1");
        assert!(tx_status.metadata.is_none());

        let mut metadata = HashMap::new();
        metadata.insert("retries".to_string(), "3".to_string());

        let tx_status_with_metadata =
            TransactionStatusCheck::new("tx123", "relayer-1").with_metadata(metadata.clone());

        assert!(tx_status_with_metadata.metadata.is_some());
        assert_eq!(tx_status_with_metadata.metadata.unwrap(), metadata);
    }

    #[test]
    fn test_job_serialization() {
        let tx_request = TransactionRequest::new("tx123", "relayer-1");
        let job = Job::new(JobType::TransactionRequest, tx_request);

        let serialized = serde_json::to_string(&job).unwrap();
        let deserialized: Job<TransactionRequest> = serde_json::from_str(&serialized).unwrap();

        assert_eq!(deserialized.job_type.to_string(), "TransactionRequest");
        assert_eq!(deserialized.data.transaction_id, "tx123");
        assert_eq!(deserialized.data.relayer_id, "relayer-1");
    }

    #[test]
    fn test_notification_send_serialization() {
        let payload = WebhookPayload::Transaction(TransactionResponse::Evm(Box::new(
            EvmTransactionResponse {
                id: "tx123".to_string(),
                hash: Some("0x123".to_string()),
                status: TransactionStatus::Confirmed,
                status_reason: None,
                created_at: "2025-01-27T15:31:10.777083+00:00".to_string(),
                sent_at: Some("2025-01-27T15:31:10.777083+00:00".to_string()),
                confirmed_at: Some("2025-01-27T15:31:10.777083+00:00".to_string()),
                gas_price: Some(1000000000),
                gas_limit: Some(21000),
                nonce: Some(1),
                value: U256::from_str("1000000000000000000").unwrap(),
                from: "0xabc".to_string(),
                to: Some("0xdef".to_string()),
                relayer_id: "relayer-1".to_string(),
                data: Some("0x123".to_string()),
                max_fee_per_gas: Some(1000000000),
                max_priority_fee_per_gas: Some(1000000000),
                signature: Some(EvmTransactionDataSignature {
                    r: "0x123".to_string(),
                    s: "0x123".to_string(),
                    v: 1,
                    sig: "0x123".to_string(),
                }),
                speed: Some(Speed::Fast),
            },
        )));

        let notification = WebhookNotification::new("transaction".to_string(), payload);
        let notification_send =
            NotificationSend::new("notification-test".to_string(), notification);

        let serialized = serde_json::to_string(&notification_send).unwrap();

        match serde_json::from_str::<NotificationSend>(&serialized) {
            Ok(deserialized) => {
                assert_eq!(notification_send, deserialized);
            }
            Err(e) => {
                panic!("Deserialization error: {}", e);
            }
        }
    }

    #[test]
    fn test_notification_send_serialization_none_values() {
        let payload = WebhookPayload::Transaction(TransactionResponse::Evm(Box::new(
            EvmTransactionResponse {
                id: "tx123".to_string(),
                hash: None,
                status: TransactionStatus::Confirmed,
                status_reason: None,
                created_at: "2025-01-27T15:31:10.777083+00:00".to_string(),
                sent_at: None,
                confirmed_at: None,
                gas_price: None,
                gas_limit: Some(21000),
                nonce: None,
                value: U256::from_str("1000000000000000000").unwrap(),
                from: "0xabc".to_string(),
                to: None,
                relayer_id: "relayer-1".to_string(),
                data: None,
                max_fee_per_gas: None,
                max_priority_fee_per_gas: None,
                signature: None,
                speed: None,
            },
        )));

        let notification = WebhookNotification::new("transaction".to_string(), payload);
        let notification_send =
            NotificationSend::new("notification-test".to_string(), notification);

        let serialized = serde_json::to_string(&notification_send).unwrap();

        match serde_json::from_str::<NotificationSend>(&serialized) {
            Ok(deserialized) => {
                assert_eq!(notification_send, deserialized);
            }
            Err(e) => {
                panic!("Deserialization error: {}", e);
            }
        }
    }
}
