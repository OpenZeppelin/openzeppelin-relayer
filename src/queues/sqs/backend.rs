//! AWS SQS backend implementation.
//!
//! This module provides an AWS SQS-backed implementation of the QueueBackend trait.
//! Supports both Standard and FIFO queues. By default (`SQS_QUEUE_TYPE=auto`),
//! the queue type is auto-detected at startup by probing a reference queue.
//! Can also be set explicitly to `standard` or `fifo`.

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
    queues::QueueBackendType,
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
    _relayer_id: &str,
    transaction_id: &str,
) -> String {
    match network_type {
        Some(_) | None => transaction_id.to_string(),
    }
}

/// Selects the status-check queue for a given network type.
///
/// EVM and Stellar use dedicated queues; all other/unknown network types use
/// the generic status-check queue.
fn status_check_queue_type(network_type: Option<&NetworkType>) -> QueueType {
    match network_type {
        Some(NetworkType::Evm) => QueueType::StatusCheckEvm,
        Some(NetworkType::Stellar) => QueueType::StatusCheckStellar,
        _ => QueueType::StatusCheck,
    }
}

/// AWS SQS backend for job queue operations.
///
/// Supports both Standard and FIFO queues (auto-detected at startup, or set via `SQS_QUEUE_TYPE`).
/// FIFO mode provides message ordering and exactly-once delivery;
/// Standard mode offers higher throughput and native per-message delays.
#[derive(Clone)]
pub struct SqsBackend {
    /// AWS SQS client for all operations (send, delete, poll, change visibility)
    sqs_client: aws_sdk_sqs::Client,
    /// Mapping of queue types to SQS queue URLs
    queue_urls: HashMap<QueueType, String>,
    /// Cached DLQ URLs resolved once at startup, keyed by the source queue type.
    /// Avoids repeated `get_queue_url` calls on every health check.
    dlq_urls: HashMap<QueueType, String>,
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

/// Resolves the queue type from the configured `SQS_QUEUE_TYPE` value and
/// probe results.
///
/// - `"standard"` / `"fifo"` → returns immediately (probes ignored).
/// - `"auto"` → decides based on which probe succeeded.
/// - anything else → error.
///
/// `probe_results` is `Option<(bool, bool)>`: `Some((standard_ok, fifo_ok))`
/// when `sqs_queue_type == "auto"`, `None` otherwise.
fn resolve_queue_type(
    sqs_queue_type: &str,
    probe_results: Option<(bool, bool)>,
    ref_standard_url: &str,
    ref_fifo_url: &str,
) -> Result<bool, QueueBackendError> {
    match sqs_queue_type {
        "standard" => {
            info!("Using explicit SQS queue type: standard");
            Ok(false)
        }
        "fifo" => {
            info!("Using explicit SQS queue type: fifo");
            Ok(true)
        }
        "auto" => {
            let (standard_exists, fifo_exists) = probe_results.unwrap_or((false, false));
            match (standard_exists, fifo_exists) {
                (true, false) => {
                    info!("Detected SQS queue type: standard");
                    Ok(false)
                }
                (false, true) => {
                    info!("Detected SQS queue type: fifo");
                    Ok(true)
                }
                (true, true) => Err(QueueBackendError::ConfigError(
                    "Ambiguous SQS queue type: both standard and FIFO \
                     'transaction-request' queues exist. Remove one set or set \
                     SQS_QUEUE_TYPE explicitly."
                        .to_string(),
                )),
                (false, false) => Err(QueueBackendError::ConfigError(format!(
                    "No SQS queues found. Neither '{ref_standard_url}' nor \
                     '{ref_fifo_url}' is accessible. Create queues before starting \
                     the relayer, or set SQS_QUEUE_TYPE explicitly."
                ))),
            }
        }
        other => Err(QueueBackendError::ConfigError(format!(
            "Unsupported SQS_QUEUE_TYPE: '{other}'. Must be 'auto', 'standard', or 'fifo'."
        ))),
    }
}

impl SqsBackend {
    fn is_fifo_queue_url(queue_url: &str) -> bool {
        queue_url.ends_with(".fifo")
    }

