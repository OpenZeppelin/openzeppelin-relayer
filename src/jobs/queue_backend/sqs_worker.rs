//! SQS worker implementation for polling and processing messages.
//!
//! This module provides worker tasks that poll SQS queues and process jobs
//! using the existing handler functions.

use std::sync::Arc;
use std::time::Duration;

use actix_web::web::ThinData;
use aws_sdk_sqs::types::{Message, MessageAttributeValue, MessageSystemAttributeName};
use sha2::{Digest, Sha256};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

use crate::{
    config::ServerConfig,
    constants::{
        DEFAULT_CONCURRENCY_HEALTH_CHECK, DEFAULT_CONCURRENCY_NOTIFICATION,
        DEFAULT_CONCURRENCY_STATUS_CHECKER_STELLAR, DEFAULT_CONCURRENCY_TOKEN_SWAP,
        DEFAULT_CONCURRENCY_TRANSACTION_REQUEST, DEFAULT_CONCURRENCY_TRANSACTION_SENDER,
    },
    jobs::{
        notification_handler, relayer_health_check_handler, token_swap_request_handler,
        transaction_request_handler_queue, transaction_status_handler_queue,
        transaction_submission_handler, Job, NotificationSend, RelayerHealthCheck,
        TokenSwapRequest, TransactionRequest, TransactionSend, TransactionStatusCheck,
    },
};

