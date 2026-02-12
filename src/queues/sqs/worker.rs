//! SQS worker implementation for polling and processing messages.
//!
//! This module provides worker tasks that poll SQS queues and process jobs
//! using the existing handler functions.

use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;

use actix_web::web::ThinData;
use aws_sdk_sqs::error::{ProvideErrorMetadata, SdkError};
use aws_sdk_sqs::types::{
    DeleteMessageBatchRequestEntry, Message, MessageAttributeValue, MessageSystemAttributeName,
};
use futures::FutureExt;
use serde::de::DeserializeOwned;
use tokio::sync::watch;
use tokio::task::{JoinHandle, JoinSet};
use tracing::{debug, error, info, warn};

use crate::{
    config::ServerConfig,
    jobs::{
        notification_handler, relayer_health_check_handler, token_swap_request_handler,
        transaction_request_handler, transaction_status_handler, transaction_submission_handler,
        Job, NotificationSend, RelayerHealthCheck, TokenSwapRequest, TransactionRequest,
        TransactionSend, TransactionStatusCheck,
    },
};

use super::{HandlerError, WorkerContext};
use super::{QueueBackendError, QueueType, WorkerHandle};

#[derive(Debug)]
enum ProcessingError {
    Retryable(String),
    Permanent(String),
}

/// Outcome of processing a single SQS message, used to decide whether the
/// message should be batch-deleted or left in the queue.
#[derive(Debug)]
enum MessageOutcome {
    /// Message processed successfully — should be deleted from queue.
    Delete { receipt_handle: String },
    /// Message should remain in queue (e.g. status-check retry via visibility
    /// change, or retryable error awaiting visibility timeout).
    Retain,
}

/// Spawns a worker task for a specific SQS queue.
///
/// The worker continuously polls the queue, processes messages, and handles
/// retries via SQS visibility timeout.
///
/// # Arguments
/// * `sqs_client` - AWS SQS client for all operations (poll, send, delete, change visibility)
/// * `queue_type` - Type of queue (determines handler and concurrency)
/// * `queue_url` - SQS queue URL
/// * `app_state` - Application state with repositories and services
///
/// # Returns
/// JoinHandle to the spawned worker task
pub async fn spawn_worker_for_queue(
    sqs_client: aws_sdk_sqs::Client,
    queue_type: QueueType,
    queue_url: String,
    app_state: Arc<ThinData<crate::models::DefaultAppState>>,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<WorkerHandle, QueueBackendError> {
    let concurrency = get_concurrency_for_queue(queue_type);
    let max_retries = queue_type.max_retries();
    let polling_interval = queue_type.polling_interval_secs();
    let visibility_timeout = queue_type.visibility_timeout_secs();
    let handler_timeout_secs = handler_timeout_secs(queue_type);
    let handler_timeout = Duration::from_secs(handler_timeout_secs);

    info!(
        queue_type = ?queue_type,
        queue_url = %queue_url,
        concurrency = concurrency,
        max_retries = max_retries,
        polling_interval_secs = polling_interval,
        visibility_timeout_secs = visibility_timeout,
        handler_timeout_secs = handler_timeout_secs,
        "Spawning SQS worker"
    );

    let handle: JoinHandle<()> = tokio::spawn(async move {
        let semaphore = Arc::new(tokio::sync::Semaphore::new(concurrency));
        let mut inflight: JoinSet<Option<String>> = JoinSet::new();
        let mut consecutive_poll_errors: u32 = 0;
        let mut pending_deletes: Vec<String> = Vec::new();

        loop {
            // Reap completed tasks and collect receipt handles for batch delete
            while let Some(result) = inflight.try_join_next() {
                match result {
                    Ok(Some(receipt_handle)) => pending_deletes.push(receipt_handle),
                    Ok(None) => {} // Retained message, no delete needed
                    Err(e) => {
                        warn!(
                            queue_type = ?queue_type,
                            error = %e,
                            "In-flight task failed"
                        );
                    }
                }
            }

            // Flush any accumulated deletes as a batch
            if !pending_deletes.is_empty() {
                flush_delete_batch(&sqs_client, &queue_url, &pending_deletes, queue_type).await;
                pending_deletes.clear();
            }

            // Check shutdown before each iteration
            if *shutdown_rx.borrow() {
                info!(queue_type = ?queue_type, "Shutdown signal received, stopping SQS worker");
                break;
            }

            // Do not poll for more messages than we can process; otherwise
            // messages sit in-flight waiting for permits.
            let available_permits = semaphore.available_permits();
            if available_permits == 0 {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(50)) => continue,
                    _ = shutdown_rx.changed() => {
                        info!(queue_type = ?queue_type, "Shutdown signal received, stopping SQS worker");
                        break;
                    }
                }
            }

            let batch_size = available_permits.min(10) as i32;

            // Poll SQS for messages, racing with shutdown signal
            let messages_result = tokio::select! {
                result = sqs_client
                    .receive_message()
                    .queue_url(&queue_url)
                    .max_number_of_messages(batch_size) // SQS max is 10
                    .wait_time_seconds(polling_interval as i32)
                    .visibility_timeout(visibility_timeout as i32)
                    .message_system_attribute_names(MessageSystemAttributeName::ApproximateReceiveCount)
                    .message_system_attribute_names(MessageSystemAttributeName::MessageGroupId)
                    .message_attribute_names("target_scheduled_on")
                    .message_attribute_names("retry_attempt")
                    .send() => result,
                _ = shutdown_rx.changed() => {
                    info!(queue_type = ?queue_type, "Shutdown signal received during SQS poll, stopping worker");
                    break;
                }
            };

            match messages_result {
                Ok(output) => {
                    if consecutive_poll_errors > 0 {
                        info!(
                            queue_type = ?queue_type,
                            previous_errors = consecutive_poll_errors,
                            "SQS polling recovered after consecutive errors"
                        );
                    }
                    consecutive_poll_errors = 0;

                    if let Some(messages) = output.messages {
                        if !messages.is_empty() {
                            debug!(
                                queue_type = ?queue_type,
                                message_count = messages.len(),
                                "Received messages from SQS"
                            );

                            // Process messages concurrently (up to semaphore limit)
                            for message in messages {
                                let permit = match semaphore.clone().acquire_owned().await {
                                    Ok(permit) => permit,
                                    Err(err) => {
                                        error!(
                                            queue_type = ?queue_type,
                                            error = %err,
                                            "Semaphore closed, stopping SQS worker loop"
                                        );
                                        return;
                                    }
                                };
                                let client = sqs_client.clone();
                                let url = queue_url.clone();
                                let state = app_state.clone();

                                inflight.spawn(async move {
                                    let _permit = permit; // always dropped, even on panic

                                    let result = tokio::time::timeout(
                                        handler_timeout,
                                        AssertUnwindSafe(process_message(
                                            client.clone(),
                                            message,
                                            queue_type,
                                            &url,
                                            state,
                                            max_retries,
                                        ))
                                        .catch_unwind(),
                                    )
                                    .await;

                                    match result {
                                        Ok(Ok(Ok(MessageOutcome::Delete { receipt_handle }))) => {
                                            Some(receipt_handle)
                                        }
                                        Ok(Ok(Ok(MessageOutcome::Retain))) => None,
                                        Ok(Ok(Err(e))) => {
                                            error!(
                                                queue_type = ?queue_type,
                                                error = %e,
                                                "Failed to process message"
                                            );
                                            None
                                        }
                                        Ok(Err(panic_info)) => {
                                            let msg = panic_info
                                                .downcast_ref::<String>()
                                                .map(|s| s.as_str())
                                                .or_else(|| {
                                                    panic_info.downcast_ref::<&str>().copied()
                                                })
                                                .unwrap_or("unknown panic");
                                            error!(
                                                queue_type = ?queue_type,
                                                panic = %msg,
                                                "Message handler panicked"
                                            );
                                            None
                                        }
                                        Err(_) => {
                                            error!(
                                                queue_type = ?queue_type,
                                                timeout_secs = handler_timeout.as_secs(),
                                                "Message handler timed out; message will be retried after visibility timeout"
                                            );
                                            None
                                        }
                                    }
                                });
                            }
                        }
                    }
                }
                Err(e) => {
                    consecutive_poll_errors = consecutive_poll_errors.saturating_add(1);
                    let backoff_secs = poll_error_backoff_secs(consecutive_poll_errors);
                    let (error_kind, error_code, error_message) = match &e {
                        SdkError::ServiceError(ctx) => {
                            ("service", ctx.err().code(), ctx.err().message())
                        }
                        SdkError::DispatchFailure(_) => ("dispatch", None, None),
                        SdkError::ResponseError(_) => ("response", None, None),
                        SdkError::TimeoutError(_) => ("timeout", None, None),
                        _ => ("other", None, None),
                    };
                    error!(
                        queue_type = ?queue_type,
                        error_kind = error_kind,
                        error_code = error_code.unwrap_or("unknown"),
                        error_message = error_message.unwrap_or("n/a"),
                        error = %e,
                        error_debug = ?e,
                        consecutive_errors = consecutive_poll_errors,
                        backoff_secs = backoff_secs,
                        "Failed to receive messages from SQS, backing off"
                    );
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_secs(backoff_secs)) => {}
                        _ = shutdown_rx.changed() => {
                            info!(queue_type = ?queue_type, "Shutdown signal received during backoff, stopping worker");
                            break;
                        }
                    }
                }
            }
        }

        // Drain in-flight tasks before shutdown, collecting final deletes
        if !inflight.is_empty() {
            info!(
                queue_type = ?queue_type,
                count = inflight.len(),
                "Draining in-flight tasks before shutdown"
            );
            match tokio::time::timeout(Duration::from_secs(30), async {
                while let Some(result) = inflight.join_next().await {
                    match result {
                        Ok(Some(receipt_handle)) => pending_deletes.push(receipt_handle),
                        Ok(None) => {}
                        Err(e) => {
                            warn!(
                                queue_type = ?queue_type,
                                error = %e,
                                "In-flight task failed during drain"
                            );
                        }
                    }
                }
            })
            .await
            {
                Ok(()) => info!(queue_type = ?queue_type, "All in-flight tasks drained"),
                Err(_) => {
                    warn!(
                        queue_type = ?queue_type,
                        remaining = inflight.len(),
                        "Drain timeout, abandoning remaining tasks"
                    );
                    inflight.abort_all();
                }
            }
        }

        // Flush any remaining deletes accumulated during drain
        if !pending_deletes.is_empty() {
            flush_delete_batch(&sqs_client, &queue_url, &pending_deletes, queue_type).await;
        }

        info!(queue_type = ?queue_type, "SQS worker stopped");
    });

    Ok(WorkerHandle::Tokio(handle))
}

