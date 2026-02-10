//! AWS SQS backend implementation.
//!
//! This module provides an AWS SQS-backed implementation of the QueueBackend trait.
//! It uses FIFO queues to maintain message ordering and prevent duplicates.

use async_trait::async_trait;
use aws_sdk_sqs::types::MessageAttributeValue;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::watch;
use tracing::{debug, error, info, warn};

use crate::{
    config::ServerConfig,
    jobs::{
        Job, NotificationSend, RelayerHealthCheck, TokenSwapRequest, TransactionRequest,
        TransactionSend, TransactionStatusCheck,
    },
    models::{DefaultAppState, NetworkType},
};
use actix_web::web::ThinData;

use super::{QueueBackend, QueueBackendError, QueueHealth, QueueType, WorkerHandle};

/// SQS maximum message body size (256 KB).
const SQS_MAX_MESSAGE_SIZE_BYTES: usize = 256 * 1024;

/// Chooses the FIFO message group ID based on network type.
///
/// EVM requires per-relayer ordering (nonce management), so uses `relayer_id`.
/// Non-EVM networks (Stellar, Solana) can safely parallelize per transaction,
/// so uses `transaction_id` for better throughput.
/// Falls back to `relayer_id` when network type is unknown (conservative/safe).
fn transaction_message_group_id(
    network_type: Option<&NetworkType>,
    relayer_id: &str,
    transaction_id: &str,
) -> String {
    match network_type {
        Some(NetworkType::Evm) | None => relayer_id.to_string(),
        Some(_) => transaction_id.to_string(),
    }
}

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
    /// Shutdown signal sender — sending `true` tells all workers and cron tasks to stop
    shutdown_tx: Arc<watch::Sender<bool>>,
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
    fn is_fifo_queue_url(queue_url: &str) -> bool {
        queue_url.ends_with(".fifo")
    }

    /// Creates a new SQS backend.
    ///
    /// Loads AWS configuration from environment and builds queue URLs.
    ///
    /// # Environment Variables
    /// - `AWS_REGION` - AWS region (required)
    /// - `SQS_QUEUE_URL_PREFIX` - Optional custom prefix
    /// - `AWS_ACCOUNT_ID` - Required only when `SQS_QUEUE_URL_PREFIX` is not set
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

        // Build queue URLs.
        // If an explicit prefix is provided, avoid forcing AWS_ACCOUNT_ID.
        let prefix = match std::env::var("SQS_QUEUE_URL_PREFIX") {
            Ok(prefix) => prefix,
            Err(_) => {
                let account_id =
                    ServerConfig::get_aws_account_id().map_err(QueueBackendError::ConfigError)?;
                format!("https://sqs.{region}.amazonaws.com/{account_id}/relayer-")
            }
        };
        info!(
            region = %region,
            queue_url_prefix = %prefix,
            "Resolved SQS queue URL prefix"
        );

        // Build queue URL mapping.
        // Status checks use per-network queues (EVM, Stellar, generic/Solana)
        // to match the Redis backend's separate worker setup with independent
        // concurrency pools and network-tuned polling intervals.
        let queue_urls = HashMap::from([
            (
                QueueType::TransactionRequest,
                format!("{prefix}transaction-request.fifo"),
            ),
            (
                QueueType::TransactionSubmission,
                format!("{prefix}transaction-submission.fifo"),
            ),
            (QueueType::StatusCheck, format!("{prefix}status-check.fifo")),
            (
                QueueType::StatusCheckEvm,
                format!("{prefix}status-check-evm.fifo"),
            ),
            (
                QueueType::StatusCheckStellar,
                format!("{prefix}status-check-stellar.fifo"),
            ),
            (
                QueueType::Notification,
                format!("{prefix}notification.fifo"),
            ),
            (
                QueueType::TokenSwapRequest,
                format!("{prefix}token-swap-request.fifo"),
            ),
            (
                QueueType::RelayerHealthCheck,
                format!("{prefix}relayer-health-check.fifo"),
            ),
        ]);

        // Fail fast at startup when expected queues are missing/misconfigured.
        // This avoids silent runtime polling failures and makes infra drift explicit.
        let mut missing_queues = Vec::new();
        for (queue_type, queue_url) in &queue_urls {
            debug!(
                queue_type = %queue_type,
                queue_url = %queue_url,
                "Probing SQS queue accessibility at startup"
            );
            let probe = sqs_client
                .get_queue_attributes()
                .queue_url(queue_url)
                .attribute_names(aws_sdk_sqs::types::QueueAttributeName::QueueArn)
                .attribute_names(aws_sdk_sqs::types::QueueAttributeName::FifoQueue)
                .send()
                .await;

            match probe {
                Ok(output) => {
                    let is_fifo = output
                        .attributes()
                        .and_then(|attrs| {
                            attrs.get(&aws_sdk_sqs::types::QueueAttributeName::FifoQueue)
                        })
                        .map(|v| v == "true")
                        .unwrap_or(false);
                    if is_fifo {
                        debug!(
                            queue_type = %queue_type,
                            queue_url = %queue_url,
                            "SQS queue probe succeeded"
                        );
                    } else {
                        missing_queues.push(format!(
                            "{queue_type} ({queue_url}): queue is not FIFO (FifoQueue != true)"
                        ));
                    }
                }
                Err(err) => {
                    // Include debug details because Display often collapses to "service error".
                    error!(
                        queue_type = %queue_type,
                        queue_url = %queue_url,
                        error = ?err,
                        "SQS queue probe failed"
                    );
                    missing_queues.push(format!("{queue_type} ({queue_url}): {err:?}"));
                }
            }
        }

        if !missing_queues.is_empty() {
            return Err(QueueBackendError::ConfigError(format!(
                "SQS backend initialization failed. Missing/inaccessible queues: {}",
                missing_queues.join(", ")
            )));
        }

        info!(
            region = %region,
            queue_count = queue_urls.len(),
            "SQS backend initialized"
        );

        let (shutdown_tx, _) = watch::channel(false);

        Ok(Self {
            sqs_client,
            queue_urls,
            region,
            shutdown_tx: Arc::new(shutdown_tx),
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
        target_scheduled_on: Option<i64>,
    ) -> Result<String, QueueBackendError> {
        if body.len() > SQS_MAX_MESSAGE_SIZE_BYTES {
            return Err(QueueBackendError::SqsError(format!(
                "Message body size ({} bytes) exceeds SQS limit ({} bytes)",
                body.len(),
                SQS_MAX_MESSAGE_SIZE_BYTES
            )));
        }

        let mut request = self
            .sqs_client
            .send_message()
            .queue_url(queue_url)
            .message_body(body)
            .message_group_id(message_group_id)
            .message_deduplication_id(message_deduplication_id);

        if let Some(timestamp) = target_scheduled_on {
            request = request.message_attributes(
                "target_scheduled_on",
                MessageAttributeValue::builder()
                    .data_type("Number")
                    .string_value(timestamp.to_string())
                    .build()
                    .map_err(|e| {
                        QueueBackendError::SqsError(format!(
                            "Failed to build scheduled-on attribute: {e}"
                        ))
                    })?,
            );
        }

        // Add delay if specified (max 900 seconds = 15 minutes).
        // FIFO queues do not support per-message DelaySeconds.
        if let Some(delay) = delay_seconds {
            let clamped_delay = delay.clamp(0, 900);
            if Self::is_fifo_queue_url(queue_url) {
                debug!(
                    queue_url = %queue_url,
                    requested_delay = delay,
                    "Skipping per-message DelaySeconds for FIFO queue"
                );
            } else {
                request = request.delay_seconds(clamped_delay);
                if delay != clamped_delay {
                    warn!(
                        requested = delay,
                        clamped = clamped_delay,
                        "Delay seconds clamped to SQS limit (0-900)"
                    );
                }
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

    async fn get_dlq_message_count(&self, queue_url: &str) -> u64 {
        let attrs_output = match self
            .sqs_client
            .get_queue_attributes()
            .queue_url(queue_url)
            .attribute_names(aws_sdk_sqs::types::QueueAttributeName::RedrivePolicy)
            .send()
            .await
        {
            Ok(output) => output,
            Err(err) => {
                warn!(error = %err, queue_url = %queue_url, "Failed to fetch redrive policy");
                return 0;
            }
        };

        let redrive_policy = attrs_output
            .attributes()
            .and_then(|attrs| attrs.get(&aws_sdk_sqs::types::QueueAttributeName::RedrivePolicy))
            .cloned();

        let Some(redrive_policy) = redrive_policy else {
            return 0;
        };

        let dlq_arn = serde_json::from_str::<serde_json::Value>(&redrive_policy)
            .ok()
            .and_then(|value| value.get("deadLetterTargetArn").cloned())
            .and_then(|value| value.as_str().map(|s| s.to_string()));

        let Some(dlq_arn) = dlq_arn else {
            warn!(queue_url = %queue_url, "Redrive policy is missing deadLetterTargetArn");
            return 0;
        };

        let Some(dlq_name) = dlq_arn.rsplit(':').next() else {
            return 0;
        };

        let dlq_url = match self
            .sqs_client
            .get_queue_url()
            .queue_name(dlq_name)
            .send()
            .await
        {
            Ok(output) => output.queue_url().map(str::to_string),
            Err(err) => {
                warn!(error = %err, dlq_name = %dlq_name, "Failed to resolve DLQ URL");
                None
            }
        };

        let Some(dlq_url) = dlq_url else {
            return 0;
        };

        match self
            .sqs_client
            .get_queue_attributes()
            .queue_url(dlq_url)
            .attribute_names(aws_sdk_sqs::types::QueueAttributeName::ApproximateNumberOfMessages)
            .send()
            .await
        {
            Ok(output) => output
                .attributes()
                .and_then(|attrs| {
                    attrs.get(&aws_sdk_sqs::types::QueueAttributeName::ApproximateNumberOfMessages)
                })
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0),
            Err(err) => {
                warn!(error = %err, queue_url = %queue_url, "Failed to fetch DLQ depth");
                0
            }
        }
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
            .get(&QueueType::TransactionRequest)
            .ok_or_else(|| QueueBackendError::QueueNotFound("TransactionRequest".to_string()))?;

        let body = serde_json::to_string(&job).map_err(|e| {
            error!(error = %e, "Failed to serialize TransactionRequest job");
            QueueBackendError::SerializationError(e.to_string())
        })?;

        let message_group_id = transaction_message_group_id(
            job.data.network_type.as_ref(),
            &job.data.relayer_id,
            &job.data.transaction_id,
        );
        let message_deduplication_id = job.message_id.clone();
        let delay_seconds = Self::calculate_delay_seconds(scheduled_on);

        self.send_message_to_sqs(
            queue_url,
            body,
            message_group_id,
            message_deduplication_id,
            delay_seconds,
            scheduled_on,
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
            .get(&QueueType::TransactionSubmission)
            .ok_or_else(|| QueueBackendError::QueueNotFound("TransactionSubmission".to_string()))?;

        let body = serde_json::to_string(&job).map_err(|e| {
            error!(error = %e, "Failed to serialize TransactionSend job");
            QueueBackendError::SerializationError(e.to_string())
        })?;

        let message_group_id = transaction_message_group_id(
            job.data.network_type.as_ref(),
            &job.data.relayer_id,
            &job.data.transaction_id,
        );
        let message_deduplication_id = job.message_id.clone();
        let delay_seconds = Self::calculate_delay_seconds(scheduled_on);

        self.send_message_to_sqs(
            queue_url,
            body,
            message_group_id,
            message_deduplication_id,
            delay_seconds,
            scheduled_on,
        )
        .await
    }

    async fn produce_transaction_status_check(
        &self,
        job: Job<TransactionStatusCheck>,
        scheduled_on: Option<i64>,
    ) -> Result<String, QueueBackendError> {
        // Route to network-specific queue based on network type.
        // EVM and Stellar get dedicated queues with tuned concurrency/polling;
        // Solana and unknown network types use the generic StatusCheck queue.
        let queue_type = match job.data.network_type {
            Some(NetworkType::Evm) => QueueType::StatusCheckEvm,
            Some(NetworkType::Stellar) => QueueType::StatusCheckStellar,
            _ => QueueType::StatusCheck,
        };
        let queue_url = self
            .queue_urls
            .get(&queue_type)
            .ok_or_else(|| QueueBackendError::QueueNotFound(format!("{queue_type}")))?;

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
            scheduled_on,
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
            .get(&QueueType::Notification)
            .ok_or_else(|| QueueBackendError::QueueNotFound("Notification".to_string()))?;

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
            scheduled_on,
        )
        .await
    }

    async fn produce_token_swap_request(
        &self,
        job: Job<TokenSwapRequest>,
        scheduled_on: Option<i64>,
    ) -> Result<String, QueueBackendError> {
        let queue_url = self
            .queue_urls
            .get(&QueueType::TokenSwapRequest)
            .ok_or_else(|| QueueBackendError::QueueNotFound("TokenSwapRequest".to_string()))?;

        let body = serde_json::to_string(&job).map_err(|e| {
            error!(error = %e, "Failed to serialize TokenSwapRequest job");
            QueueBackendError::SerializationError(e.to_string())
        })?;

        let message_group_id = job.data.relayer_id.clone();
        let message_deduplication_id = job.message_id.clone();
        let delay_seconds = Self::calculate_delay_seconds(scheduled_on);

        self.send_message_to_sqs(
            queue_url,
            body,
            message_group_id,
            message_deduplication_id,
            delay_seconds,
            scheduled_on,
        )
        .await
    }

    async fn produce_relayer_health_check(
        &self,
        job: Job<RelayerHealthCheck>,
        scheduled_on: Option<i64>,
    ) -> Result<String, QueueBackendError> {
        let queue_url = self
            .queue_urls
            .get(&QueueType::RelayerHealthCheck)
            .ok_or_else(|| QueueBackendError::QueueNotFound("RelayerHealthCheck".to_string()))?;

        let body = serde_json::to_string(&job).map_err(|e| {
            error!(error = %e, "Failed to serialize RelayerHealthCheck job");
            QueueBackendError::SerializationError(e.to_string())
        })?;

        let message_group_id = job.data.relayer_id.clone();
        let message_deduplication_id = job.message_id.clone();
        let delay_seconds = Self::calculate_delay_seconds(scheduled_on);

        self.send_message_to_sqs(
            queue_url,
            body,
            message_group_id,
            message_deduplication_id,
            delay_seconds,
            scheduled_on,
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
                self.shutdown_tx.subscribe(),
            )
            .await?;

            handles.push(handle);
        }

        // Start cron scheduler for periodic tasks (cleanup, token swaps)
        let cron_scheduler =
            super::sqs_cron::SqsCronScheduler::new(app_state.clone(), self.shutdown_tx.subscribe());
        let cron_handles = cron_scheduler.start().await?;
        handles.extend(cron_handles);

        // Internal shutdown signal handler — listens for SIGINT/SIGTERM and
        // broadcasts shutdown to all SQS workers and cron tasks.
        // (Redis/Apalis workers handle signals via their own Monitor.)
        {
            let shutdown_tx = self.shutdown_tx.clone();
            let handle = tokio::spawn(async move {
                let mut sigint =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
                        .expect("Failed to create SIGINT handler");
                let mut sigterm =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                        .expect("Failed to create SIGTERM handler");

                tokio::select! {
                    _ = sigint.recv() => info!("SQS backend: received SIGINT, shutting down workers"),
                    _ = sigterm.recv() => info!("SQS backend: received SIGTERM, shutting down workers"),
                }

                let _ = shutdown_tx.send(true);
            });
            handles.push(WorkerHandle::Tokio(handle));
        }

        info!(
            "Successfully spawned {} SQS workers and cron tasks",
            handles.len()
        );
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
                .attribute_names(aws_sdk_sqs::types::QueueAttributeName::RedrivePolicy)
                .send()
                .await;

            let (messages_visible, messages_in_flight, messages_dlq, is_healthy) = match result {
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
                    let dlq_count = self.get_dlq_message_count(queue_url).await;
                    (visible, in_flight, dlq_count, true)
                }
                Err(e) => {
                    error!(
                        error = %e,
                        queue_type = ?queue_type,
                        "Failed to get queue attributes"
                    );
                    (0, 0, 0, false)
                }
            };

            health_statuses.push(QueueHealth {
                queue_type: *queue_type,
                messages_visible,
                messages_in_flight,
                messages_dlq,
                backend: "sqs".to_string(),
                is_healthy,
            });
        }

        Ok(health_statuses)
    }

    fn backend_type(&self) -> &'static str {
        "sqs"
    }

    fn shutdown(&self) {
        info!("SQS backend: broadcasting shutdown signal to all workers");
        let _ = self.shutdown_tx.send(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::{Job, JobType, TransactionStatusCheck};
    use crate::models::NetworkType;

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
            shutdown_tx: Arc::new(watch::channel(false).0),
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
            shutdown_tx: Arc::new(watch::channel(false).0),
        };

        assert_eq!(backend.backend_type(), "sqs");
    }

    #[test]
    fn test_queue_url_construction() {
        // Test that queue URLs are correctly constructed
        let mut queue_urls = HashMap::new();
        let prefix = "https://sqs.us-east-1.amazonaws.com/123456789/relayer-";

        queue_urls.insert(
            QueueType::TransactionRequest,
            format!("{prefix}transaction-request.fifo"),
        );
        queue_urls.insert(
            QueueType::TransactionSubmission,
            format!("{prefix}transaction-submission.fifo"),
        );
        queue_urls.insert(QueueType::StatusCheck, format!("{prefix}status-check.fifo"));
        queue_urls.insert(
            QueueType::Notification,
            format!("{prefix}notification.fifo"),
        );
        queue_urls.insert(
            QueueType::TokenSwapRequest,
            format!("{prefix}token-swap-request.fifo"),
        );
        queue_urls.insert(
            QueueType::RelayerHealthCheck,
            format!("{prefix}relayer-health-check.fifo"),
        );

        // Verify all queue types have URLs
        assert_eq!(queue_urls.len(), 6);
        assert!(queue_urls
            .get(&QueueType::TransactionRequest)
            .unwrap()
            .ends_with(".fifo"));
        assert!(queue_urls
            .get(&QueueType::TransactionSubmission)
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
                QueueType::TransactionRequest,
                "https://sqs.us-east-1.amazonaws.com/123/test.fifo".to_string(),
            )]),
            region: "us-east-1".to_string(),
            shutdown_tx: Arc::new(watch::channel(false).0),
        };

        // Test that backend is cloneable
        let cloned = backend.clone();
        assert_eq!(cloned.region, backend.region);
        assert_eq!(cloned.queue_urls.len(), backend.queue_urls.len());
    }

    #[test]
    fn test_is_fifo_queue_url() {
        assert!(SqsBackend::is_fifo_queue_url(
            "https://sqs.us-east-1.amazonaws.com/123/queue.fifo"
        ));
        assert!(!SqsBackend::is_fifo_queue_url(
            "https://sqs.us-east-1.amazonaws.com/123/queue"
        ));
    }

    #[test]
    fn test_transaction_message_group_id_evm_uses_relayer() {
        let group = transaction_message_group_id(Some(&NetworkType::Evm), "relayer-1", "tx-123");
        assert_eq!(group, "relayer-1");
    }

    #[test]
    fn test_transaction_message_group_id_stellar_uses_transaction() {
        let group =
            transaction_message_group_id(Some(&NetworkType::Stellar), "relayer-1", "tx-123");
        assert_eq!(group, "tx-123");
    }

    #[test]
    fn test_transaction_message_group_id_solana_uses_transaction() {
        let group = transaction_message_group_id(Some(&NetworkType::Solana), "relayer-1", "tx-123");
        assert_eq!(group, "tx-123");
    }

    #[test]
    fn test_transaction_message_group_id_none_defaults_to_relayer() {
        let group = transaction_message_group_id(None, "relayer-1", "tx-123");
        assert_eq!(
            group, "relayer-1",
            "Unknown network should default to relayer_id (safe/conservative)"
        );
    }

    #[test]
    fn test_sqs_max_message_size_constant() {
        assert_eq!(SQS_MAX_MESSAGE_SIZE_BYTES, 256 * 1024);
    }

    #[tokio::test]
    #[ignore]
    async fn smoke_push_status_check_to_sqs() {
        // Requires real AWS credentials and queue env config.
        // Expected env:
        // - AWS_REGION
        // - SQS_QUEUE_URL_PREFIX
        // - optional AWS_ACCESS_KEY_ID/AWS_SECRET_ACCESS_KEY/AWS_SESSION_TOKEN
        let backend = SqsBackend::new()
            .await
            .expect("SQS backend initialization failed");
        let job = Job::new(
            JobType::TransactionStatusCheck,
            TransactionStatusCheck::new("smoke-tx-id", "smoke-relayer", NetworkType::Stellar),
        );
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_secs() as i64;
        let scheduled_on = Some(now + 2);
        let result = backend
            .produce_transaction_status_check(job, scheduled_on)
            .await;
        assert!(
            result.is_ok(),
            "Expected SendMessage via SQS backend to succeed, got: {result:?}"
        );
    }
}
