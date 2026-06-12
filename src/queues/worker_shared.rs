//! Shared worker helpers for the dumb-pipe queue backends (Pub/Sub, RabbitMQ).
//!
//! These are the pure-logic, transport-agnostic pieces both workers need: the
//! `QueueType` → handler dispatch table, the 600s-timeout + `catch_unwind`
//! handler wrapper, retry-exhaustion accounting, network-aware status-check
//! backoff, correlation-id extraction, and per-queue concurrency resolution.
//!
//! The transport mechanics (Pub/Sub pull + lease, RabbitMQ consume + ack) stay
//! per-backend — there is deliberately no generic "worker" trait, since the
//! delivery models share no clean abstraction; only this shared logic is
//! factored out.

use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;

use actix_web::web::ThinData;
use futures::FutureExt;
use serde::de::DeserializeOwned;
use tracing::{error, warn};

use crate::config::ServerConfig;
use crate::{
    jobs::{
        notification_handler, relayer_health_check_handler, token_swap_request_handler,
        transaction_request_handler, transaction_status_handler, transaction_submission_handler,
        Job, NotificationSend, RelayerHealthCheck, TokenSwapRequest, TransactionRequest,
        TransactionSend, TransactionStatusCheck,
    },
    models::DefaultAppState,
};

use super::{HandlerError, QueueType, WorkerContext};

/// Handler timeout bound. Every handler is well under this; it cancels a stuck
/// handler so a hung job is reprocessed (via the bounded-retry path), never run
/// forever. For Pub/Sub this matches the 600s lease; for RabbitMQ it sits far
/// below the broker's 30-minute `consumer_timeout`.
pub(crate) const HANDLER_TIMEOUT: Duration = Duration::from_secs(600);

/// Max time to await in-flight handlers during a graceful-shutdown drain.
pub(crate) const DRAIN_TIMEOUT: Duration = Duration::from_secs(30);

/// Internal classification of a handler error.
#[derive(Debug)]
pub(crate) enum ProcessingError {
    Retryable(String),
    Permanent(String),
}

/// Normalized outcome of running a handler under the timeout + panic guard.
///
/// Panics and timeouts are folded into `Retryable` (with a descriptive reason)
/// so a consistently-failing handler on a bounded queue still hits `max_retries`
/// instead of looping forever.
#[derive(Debug)]
pub(crate) enum HandlerOutcome {
    Success,
    Permanent(String),
    Retryable(String),
}

/// Runs the queue's handler for `body` under the 600s timeout + `catch_unwind`
/// panic guard and normalizes the result into a [`HandlerOutcome`].
pub(crate) async fn run_handler_with_timeout(
    body: &[u8],
    queue_type: QueueType,
    app_state: Arc<ThinData<DefaultAppState>>,
    attempt: usize,
    task_id: String,
) -> HandlerOutcome {
    let outcome = tokio::time::timeout(
        HANDLER_TIMEOUT,
        AssertUnwindSafe(dispatch(body, queue_type, app_state, attempt, task_id)).catch_unwind(),
    )
    .await;

    match outcome {
        Ok(Ok(Ok(()))) => HandlerOutcome::Success,
        Ok(Ok(Err(ProcessingError::Permanent(e)))) => HandlerOutcome::Permanent(e),
        Ok(Ok(Err(ProcessingError::Retryable(e)))) => HandlerOutcome::Retryable(e),
        Ok(Err(_panic)) => HandlerOutcome::Retryable("handler panicked".to_string()),
        Err(_elapsed) => HandlerOutcome::Retryable(format!(
            "handler exceeded the {}s timeout",
            HANDLER_TIMEOUT.as_secs()
        )),
    }
}

/// Routes a message body to the appropriate handler based on queue type.
pub(crate) async fn dispatch(
    body: &[u8],
    queue_type: QueueType,
    app_state: Arc<ThinData<DefaultAppState>>,
    attempt: usize,
    task_id: String,
) -> Result<(), ProcessingError> {
    match queue_type {
        QueueType::TransactionRequest => {
            process_job::<TransactionRequest, _, _>(
                body,
                app_state,
                attempt,
                task_id,
                "TransactionRequest",
                transaction_request_handler,
            )
            .await
        }
        QueueType::TransactionSubmission => {
            process_job::<TransactionSend, _, _>(
                body,
                app_state,
                attempt,
                task_id,
                "TransactionSend",
                transaction_submission_handler,
            )
            .await
        }
        QueueType::StatusCheck | QueueType::StatusCheckEvm | QueueType::StatusCheckStellar => {
            process_job::<TransactionStatusCheck, _, _>(
                body,
                app_state,
                attempt,
                task_id,
                "TransactionStatusCheck",
                transaction_status_handler,
            )
            .await
        }
        QueueType::Notification => {
            process_job::<NotificationSend, _, _>(
                body,
                app_state,
                attempt,
                task_id,
                "NotificationSend",
                notification_handler,
            )
            .await
        }
        QueueType::TokenSwapRequest => {
            process_job::<TokenSwapRequest, _, _>(
                body,
                app_state,
                attempt,
                task_id,
                "TokenSwapRequest",
                token_swap_request_handler,
            )
            .await
        }
        QueueType::RelayerHealthCheck => {
            process_job::<RelayerHealthCheck, _, _>(
                body,
                app_state,
                attempt,
                task_id,
                "RelayerHealthCheck",
                relayer_health_check_handler,
            )
            .await
        }
    }
}