/// Processes a single SQS message.
///
/// Routes the message to the appropriate handler based on queue type,
/// handles success/failure, and manages message deletion/retry.
async fn process_message(
    sqs_client: aws_sdk_sqs::Client,
    message: Message,
    queue_type: QueueType,
    queue_url: &str,
    app_state: Arc<ThinData<crate::models::DefaultAppState>>,
    max_retries: usize,
) -> Result<MessageOutcome, QueueBackendError> {
    let body = message
        .body()
        .ok_or_else(|| QueueBackendError::QueueError("Empty message body".to_string()))?;

    let receipt_handle = message
        .receipt_handle()
        .ok_or_else(|| QueueBackendError::QueueError("Missing receipt handle".to_string()))?;

    // For jobs with scheduling beyond SQS 15-minute max delay, keep deferring in hops.
    if let Some(target_scheduled_on) = parse_target_scheduled_on(&message) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .map_err(|e| QueueBackendError::QueueError(format!("System clock error: {e}")))?
            .as_secs() as i64;
        let remaining = target_scheduled_on - now;
        if remaining > 0 {
            let should_delete_original = defer_message(
                &sqs_client,
                queue_url,
                body.to_string(),
                &message,
                target_scheduled_on,
                remaining.min(900) as i32,
            )
            .await?;

            debug!(
                queue_type = ?queue_type,
                remaining_seconds = remaining,
                "Deferred scheduled SQS message for next delay hop"
            );
            return if should_delete_original {
                Ok(MessageOutcome::Delete {
                    receipt_handle: receipt_handle.to_string(),
                })
            } else {
                Ok(MessageOutcome::Retain)
            };
        }
    }

    // Get retry attempt count from message attributes
    let receive_count = message
        .attributes()
        .and_then(|attrs| attrs.get(&MessageSystemAttributeName::ApproximateReceiveCount))
        .and_then(|count| count.parse::<usize>().ok())
        .unwrap_or(1);
    // SQS receive count starts at 1; Apalis Attempt starts at 0.
    let attempt_number = receive_count.saturating_sub(1);
    // Persisted retry attempt for self-reenqueued status checks. Falls back to receive_count-based
    // attempt when attribute is missing.
    let logical_retry_attempt = parse_retry_attempt(&message).unwrap_or(attempt_number);

    // Use SQS MessageId as the worker task_id for log correlation.
    let sqs_message_id = message.message_id().unwrap_or("unknown").to_string();

    debug!(
        queue_type = ?queue_type,
        message_id = %sqs_message_id,
        attempt = attempt_number,
        receive_count = receive_count,
        max_retries = max_retries,
        "Processing message"
    );

    // Route to appropriate handler
    let result = match queue_type {
        QueueType::TransactionRequest => {
            process_job::<TransactionRequest, _, _>(
                body,
                app_state,
                attempt_number,
                sqs_message_id,
                "TransactionRequest",
                transaction_request_handler,
            )
            .await
        }
        QueueType::TransactionSubmission => {
            process_job::<TransactionSend, _, _>(
                body,
                app_state,
                attempt_number,
                sqs_message_id,
                "TransactionSend",
                transaction_submission_handler,
            )
            .await
        }
        QueueType::StatusCheck | QueueType::StatusCheckEvm | QueueType::StatusCheckStellar => {
            process_job::<TransactionStatusCheck, _, _>(
                body,
                app_state,
                attempt_number,
                sqs_message_id,
                "TransactionStatusCheck",
                transaction_status_handler,
            )
            .await
        }
        QueueType::Notification => {
            process_job::<NotificationSend, _, _>(
                body,
                app_state,
                attempt_number,
                sqs_message_id,
                "NotificationSend",
                notification_handler,
            )
            .await
        }
        QueueType::TokenSwapRequest => {
            process_job::<TokenSwapRequest, _, _>(
                body,
                app_state,
                attempt_number,
                sqs_message_id,
                "TokenSwapRequest",
                token_swap_request_handler,
            )
            .await
        }
        QueueType::RelayerHealthCheck => {
            process_job::<RelayerHealthCheck, _, _>(
                body,
                app_state,
                attempt_number,
                sqs_message_id,
                "RelayerHealthCheck",
                relayer_health_check_handler,
            )
            .await
        }
    };

    match result {
        Ok(()) => {
            debug!(
                queue_type = ?queue_type,
                attempt = attempt_number,
                "Message processed successfully"
            );

            Ok(MessageOutcome::Delete {
                receipt_handle: receipt_handle.to_string(),
            })
        }
        Err(ProcessingError::Permanent(e)) => {
            error!(
                queue_type = ?queue_type,
                attempt = attempt_number,
                error = %e,
                "Permanent handler failure, message will be deleted"
            );

            Ok(MessageOutcome::Delete {
                receipt_handle: receipt_handle.to_string(),
            })
        }
        Err(ProcessingError::Retryable(e)) => {
            // StatusCheck queues use self-re-enqueue with short delay instead of
            // relying on the 300s visibility timeout. This brings retry intervals
            // from ~5 minutes down to 3-10 seconds, matching Apalis backoff behavior.
            if queue_type.is_status_check() {
                let delay = compute_status_retry_delay(body, logical_retry_attempt);

                // FIFO queues do not support per-message DelaySeconds. Use visibility
                // timeout on the in-flight message to schedule the retry.
                if is_fifo_queue_url(queue_url) {
                    if let Err(err) = sqs_client
                        .change_message_visibility()
                        .queue_url(queue_url)
                        .receipt_handle(receipt_handle)
                        .visibility_timeout(delay.clamp(1, 900))
                        .send()
                        .await
                    {
                        error!(
                            queue_type = ?queue_type,
                            error = %err,
                            "Failed to set visibility timeout for status check retry; falling back to existing visibility timeout"
                        );
                        return Ok(MessageOutcome::Retain);
                    }

                    debug!(
                        queue_type = ?queue_type,
                        attempt = logical_retry_attempt,
                        delay_seconds = delay,
                        error = %e,
                        "Status check retry scheduled via visibility timeout"
                    );

                    return Ok(MessageOutcome::Retain);
                }

                let next_retry_attempt = logical_retry_attempt.saturating_add(1);

                // Standard queues: re-enqueue with native DelaySeconds,
                // no group_id or dedup_id needed. Duplicate deliveries are
                // harmless because handlers are idempotent.
                if let Err(send_err) = sqs_client
                    .send_message()
                    .queue_url(queue_url)
                    .message_body(body.to_string())
                    .delay_seconds(delay)
                    .message_attributes(
                        "retry_attempt",
                        MessageAttributeValue::builder()
                            .data_type("Number")
                            .string_value(next_retry_attempt.to_string())
                            .build()
                            .map_err(|err| {
                                QueueBackendError::SqsError(format!(
                                    "Failed to build retry_attempt attribute: {err}"
                                ))
                            })?,
                    )
                    .send()
                    .await
                {
                    error!(
                        queue_type = ?queue_type,
                        error = %send_err,
                        "Failed to re-enqueue status check message; leaving original for visibility timeout retry"
                    );
                    // Fall through — original message will retry after visibility timeout
                    return Ok(MessageOutcome::Retain);
                }

                debug!(
                    queue_type = ?queue_type,
                    attempt = logical_retry_attempt,
                    delay_seconds = delay,
                    error = %e,
                    "Status check re-enqueued with short delay"
                );

                // Delete the original message now that the re-enqueue succeeded
                return Ok(MessageOutcome::Delete {
                    receipt_handle: receipt_handle.to_string(),
                });
            }

            // Non-StatusCheck queues: let message return via visibility timeout
            if max_retries != usize::MAX && receive_count > max_retries {
                error!(
                    queue_type = ?queue_type,
                    attempt = attempt_number,
                    receive_count = receive_count,
                    max_retries = max_retries,
                    error = %e,
                    "Max retries exceeded; message will be automatically moved to DLQ by SQS redrive policy"
                );
            } else {
                warn!(
                    queue_type = ?queue_type,
                    attempt = attempt_number,
                    receive_count = receive_count,
                    max_retries = max_retries,
                    error = %e,
                    "Message processing failed, will retry after visibility timeout"
                );
            }

            // Don't delete message - it will automatically return to queue
            // after visibility timeout expires. SQS will move to DLQ after
            // exceeding maxReceiveCount configured in redrive policy.
            Ok(MessageOutcome::Retain)
        }
    }
}

