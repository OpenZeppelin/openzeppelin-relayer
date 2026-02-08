//! AWS SQS backend implementation.
//!
//! This module provides an AWS SQS-backed implementation of the QueueBackend trait.
//! It uses FIFO queues to maintain message ordering and prevent duplicates.

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;
use tracing::{debug, error, info, warn};

use crate::{
    jobs::{Job, NotificationSend, TransactionRequest, TransactionSend, TransactionStatusCheck},
    models::DefaultAppState,
};
use actix_web::web::ThinData;

use super::{QueueBackend, QueueBackendError, QueueHealth, QueueType, WorkerHandle};

/// AWS SQS backend for job queue operations.
///
/// Uses FIFO queues to ensure:
/// - Message ordering per transaction (via MessageGroupId)
/// - Exactly-once delivery (via MessageDeduplicationId)
/// - Automatic retry via visibility timeout
#[derive(Clone)]
pub struct SqsBackend {
    /// AWS SQS client
    sqs_client: aws_sdk_sqs::Client,
    /// Mapping of queue types to SQS queue URLs
    queue_urls: HashMap<QueueType, String>,
    /// AWS region
    region: String,
}

impl std::fmt::Debug for SqsBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqsBackend")
            .field("backend_type", &"sqs")
            .field("region", &self.region)
            .field("queue_count", &self.queue_urls.len())
            .finish()
    }
}

impl SqsBackend {
    /// Creates a new SQS backend.
    ///
    /// Loads AWS configuration from environment and builds queue URLs.
    ///
    /// # Environment Variables
    /// - `AWS_REGION` - AWS region (required)
    /// - `AWS_ACCOUNT_ID` - AWS account ID (required for queue URLs)
    /// - `SQS_QUEUE_URL_PREFIX` - Optional custom prefix (default: auto-generated)
    ///
    /// # Errors
    /// Returns ConfigError if required environment variables are missing
    pub async fn new() -> Result<Self, QueueBackendError> {
        info!("Initializing SQS queue backend");

        // Load AWS config from environment
        let config = aws_config::load_from_env().await;
        let sqs_client = aws_sdk_sqs::Client::new(&config);
        let region = config
            .region()
            .ok_or_else(|| {
                QueueBackendError::ConfigError(
                    "AWS_REGION not set. Required for SQS backend.".to_string(),
                )
            })?
            .to_string();

        // Build queue URLs
        let account_id = std::env::var("AWS_ACCOUNT_ID").map_err(|_| {
            QueueBackendError::ConfigError(
                "AWS_ACCOUNT_ID not set. Required for SQS queue URLs.".to_string(),
            )
        })?;

        let prefix = std::env::var("SQS_QUEUE_URL_PREFIX").unwrap_or_else(|_| {
            format!("https://sqs.{region}.amazonaws.com/{account_id}/relayer-stellar-")
        });

        // Build queue URL mapping for Stellar queues
        let queue_urls = HashMap::from([
            (
                QueueType::StellarTransactionRequest,
                format!("{prefix}transaction-request.fifo"),
            ),
            (
                QueueType::StellarTransactionSubmission,
                format!("{prefix}transaction-submission.fifo"),
            ),
            (
                QueueType::StellarStatusCheck,
                format!("{prefix}status-check.fifo"),
            ),
            (
                QueueType::StellarNotification,
                format!("{prefix}notification.fifo"),
            ),
        ]);

        info!(
            region = %region,
            queue_count = queue_urls.len(),
            "SQS backend initialized"
        );

        Ok(Self {
            sqs_client,
            queue_urls,
            region,
        })
    }