/// Generic job processor — deserializes `Job<T>`, builds a `WorkerContext`, and
/// delegates to the handler. A malformed payload is a permanent failure.
async fn process_job<T, F, Fut>(
    body: &[u8],
    app_state: Arc<ThinData<DefaultAppState>>,
    attempt: usize,
    task_id: String,
    type_name: &str,
    handler: F,
) -> Result<(), ProcessingError>
where
    T: DeserializeOwned,
    F: FnOnce(Job<T>, ThinData<DefaultAppState>, WorkerContext) -> Fut,
    Fut: Future<Output = Result<(), HandlerError>>,
{
    let job: Job<T> = serde_json::from_slice(body).map_err(|e| {
        error!(error = %e, "Failed to deserialize {} job", type_name);
        ProcessingError::Permanent(format!("Failed to deserialize {type_name} job: {e}"))
    })?;

    let ctx = WorkerContext::new(attempt, task_id);
    handler(job, (*app_state).clone(), ctx)
        .await
        .map_err(map_handler_error)
}

pub(crate) fn map_handler_error(error: HandlerError) -> ProcessingError {
    match error {
        HandlerError::Abort(msg) => ProcessingError::Permanent(msg),
        HandlerError::Retry(msg) => ProcessingError::Retryable(msg),
    }
}

/// Whether a retryable failure has exhausted a bounded queue's retry budget.
///
/// Status-check queues are unbounded (`max_retries == usize::MAX`) and are never
/// exhausted — they re-run until the transaction finalizes. A bounded queue is
/// exhausted once the *next* attempt would exceed `max_retries`.
pub(crate) fn is_retry_exhausted(max_retries: usize, retry_attempt: usize) -> bool {
    max_retries != usize::MAX && retry_attempt.saturating_add(1) > max_retries
}

/// Partial view of a status-check job body to extract only `network_type`.
#[derive(serde::Deserialize)]
struct StatusCheckData {
    network_type: Option<crate::models::NetworkType>,
}

#[derive(serde::Deserialize)]
struct PartialStatusCheckJob {
    data: StatusCheckData,
}

/// Network-aware retry delay for status checks (EVM 8→12s, Stellar 2→3s,
/// Solana/default 5→8s), aligned with Redis/Apalis and the SQS backend.
pub(crate) fn compute_status_retry_delay(body: &[u8], attempt: usize) -> i32 {
    let network_type = serde_json::from_slice::<PartialStatusCheckJob>(body)
        .ok()
        .and_then(|j| j.data.network_type);
    crate::queues::status_check_retry_delay_secs(network_type, attempt)
}

/// Selects the retry delay (seconds) for a failed job: network-aware backoff for
/// status-check queues, the queue's configured backoff otherwise.
pub(crate) fn retry_delay_for_queue(
    queue_type: QueueType,
    body: &[u8],
    retry_attempt: usize,
) -> i32 {
    if queue_type.is_status_check() {
        compute_status_retry_delay(body, retry_attempt)
    } else {
        crate::queues::retry_delay_secs(
            crate::queues::backoff_config_for_queue(queue_type),
            retry_attempt,
        )
    }
}

/// Partial view of any job body to extract the correlation id for log lines —
/// never logs the body itself.
#[derive(serde::Deserialize)]
struct JobMeta {
    message_id: String,
}

/// Extracts the job's stable correlation id (its `message_id`) for logging.
pub(crate) fn job_correlation_id(body: &[u8]) -> String {
    serde_json::from_slice::<JobMeta>(body)
        .map(|m| m.message_id)
        .unwrap_or_else(|_| "unknown".to_string())
}

