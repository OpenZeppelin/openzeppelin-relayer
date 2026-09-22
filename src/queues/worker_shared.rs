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
    constants::DEFAULT_EVM_STATUS_CHECK_RETRY_DELAY_SECONDS,
    jobs::{
        notification_handler, relayer_health_check_handler, token_swap_request_handler,
        transaction_request_handler, transaction_status_handler, transaction_submission_handler,
        Job, NotificationSend, RelayerHealthCheck, TokenSwapRequest, TransactionRequest,
        TransactionSend, TransactionStatusCheck,
    },
    models::{DefaultAppState, NetworkType},
};

use super::{
    worker_types::{NotYetFinal, RetryKind},
    HandlerError, QueueType, WorkerContext,
};

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
    Retryable { message: String, kind: RetryKind },
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
    Retryable { message: String, kind: RetryKind },
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
        Ok(Ok(Err(ProcessingError::Retryable { message, kind }))) => {
            HandlerOutcome::Retryable { message, kind }
        }
        Ok(Err(_panic)) => HandlerOutcome::Retryable {
            message: "handler panicked".to_string(),
            kind: RetryKind::Other,
        },
        Err(_elapsed) => HandlerOutcome::Retryable {
            message: format!(
                "handler exceeded the {}s timeout",
                HANDLER_TIMEOUT.as_secs()
            ),
            kind: RetryKind::Other,
        },
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
        HandlerError::Retry(message) => ProcessingError::Retryable {
            message,
            kind: RetryKind::Other,
        },
        HandlerError::NotYetFinal(status) => ProcessingError::Retryable {
            message: NotYetFinal { status }.to_string(),
            kind: RetryKind::NotYetFinal,
        },
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

/// Partial view of the status-check fields needed for retry delay selection.
#[derive(serde::Deserialize)]
struct StatusCheckData {
    network_type: Option<crate::models::NetworkType>,
    /// Missing on messages queued before the field existed.
    #[serde(default)]
    status_check_retry_delay_seconds: Option<u64>,
}

#[derive(serde::Deserialize)]
struct PartialStatusCheckJob {
    data: StatusCheckData,
}

fn parse_status_check_retry_data(body: &[u8]) -> (Option<NetworkType>, Option<u64>) {
    serde_json::from_slice::<PartialStatusCheckJob>(body)
        .map(|job| {
            (
                job.data.network_type,
                job.data.status_check_retry_delay_seconds,
            )
        })
        .unwrap_or_default()
}

