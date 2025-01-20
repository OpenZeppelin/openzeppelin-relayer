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
    TransactionSubmit,
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

// Example message data for order creation
#[derive(Debug, Serialize, Deserialize)]
pub struct TransactionSubmit {
    pub transaction_id: String,
    pub relayer_id: String,
    pub metadata: Option<HashMap<String, String>>,
}

impl TransactionSubmit {
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

// Struct for individual order item
#[derive(Debug, Serialize, Deserialize)]
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

// Example message data for notifications
#[derive(Debug, Serialize, Deserialize)]
pub struct NotificationSend {
    pub notification_id: String,
    pub message: String,
}

impl NotificationSend {
    pub fn new(notification_id: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            notification_id: notification_id.into(),
            message: message.into(),
        }
    }
}