    /// Sends a message to SQS with FIFO parameters.
    ///
    /// # Arguments
    /// * `queue_url` - SQS queue URL
    /// * `body` - JSON-serialized job
    /// * `message_group_id` - FIFO group ID (for ordering)
    /// * `message_deduplication_id` - Deduplication ID (prevent duplicates)
    /// * `delay_seconds` - Optional delay (0-900 seconds)
    ///
    /// # Returns
    /// SQS message ID on success
    async fn send_message_to_sqs(
        &self,
        queue_url: &str,
        body: String,
        message_group_id: String,
        message_deduplication_id: String,
        delay_seconds: Option<i32>,
    ) -> Result<String, QueueBackendError> {
        let mut request = self
            .sqs_client
            .send_message()
            .queue_url(queue_url)
            .message_body(body)
            .message_group_id(message_group_id)
            .message_deduplication_id(message_deduplication_id);

        // Add delay if specified (max 900 seconds = 15 minutes)
        if let Some(delay) = delay_seconds {
            let clamped_delay = delay.clamp(0, 900);
            request = request.delay_seconds(clamped_delay);

            if delay != clamped_delay {
                warn!(
                    requested = delay,
                    clamped = clamped_delay,
                    "Delay seconds clamped to SQS limit (0-900)"
                );
            }
        }

        let response = request.send().await.map_err(|e| {
            error!(error = %e, queue_url = %queue_url, "Failed to send message to SQS");
            QueueBackendError::SqsError(format!("SendMessage failed: {e}"))
        })?;

        let message_id = response
            .message_id()
            .ok_or_else(|| QueueBackendError::SqsError("No message_id returned".to_string()))?
            .to_string();

        debug!(
            message_id = %message_id,
            queue_url = %queue_url,
            "Message sent to SQS"
        );

        Ok(message_id)
    }

    /// Calculates delay in seconds from Unix timestamp.
    ///
    /// Returns None if scheduled_on is in the past or None.
    fn calculate_delay_seconds(scheduled_on: Option<i64>) -> Option<i32> {
        scheduled_on.and_then(|timestamp| {
            let now = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .ok()?
                .as_secs() as i64;

            let delay = timestamp - now;
            if delay > 0 {
                Some(delay.min(900) as i32) // SQS max delay: 900 seconds
            } else {
                None // Already past scheduled time
            }
        })
    }
}

#[async_trait]
impl QueueBackend for SqsBackend {
    async fn produce_transaction_request(
        &self,
        job: Job<TransactionRequest>,
        scheduled_on: Option<i64>,
    ) -> Result<String, QueueBackendError> {
        let queue_url = self
            .queue_urls
            .get(&QueueType::StellarTransactionRequest)
            .ok_or_else(|| {
                QueueBackendError::QueueNotFound("StellarTransactionRequest".to_string())
            })?;

        let body = serde_json::to_string(&job).map_err(|e| {
            error!(error = %e, "Failed to serialize TransactionRequest job");
            QueueBackendError::SerializationError(e.to_string())
        })?;

        let message_group_id = job.data.transaction_id.clone();
        let message_deduplication_id = job.message_id.clone();
        let delay_seconds = Self::calculate_delay_seconds(scheduled_on);

        self.send_message_to_sqs(
            queue_url,
            body,
            message_group_id,
            message_deduplication_id,
            delay_seconds,
        )
        .await
    }

    async fn produce_transaction_submission(
        &self,
        job: Job<TransactionSend>,
        scheduled_on: Option<i64>,
    ) -> Result<String, QueueBackendError> {
        let queue_url = self
            .queue_urls
            .get(&QueueType::StellarTransactionSubmission)
            .ok_or_else(|| {
                QueueBackendError::QueueNotFound("StellarTransactionSubmission".to_string())
            })?;

        let body = serde_json::to_string(&job).map_err(|e| {
            error!(error = %e, "Failed to serialize TransactionSend job");
            QueueBackendError::SerializationError(e.to_string())
        })?;

        let message_group_id = job.data.transaction_id.clone();
        let message_deduplication_id = job.message_id.clone();
        let delay_seconds = Self::calculate_delay_seconds(scheduled_on);

        self.send_message_to_sqs(
            queue_url,
            body,
            message_group_id,
            message_deduplication_id,
            delay_seconds,
        )
        .await
    }

    async fn produce_transaction_status_check(
        &self,
        job: Job<TransactionStatusCheck>,
        scheduled_on: Option<i64>,
    ) -> Result<String, QueueBackendError> {
        let queue_url = self
            .queue_urls
            .get(&QueueType::StellarStatusCheck)
            .ok_or_else(|| QueueBackendError::QueueNotFound("StellarStatusCheck".to_string()))?;

        let body = serde_json::to_string(&job).map_err(|e| {
            error!(error = %e, "Failed to serialize TransactionStatusCheck job");
            QueueBackendError::SerializationError(e.to_string())
        })?;

        let message_group_id = job.data.transaction_id.clone();
        let message_deduplication_id = job.message_id.clone();
        let delay_seconds = Self::calculate_delay_seconds(scheduled_on);

        self.send_message_to_sqs(
            queue_url,
            body,
            message_group_id,
            message_deduplication_id,
            delay_seconds,
        )
        .await
    }