/// Selects the retry delay (seconds) for a failed job: network-aware backoff for
/// status-check queues, the queue's configured backoff otherwise.
pub(crate) fn retry_delay_for_queue(
    queue_type: QueueType,
    body: &[u8],
    retry_attempt: usize,
    retry_kind: RetryKind,
) -> i32 {
    if queue_type.is_status_check() {
        let (network_type, delay) = parse_status_check_retry_data(body);
        if queue_type == QueueType::StatusCheckEvm && retry_kind == RetryKind::NotYetFinal {
            // Healthy EVM checks back off from the network's retry delay.
            let delay = delay.unwrap_or(DEFAULT_EVM_STATUS_CHECK_RETRY_DELAY_SECONDS);
            return crate::queues::retry_delay_secs(
                crate::queues::evm_status_check_backoff(delay),
                retry_attempt,
            );
        }
        crate::queues::status_check_retry_delay_secs(network_type, retry_attempt)
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
    use crate::models::TransactionStatus;

    /// Production routing for a generic status-check body (no configured interval).
    fn status_delay(body: &[u8], attempt: usize) -> i32 {
        retry_delay_for_queue(QueueType::StatusCheck, body, attempt, RetryKind::Other)
    }

    #[test]
    fn test_map_handler_error() {
        assert!(matches!(
            map_handler_error(HandlerError::Abort("x".into())),
            ProcessingError::Permanent(m) if m == "x"
        ));
        assert!(matches!(
            map_handler_error(HandlerError::Retry("y".into())),
            ProcessingError::Retryable { message, .. } if message == "y"
        ));
        assert!(matches!(
            map_handler_error(HandlerError::Abort(String::new())),
            ProcessingError::Permanent(m) if m.is_empty()
        ));
        assert!(matches!(
            map_handler_error(HandlerError::NotYetFinal(TransactionStatus::Submitted)),
            ProcessingError::Retryable {
                kind: RetryKind::NotYetFinal,
                ..
            }
        ));
    }

    #[test]
    fn test_status_retry_delay_by_network() {
        let evm = br#"{"message_id":"m","version":"1","timestamp":"0","job_type":"TransactionStatusCheck","data":{"transaction_id":"t","relayer_id":"r","network_type":"evm"}}"#;
        assert_eq!(status_delay(evm, 0), 8);
        assert_eq!(status_delay(evm, 1), 12);

        let stellar = br#"{"message_id":"m","version":"1","timestamp":"0","job_type":"TransactionStatusCheck","data":{"transaction_id":"t","relayer_id":"r","network_type":"stellar"}}"#;
        assert_eq!(status_delay(stellar, 0), 2);

        // Missing network → generic (Solana/default) profile.
        let none = br#"{"message_id":"m","version":"1","timestamp":"0","job_type":"TransactionStatusCheck","data":{"transaction_id":"t","relayer_id":"r"}}"#;
        assert_eq!(status_delay(none, 0), 5);

        // Invalid body → generic fallback.
        assert_eq!(status_delay(b"not json", 0), 5);
    }

    #[test]
    fn test_status_retry_delay_malformed_bodies() {
        assert_eq!(status_delay(b"", 0), 5);
        assert_eq!(status_delay(b"{}", 0), 5);
        assert_eq!(status_delay(br#"{"data":{}}"#, 0), 5);
        assert_eq!(status_delay(br#"{"data":{"network_type":"evm"}}"#, 0), 8);
        assert_eq!(status_delay(br#"{"not_data":{}}"#, 0), 5);
        let evm = br#"{"message_id":"m","version":"1","timestamp":"0","job_type":"TransactionStatusCheck","data":{"transaction_id":"t","relayer_id":"r","network_type":"evm"}}"#;
        assert_eq!(status_delay(evm, 1000), 12);
        assert_eq!(status_delay(evm, usize::MAX), 12);
    }

    #[test]
    fn test_retry_delay_for_queue_routes_status_vs_bounded() {
        // Status-check queue uses the network-aware status delay.
        let evm = br#"{"message_id":"m","version":"1","timestamp":"0","job_type":"TransactionStatusCheck","data":{"transaction_id":"t","relayer_id":"r","network_type":"evm"}}"#;
        assert_eq!(
            retry_delay_for_queue(QueueType::StatusCheckEvm, evm, 0, RetryKind::Other),
            status_delay(evm, 0)
        );
        // Bounded queue uses its configured backoff (body ignored).
        assert_eq!(
            retry_delay_for_queue(QueueType::TransactionRequest, b"{}", 0, RetryKind::Other),
            crate::queues::retry_delay_secs(
                crate::queues::backoff_config_for_queue(QueueType::TransactionRequest),
                0
            )
        );
    }

    #[test]
    fn test_healthy_evm_check_backs_off_from_configured_delay() {
        let fast_evm = br#"{"data":{"network_type":"evm","status_check_retry_delay_seconds":5}}"#;
        let healthy = |body: &[u8], attempt| {
            retry_delay_for_queue(
                QueueType::StatusCheckEvm,
                body,
                attempt,
                RetryKind::NotYetFinal,
            )
        };

        // 5s -> capped at 1.5x (7.5s, rounded up).
        assert_eq!(healthy(fast_evm, 0), 5);
        assert_eq!(healthy(fast_evm, 1), 8);
        assert_eq!(healthy(fast_evm, 1000), 8);

        // Failed checks keep the stock 8->12s backoff regardless of the payload.
        assert_eq!(
            retry_delay_for_queue(QueueType::StatusCheckEvm, fast_evm, 0, RetryKind::Other),
            8
        );
        // Only the EVM status queue honours the payload.
        assert_eq!(
            retry_delay_for_queue(QueueType::StatusCheck, fast_evm, 0, RetryKind::NotYetFinal),
            8
        );

        // Old payloads without the field behave exactly like main.
        let legacy = br#"{"data":{"network_type":"evm"}}"#;
        assert_eq!(healthy(legacy, 0), 8);
        assert_eq!(healthy(legacy, 1), 12);

        let max_delay =
            br#"{"data":{"network_type":"evm","status_check_retry_delay_seconds":100}}"#;
        assert_eq!(healthy(max_delay, 0), 100);
        assert_eq!(healthy(max_delay, 1), 150);
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
            let d = status_delay(evm, attempt);
            assert!(d >= prev, "status backoff must be non-decreasing");
            assert!(d <= 12, "status backoff must stay <= cap");
            prev = d;
        }
        // It actually increases at least once before capping.
        assert!(status_delay(evm, 1) > status_delay(evm, 0));
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