/// Generic job processor — deserializes `Job<T>`, creates a `WorkerContext`,
/// and delegates to the provided handler function.
async fn process_job<T, F, Fut>(
    body: &str,
    app_state: Arc<ThinData<crate::models::DefaultAppState>>,
    attempt: usize,
    task_id: String,
    type_name: &str,
    handler: F,
) -> Result<(), ProcessingError>
where
    T: DeserializeOwned,
    F: FnOnce(Job<T>, ThinData<crate::models::DefaultAppState>, WorkerContext) -> Fut,
    Fut: Future<Output = Result<(), HandlerError>>,
{
    let job: Job<T> = serde_json::from_str(body).map_err(|e| {
        error!(error = %e, "Failed to deserialize {} job", type_name);
        // Malformed payload is not recoverable by retrying the same message body.
        ProcessingError::Permanent(format!("Failed to deserialize {type_name} job: {e}"))
    })?;

    let ctx = WorkerContext::new(attempt, task_id);
    handler(job, (*app_state).clone(), ctx)
        .await
        .map_err(map_handler_error)
}

fn map_handler_error(error: HandlerError) -> ProcessingError {
    match error {
        HandlerError::Abort(msg) => ProcessingError::Permanent(msg),
        HandlerError::Retry(msg) => ProcessingError::Retryable(msg),
    }
}