/// Gets the concurrency limit for a queue type from env or default (reuses the
/// existing `WORKER_*_CONCURRENCY` controls). Clamps a configured 0 to 1.
pub(crate) fn get_concurrency_for_queue(queue_type: QueueType) -> usize {
    let configured = ServerConfig::get_worker_concurrency(
        queue_type.concurrency_env_key(),
        queue_type.default_concurrency(),
    );
    if configured == 0 {
        warn!(queue_type = %queue_type, "Configured concurrency is 0; clamping to 1");
        1
    } else {
        configured
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_map_handler_error() {
        assert!(matches!(
            map_handler_error(HandlerError::Abort("x".into())),
            ProcessingError::Permanent(_)
        ));
        assert!(matches!(
            map_handler_error(HandlerError::Retry("x".into())),
            ProcessingError::Retryable(_)
        ));
    }

    #[test]
    fn test_compute_status_retry_delay_by_network() {
        let evm = br#"{"message_id":"m","version":"1","timestamp":"0","job_type":"TransactionStatusCheck","data":{"transaction_id":"t","relayer_id":"r","network_type":"evm"}}"#;
        assert_eq!(compute_status_retry_delay(evm, 0), 8);
        assert_eq!(compute_status_retry_delay(evm, 1), 12);

        let stellar = br#"{"message_id":"m","version":"1","timestamp":"0","job_type":"TransactionStatusCheck","data":{"transaction_id":"t","relayer_id":"r","network_type":"stellar"}}"#;
        assert_eq!(compute_status_retry_delay(stellar, 0), 2);

        // Missing network → generic (Solana/default) profile.
        let none = br#"{"message_id":"m","version":"1","timestamp":"0","job_type":"TransactionStatusCheck","data":{"transaction_id":"t","relayer_id":"r"}}"#;
        assert_eq!(compute_status_retry_delay(none, 0), 5);

        // Invalid body → generic fallback.
        assert_eq!(compute_status_retry_delay(b"not json", 0), 5);
    }

    #[test]
    fn test_retry_delay_for_queue_routes_status_vs_bounded() {
        // Status-check queue uses the network-aware status delay.
        let evm = br#"{"message_id":"m","version":"1","timestamp":"0","job_type":"TransactionStatusCheck","data":{"transaction_id":"t","relayer_id":"r","network_type":"evm"}}"#;
        assert_eq!(
            retry_delay_for_queue(QueueType::StatusCheckEvm, evm, 0),
            compute_status_retry_delay(evm, 0)
        );
        // Bounded queue uses its configured backoff (body ignored).
        assert_eq!(
            retry_delay_for_queue(QueueType::TransactionRequest, b"{}", 0),
            crate::queues::retry_delay_secs(
                crate::queues::backoff_config_for_queue(QueueType::TransactionRequest),
                0
            )
        );
    }

    #[test]
    fn test_job_correlation_id_extraction() {
        let body = br#"{"message_id":"job-123","version":"1","timestamp":"0","job_type":"NotificationSend","data":{}}"#;
        assert_eq!(job_correlation_id(body), "job-123");
        assert_eq!(job_correlation_id(b"garbage"), "unknown");
    }

    #[test]
    fn test_get_concurrency_for_queue_positive() {
        assert!(get_concurrency_for_queue(QueueType::TransactionRequest) > 0);
        assert!(get_concurrency_for_queue(QueueType::StatusCheck) > 0);
    }

    #[test]
    fn test_handler_timeout_constants() {
        assert_eq!(HANDLER_TIMEOUT, Duration::from_secs(600));
        assert_eq!(DRAIN_TIMEOUT, Duration::from_secs(30));
    }

    #[test]
    fn test_is_retry_exhausted_bounded_queue() {
        // A queue with max_retries = 5 exhausts once the next attempt (> 5).
        // retry_attempt is the attempt that just failed; next = retry_attempt+1.
        assert!(!is_retry_exhausted(5, 0)); // next=1 <= 5
        assert!(!is_retry_exhausted(5, 4)); // next=5 <= 5
        assert!(is_retry_exhausted(5, 5)); // next=6 > 5 → exhausted (drop)
        assert!(is_retry_exhausted(5, 100));
    }

    #[test]
    fn test_is_retry_exhausted_status_checks_never_exhaust() {
        // Status-check queues are unbounded and must never be force-dropped.
        assert!(!is_retry_exhausted(usize::MAX, 0));
        assert!(!is_retry_exhausted(usize::MAX, 1_000_000));
        assert!(!is_retry_exhausted(usize::MAX, usize::MAX - 1));
    }

    #[test]
    fn test_status_retry_backoff_is_monotonic_and_capped() {
        // EVM: 8s → 12s cap. Non-decreasing and never above the cap.
        let evm = br#"{"message_id":"m","version":"1","timestamp":"0","job_type":"TransactionStatusCheck","data":{"transaction_id":"t","relayer_id":"r","network_type":"evm"}}"#;
        let mut prev = 0;
        for attempt in 0..10 {
            let d = compute_status_retry_delay(evm, attempt);
            assert!(d >= prev, "status backoff must be non-decreasing");
            assert!(d <= 12, "status backoff must stay <= cap");
            prev = d;
        }
        // It actually increases at least once before capping.
        assert!(compute_status_retry_delay(evm, 1) > compute_status_retry_delay(evm, 0));
    }

    #[test]
    fn test_bounded_queue_backoff_is_monotonic_and_capped() {
        use crate::queues::{backoff_config_for_queue, retry_delay_secs};
        let cfg = backoff_config_for_queue(QueueType::TransactionRequest);
        let cap = (cfg.max_ms.div_ceil(1000)) as i32;
        let mut prev = 0;
        for attempt in 0..12 {
            let d = retry_delay_secs(cfg, attempt);
            assert!(d >= prev, "backoff must be non-decreasing");
            assert!(d <= cap, "backoff must stay <= cap");
            prev = d;
        }
    }
}