use super::{
    QueueBackendError, QueueType, QueueWorkerAttempt, QueueWorkerData, QueueWorkerError,
    QueueWorkerTaskId, WorkerHandle,
};

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

        loop {
            // Do not poll for more messages than we can process; otherwise
            // messages sit in-flight waiting for permits.
            let available_permits = semaphore.available_permits();
            if available_permits == 0 {
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }

            let batch_size = available_permits.min(10) as i32;

            // Poll SQS for messages
            let messages_result = sqs_client
                .receive_message()
                .queue_url(&queue_url)
                .max_number_of_messages(batch_size) // SQS max is 10
                .wait_time_seconds(polling_interval as i32) // Long polling
                .visibility_timeout(visibility_timeout as i32)
                .message_system_attribute_names(MessageSystemAttributeName::All)
                .message_attribute_names("All")
                .send()
                .await;

            match messages_result {
                Ok(output) => {
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

                                tokio::spawn(async move {
                                    let result = process_message(
                                        client.clone(),
                                        message,
                                        queue_type,
                                        &url,
                                        state,
                                        max_retries,
                                    )
                                    .await;

                                    if let Err(e) = result {
                                        error!(
                                            queue_type = ?queue_type,
                                            error = %e,
                                            "Failed to process message"
                                        );
                                    }

                                    drop(permit); // Release semaphore
                                });
                            }
                        }
                    }
                }
                Err(e) => {
                    error!(
                        queue_type = ?queue_type,
                        error = %e,
                        "Failed to receive messages from SQS"
                    );
                    // Back off on error
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        }
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
            defer_message(
                &sqs_client,
                queue_url,
                body.to_string(),
                &message,
                target_scheduled_on,
                remaining.min(900) as i32,
            )
            .await?;

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

    debug!(
        queue_type = ?queue_type,
        attempt = attempt_number,
        receive_count = receive_count,
        max_retries = max_retries,
        "Processing message"
    );

    // Route to appropriate handler
    let result = match queue_type {
        QueueType::TransactionRequest => {
            process_transaction_request(body, app_state, attempt_number).await
        }
        QueueType::TransactionSubmission => {
            process_transaction_submission(body, app_state, attempt_number).await
        }
        QueueType::StatusCheck => {
            process_transaction_status_check(body, app_state, attempt_number).await
        }
        QueueType::Notification => process_notification(body, app_state, attempt_number).await,
        QueueType::TokenSwapRequest => {
            process_token_swap_request(body, app_state, attempt_number).await
        }
        QueueType::RelayerHealthCheck => {
            process_relayer_health_check(body, app_state, attempt_number).await
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
            // Retry: Let message return to queue after visibility timeout
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

/// Processes a TransactionRequest job.
///
/// transaction_request_handler signature: 6 args (includes Worker and RedisContext)
async fn process_transaction_request(
    body: &str,
    app_state: Arc<ThinData<crate::models::DefaultAppState>>,
    attempt: usize,
) -> Result<(), ProcessingError> {
    let job: Job<TransactionRequest> = serde_json::from_str(body).map_err(|e| {
        error!(error = %e, "Failed to deserialize TransactionRequest job");
        ProcessingError::Retryable(format!("Failed to deserialize TransactionRequest job: {e}"))
    })?;

    transaction_request_handler_queue(
        job,
        QueueWorkerData::new((*app_state).clone()),
        QueueWorkerAttempt::new_with_value(attempt),
        QueueWorkerTaskId::new(),
    )
    .await
    .map_err(map_apalis_error)
}

/// Processes a TransactionSend job.
///
/// transaction_submission_handler signature: 4 args (no Worker, no RedisContext)
async fn process_transaction_submission(
    body: &str,
    app_state: Arc<ThinData<crate::models::DefaultAppState>>,
    attempt: usize,
) -> Result<(), ProcessingError> {
    let job: Job<TransactionSend> = serde_json::from_str(body).map_err(|e| {
        error!(error = %e, "Failed to deserialize TransactionSend job");
        ProcessingError::Retryable(format!("Failed to deserialize TransactionSend job: {e}"))
    })?;

    transaction_submission_handler(
        job,
        QueueWorkerData::new((*app_state).clone()),
        QueueWorkerAttempt::new_with_value(attempt),
        QueueWorkerTaskId::new(),
    )
    .await
    .map_err(map_apalis_error)
}

/// Processes a TransactionStatusCheck job.
///
/// transaction_status_handler signature: 5 args (no Worker, includes RedisContext)
async fn process_transaction_status_check(
    body: &str,
    app_state: Arc<ThinData<crate::models::DefaultAppState>>,
    attempt: usize,
) -> Result<(), ProcessingError> {
    let job: Job<TransactionStatusCheck> = serde_json::from_str(body).map_err(|e| {
        error!(error = %e, "Failed to deserialize TransactionStatusCheck job");
        ProcessingError::Retryable(format!(
            "Failed to deserialize TransactionStatusCheck job: {e}"
        ))
    })?;

    transaction_status_handler_queue(
        job,
        QueueWorkerData::new((*app_state).clone()),
        QueueWorkerAttempt::new_with_value(attempt),
        QueueWorkerTaskId::new(),
    )
    .await
    .map_err(map_apalis_error)
}

/// Processes a NotificationSend job.
///
/// notification_handler signature: 4 args (no Worker, no RedisContext)
async fn process_notification(
    body: &str,
    app_state: Arc<ThinData<crate::models::DefaultAppState>>,
    attempt: usize,
) -> Result<(), ProcessingError> {
    let job: Job<NotificationSend> = serde_json::from_str(body).map_err(|e| {
        error!(error = %e, "Failed to deserialize NotificationSend job");
        ProcessingError::Retryable(format!("Failed to deserialize NotificationSend job: {e}"))
    })?;

    notification_handler(
        job,
        QueueWorkerData::new((*app_state).clone()),
        QueueWorkerAttempt::new_with_value(attempt),
        QueueWorkerTaskId::new(),
    )
    .await
    .map_err(map_apalis_error)
}

async fn process_token_swap_request(
    body: &str,
    app_state: Arc<ThinData<crate::models::DefaultAppState>>,
    attempt: usize,
) -> Result<(), ProcessingError> {
    let job: Job<TokenSwapRequest> = serde_json::from_str(body).map_err(|e| {
        error!(error = %e, "Failed to deserialize TokenSwapRequest job");
        ProcessingError::Retryable(format!("Failed to deserialize TokenSwapRequest job: {e}"))
    })?;

    token_swap_request_handler(
        job,
        QueueWorkerData::new((*app_state).clone()),
        QueueWorkerAttempt::new_with_value(attempt),
        QueueWorkerTaskId::new(),
    )
    .await
    .map_err(map_apalis_error)
}

async fn process_relayer_health_check(
    body: &str,
    app_state: Arc<ThinData<crate::models::DefaultAppState>>,
    attempt: usize,
) -> Result<(), ProcessingError> {
    let job: Job<RelayerHealthCheck> = serde_json::from_str(body).map_err(|e| {
        error!(error = %e, "Failed to deserialize RelayerHealthCheck job");
        ProcessingError::Retryable(format!("Failed to deserialize RelayerHealthCheck job: {e}"))
    })?;

    relayer_health_check_handler(
        job,
        QueueWorkerData::new((*app_state).clone()),
        QueueWorkerAttempt::new_with_value(attempt),
        QueueWorkerTaskId::new(),
    )
    .await
    .map_err(map_apalis_error)
}

fn map_apalis_error(error: QueueWorkerError) -> ProcessingError {
    match error {
        QueueWorkerError::Abort(err) => ProcessingError::Permanent(err.to_string()),
        QueueWorkerError::Failed(err) => ProcessingError::Retryable(err.to_string()),
        other => ProcessingError::Retryable(other.to_string()),
    }
}

fn parse_target_scheduled_on(message: &Message) -> Option<i64> {
    message
        .message_attributes()
        .and_then(|attrs| attrs.get("target_scheduled_on"))
        .and_then(|value| value.string_value())
        .and_then(|value| value.parse::<i64>().ok())
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

async fn defer_message(
    sqs_client: &aws_sdk_sqs::Client,
    queue_url: &str,
    body: String,
    message: &Message,
    target_scheduled_on: i64,
    delay_seconds: i32,
) -> Result<(), QueueBackendError> {
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

    Ok(())
}

/// Gets the concurrency limit for a queue type from environment.
fn get_concurrency_for_queue(queue_type: QueueType) -> usize {
    match queue_type {
        QueueType::TransactionRequest => ServerConfig::get_worker_concurrency(
            "transaction_request",
            DEFAULT_CONCURRENCY_TRANSACTION_REQUEST,
        ),
        QueueType::TransactionSubmission => ServerConfig::get_worker_concurrency(
            "transaction_sender",
            DEFAULT_CONCURRENCY_TRANSACTION_SENDER,
        ),
        QueueType::StatusCheck => ServerConfig::get_worker_concurrency(
            "transaction_status_checker_stellar",
            DEFAULT_CONCURRENCY_STATUS_CHECKER_STELLAR,
        ),
        QueueType::Notification => ServerConfig::get_worker_concurrency(
            "notification_sender",
            DEFAULT_CONCURRENCY_NOTIFICATION,
        ),
        QueueType::TokenSwapRequest => ServerConfig::get_worker_concurrency(
            "token_swap_request",
            DEFAULT_CONCURRENCY_TOKEN_SWAP,
        ),
        QueueType::RelayerHealthCheck => ServerConfig::get_worker_concurrency(
            "relayer_health_check",
            DEFAULT_CONCURRENCY_HEALTH_CHECK,
        ),
    }
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
    fn test_extract_group_id() {
        // Test extracting MessageGroupId from message attributes
        let message = Message::builder().build();

        // Message without group ID should return None
        assert_eq!(extract_group_id(&message), None);

        // Note: Setting MessageGroupId via attributes requires the actual SQS response format
        // This is more of an integration test, so we just verify the None case
    }

    #[test]
    fn test_map_apalis_error() {
        // Test error type mapping
        use std::io;

        // Test Abort maps to Permanent
        let error = QueueWorkerError::Abort(Arc::new(Box::new(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Validation failed",
        ))));
        let result = map_apalis_error(error);
        assert!(matches!(result, ProcessingError::Permanent(_)));

        // Test Failed maps to Retryable
        let error = QueueWorkerError::Failed(Arc::new(Box::new(io::Error::new(
            io::ErrorKind::TimedOut,
            "Network timeout",
        ))));
        let result = map_apalis_error(error);
        assert!(matches!(result, ProcessingError::Retryable(_)));
    }
}
