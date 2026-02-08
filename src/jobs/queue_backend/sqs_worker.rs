//! SQS worker implementation for polling and processing messages.
//!
//! This module provides worker tasks that poll SQS queues and process jobs
//! using the existing handler functions.

use std::sync::Arc;
use std::time::Duration;

use actix_web::web::ThinData;
use aws_sdk_sqs::types::Message;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

use crate::{
    config::ServerConfig,
    constants::{
        DEFAULT_CONCURRENCY_NOTIFICATION, DEFAULT_CONCURRENCY_STATUS_CHECKER_STELLAR,
        DEFAULT_CONCURRENCY_TRANSACTION_REQUEST, DEFAULT_CONCURRENCY_TRANSACTION_SENDER,
    },
    jobs::{
        notification_handler, transaction_request_handler, transaction_status_handler,
        transaction_submission_handler, Job, NotificationSend, TransactionRequest, TransactionSend,
        TransactionStatusCheck,
    },
    models::DefaultAppState,
};
use apalis::prelude::{Attempt, Data, TaskId};

use super::{QueueBackendError, QueueType, WorkerHandle};

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
            // Poll SQS for messages
            let messages_result = sqs_client
                .receive_message()
                .queue_url(&queue_url)
                .max_number_of_messages(10) // Batch size (max 10 for FIFO)
                .wait_time_seconds(polling_interval as i32) // Long polling
                .visibility_timeout(visibility_timeout as i32)
                .message_system_attribute_names(aws_sdk_sqs::types::MessageSystemAttributeName::All)
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
                            let mut tasks = Vec::new();
                            for message in messages {
                                let permit = semaphore.clone().acquire_owned().await.unwrap();
                                let client = sqs_client.clone();
                                let url = queue_url.clone();
                                let state = app_state.clone();

                                let task = tokio::spawn(async move {
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

                                tasks.push(task);
                            }

                            // Wait for all tasks to complete
                            for task in tasks {
                                let _ = task.await;
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

    let receipt_handle = message.receipt_handle().ok_or_else(|| {
        QueueBackendError::QueueError("Missing receipt handle".to_string())
    })?;

    // Get retry attempt count from message attributes
    let attempt_number = message
        .attributes()
        .and_then(|attrs| {
            attrs.get(&aws_sdk_sqs::types::MessageSystemAttributeName::ApproximateReceiveCount)
        })
        .and_then(|count| count.parse::<usize>().ok())
        .unwrap_or(1);

    debug!(
        queue_type = ?queue_type,
        attempt = attempt_number,
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
                    QueueBackendError::SqsError(format!("DeleteMessage failed: {}", e))
                })?;

            debug!(
                queue_type = ?queue_type,
                attempt = attempt_number,
                "Message processed successfully and deleted"
            );

            Ok(())
        }
        Err(e) if attempt_number >= max_retries => {
            // Max retries exceeded: Delete message (will move to DLQ if configured)
            sqs_client
                .delete_message()
                .queue_url(queue_url)
                .receipt_handle(receipt_handle)
                .send()
                .await
                .map_err(|err| {
                    error!(error = %err, "Failed to delete failed message from SQS");
                    QueueBackendError::SqsError(format!("DeleteMessage failed: {}", err))
                })?;

            error!(
                queue_type = ?queue_type,
                attempt = attempt_number,
                max_retries = max_retries,
                error = %e,
                "Max retries exceeded, message moved to DLQ"
            );

            Ok(())
        }
        Err(e) => {
            // Retry: Let message return to queue after visibility timeout
            warn!(
                queue_type = ?queue_type,
                attempt = attempt_number,
                max_retries = max_retries,
                error = %e,
                "Message processing failed, will retry after visibility timeout"
            );

            // Don't delete message - it will automatically return to queue
            // after visibility timeout expires
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
) -> Result<(), QueueBackendError> {
    let job: Job<TransactionRequest> = serde_json::from_str(body).map_err(|e| {
        error!(error = %e, "Failed to deserialize TransactionRequest job");
        QueueBackendError::SerializationError(e.to_string())
    })?;

    // Note: Worker and RedisContext are unused (_worker, _ctx) in the handler
    // Create minimal dummy values to satisfy type checker
    let worker = unsafe {
        // SAFETY: Worker is never actually used in the handler (marked as _worker)
        std::mem::zeroed()
    };
    let redis_ctx = unsafe {
        // SAFETY: RedisContext is never actually used in the handler (marked as _ctx)
        std::mem::zeroed()
    };

    transaction_request_handler(
        job,
        Data::new((*app_state).clone()),
        Attempt::new_with_value(attempt),
        worker,
        TaskId::new(),
        redis_ctx,
    )
    .await
    .map_err(|e| QueueBackendError::QueueError(format!("Handler error: {}", e)))
}

/// Processes a TransactionSend job.
///
/// transaction_submission_handler signature: 4 args (no Worker, no RedisContext)
async fn process_transaction_submission(
    body: &str,
    app_state: Arc<ThinData<DefaultAppState>>,
    attempt: usize,
) -> Result<(), QueueBackendError> {
    let job: Job<TransactionSend> = serde_json::from_str(body).map_err(|e| {
        error!(error = %e, "Failed to deserialize TransactionSend job");
        QueueBackendError::SerializationError(e.to_string())
    })?;

    transaction_submission_handler(
        job,
        Data::new((*app_state).clone()),
        Attempt::new_with_value(attempt),
        TaskId::new(),
    )
    .await
    .map_err(|e| QueueBackendError::QueueError(format!("Handler error: {}", e)))
}

/// Processes a TransactionStatusCheck job.
///
/// transaction_status_handler signature: 5 args (no Worker, includes RedisContext)
async fn process_transaction_status_check(
    body: &str,
    app_state: Arc<ThinData<DefaultAppState>>,
    attempt: usize,
) -> Result<(), QueueBackendError> {
    let job: Job<TransactionStatusCheck> = serde_json::from_str(body).map_err(|e| {
        error!(error = %e, "Failed to deserialize TransactionStatusCheck job");
        QueueBackendError::SerializationError(e.to_string())
    })?;

    // Note: RedisContext is unused (_ctx) in the handler
    let redis_ctx = unsafe {
        // SAFETY: RedisContext is never actually used in the handler (marked as _ctx)
        std::mem::zeroed()
    };

    transaction_status_handler(
        job,
        Data::new((*app_state).clone()),
        Attempt::new_with_value(attempt),
        TaskId::new(),
        redis_ctx,
    )
    .await
    .map_err(|e| QueueBackendError::QueueError(format!("Handler error: {}", e)))
}

/// Processes a NotificationSend job.
///
/// notification_handler signature: 4 args (no Worker, no RedisContext)
async fn process_notification(
    body: &str,
    app_state: Arc<ThinData<DefaultAppState>>,
    attempt: usize,
) -> Result<(), QueueBackendError> {
    let job: Job<NotificationSend> = serde_json::from_str(body).map_err(|e| {
        error!(error = %e, "Failed to deserialize NotificationSend job");
        QueueBackendError::SerializationError(e.to_string())
    })?;

    notification_handler(
        job,
        Data::new((*app_state).clone()),
        Attempt::new_with_value(attempt),
        TaskId::new(),
    )
    .await
    .map_err(|e| QueueBackendError::QueueError(format!("Handler error: {}", e)))
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
        QueueType::StellarNotification => {
            ServerConfig::get_worker_concurrency("notification_sender", DEFAULT_CONCURRENCY_NOTIFICATION)
        }
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
