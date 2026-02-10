//! SQS worker implementation for polling and processing messages.
//!
//! This module provides worker tasks that poll SQS queues and process jobs
//! using the existing handler functions.

use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;

use actix_web::web::ThinData;
use aws_sdk_sqs::types::{Message, MessageAttributeValue, MessageSystemAttributeName};
use futures::FutureExt;
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
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

/// Spawns a worker task for a specific SQS queue.
///
/// The worker continuously polls the queue, processes messages, and handles
/// retries via SQS visibility timeout.
///
/// # Arguments
/// * `sqs_client` - AWS SQS client
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

    info!(
        queue_type = ?queue_type,
        queue_url = %queue_url,
        concurrency = concurrency,
        max_retries = max_retries,
        polling_interval_secs = polling_interval,
        visibility_timeout_secs = visibility_timeout,
        "Spawning SQS worker"
    );

    let handle: JoinHandle<()> = tokio::spawn(async move {
        let semaphore = Arc::new(tokio::sync::Semaphore::new(concurrency));
        let mut inflight = JoinSet::new();
        let mut consecutive_poll_errors: u32 = 0;

        loop {
            // Reap completed tasks each iteration (prevents unbounded memory)
            while let Some(result) = inflight.try_join_next() {
                if let Err(e) = result {
                    warn!(
                        queue_type = ?queue_type,
                        error = %e,
                        "In-flight task failed"
                    );
                }
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
                    .wait_time_seconds(polling_interval as i32) // Long polling
                    .visibility_timeout(visibility_timeout as i32)
                    .message_system_attribute_names(MessageSystemAttributeName::All)
                    .message_attribute_names("All")
                    .send() => result,
                _ = shutdown_rx.changed() => {
                    info!(queue_type = ?queue_type, "Shutdown signal received during SQS poll, stopping worker");
                    break;
                }
            };

            match messages_result {
                Ok(output) => {
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

                                    let result = AssertUnwindSafe(process_message(
                                        client.clone(),
                                        message,
                                        queue_type,
                                        &url,
                                        state,
                                        max_retries,
                                    ))
                                    .catch_unwind()
                                    .await;

                                    match result {
                                        Ok(Ok(())) => {}
                                        Ok(Err(e)) => {
                                            error!(
                                                queue_type = ?queue_type,
                                                error = %e,
                                                "Failed to process message"
                                            );
                                        }
                                        Err(panic_info) => {
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
                    error!(
                        queue_type = ?queue_type,
                        error = %e,
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

        // Drain in-flight tasks before shutdown
        if !inflight.is_empty() {
            info!(
                queue_type = ?queue_type,
                count = inflight.len(),
                "Draining in-flight tasks before shutdown"
            );
            match tokio::time::timeout(Duration::from_secs(30), async {
                while let Some(result) = inflight.join_next().await {
                    if let Err(e) = result {
                        warn!(
                            queue_type = ?queue_type,
                            error = %e,
                            "In-flight task failed during drain"
                        );
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
) -> Result<(), QueueBackendError> {
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

            if should_delete_original {
                sqs_client
                    .delete_message()
                    .queue_url(queue_url)
                    .receipt_handle(receipt_handle)
                    .send()
                    .await
                    .map_err(|e| {
                        QueueBackendError::SqsError(format!(
                            "Deferred message but failed to delete original: {e}"
                        ))
                    })?;
            }

            debug!(
                queue_type = ?queue_type,
                remaining_seconds = remaining,
                "Deferred scheduled SQS message for next delay hop"
            );
            return Ok(());
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
            // Success: Delete message from queue
            sqs_client
                .delete_message()
                .queue_url(queue_url)
                .receipt_handle(receipt_handle)
                .send()
                .await
                .map_err(|e| {
                    error!(error = %e, "Failed to delete message from SQS");
                    QueueBackendError::SqsError(format!("DeleteMessage failed: {e}"))
                })?;

            debug!(
                queue_type = ?queue_type,
                attempt = attempt_number,
                "Message processed successfully and deleted"
            );

            Ok(())
        }
        Err(ProcessingError::Permanent(e)) => {
            // Permanent failure: delete immediately (do not retry).
            sqs_client
                .delete_message()
                .queue_url(queue_url)
                .receipt_handle(receipt_handle)
                .send()
                .await
                .map_err(|err| {
                    error!(error = %err, "Failed to delete permanently failed message from SQS");
                    QueueBackendError::SqsError(format!("DeleteMessage failed: {err}"))
                })?;

            error!(
                queue_type = ?queue_type,
                attempt = attempt_number,
                error = %e,
                "Permanent handler failure, message deleted"
            );

            Ok(())
        }
        Err(ProcessingError::Retryable(e)) => {
            // StatusCheck queues use self-re-enqueue with short delay instead of
            // relying on the 300s visibility timeout. This brings retry intervals
            // from ~5 minutes down to 3-10 seconds, matching Apalis backoff behavior.
            if queue_type.is_status_check() {
                let delay = compute_status_retry_delay(body, logical_retry_attempt);
                let requeue_dedup_id = generate_requeue_dedup_id(
                    body,
                    receive_count,
                    message.message_id().unwrap_or("unknown"),
                );
                let group_id = match extract_group_id(&message) {
                    Some(id) => id,
                    None => {
                        error!(
                            queue_type = ?queue_type,
                            attempt = logical_retry_attempt,
                            "Cannot re-enqueue status check: missing MessageGroupId"
                        );
                        // Keep original message for visibility-timeout retry path.
                        return Ok(());
                    }
                };
                let next_retry_attempt = logical_retry_attempt.saturating_add(1);

                // Re-enqueue with short delay
                if let Err(send_err) = sqs_client
                    .send_message()
                    .queue_url(queue_url)
                    .message_body(body.to_string())
                    .message_group_id(group_id)
                    .message_deduplication_id(requeue_dedup_id)
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
                    return Ok(());
                }

                // Delete the original message now that the re-enqueue succeeded
                sqs_client
                    .delete_message()
                    .queue_url(queue_url)
                    .receipt_handle(receipt_handle)
                    .send()
                    .await
                    .map_err(|err| {
                        // Not fatal: worst case is a duplicate delivery after
                        // the visibility timeout expires, which the handler is
                        // idempotent to handle.
                        error!(error = %err, "Failed to delete original status check message after re-enqueue");
                        QueueBackendError::SqsError(format!("DeleteMessage failed: {err}"))
                    })?;

                debug!(
                    queue_type = ?queue_type,
                    attempt = logical_retry_attempt,
                    delay_seconds = delay,
                    error = %e,
                    "Status check re-enqueued with short delay"
                );

                return Ok(());
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
            Ok(())
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

fn extract_group_id(message: &Message) -> Option<String> {
    message
        .attributes()
        .and_then(|attrs| attrs.get(&MessageSystemAttributeName::MessageGroupId))
        .cloned()
}

/// Generates a deterministic deduplication ID for deferred messages.
///
/// The dedup ID is deterministic for a single receive-attempt (idempotent retry of
/// the same defer send) but changes across receive attempts so defer hops are not
/// accidentally dropped by FIFO deduplication windows.
fn generate_dedup_id(
    body: &str,
    target_scheduled_on: i64,
    source_message_id: &str,
    source_receive_count: usize,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(body.as_bytes());
    hasher.update(target_scheduled_on.to_le_bytes());
    hasher.update(source_message_id.as_bytes());
    hasher.update(source_receive_count.to_le_bytes());
    let hash = format!("{:x}", hasher.finalize());
    // SQS deduplication ID max length is 128 chars, use first 64 for safety
    hash[..64.min(hash.len())].to_string()
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

    // CRITICAL: Preserve original MessageGroupId to maintain FIFO ordering.
    // If we create a new group ID, messages from the same transaction will
    // be processed out of order, breaking transaction semantics.
    let message_group_id = extract_group_id(message).ok_or_else(|| {
        QueueBackendError::QueueError(
            "Cannot defer message: missing MessageGroupId for FIFO queue".to_string(),
        )
    })?;

    let source_message_id = message
        .message_id()
        .ok_or_else(|| {
            QueueBackendError::QueueError(
                "Cannot defer message: missing MessageId from SQS message".to_string(),
            )
        })?
        .to_string();
    let source_receive_count = message
        .attributes()
        .and_then(|attrs| attrs.get(&MessageSystemAttributeName::ApproximateReceiveCount))
        .and_then(|count| count.parse::<usize>().ok())
        .unwrap_or(1);

    // Deterministic per receive-attempt for idempotent retries of send_message,
    // but unique across defer hops (receive_count changes).
    let dedup_id = generate_dedup_id(
        &body,
        target_scheduled_on,
        &source_message_id,
        source_receive_count,
    );

    sqs_client
        .send_message()
        .queue_url(queue_url)
        .message_body(body)
        .message_group_id(message_group_id)
        .message_deduplication_id(dedup_id)
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
        )
        .send()
        .await
        .map_err(|e| {
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

/// Generates a unique deduplication ID for re-enqueued status check messages.
///
/// Must be unique per retry hop to avoid the 5-minute FIFO dedup window
/// silently dropping the re-enqueued message.
fn generate_requeue_dedup_id(body: &str, receive_count: usize, message_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"status-requeue:");
    hasher.update(body.as_bytes());
    hasher.update(message_id.as_bytes());
    hasher.update(receive_count.to_le_bytes());
    let hash = format!("{:x}", hasher.finalize());
    hash[..64.min(hash.len())].to_string()
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

/// Maximum backoff duration for poll errors (5 minutes).
const MAX_POLL_BACKOFF_SECS: u64 = 300;

/// Computes exponential backoff for consecutive poll errors.
///
/// Returns: 5, 10, 20, 40, 80, 160, 300, 300, ... seconds
fn poll_error_backoff_secs(consecutive_errors: u32) -> u64 {
    let base: u64 = 5;
    let exponent = consecutive_errors.saturating_sub(1).min(16);
    base.saturating_mul(2_u64.saturating_pow(exponent))
        .min(MAX_POLL_BACKOFF_SECS)
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
    fn test_generate_dedup_id_deterministic() {
        // Same inputs should always produce same dedup ID
        let body = r#"{"data":{"transaction_id":"tx-123"}}"#;
        let timestamp = 1234567890i64;
        let message_id = "msg-123";
        let receive_count = 2usize;

        let dedup_id_1 = generate_dedup_id(body, timestamp, message_id, receive_count);
        let dedup_id_2 = generate_dedup_id(body, timestamp, message_id, receive_count);

        assert_eq!(dedup_id_1, dedup_id_2, "Dedup ID should be deterministic");
    }

    #[test]
    fn test_generate_dedup_id_different_inputs() {
        // Different inputs should produce different dedup IDs
        let body1 = r#"{"data":{"transaction_id":"tx-123"}}"#;
        let body2 = r#"{"data":{"transaction_id":"tx-456"}}"#;
        let timestamp = 1234567890i64;
        let message_id = "msg-123";
        let receive_count = 2usize;

        let dedup_id_1 = generate_dedup_id(body1, timestamp, message_id, receive_count);
        let dedup_id_2 = generate_dedup_id(body2, timestamp, message_id, receive_count);

        assert_ne!(
            dedup_id_1, dedup_id_2,
            "Different bodies should produce different dedup IDs"
        );

        // Same body, different timestamp
        let dedup_id_3 = generate_dedup_id(body1, timestamp + 1000, message_id, receive_count);
        assert_ne!(
            dedup_id_1, dedup_id_3,
            "Different timestamps should produce different dedup IDs"
        );

        // Same body/timestamp, different source receive_count (new defer hop)
        // should produce a different dedup ID to avoid accidental hop drops.
        let dedup_id_4 = generate_dedup_id(body1, timestamp, message_id, receive_count + 1);
        assert_ne!(
            dedup_id_1, dedup_id_4,
            "Different receive counts should produce different dedup IDs"
        );
    }

    #[test]
    fn test_generate_dedup_id_length() {
        // Dedup ID should be valid length for SQS (max 128 chars)
        let body = r#"{"data":{"transaction_id":"tx-123"}}"#;
        let timestamp = 1234567890i64;
        let message_id = "msg-123";
        let receive_count = 2usize;

        let dedup_id = generate_dedup_id(body, timestamp, message_id, receive_count);

        assert!(
            dedup_id.len() <= 128,
            "Dedup ID length {} exceeds SQS limit of 128",
            dedup_id.len()
        );
        assert!(
            dedup_id.len() >= 32,
            "Dedup ID length {} is too short",
            dedup_id.len()
        );
    }

    #[test]
    fn test_generate_dedup_id_valid_chars() {
        // Dedup ID should only contain valid hex chars
        let body = r#"{"data":{"transaction_id":"tx-123"}}"#;
        let timestamp = 1234567890i64;
        let message_id = "msg-123";
        let receive_count = 2usize;

        let dedup_id = generate_dedup_id(body, timestamp, message_id, receive_count);

        assert!(
            dedup_id.chars().all(|c| c.is_ascii_hexdigit()),
            "Dedup ID should only contain hex digits"
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
    fn test_extract_group_id() {
        // Test extracting MessageGroupId from message attributes
        let message = Message::builder().build();

        // Message without group ID should return None
        assert_eq!(extract_group_id(&message), None);

        // Note: Setting MessageGroupId via attributes requires the actual SQS response format
        // This is more of an integration test, so we just verify the None case
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

    #[test]
    fn test_generate_requeue_dedup_id_deterministic() {
        let body = r#"{"data":{"transaction_id":"tx-123"}}"#;
        let id1 = generate_requeue_dedup_id(body, 1, "msg-1");
        let id2 = generate_requeue_dedup_id(body, 1, "msg-1");
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_generate_requeue_dedup_id_varies_with_receive_count() {
        let body = r#"{"data":{"transaction_id":"tx-123"}}"#;
        let id1 = generate_requeue_dedup_id(body, 1, "msg-1");
        let id2 = generate_requeue_dedup_id(body, 2, "msg-1");
        assert_ne!(
            id1, id2,
            "Different receive counts must produce different dedup IDs"
        );
    }

    #[test]
    fn test_generate_requeue_dedup_id_length() {
        let body = r#"{"data":{"transaction_id":"tx-123"}}"#;
        let id = generate_requeue_dedup_id(body, 1, "msg-1");
        assert!(id.len() <= 128, "Dedup ID must fit SQS 128-char limit");
        assert!(id.len() >= 32);
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
        // Fifth: 80s
        assert_eq!(poll_error_backoff_secs(5), 80);
        // Sixth: 160s
        assert_eq!(poll_error_backoff_secs(6), 160);
        // Seventh: capped at 300s
        assert_eq!(poll_error_backoff_secs(7), 300);
        // Many errors: still capped at 300s
        assert_eq!(poll_error_backoff_secs(100), 300);
    }

    #[test]
    fn test_poll_error_backoff_zero_errors() {
        // Zero consecutive errors should still produce a reasonable value
        assert_eq!(poll_error_backoff_secs(0), 5);
    }
}