    /// Creates a new SQS backend.
    ///
    /// Loads AWS configuration from environment and builds queue URLs.
    /// Queue type is determined by `SQS_QUEUE_TYPE`:
    /// - `auto` (default): probes a reference queue at startup to detect the type
    /// - `standard` / `fifo`: uses the specified type directly, skipping probing
    ///
    /// # Environment Variables
    /// - `AWS_REGION` - AWS region (required)
    /// - `SQS_QUEUE_URL_PREFIX` - Optional custom prefix
    /// - `AWS_ACCOUNT_ID` - Required only when `SQS_QUEUE_URL_PREFIX` is not set
    /// - `SQS_QUEUE_TYPE` - Queue type: `auto` (default), `standard`, or `fifo`
    ///
    /// # Errors
    /// Returns ConfigError if required environment variables are missing or
    /// if queue type cannot be determined (no queues found, or both types exist).
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

        // Build queue URL prefix.
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

        // Determine queue type: explicit override or auto-detect by probing.
        let sqs_queue_type = ServerConfig::get_sqs_queue_type().to_lowercase();
        let ref_standard_url = format!("{prefix}transaction-request");
        let ref_fifo_url = format!("{prefix}transaction-request.fifo");

        // Only probe when auto-detecting; explicit values skip the network call.
        let probe_results = if sqs_queue_type == "auto" {
            let (standard_probe, fifo_probe) = {
                let client_s = sqs_client.clone();
                let client_f = sqs_client.clone();
                let url_s = ref_standard_url.clone();
                let url_f = ref_fifo_url.clone();
                tokio::join!(
                    async move {
                        client_s
                            .get_queue_attributes()
                            .queue_url(&url_s)
                            .attribute_names(aws_sdk_sqs::types::QueueAttributeName::QueueArn)
                            .send()
                            .await
                    },
                    async move {
                        client_f
                            .get_queue_attributes()
                            .queue_url(&url_f)
                            .attribute_names(aws_sdk_sqs::types::QueueAttributeName::QueueArn)
                            .send()
                            .await
                    }
                )
            };
            Some((standard_probe.is_ok(), fifo_probe.is_ok()))
        } else {
            None
        };

        let is_fifo = resolve_queue_type(
            &sqs_queue_type,
            probe_results,
            &ref_standard_url,
            &ref_fifo_url,
        )?;
        let suffix = if is_fifo { ".fifo" } else { "" };

        // Build queue URL mapping.
        // Status checks use per-network queues (EVM, Stellar, generic/Solana)
        // to match the Redis backend's separate worker setup with independent
        // concurrency pools and network-tuned polling intervals.
        let queue_urls = HashMap::from([
            (
                QueueType::TransactionRequest,
                format!("{prefix}transaction-request{suffix}"),
            ),
            (
                QueueType::TransactionSubmission,
                format!("{prefix}transaction-submission{suffix}"),
            ),
            (
                QueueType::StatusCheck,
                format!("{prefix}status-check{suffix}"),
            ),
            (
                QueueType::StatusCheckEvm,
                format!("{prefix}status-check-evm{suffix}"),
            ),
            (
                QueueType::StatusCheckStellar,
                format!("{prefix}status-check-stellar{suffix}"),
            ),
            (
                QueueType::Notification,
                format!("{prefix}notification{suffix}"),
            ),
            (
                QueueType::TokenSwapRequest,
                format!("{prefix}token-swap-request{suffix}"),
            ),
            (
                QueueType::RelayerHealthCheck,
                format!("{prefix}relayer-health-check{suffix}"),
            ),
        ]);

