//! Job processing module for handling asynchronous tasks.
//!
//! Provides generic job structure for different types of operations:
//! - Transaction processing
//! - Status monitoring
//! - Notifications
use crate::models::{NetworkType, WebhookNotification};
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

impl<T> Job<T> {
    pub fn new(job_type: JobType, data: T) -> Self {
        Self {
            message_id: Uuid::new_v4().to_string(),
            version: "1.0".to_string(),
            timestamp: Utc::now().timestamp().to_string(),
            job_type,
            data,
            request_id: None,
        }
    }
    pub fn with_request_id(mut self, id: Option<String>) -> Self {
        self.request_id = id;
        self
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
    RelayerHealthCheck,
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
    /// Network type for this transaction status check.
    /// Optional for backward compatibility with older queued messages.
    #[serde(default)]
    pub network_type: Option<NetworkType>,
    pub metadata: Option<HashMap<String, String>>,
}

impl TransactionStatusCheck {
    pub fn new(
        transaction_id: impl Into<String>,
        relayer_id: impl Into<String>,
        network_type: NetworkType,
    ) -> Self {
        Self {
            transaction_id: transaction_id.into(),
            relayer_id: relayer_id.into(),
            network_type: Some(network_type),
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

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct RelayerHealthCheck {
    pub relayer_id: String,
    pub retry_count: u32,
}

impl RelayerHealthCheck {
    pub fn new(relayer_id: String) -> Self {
        Self {
            relayer_id,
            retry_count: 0,
        }
    }

    pub fn with_retry_count(relayer_id: String, retry_count: u32) -> Self {
        Self {
            relayer_id,
            retry_count,
        }
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
        let tx_status = TransactionStatusCheck::new("tx123", "relayer-1", NetworkType::Evm);
        assert_eq!(tx_status.transaction_id, "tx123");
        assert_eq!(tx_status.relayer_id, "relayer-1");
        assert_eq!(tx_status.network_type, Some(NetworkType::Evm));
        assert!(tx_status.metadata.is_none());

        let mut metadata = HashMap::new();
        metadata.insert("retries".to_string(), "3".to_string());

        let tx_status_with_metadata =
            TransactionStatusCheck::new("tx123", "relayer-1", NetworkType::Stellar)
                .with_metadata(metadata.clone());

        assert!(tx_status_with_metadata.metadata.is_some());
        assert_eq!(tx_status_with_metadata.metadata.unwrap(), metadata);
    }

    #[test]
    fn test_transaction_status_check_backward_compatibility() {
        // Simulate an old message without network_type field
        let old_json = r#"{
            "transaction_id": "tx456",
            "relayer_id": "relayer-2",
            "metadata": null
        }"#;

        // Should deserialize successfully with network_type defaulting to None
        let deserialized: TransactionStatusCheck = serde_json::from_str(old_json).unwrap();
        assert_eq!(deserialized.transaction_id, "tx456");
        assert_eq!(deserialized.relayer_id, "relayer-2");
        assert_eq!(deserialized.network_type, None);
        assert!(deserialized.metadata.is_none());

        // New messages should include network_type
        let new_status = TransactionStatusCheck::new("tx789", "relayer-3", NetworkType::Solana);
        assert_eq!(new_status.network_type, Some(NetworkType::Solana));
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

    #[test]
    fn test_relayer_health_check_new() {
        let health_check = RelayerHealthCheck::new("relayer-1".to_string());

        assert_eq!(health_check.relayer_id, "relayer-1");
        assert_eq!(health_check.retry_count, 0);
    }

    #[test]
    fn test_relayer_health_check_with_retry_count() {
        let health_check = RelayerHealthCheck::with_retry_count("relayer-1".to_string(), 5);

        assert_eq!(health_check.relayer_id, "relayer-1");
        assert_eq!(health_check.retry_count, 5);
    }

    #[test]
    fn test_relayer_health_check_correct_field_values() {
        // Test with zero retry count
        let health_check_zero = RelayerHealthCheck::new("relayer-test-123".to_string());
        assert_eq!(health_check_zero.relayer_id, "relayer-test-123");
        assert_eq!(health_check_zero.retry_count, 0);

        // Test with specific retry count
        let health_check_custom =
            RelayerHealthCheck::with_retry_count("relayer-abc".to_string(), 10);
        assert_eq!(health_check_custom.relayer_id, "relayer-abc");
        assert_eq!(health_check_custom.retry_count, 10);

        // Test with large retry count
        let health_check_large =
            RelayerHealthCheck::with_retry_count("relayer-xyz".to_string(), 999);
        assert_eq!(health_check_large.relayer_id, "relayer-xyz");
        assert_eq!(health_check_large.retry_count, 999);
    }

    #[test]
    fn test_relayer_health_check_job_serialization() {
        let health_check = RelayerHealthCheck::new("relayer-1".to_string());
        let job = Job::new(JobType::RelayerHealthCheck, health_check);

        let serialized = serde_json::to_string(&job).unwrap();
        let deserialized: Job<RelayerHealthCheck> = serde_json::from_str(&serialized).unwrap();

        assert_eq!(deserialized.job_type.to_string(), "RelayerHealthCheck");
        assert_eq!(deserialized.data.relayer_id, "relayer-1");
        assert_eq!(deserialized.data.retry_count, 0);
    }

    #[test]
    fn test_relayer_health_check_job_serialization_with_retry_count() {
        let health_check = RelayerHealthCheck::with_retry_count("relayer-2".to_string(), 3);
        let job = Job::new(JobType::RelayerHealthCheck, health_check.clone());

        let serialized = serde_json::to_string(&job).unwrap();
        let deserialized: Job<RelayerHealthCheck> = serde_json::from_str(&serialized).unwrap();

        assert_eq!(deserialized.job_type.to_string(), "RelayerHealthCheck");
        assert_eq!(deserialized.data.relayer_id, health_check.relayer_id);
        assert_eq!(deserialized.data.retry_count, health_check.retry_count);
        assert_eq!(deserialized.data, health_check);
    }

    #[test]
    fn test_relayer_health_check_equality_after_deserialization() {
        let original_health_check =
            RelayerHealthCheck::with_retry_count("relayer-test".to_string(), 7);
        let job = Job::new(JobType::RelayerHealthCheck, original_health_check.clone());

        let serialized = serde_json::to_string(&job).unwrap();
        let deserialized: Job<RelayerHealthCheck> = serde_json::from_str(&serialized).unwrap();

        // Assert job type string
        assert_eq!(deserialized.job_type.to_string(), "RelayerHealthCheck");

        // Assert data equality
        assert_eq!(deserialized.data, original_health_check);
        assert_eq!(
            deserialized.data.relayer_id,
            original_health_check.relayer_id
        );
        assert_eq!(
            deserialized.data.retry_count,
            original_health_check.retry_count
        );
    }
}