fn parse_target_scheduled_on(message: &Message) -> Option<i64> {
    message
        .message_attributes()
        .and_then(|attrs| attrs.get("target_scheduled_on"))
        .and_then(|value| value.string_value())
        .and_then(|value| value.parse::<i64>().ok())
}

fn parse_retry_attempt(message: &Message) -> Option<usize> {
    message
        .message_attributes()
        .and_then(|attrs| attrs.get("retry_attempt"))
        .and_then(|value| value.string_value())
        .and_then(|value| value.parse::<usize>().ok())
}

fn is_fifo_queue_url(queue_url: &str) -> bool {
    queue_url.ends_with(".fifo")
}

async fn defer_message(
    sqs_client: &aws_sdk_sqs::Client,
    queue_url: &str,
    body: String,
    message: &Message,
    target_scheduled_on: i64,
    delay_seconds: i32,
) -> Result<bool, QueueBackendError> {
    if is_fifo_queue_url(queue_url) {
        let receipt_handle = message.receipt_handle().ok_or_else(|| {
            QueueBackendError::QueueError(
                "Cannot defer FIFO message: missing receipt handle".to_string(),
            )
        })?;

        sqs_client
            .change_message_visibility()
            .queue_url(queue_url)
            .receipt_handle(receipt_handle)
            .visibility_timeout(delay_seconds.clamp(1, 900))
            .send()
            .await
            .map_err(|e| {
                QueueBackendError::SqsError(format!(
                    "Failed to defer FIFO message via visibility timeout: {e}"
                ))
            })?;

        return Ok(false);
    }

    // Standard queues support native per-message DelaySeconds — no need for
    // group_id or dedup_id. Just re-send with the delay and scheduling attribute.
    let request = sqs_client
        .send_message()
        .queue_url(queue_url)
        .message_body(body)
        .delay_seconds(delay_seconds.clamp(1, 900))
        .message_attributes(
            "target_scheduled_on",
            MessageAttributeValue::builder()
                .data_type("Number")
                .string_value(target_scheduled_on.to_string())
                .build()
                .map_err(|e| {
                    QueueBackendError::SqsError(format!(
                        "Failed to build deferred scheduled attribute: {e}"
                    ))
                })?,
        );

    request.send().await.map_err(|e| {
        QueueBackendError::SqsError(format!("Failed to defer scheduled message: {e}"))
    })?;

    Ok(true)
}

/// Partial struct for deserializing only the `network_type` field from a status check job.
///
/// Used to avoid deserializing the entire `Job<TransactionStatusCheck>` when we only
/// need the network type to determine retry delay.
#[derive(serde::Deserialize)]
struct StatusCheckData {
    network_type: Option<crate::models::NetworkType>,
}

/// Partial struct matching `Job<TransactionStatusCheck>` structure.
///
/// Used for efficient partial deserialization to extract only the `network_type`
/// field without parsing the entire job payload.
#[derive(serde::Deserialize)]
struct PartialStatusCheckJob {
    data: StatusCheckData,
}

/// Extracts `network_type` from a status check payload and computes retry delay.
///
/// This uses hardcoded network-specific backoff windows aligned with Redis/Apalis:
/// - EVM: 8s -> 12s cap
/// - Stellar: 2s -> 3s cap
/// - Solana/default: 5s -> 8s cap
fn compute_status_retry_delay(body: &str, attempt: usize) -> i32 {
    let network_type = serde_json::from_str::<PartialStatusCheckJob>(body)
        .ok()
        .and_then(|j| j.data.network_type);

    crate::queues::retry_config::status_check_retry_delay_secs(network_type, attempt)
}

/// Gets the concurrency limit for a queue type from environment.
fn get_concurrency_for_queue(queue_type: QueueType) -> usize {
    let configured = ServerConfig::get_worker_concurrency(
        queue_type.concurrency_env_key(),
        queue_type.default_concurrency(),
    );
    if configured == 0 {
        warn!(
            queue_type = ?queue_type,
            "Configured concurrency is 0; clamping to 1"
        );
        1
    } else {
        configured
    }
}

/// Maximum allowed wall-clock processing time per message before the handler task is canceled.
///
/// Keep this bounded so permits cannot be held forever by hung handlers.
fn handler_timeout_secs(queue_type: QueueType) -> u64 {
    u64::from(queue_type.visibility_timeout_secs().max(1))
}

/// Maximum backoff duration for poll errors (1 minute).
const MAX_POLL_BACKOFF_SECS: u64 = 60;

/// Number of consecutive errors between recovery probes at the backoff ceiling.
/// Once the backoff reaches `MAX_POLL_BACKOFF_SECS`, every Nth error cycle uses
/// the base interval (5s) to quickly detect when the SQS endpoint recovers.
const RECOVERY_PROBE_EVERY: u32 = 4;

