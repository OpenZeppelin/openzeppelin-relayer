//! SQS worker implementation for polling and processing messages.
//!
//! This module provides worker tasks that poll SQS queues and process jobs
//! using the existing handler functions.

use std::sync::Arc;
use std::time::Duration;

use actix_web::web::ThinData;
use aws_sdk_sqs::types::{Message, MessageAttributeValue, MessageSystemAttributeName};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::{
    config::ServerConfig,
    constants::{
        DEFAULT_CONCURRENCY_NOTIFICATION, DEFAULT_CONCURRENCY_STATUS_CHECKER_STELLAR,
        DEFAULT_CONCURRENCY_TRANSACTION_REQUEST, DEFAULT_CONCURRENCY_TRANSACTION_SENDER,
    },
    jobs::{
        notification_handler, transaction_request_handler_sqs, transaction_status_handler_sqs,
        transaction_submission_handler, Job, NotificationSend, TransactionRequest, TransactionSend,
        TransactionStatusCheck,
    },
    models::DefaultAppState,
};
use apalis::prelude::{Attempt, Data, Error as ApalisError, TaskId};

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
    app_state: Arc<ThinData<DefaultAppState>>,
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
                                let permit = semaphore.clone().acquire_owned().await.unwrap();
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
    app_state: Arc<ThinData<DefaultAppState>>,
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
        QueueType::StellarTransactionRequest => {
            process_transaction_request(body, app_state, attempt_number).await
        }
        QueueType::StellarTransactionSubmission => {
            process_transaction_submission(body, app_state, attempt_number).await
        }
        QueueType::StellarStatusCheck => {
            process_transaction_status_check(body, app_state, attempt_number).await
        }
        QueueType::StellarNotification => {
            process_notification(body, app_state, attempt_number).await
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
            if max_retries != usize::MAX && attempt_number >= max_retries {
                error!(
                    queue_type = ?queue_type,
                    attempt = attempt_number,
                    receive_count = receive_count,
                    max_retries = max_retries,
                    error = %e,
                    "Max retries exceeded; leaving message for SQS redrive policy / DLQ"
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
            // after visibility timeout expires and can be redriven to DLQ.
            Ok(())
        }
    }
}

/// Processes a TransactionRequest job.
///
/// transaction_request_handler signature: 6 args (includes Worker and RedisContext)
async fn process_transaction_request(
    body: &str,
    app_state: Arc<ThinData<DefaultAppState>>,
    attempt: usize,
) -> Result<(), ProcessingError> {
    let job: Job<TransactionRequest> = serde_json::from_str(body).map_err(|e| {
        error!(error = %e, "Failed to deserialize TransactionRequest job");
        ProcessingError::Retryable(format!("Failed to deserialize TransactionRequest job: {e}"))
    })?;

    transaction_request_handler_sqs(
        job,
        Data::new((*app_state).clone()),
        Attempt::new_with_value(attempt),
        TaskId::new(),
    )
    .await
    .map_err(map_apalis_error)
}

/// Processes a TransactionSend job.
///
/// transaction_submission_handler signature: 4 args (no Worker, no RedisContext)
async fn process_transaction_submission(
    body: &str,
    app_state: Arc<ThinData<DefaultAppState>>,
    attempt: usize,
) -> Result<(), ProcessingError> {
    let job: Job<TransactionSend> = serde_json::from_str(body).map_err(|e| {
        error!(error = %e, "Failed to deserialize TransactionSend job");
        ProcessingError::Retryable(format!("Failed to deserialize TransactionSend job: {e}"))
    })?;

    transaction_submission_handler(
        job,
        Data::new((*app_state).clone()),
        Attempt::new_with_value(attempt),
        TaskId::new(),
    )
    .await
    .map_err(map_apalis_error)
}

/// Processes a TransactionStatusCheck job.
///
/// transaction_status_handler signature: 5 args (no Worker, includes RedisContext)
async fn process_transaction_status_check(
    body: &str,
    app_state: Arc<ThinData<DefaultAppState>>,
    attempt: usize,
) -> Result<(), ProcessingError> {
    let job: Job<TransactionStatusCheck> = serde_json::from_str(body).map_err(|e| {
        error!(error = %e, "Failed to deserialize TransactionStatusCheck job");
        ProcessingError::Retryable(format!(
            "Failed to deserialize TransactionStatusCheck job: {e}"
        ))
    })?;

    transaction_status_handler_sqs(
        job,
        Data::new((*app_state).clone()),
        Attempt::new_with_value(attempt),
        TaskId::new(),
    )
    .await
    .map_err(map_apalis_error)
}

/// Processes a NotificationSend job.
///
/// notification_handler signature: 4 args (no Worker, no RedisContext)
async fn process_notification(
    body: &str,
    app_state: Arc<ThinData<DefaultAppState>>,
    attempt: usize,
) -> Result<(), ProcessingError> {
    let job: Job<NotificationSend> = serde_json::from_str(body).map_err(|e| {
        error!(error = %e, "Failed to deserialize NotificationSend job");
        ProcessingError::Retryable(format!("Failed to deserialize NotificationSend job: {e}"))
    })?;

    notification_handler(
        job,
        Data::new((*app_state).clone()),
        Attempt::new_with_value(attempt),
        TaskId::new(),
    )
    .await
    .map_err(map_apalis_error)
}

fn map_apalis_error(error: ApalisError) -> ProcessingError {
    match error {
        ApalisError::Abort(err) => ProcessingError::Permanent(err.to_string()),
        ApalisError::Failed(err) => ProcessingError::Retryable(err.to_string()),
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

async fn defer_message(
    sqs_client: &aws_sdk_sqs::Client,
    queue_url: &str,
    body: String,
    message: &Message,
    target_scheduled_on: i64,
    delay_seconds: i32,
) -> Result<(), QueueBackendError> {
    let message_group_id = extract_group_id(message)
        .unwrap_or_else(|| format!("deferred-{}", Uuid::new_v4().as_simple()));

    sqs_client
        .send_message()
        .queue_url(queue_url)
        .message_body(body)
        .message_group_id(message_group_id)
        .message_deduplication_id(Uuid::new_v4().to_string())
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
        QueueType::StellarTransactionRequest => ServerConfig::get_worker_concurrency(
            "transaction_request",
            DEFAULT_CONCURRENCY_TRANSACTION_REQUEST,
        ),
        QueueType::StellarTransactionSubmission => ServerConfig::get_worker_concurrency(
            "transaction_sender",
            DEFAULT_CONCURRENCY_TRANSACTION_SENDER,
        ),
        QueueType::StellarStatusCheck => ServerConfig::get_worker_concurrency(
            "transaction_status_checker_stellar",
            DEFAULT_CONCURRENCY_STATUS_CHECKER_STELLAR,
        ),
        QueueType::StellarNotification => ServerConfig::get_worker_concurrency(
            "notification_sender",
            DEFAULT_CONCURRENCY_NOTIFICATION,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_concurrency_for_queue() {
        // Test that concurrency is retrieved (exact value depends on env)
        let concurrency = get_concurrency_for_queue(QueueType::StellarTransactionRequest);
        assert!(concurrency > 0);

        let concurrency = get_concurrency_for_queue(QueueType::StellarStatusCheck);
        assert!(concurrency > 0);
    }
}