        // Fail fast at startup when expected queues are missing/misconfigured.
        // This avoids silent runtime polling failures and makes infra drift explicit.
        // Probe all queues concurrently to avoid slow sequential startup under
        // SQS throttling during scale-out events.
        let probe_futures: Vec<_> = queue_urls
            .iter()
            .map(|(queue_type, queue_url)| {
                let client = sqs_client.clone();
                let qt = *queue_type;
                let url = queue_url.clone();
                async move {
                    debug!(
                        queue_type = %qt,
                        queue_url = %url,
                        "Probing SQS queue accessibility at startup"
                    );
                    let probe = client
                        .get_queue_attributes()
                        .queue_url(&url)
                        .attribute_names(aws_sdk_sqs::types::QueueAttributeName::QueueArn)
                        .attribute_names(aws_sdk_sqs::types::QueueAttributeName::RedrivePolicy)
                        .send()
                        .await;
                    (qt, url, probe)
                }
            })
            .collect();

        let probe_results = futures::future::join_all(probe_futures).await;

        let mut missing_queues = Vec::new();
        let mut dlq_urls: HashMap<QueueType, String> = HashMap::new();

        for (queue_type, queue_url, probe) in probe_results {
            match probe {
                Ok(output) => {
                    debug!(
                        queue_type = %queue_type,
                        queue_url = %queue_url,
                        is_fifo = is_fifo,
                        "SQS queue probe succeeded"
                    );

                    // Resolve and cache DLQ URL from the redrive policy while we
                    // already have the attributes, avoiding per-health-check lookups.
                    if let Some(dlq_url) =
                        Self::resolve_dlq_url_from_attrs(&sqs_client, output.attributes()).await
                    {
                        dlq_urls.insert(queue_type, dlq_url);
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
            dlq_urls,
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
    /// * `delay_seconds` - Optional delay (0-900 seconds). Applied only for non-FIFO queues.
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
            .message_body(body);

        // FIFO queues require MessageGroupId and MessageDeduplicationId;
        // standard queues reject these parameters.
        if Self::is_fifo_queue_url(queue_url) {
            request = request
                .message_group_id(message_group_id)
                .message_deduplication_id(message_deduplication_id);
        }

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
                    requested_delay_seconds = delay,
                    "Skipping per-message DelaySeconds for FIFO queue; worker-side scheduling will enforce target_scheduled_on"
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

    /// Extracts the DLQ ARN from a redrive policy and resolves its queue URL.
    ///
    /// Called once at startup so the URL can be cached in `dlq_urls`.
    async fn resolve_dlq_url_from_attrs(
        sqs_client: &aws_sdk_sqs::Client,
        attrs: Option<&HashMap<aws_sdk_sqs::types::QueueAttributeName, String>>,
    ) -> Option<String> {
        let redrive_policy =
            attrs.and_then(|a| a.get(&aws_sdk_sqs::types::QueueAttributeName::RedrivePolicy))?;

        let dlq_arn = serde_json::from_str::<serde_json::Value>(redrive_policy)
            .ok()
            .and_then(|v| v.get("deadLetterTargetArn").cloned())
            .and_then(|v| v.as_str().map(|s| s.to_string()));

        let dlq_name = dlq_arn.as_deref()?.rsplit(':').next()?;

        match sqs_client.get_queue_url().queue_name(dlq_name).send().await {
            Ok(output) => output.queue_url().map(str::to_string),
            Err(err) => {
                warn!(error = %err, dlq_name = %dlq_name, "Failed to resolve DLQ URL at startup");
                None
            }
        }
    }

    /// Returns the approximate message count for a cached DLQ URL.
    ///
    /// Uses URLs resolved and cached at startup, requiring only a single
    /// `get_queue_attributes` call per health check (no URL resolution).
    async fn get_dlq_message_count(&self, queue_type: &QueueType) -> u64 {
        let Some(dlq_url) = self.dlq_urls.get(queue_type) else {
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
                warn!(error = %err, dlq_url = %dlq_url, "Failed to fetch DLQ depth");
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
        let queue_type = status_check_queue_type(job.data.network_type.as_ref());
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
                .send()
                .await;

            let (messages_visible, messages_in_flight, messages_dlq, is_healthy) = match result {
                Ok(output) => {
                    let attrs = output.attributes();
                    let visible = attrs
                        .and_then(|a| {
                            a.get(&aws_sdk_sqs::types::QueueAttributeName::ApproximateNumberOfMessages)
                        })
                        .and_then(|v| v.parse::<u64>().ok())
                        .unwrap_or(0);
                    let in_flight = attrs
                        .and_then(|a| {
                            a.get(
                                &aws_sdk_sqs::types::QueueAttributeName::ApproximateNumberOfMessagesNotVisible,
                            )
                        })
                        .and_then(|v| v.parse::<u64>().ok())
                        .unwrap_or(0);
                    let dlq_count = self.get_dlq_message_count(queue_type).await;
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

    fn backend_type(&self) -> QueueBackendType {
        QueueBackendType::Sqs
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
    fn test_sqs_backend_type_value() {
        assert_eq!(QueueBackendType::Sqs.as_str(), "sqs");
        assert_eq!(QueueBackendType::Sqs.to_string(), "sqs");
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
        queue_urls.insert(
            QueueType::StatusCheckEvm,
            format!("{prefix}status-check-evm.fifo"),
        );
        queue_urls.insert(
            QueueType::StatusCheckStellar,
            format!("{prefix}status-check-stellar.fifo"),
        );

        // Verify all queue types have URLs
        assert_eq!(queue_urls.len(), 8);
        assert!(queue_urls
            .get(&QueueType::TransactionRequest)
            .unwrap()
            .ends_with(".fifo"));
        assert!(queue_urls
            .get(&QueueType::TransactionSubmission)
            .unwrap()
            .contains("transaction-submission"));
        assert!(queue_urls
            .get(&QueueType::StatusCheckEvm)
            .unwrap()
            .contains("status-check-evm"));
        assert!(queue_urls
            .get(&QueueType::StatusCheckStellar)
            .unwrap()
            .contains("status-check-stellar"));
    }

    #[test]
    fn test_queue_url_construction_standard() {
        // Test that standard queue URLs do not have .fifo suffix
        let mut queue_urls = HashMap::new();
        let prefix = "https://sqs.us-east-1.amazonaws.com/123456789/relayer-";

        queue_urls.insert(
            QueueType::TransactionRequest,
            format!("{prefix}transaction-request"),
        );
        queue_urls.insert(
            QueueType::TransactionSubmission,
            format!("{prefix}transaction-submission"),
        );
        queue_urls.insert(QueueType::StatusCheck, format!("{prefix}status-check"));
        queue_urls.insert(QueueType::Notification, format!("{prefix}notification"));
        queue_urls.insert(
            QueueType::TokenSwapRequest,
            format!("{prefix}token-swap-request"),
        );
        queue_urls.insert(
            QueueType::RelayerHealthCheck,
            format!("{prefix}relayer-health-check"),
        );
        queue_urls.insert(
            QueueType::StatusCheckEvm,
            format!("{prefix}status-check-evm"),
        );
        queue_urls.insert(
            QueueType::StatusCheckStellar,
            format!("{prefix}status-check-stellar"),
        );

        assert_eq!(queue_urls.len(), 8);
        // Standard queue URLs should NOT end with .fifo
        for (_, url) in &queue_urls {
            assert!(
                !url.ends_with(".fifo"),
                "Standard queue URL should not end with .fifo: {url}"
            );
        }
        assert!(queue_urls
            .get(&QueueType::TransactionRequest)
            .unwrap()
            .contains("transaction-request"));
    }

    #[test]
    fn test_is_fifo_queue_url_standard() {
        assert!(!SqsBackend::is_fifo_queue_url(
            "https://sqs.us-east-1.amazonaws.com/123/relayer-transaction-request"
        ));
        assert!(!SqsBackend::is_fifo_queue_url(
            "http://localstack:4566/000000000000/relayer-status-check"
        ));
    }

    #[test]
    fn test_transaction_message_group_id_evm_uses_transaction() {
        let group = transaction_message_group_id(Some(&NetworkType::Evm), "relayer-1", "tx-123");
        assert_eq!(group, "tx-123");
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
    fn test_transaction_message_group_id_none_defaults_to_transaction() {
        let group = transaction_message_group_id(None, "relayer-1", "tx-123");
        assert_eq!(
            group, "tx-123",
            "Unknown network should default to transaction id"
        );
    }

    #[test]
    fn test_status_check_queue_type_evm() {
        assert_eq!(
            status_check_queue_type(Some(&NetworkType::Evm)),
            QueueType::StatusCheckEvm
        );
    }

    #[test]
    fn test_status_check_queue_type_stellar() {
        assert_eq!(
            status_check_queue_type(Some(&NetworkType::Stellar)),
            QueueType::StatusCheckStellar
        );
    }

    #[test]
    fn test_status_check_queue_type_solana_defaults_to_generic() {
        assert_eq!(
            status_check_queue_type(Some(&NetworkType::Solana)),
            QueueType::StatusCheck
        );
    }

    #[test]
    fn test_status_check_queue_type_none_defaults_to_generic() {
        assert_eq!(status_check_queue_type(None), QueueType::StatusCheck);
    }

    #[test]
    fn test_sqs_max_message_size_constant() {
        assert_eq!(SQS_MAX_MESSAGE_SIZE_BYTES, 256 * 1024);
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

    // --- resolve_queue_type tests ---

    const REF_STD: &str = "http://localhost:4566/000000000000/relayer-transaction-request";
    const REF_FIFO: &str = "http://localhost:4566/000000000000/relayer-transaction-request.fifo";

    #[test]
    fn test_resolve_queue_type_explicit_standard() {
        let result = resolve_queue_type("standard", None, REF_STD, REF_FIFO);
        assert_eq!(result.unwrap(), false);
    }

    #[test]
    fn test_resolve_queue_type_explicit_fifo() {
        let result = resolve_queue_type("fifo", None, REF_STD, REF_FIFO);
        assert_eq!(result.unwrap(), true);
    }

    #[test]
    fn test_resolve_queue_type_explicit_ignores_probes() {
        // Even if probes say FIFO exists, explicit "standard" wins
        let result = resolve_queue_type("standard", Some((false, true)), REF_STD, REF_FIFO);
        assert_eq!(result.unwrap(), false);
    }

    #[test]
    fn test_resolve_queue_type_auto_standard_only() {
        let result = resolve_queue_type("auto", Some((true, false)), REF_STD, REF_FIFO);
        assert_eq!(result.unwrap(), false);
    }

    #[test]
    fn test_resolve_queue_type_auto_fifo_only() {
        let result = resolve_queue_type("auto", Some((false, true)), REF_STD, REF_FIFO);
        assert_eq!(result.unwrap(), true);
    }

    #[test]
    fn test_resolve_queue_type_auto_both_exist_errors() {
        let result = resolve_queue_type("auto", Some((true, true)), REF_STD, REF_FIFO);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Ambiguous"),
            "Expected 'Ambiguous' error, got: {err}"
        );
    }

    #[test]
    fn test_resolve_queue_type_auto_neither_exists_errors() {
        let result = resolve_queue_type("auto", Some((false, false)), REF_STD, REF_FIFO);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("No SQS queues found"),
            "Expected 'No SQS queues found' error, got: {err}"
        );
    }

    #[test]
    fn test_resolve_queue_type_auto_no_probes_defaults_to_neither() {
        // None probe results (shouldn't happen in practice) treated as (false, false)
        let result = resolve_queue_type("auto", None, REF_STD, REF_FIFO);
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_queue_type_unknown_value_errors() {
        let result = resolve_queue_type("invalid", None, REF_STD, REF_FIFO);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Unsupported SQS_QUEUE_TYPE"),
            "Expected unsupported error, got: {err}"
        );
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