/// Computes exponential backoff for consecutive poll errors with recovery probes.
///
/// Returns: 5, 10, 20, 40, 60, 60, 60, **5** (probe), 60, 60, 60, **5**, ...
fn poll_error_backoff_secs(consecutive_errors: u32) -> u64 {
    let base: u64 = 5;

    // Once well past the ceiling, periodically try the base interval
    // to quickly detect when the SQS endpoint recovers.
    if consecutive_errors >= 7 && consecutive_errors % RECOVERY_PROBE_EVERY == 0 {
        return base;
    }

    let exponent = consecutive_errors.saturating_sub(1).min(16);
    base.saturating_mul(2_u64.saturating_pow(exponent))
        .min(MAX_POLL_BACKOFF_SECS)
}

/// Deletes messages from SQS in batches of up to 10 (the SQS maximum per call).
///
/// Returns the total number of successfully deleted messages. Any per-entry
/// failures are logged as warnings — SQS will redeliver those messages after
/// the visibility timeout expires.
async fn flush_delete_batch(
    sqs_client: &aws_sdk_sqs::Client,
    queue_url: &str,
    batch: &[String],
    queue_type: QueueType,
) -> usize {
    if batch.is_empty() {
        return 0;
    }

    let mut deleted = 0;

    for chunk in batch.chunks(10) {
        let entries: Vec<DeleteMessageBatchRequestEntry> = chunk
            .iter()
            .enumerate()
            .map(|(i, handle)| {
                DeleteMessageBatchRequestEntry::builder()
                    .id(i.to_string())
                    .receipt_handle(handle)
                    .build()
                    .expect("id and receipt_handle are always set")
            })
            .collect();

        match sqs_client
            .delete_message_batch()
            .queue_url(queue_url)
            .set_entries(Some(entries))
            .send()
            .await
        {
            Ok(output) => {
                deleted += output.successful().len();

                for f in output.failed() {
                    warn!(
                        queue_type = ?queue_type,
                        id = %f.id(),
                        code = %f.code(),
                        message = f.message().unwrap_or("unknown"),
                        "Batch delete entry failed (message will be redelivered)"
                    );
                }
            }
            Err(e) => {
                error!(
                    queue_type = ?queue_type,
                    error = %e,
                    batch_size = chunk.len(),
                    "Batch delete API call failed (messages will be redelivered)"
                );
            }
        }
    }

    deleted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_concurrency_for_queue() {
        // Test that concurrency is retrieved (exact value depends on env)
        let concurrency = get_concurrency_for_queue(QueueType::TransactionRequest);
        assert!(concurrency > 0);

        let concurrency = get_concurrency_for_queue(QueueType::StatusCheck);
        assert!(concurrency > 0);
    }

    #[test]
    fn test_handler_timeout_secs_is_positive() {
        let all = [
            QueueType::TransactionRequest,
            QueueType::TransactionSubmission,
            QueueType::StatusCheck,
            QueueType::StatusCheckEvm,
            QueueType::StatusCheckStellar,
            QueueType::Notification,
            QueueType::TokenSwapRequest,
            QueueType::RelayerHealthCheck,
        ];
        for queue_type in all {
            assert!(handler_timeout_secs(queue_type) > 0);
        }
    }

    #[test]
    fn test_handler_timeout_secs_uses_visibility_timeout() {
        assert_eq!(
            handler_timeout_secs(QueueType::StatusCheckEvm),
            QueueType::StatusCheckEvm.visibility_timeout_secs() as u64
        );
        assert_eq!(
            handler_timeout_secs(QueueType::Notification),
            QueueType::Notification.visibility_timeout_secs() as u64
        );
    }

    #[test]
    fn test_parse_target_scheduled_on() {
        // Test parsing target_scheduled_on from message attributes
        let message = Message::builder().build();

        // Message without attribute should return None
        assert_eq!(parse_target_scheduled_on(&message), None);

        // Message with valid attribute
        let message = Message::builder()
            .message_attributes(
                "target_scheduled_on",
                MessageAttributeValue::builder()
                    .data_type("Number")
                    .string_value("1234567890")
                    .build()
                    .unwrap(),
            )
            .build();

        assert_eq!(parse_target_scheduled_on(&message), Some(1234567890));
    }

    #[test]
    fn test_parse_retry_attempt() {
        let message = Message::builder().build();
        assert_eq!(parse_retry_attempt(&message), None);

        let message = Message::builder()
            .message_attributes(
                "retry_attempt",
                MessageAttributeValue::builder()
                    .data_type("Number")
                    .string_value("7")
                    .build()
                    .unwrap(),
            )
            .build();
        assert_eq!(parse_retry_attempt(&message), Some(7));
    }

    #[test]
    fn test_map_handler_error() {
        // Test Abort maps to Permanent
        let error = HandlerError::Abort("Validation failed".to_string());
        let result = map_handler_error(error);
        assert!(matches!(result, ProcessingError::Permanent(_)));

        // Test Retry maps to Retryable
        let error = HandlerError::Retry("Network timeout".to_string());
        let result = map_handler_error(error);
        assert!(matches!(result, ProcessingError::Retryable(_)));
    }

    #[test]
    fn test_is_fifo_queue_url() {
        assert!(is_fifo_queue_url(
            "https://sqs.us-east-1.amazonaws.com/123/queue.fifo"
        ));
        assert!(!is_fifo_queue_url(
            "https://sqs.us-east-1.amazonaws.com/123/queue"
        ));
    }

    #[test]
    fn test_compute_status_retry_delay_evm() {
        // NetworkType uses #[serde(rename_all = "lowercase")]
        let body = r#"{"message_id":"m1","version":"1","timestamp":"0","job_type":"TransactionStatusCheck","data":{"transaction_id":"tx1","relayer_id":"r1","network_type":"evm"}}"#;
        assert_eq!(compute_status_retry_delay(body, 0), 8);
        assert_eq!(compute_status_retry_delay(body, 1), 12);
        assert_eq!(compute_status_retry_delay(body, 8), 12);
    }

    #[test]
    fn test_compute_status_retry_delay_stellar() {
        let body = r#"{"message_id":"m1","version":"1","timestamp":"0","job_type":"TransactionStatusCheck","data":{"transaction_id":"tx1","relayer_id":"r1","network_type":"stellar"}}"#;
        assert_eq!(compute_status_retry_delay(body, 0), 2);
        assert_eq!(compute_status_retry_delay(body, 1), 3);
        assert_eq!(compute_status_retry_delay(body, 8), 3);
    }

    #[test]
    fn test_compute_status_retry_delay_solana() {
        let body = r#"{"message_id":"m1","version":"1","timestamp":"0","job_type":"TransactionStatusCheck","data":{"transaction_id":"tx1","relayer_id":"r1","network_type":"solana"}}"#;
        assert_eq!(compute_status_retry_delay(body, 0), 5);
        assert_eq!(compute_status_retry_delay(body, 1), 8);
        assert_eq!(compute_status_retry_delay(body, 8), 8);
    }

    #[test]
    fn test_compute_status_retry_delay_missing_network() {
        let body = r#"{"message_id":"m1","version":"1","timestamp":"0","job_type":"TransactionStatusCheck","data":{"transaction_id":"tx1","relayer_id":"r1"}}"#;
        assert_eq!(compute_status_retry_delay(body, 0), 5);
        assert_eq!(compute_status_retry_delay(body, 1), 8);
        assert_eq!(compute_status_retry_delay(body, 8), 8);
    }

    #[test]
    fn test_compute_status_retry_delay_invalid_body() {
        assert_eq!(compute_status_retry_delay("not json", 0), 5);
        assert_eq!(compute_status_retry_delay("not json", 1), 8);
        assert_eq!(compute_status_retry_delay("not json", 8), 8);
    }

    #[tokio::test]
    async fn test_semaphore_released_on_panic() {
        let sem = Arc::new(tokio::sync::Semaphore::new(1));
        let permit = sem.clone().acquire_owned().await.unwrap();

        let handle = tokio::spawn(async move {
            let _permit = permit; // dropped on scope exit, even after panic
            let _ = AssertUnwindSafe(async { panic!("test panic") })
                .catch_unwind()
                .await;
        });

        handle.await.unwrap();
        // Would hang forever if permit leaked
        let _p = tokio::time::timeout(Duration::from_millis(100), sem.acquire())
            .await
            .expect("permit should be available after panic");
    }

    #[test]
    fn test_poll_error_backoff_secs() {
        // First error: 5s
        assert_eq!(poll_error_backoff_secs(1), 5);
        // Second: 10s
        assert_eq!(poll_error_backoff_secs(2), 10);
        // Third: 20s
        assert_eq!(poll_error_backoff_secs(3), 20);
        // Fourth: 40s
        assert_eq!(poll_error_backoff_secs(4), 40);
        // Capped at MAX_POLL_BACKOFF_SECS (60)
        assert_eq!(poll_error_backoff_secs(5), 60);
        assert_eq!(poll_error_backoff_secs(6), 60);
        assert_eq!(poll_error_backoff_secs(7), 60);
        // Recovery probe: base interval at multiples of RECOVERY_PROBE_EVERY (>= 7)
        assert_eq!(poll_error_backoff_secs(8), 5);
        assert_eq!(poll_error_backoff_secs(9), 60);
        assert_eq!(poll_error_backoff_secs(12), 5); // next probe
    }

    #[test]
    fn test_poll_error_backoff_zero_errors() {
        // Zero consecutive errors should still produce a reasonable value
        assert_eq!(poll_error_backoff_secs(0), 5);
    }

    #[test]
    fn test_poll_error_backoff_recovery_probes() {
        // Verify probes repeat at regular intervals once past threshold
        for i in (8..=100).step_by(RECOVERY_PROBE_EVERY as usize) {
            assert_eq!(
                poll_error_backoff_secs(i as u32),
                5,
                "Expected recovery probe at error {i}"
            );
        }
    }

    #[test]
    fn test_message_outcome_delete_carries_receipt_handle() {
        let handle = "test-receipt-handle-123".to_string();
        let outcome = MessageOutcome::Delete {
            receipt_handle: handle.clone(),
        };
        match outcome {
            MessageOutcome::Delete { receipt_handle } => {
                assert_eq!(receipt_handle, handle);
            }
            MessageOutcome::Retain => panic!("Expected Delete variant"),
        }
    }

    #[test]
    fn test_message_outcome_retain() {
        let outcome = MessageOutcome::Retain;
        assert!(matches!(outcome, MessageOutcome::Retain));
    }

    #[test]
    fn test_batch_delete_entry_builder() {
        // Verify DeleteMessageBatchRequestEntry builds correctly with sequential IDs,
        // matching the pattern used in flush_delete_batch.
        let handles = vec![
            "receipt-0".to_string(),
            "receipt-1".to_string(),
            "receipt-2".to_string(),
        ];
        let entries: Vec<DeleteMessageBatchRequestEntry> = handles
            .iter()
            .enumerate()
            .map(|(i, handle)| {
                DeleteMessageBatchRequestEntry::builder()
                    .id(i.to_string())
                    .receipt_handle(handle)
                    .build()
                    .expect("id and receipt_handle are set")
            })
            .collect();

        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].id(), "0");
        assert_eq!(entries[0].receipt_handle(), "receipt-0");
        assert_eq!(entries[2].id(), "2");
        assert_eq!(entries[2].receipt_handle(), "receipt-2");
    }

    #[test]
    fn test_batch_chunking_logic() {
        // Verify that chunks(10) correctly splits receipt handles,
        // matching the pattern used in flush_delete_batch.
        let handles: Vec<String> = (0..25).map(|i| format!("receipt-{i}")).collect();
        let chunks: Vec<&[String]> = handles.chunks(10).collect();

        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].len(), 10);
        assert_eq!(chunks[1].len(), 10);
        assert_eq!(chunks[2].len(), 5);
    }

    #[test]
    fn test_outcome_collection_pattern() {
        // Verify the pattern used in the main loop to collect receipt handles
        // from a mix of Delete and Retain outcomes.
        let outcomes = vec![
            Some("receipt-1".to_string()), // Delete
            None,                          // Retain
            Some("receipt-2".to_string()), // Delete
            None,                          // Retain
            Some("receipt-3".to_string()), // Delete
        ];

        let pending_deletes: Vec<String> = outcomes.into_iter().flatten().collect();

        assert_eq!(pending_deletes.len(), 3);
        assert_eq!(pending_deletes[0], "receipt-1");
        assert_eq!(pending_deletes[1], "receipt-2");
        assert_eq!(pending_deletes[2], "receipt-3");
    }

    // ── parse_target_scheduled_on: edge cases ─────────────────────────

    #[test]
    fn test_parse_target_scheduled_on_non_numeric_string() {
        let message = Message::builder()
            .message_attributes(
                "target_scheduled_on",
                MessageAttributeValue::builder()
                    .data_type("String")
                    .string_value("not-a-number")
                    .build()
                    .unwrap(),
            )
            .build();
        assert_eq!(parse_target_scheduled_on(&message), None);
    }

    #[test]
    fn test_parse_target_scheduled_on_empty_string() {
        let message = Message::builder()
            .message_attributes(
                "target_scheduled_on",
                MessageAttributeValue::builder()
                    .data_type("Number")
                    .string_value("")
                    .build()
                    .unwrap(),
            )
            .build();
        assert_eq!(parse_target_scheduled_on(&message), None);
    }

    #[test]
    fn test_parse_target_scheduled_on_negative_value() {
        let message = Message::builder()
            .message_attributes(
                "target_scheduled_on",
                MessageAttributeValue::builder()
                    .data_type("Number")
                    .string_value("-1000")
                    .build()
                    .unwrap(),
            )
            .build();
        // Negative values parse fine as i64
        assert_eq!(parse_target_scheduled_on(&message), Some(-1000));
    }

    #[test]
    fn test_parse_target_scheduled_on_float_string() {
        let message = Message::builder()
            .message_attributes(
                "target_scheduled_on",
                MessageAttributeValue::builder()
                    .data_type("Number")
                    .string_value("1234567890.5")
                    .build()
                    .unwrap(),
            )
            .build();
        // Floats can't parse as i64
        assert_eq!(parse_target_scheduled_on(&message), None);
    }

    #[test]
    fn test_parse_target_scheduled_on_zero() {
        let message = Message::builder()
            .message_attributes(
                "target_scheduled_on",
                MessageAttributeValue::builder()
                    .data_type("Number")
                    .string_value("0")
                    .build()
                    .unwrap(),
            )
            .build();
        assert_eq!(parse_target_scheduled_on(&message), Some(0));
    }

    #[test]
    fn test_parse_target_scheduled_on_wrong_attribute_name() {
        // Attribute exists but under a different key
        let message = Message::builder()
            .message_attributes(
                "wrong_key",
                MessageAttributeValue::builder()
                    .data_type("Number")
                    .string_value("1234567890")
                    .build()
                    .unwrap(),
            )
            .build();
        assert_eq!(parse_target_scheduled_on(&message), None);
    }

    // ── parse_retry_attempt: edge cases ───────────────────────────────

    #[test]
    fn test_parse_retry_attempt_non_numeric_string() {
        let message = Message::builder()
            .message_attributes(
                "retry_attempt",
                MessageAttributeValue::builder()
                    .data_type("String")
                    .string_value("abc")
                    .build()
                    .unwrap(),
            )
            .build();
        assert_eq!(parse_retry_attempt(&message), None);
    }

    #[test]
    fn test_parse_retry_attempt_negative_value() {
        let message = Message::builder()
            .message_attributes(
                "retry_attempt",
                MessageAttributeValue::builder()
                    .data_type("Number")
                    .string_value("-1")
                    .build()
                    .unwrap(),
            )
            .build();
        // Negative values can't parse as usize
        assert_eq!(parse_retry_attempt(&message), None);
    }

    #[test]
    fn test_parse_retry_attempt_zero() {
        let message = Message::builder()
            .message_attributes(
                "retry_attempt",
                MessageAttributeValue::builder()
                    .data_type("Number")
                    .string_value("0")
                    .build()
                    .unwrap(),
            )
            .build();
        assert_eq!(parse_retry_attempt(&message), Some(0));
    }

    #[test]
    fn test_parse_retry_attempt_large_value() {
        let message = Message::builder()
            .message_attributes(
                "retry_attempt",
                MessageAttributeValue::builder()
                    .data_type("Number")
                    .string_value("999999")
                    .build()
                    .unwrap(),
            )
            .build();
        assert_eq!(parse_retry_attempt(&message), Some(999999));
    }

    // ── is_fifo_queue_url: comprehensive cases ────────────────────────

    #[test]
    fn test_is_fifo_queue_url_empty_string() {
        assert!(!is_fifo_queue_url(""));
    }

    #[test]
    fn test_is_fifo_queue_url_just_fifo_suffix() {
        assert!(is_fifo_queue_url("my-queue.fifo"));
    }

    #[test]
    fn test_is_fifo_queue_url_fifo_in_middle() {
        // .fifo appearing in the path but not as suffix
        assert!(!is_fifo_queue_url(
            "https://sqs.us-east-1.amazonaws.com/123/.fifo/queue"
        ));
    }

    #[test]
    fn test_is_fifo_queue_url_case_sensitive() {
        assert!(!is_fifo_queue_url(
            "https://sqs.us-east-1.amazonaws.com/123/queue.FIFO"
        ));
        assert!(!is_fifo_queue_url(
            "https://sqs.us-east-1.amazonaws.com/123/queue.Fifo"
        ));
    }

    #[test]
    fn test_is_fifo_queue_url_standard_queue_variations() {
        assert!(!is_fifo_queue_url(
            "https://sqs.us-east-1.amazonaws.com/123456789/my-queue"
        ));
        assert!(!is_fifo_queue_url(
            "https://sqs.eu-west-1.amazonaws.com/123456789/relayer-tx-request"
        ));
        assert!(!is_fifo_queue_url(
            "http://localhost:4566/000000000000/test-queue"
        ));
    }

    #[test]
    fn test_is_fifo_queue_url_localstack() {
        // LocalStack FIFO queue URL format
        assert!(is_fifo_queue_url(
            "http://localhost:4566/000000000000/test-queue.fifo"
        ));
    }

    // ── map_handler_error: message preservation ───────────────────────

    #[test]
    fn test_map_handler_error_preserves_abort_message() {
        let msg = "Validation failed: invalid nonce";
        let error = HandlerError::Abort(msg.to_string());
        match map_handler_error(error) {
            ProcessingError::Permanent(s) => assert_eq!(s, msg),
            ProcessingError::Retryable(_) => panic!("Expected Permanent"),
        }
    }

    #[test]
    fn test_map_handler_error_preserves_retry_message() {
        let msg = "RPC timeout after 30s";
        let error = HandlerError::Retry(msg.to_string());
        match map_handler_error(error) {
            ProcessingError::Retryable(s) => assert_eq!(s, msg),
            ProcessingError::Permanent(_) => panic!("Expected Retryable"),
        }
    }

    #[test]
    fn test_map_handler_error_empty_message() {
        let error = HandlerError::Abort(String::new());
        match map_handler_error(error) {
            ProcessingError::Permanent(s) => assert!(s.is_empty()),
            ProcessingError::Retryable(_) => panic!("Expected Permanent"),
        }
    }

    // ── handler_timeout_secs: all queue types ─────────────────────────

    #[test]
    fn test_handler_timeout_secs_matches_visibility_timeout_for_all_queues() {
        let all = [
            QueueType::TransactionRequest,
            QueueType::TransactionSubmission,
            QueueType::StatusCheck,
            QueueType::StatusCheckEvm,
            QueueType::StatusCheckStellar,
            QueueType::Notification,
            QueueType::TokenSwapRequest,
            QueueType::RelayerHealthCheck,
        ];
        for qt in all {
            assert_eq!(
                handler_timeout_secs(qt),
                qt.visibility_timeout_secs().max(1) as u64,
                "{qt:?}: handler timeout should equal max(visibility_timeout, 1)"
            );
        }
    }

    // ── get_concurrency_for_queue: all queue types ────────────────────

    #[test]
    fn test_get_concurrency_for_queue_all_types_positive() {
        let all = [
            QueueType::TransactionRequest,
            QueueType::TransactionSubmission,
            QueueType::StatusCheck,
            QueueType::StatusCheckEvm,
            QueueType::StatusCheckStellar,
            QueueType::Notification,
            QueueType::TokenSwapRequest,
            QueueType::RelayerHealthCheck,
        ];
        for qt in all {
            assert!(
                get_concurrency_for_queue(qt) > 0,
                "{qt:?}: concurrency must be positive (clamped to at least 1)"
            );
        }
    }

    // ── poll_error_backoff_secs: overflow and invariants ───────────────

    #[test]
    fn test_poll_error_backoff_never_exceeds_max() {
        for i in 0..200 {
            let backoff = poll_error_backoff_secs(i);
            assert!(
                backoff <= MAX_POLL_BACKOFF_SECS,
                "Error count {i}: backoff {backoff}s exceeds MAX {MAX_POLL_BACKOFF_SECS}s"
            );
        }
    }

    #[test]
    fn test_poll_error_backoff_u32_max_does_not_overflow() {
        let backoff = poll_error_backoff_secs(u32::MAX);
        assert!(backoff <= MAX_POLL_BACKOFF_SECS);
        assert!(backoff > 0);
    }

    #[test]
    fn test_poll_error_backoff_always_positive() {
        for i in 0..200 {
            assert!(
                poll_error_backoff_secs(i) > 0,
                "Error count {i}: backoff must be positive"
            );
        }
    }

    #[test]
    fn test_poll_error_backoff_monotonic_before_cap() {
        // Before hitting the cap, backoff should be non-decreasing
        let mut prev = poll_error_backoff_secs(0);
        for i in 1..=4 {
            let curr = poll_error_backoff_secs(i);
            assert!(
                curr >= prev,
                "Backoff should be non-decreasing before cap: {prev} -> {curr} at error {i}"
            );
            prev = curr;
        }
    }

    // ── Constants validation ──────────────────────────────────────────

    #[test]
    fn test_max_poll_backoff_is_reasonable() {
        assert!(
            MAX_POLL_BACKOFF_SECS >= 10,
            "Max backoff should be at least 10s to avoid tight error loops"
        );
        assert!(
            MAX_POLL_BACKOFF_SECS <= 300,
            "Max backoff should be at most 5 minutes to detect recovery promptly"
        );
    }

    #[test]
    fn test_recovery_probe_every_is_valid() {
        assert!(
            RECOVERY_PROBE_EVERY >= 2,
            "Recovery probe interval must be at least 2 to avoid probing every attempt"
        );
        assert!(
            RECOVERY_PROBE_EVERY <= 10,
            "Recovery probe interval should not be too large or recovery detection is slow"
        );
    }

    // ── compute_status_retry_delay: edge cases ────────────────────────

    #[test]
    fn test_compute_status_retry_delay_very_high_attempt() {
        let body = r#"{"message_id":"m1","version":"1","timestamp":"0","job_type":"TransactionStatusCheck","data":{"transaction_id":"tx1","relayer_id":"r1","network_type":"evm"}}"#;
        // Very high attempts should stay capped at the max (12s for EVM)
        assert_eq!(compute_status_retry_delay(body, 1000), 12);
        assert_eq!(compute_status_retry_delay(body, usize::MAX), 12);
    }

    #[test]
    fn test_compute_status_retry_delay_empty_body() {
        // Empty JSON body should fall back to generic/Solana defaults
        assert_eq!(compute_status_retry_delay("", 0), 5);
        assert_eq!(compute_status_retry_delay("{}", 0), 5);
    }

    #[test]
    fn test_compute_status_retry_delay_partial_json() {
        // JSON with missing inner structure
        assert_eq!(compute_status_retry_delay(r#"{"data":{}}"#, 0), 5);
        assert_eq!(
            compute_status_retry_delay(r#"{"data":{"network_type":"evm"}}"#, 0),
            8
        );
    }

    // ── PartialStatusCheckJob deserialization ──────────────────────────

    #[test]
    fn test_partial_status_check_job_deserializes_network_type() {
        let body = r#"{"data":{"network_type":"evm","extra_field":"ignored"}}"#;
        let parsed: PartialStatusCheckJob = serde_json::from_str(body).unwrap();
        assert_eq!(
            parsed.data.network_type,
            Some(crate::models::NetworkType::Evm)
        );
    }

    #[test]
    fn test_partial_status_check_job_handles_missing_network_type() {
        let body = r#"{"data":{"transaction_id":"tx1"}}"#;
        let parsed: PartialStatusCheckJob = serde_json::from_str(body).unwrap();
        assert_eq!(parsed.data.network_type, None);
    }

    #[test]
    fn test_partial_status_check_job_rejects_missing_data() {
        let body = r#"{"not_data":{}}"#;
        let result = serde_json::from_str::<PartialStatusCheckJob>(body);
        assert!(result.is_err());
    }

    // ── is_fifo_queue_url used consistently ───────────────────────────

    #[test]
    fn test_fifo_detection_consistent_with_defer_and_retry_logic() {
        // Both defer_message and the retry path in process_message use
        // is_fifo_queue_url to decide between visibility-timeout vs re-enqueue.
        // Verify our standard and FIFO URLs are classified identically by both
        // call sites (they both call the same function).
        let standard = "https://sqs.us-east-1.amazonaws.com/123/relayer-status-check";
        let fifo = "https://sqs.us-east-1.amazonaws.com/123/relayer-status-check.fifo";

        assert!(!is_fifo_queue_url(standard));
        assert!(is_fifo_queue_url(fifo));
    }
}