    async fn produce_notification(
        &self,
        job: Job<NotificationSend>,
        scheduled_on: Option<i64>,
    ) -> Result<String, QueueBackendError> {
        let queue_url = self
            .queue_urls
            .get(&QueueType::StellarNotification)
            .ok_or_else(|| QueueBackendError::QueueNotFound("StellarNotification".to_string()))?;

        let body = serde_json::to_string(&job).map_err(|e| {
            error!(error = %e, "Failed to serialize NotificationSend job");
            QueueBackendError::SerializationError(e.to_string())
        })?;

        // Notifications use notification_id as the group ID
        let message_group_id = job.data.notification_id.clone();
        let message_deduplication_id = job.message_id.clone();
        let delay_seconds = Self::calculate_delay_seconds(scheduled_on);

        self.send_message_to_sqs(
            queue_url,
            body,
            message_group_id,
            message_deduplication_id,
            delay_seconds,
        )
        .await
    }

    async fn initialize_workers(
        &self,
        app_state: Arc<ThinData<DefaultAppState>>,
    ) -> Result<Vec<WorkerHandle>, QueueBackendError> {
        info!(
            "Initializing SQS workers for {} queues",
            self.queue_urls.len()
        );

        let mut handles = Vec::new();

        // Spawn a worker for each queue type
        for (queue_type, queue_url) in &self.queue_urls {
            let handle = super::sqs_worker::spawn_worker_for_queue(
                self.sqs_client.clone(),
                *queue_type,
                queue_url.clone(),
                app_state.clone(),
            )
            .await?;

            handles.push(handle);
        }

        info!("Successfully spawned {} SQS workers", handles.len());
        Ok(handles)
    }

    async fn health_check(&self) -> Result<Vec<QueueHealth>, QueueBackendError> {
        let mut health_statuses = Vec::new();

        for (queue_type, queue_url) in &self.queue_urls {
            // Get queue attributes to check health
            let result = self
                .sqs_client
                .get_queue_attributes()
                .queue_url(queue_url)
                .attribute_names(
                    aws_sdk_sqs::types::QueueAttributeName::ApproximateNumberOfMessages,
                )
                .attribute_names(
                    aws_sdk_sqs::types::QueueAttributeName::ApproximateNumberOfMessagesNotVisible,
                )
                .send()
                .await;

            let (messages_visible, messages_in_flight, is_healthy) = match result {
                Ok(output) => {
                    let visible = output
                        .attributes()
                        .and_then(|attrs| {
                            attrs
                                .get(&aws_sdk_sqs::types::QueueAttributeName::ApproximateNumberOfMessages)
                        })
                        .and_then(|v| v.parse::<u64>().ok())
                        .unwrap_or(0);
                    let in_flight = output
                        .attributes()
                        .and_then(|attrs| {
                            attrs.get(
                                &aws_sdk_sqs::types::QueueAttributeName::ApproximateNumberOfMessagesNotVisible,
                            )
                        })
                        .and_then(|v| v.parse::<u64>().ok())
                        .unwrap_or(0);
                    (visible, in_flight, true)
                }
                Err(e) => {
                    error!(
                        error = %e,
                        queue_type = ?queue_type,
                        "Failed to get queue attributes"
                    );
                    (0, 0, false)
                }
            };

            health_statuses.push(QueueHealth {
                queue_type: *queue_type,
                messages_visible,
                messages_in_flight,
                messages_dlq: 0, // Would need separate DLQ query
                backend: "sqs".to_string(),
                is_healthy,
            });
        }

        Ok(health_statuses)
    }

    fn backend_type(&self) -> &'static str {
        "sqs"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_delay_seconds() {
        // No scheduled time
        assert_eq!(SqsBackend::calculate_delay_seconds(None), None);

        // Past time
        let past = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            - 10;
        assert_eq!(SqsBackend::calculate_delay_seconds(Some(past)), None);

        // Future time within SQS limit (< 900s)
        let future_5s = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            + 5;
        assert_eq!(
            SqsBackend::calculate_delay_seconds(Some(future_5s)),
            Some(5)
        );

        // Future time beyond SQS limit (> 900s) - should clamp to 900
        let future_1000s = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            + 1000;
        assert_eq!(
            SqsBackend::calculate_delay_seconds(Some(future_1000s)),
            Some(900)
        );
    }

    #[test]
    fn test_calculate_delay_seconds_edge_cases() {
        // Exactly at current time (should return None)
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert_eq!(SqsBackend::calculate_delay_seconds(Some(now)), None);

        // Exactly at SQS limit (900s)
        let future_900s = now + 900;
        assert_eq!(
            SqsBackend::calculate_delay_seconds(Some(future_900s)),
            Some(900)
        );

        // Just over SQS limit (901s) - should clamp to 900
        let future_901s = now + 901;
        assert_eq!(
            SqsBackend::calculate_delay_seconds(Some(future_901s)),
            Some(900)
        );
    }

    #[test]
    fn test_sqs_backend_debug() {
        // Test Debug implementation (doesn't require AWS credentials)
        let backend = SqsBackend {
            sqs_client: aws_sdk_sqs::Client::from_conf(
                aws_sdk_sqs::Config::builder()
                    .region(aws_sdk_sqs::config::Region::new("us-east-1"))
                    .behavior_version(aws_sdk_sqs::config::BehaviorVersion::latest())
                    .build(),
            ),
            queue_urls: HashMap::new(),
            region: "us-east-1".to_string(),
        };

        let debug_str = format!("{backend:?}");
        assert!(debug_str.contains("SqsBackend"));
        assert!(debug_str.contains("us-east-1"));
        assert!(debug_str.contains("queue_count"));
    }

    #[test]
    fn test_sqs_backend_type() {
        let backend = SqsBackend {
            sqs_client: aws_sdk_sqs::Client::from_conf(
                aws_sdk_sqs::Config::builder()
                    .region(aws_sdk_sqs::config::Region::new("us-west-2"))
                    .behavior_version(aws_sdk_sqs::config::BehaviorVersion::latest())
                    .build(),
            ),
            queue_urls: HashMap::new(),
            region: "us-west-2".to_string(),
        };

        assert_eq!(backend.backend_type(), "sqs");
    }

    #[test]
    fn test_queue_url_construction() {
        // Test that queue URLs are correctly constructed
        let mut queue_urls = HashMap::new();
        let prefix = "https://sqs.us-east-1.amazonaws.com/123456789/relayer-stellar-";

        queue_urls.insert(
            QueueType::StellarTransactionRequest,
            format!("{prefix}transaction-request.fifo"),
        );
        queue_urls.insert(
            QueueType::StellarTransactionSubmission,
            format!("{prefix}transaction-submission.fifo"),
        );
        queue_urls.insert(
            QueueType::StellarStatusCheck,
            format!("{prefix}status-check.fifo"),
        );
        queue_urls.insert(
            QueueType::StellarNotification,
            format!("{prefix}notification.fifo"),
        );

        // Verify all queue types have URLs
        assert_eq!(queue_urls.len(), 4);
        assert!(queue_urls
            .get(&QueueType::StellarTransactionRequest)
            .unwrap()
            .ends_with(".fifo"));
        assert!(queue_urls
            .get(&QueueType::StellarTransactionSubmission)
            .unwrap()
            .contains("transaction-submission"));
    }

    #[test]
    fn test_backend_clone() {
        let backend = SqsBackend {
            sqs_client: aws_sdk_sqs::Client::from_conf(
                aws_sdk_sqs::Config::builder()
                    .region(aws_sdk_sqs::config::Region::new("us-east-1"))
                    .behavior_version(aws_sdk_sqs::config::BehaviorVersion::latest())
                    .build(),
            ),
            queue_urls: HashMap::from([(
                QueueType::StellarTransactionRequest,
                "https://sqs.us-east-1.amazonaws.com/123/test.fifo".to_string(),
            )]),
            region: "us-east-1".to_string(),
        };

        // Test that backend is cloneable
        let cloned = backend.clone();
        assert_eq!(cloned.region, backend.region);
        assert_eq!(cloned.queue_urls.len(), backend.queue_urls.len());
    }
}
