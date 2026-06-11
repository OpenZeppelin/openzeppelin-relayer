//! Pub/Sub worker implementation for pulling and processing messages.
//!
//! One pull-loop worker per subscription. Key properties:
//! **permit-before-pull** (a pulled message's lease isn't ticking while it
//! waits locally for a permit), a single **600s ack deadline** extended up front
//! and the handler bounded to it (no renewal loop; the extension is retried and,
//! if it can't be secured, the message is released rather than run under a
//! too-short lease), **re-enqueue-to-Redis** on retry (including panics and
//! timeouts, so bounded queues honor max_retries), **drop** on bounded
//! exhaustion, and **never ack incomplete work**.
//!
//! The transport-agnostic pieces (handler dispatch, the 600s-timeout +
//! `catch_unwind` wrapper, retry-exhaustion + backoff selection, correlation-id
//! extraction, concurrency resolution) live in `crate::queues::worker_shared`;
//! the pull/lease/ack mechanics below stay Pub/Sub-specific.

use std::sync::Arc;
use std::time::Duration;

use actix_web::web::ThinData;
use deadpool_redis::Pool;
use gcloud_pubsub::client::Client;
use gcloud_pubsub::subscriber::ReceivedMessage;
use tokio::sync::{watch, Semaphore};
use tokio::task::{JoinHandle, JoinSet};
use tracing::{debug, error, info, warn};

use crate::metrics::observe_queue_pickup_latency;
use crate::models::DefaultAppState;
use crate::queues::worker_shared::{
    get_concurrency_for_queue, is_retry_exhausted, job_correlation_id, retry_delay_for_queue,
    run_handler_with_timeout, HandlerOutcome, DRAIN_TIMEOUT,
};

use super::backend::retry_attempt_from_attrs;
use super::schedule::{zadd_scheduled, ScheduledJob};
use super::{QueueType, WorkerHandle};

/// Single 600s lease (Pub/Sub's max). Every handler is <= 60s, so this is a
/// ~10x margin and needs no renewal loop (the crate doesn't auto-extend).
const ACK_DEADLINE_SECS: i32 = 600;

/// How many times to try extending a message's ack deadline before releasing it
/// for redelivery instead of processing under a too-short lease.
const ACK_EXTEND_ATTEMPTS: usize = 3;

/// Backoff between ack-deadline extension attempts (lets a transient gRPC blip
/// clear; negligible against the subscription's default lease).
const ACK_EXTEND_BACKOFF: Duration = Duration::from_millis(100);

/// Bundles per-worker parameters threaded through the pull loop.
#[derive(Clone)]
struct WorkerConfig {
    queue_type: QueueType,
    max_retries: usize,
    idle_wait: Duration,
    key_prefix: String,
}

/// Spawns a pull-loop worker for one subscription.
///
/// The worker pulls messages (sized to available concurrency permits), extends
/// each message's lease to 600s, runs the handler under a 600s timeout, then
/// acks / re-enqueues-for-retry / leaves un-acked. Fully interruptible by
/// `shutdown_rx`; drains in-flight handlers on shutdown.
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_worker_for_subscription(
    client: Client,
    queue_type: QueueType,
    subscription_name: String,
    app_state: Arc<ThinData<DefaultAppState>>,
    redis_pool: Arc<Pool>,
    key_prefix: String,
    shutdown_rx: watch::Receiver<bool>,
) -> WorkerHandle {
    let concurrency = get_concurrency_for_queue(queue_type);
    let config = WorkerConfig {
        queue_type,
        max_retries: queue_type.max_retries(),
        idle_wait: Duration::from_secs(queue_type.default_wait_time_secs()),
        key_prefix,
    };

    info!(
        queue_type = %queue_type,
        subscription = %subscription_name,
        concurrency = concurrency,
        max_retries = config.max_retries,
        ack_deadline_secs = ACK_DEADLINE_SECS,
        "Spawning Pub/Sub worker"
    );

    let handle: JoinHandle<()> = tokio::spawn(async move {
        let subscription = client.subscription(&subscription_name);
        let semaphore = Arc::new(Semaphore::new(concurrency));
        run_pull_loop(
            subscription,
            semaphore,
            app_state,
            redis_pool,
            config,
            shutdown_rx,
        )
        .await;
        info!(queue_type = %queue_type, "Pub/Sub worker stopped");
    });

    WorkerHandle::Tokio(handle)
}

async fn run_pull_loop(
    subscription: gcloud_pubsub::subscription::Subscription,
    semaphore: Arc<Semaphore>,
    app_state: Arc<ThinData<DefaultAppState>>,
    redis_pool: Arc<Pool>,
    config: WorkerConfig,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let queue_type = config.queue_type;
    let mut inflight: JoinSet<()> = JoinSet::new();

    loop {
        // Reap finished handlers so the JoinSet doesn't grow unbounded.
        while inflight.try_join_next().is_some() {}

        if *shutdown_rx.borrow() {
            break;
        }

        // Permit-before-pull: only pull as many as we can process now, so a
        // pulled message's lease is not ticking while it waits for a permit.
        let available = semaphore.available_permits();
        if available == 0 {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(50)) => continue,
                _ = shutdown_rx.changed() => break,
            }
        }

        let pull_result = tokio::select! {
            r = subscription.pull(available as i32, None) => r,
            _ = shutdown_rx.changed() => break,
        };

        match pull_result {
            Ok(messages) => {
                if messages.is_empty() {
                    // Idle: back off, but stay responsive to shutdown.
                    tokio::select! {
                        _ = tokio::time::sleep(config.idle_wait) => {}
                        _ = shutdown_rx.changed() => break,
                    }
                    continue;
                }

                for message in messages {
                    let permit = match semaphore.clone().acquire_owned().await {
                        Ok(p) => p,
                        Err(_) => {
                            error!(queue_type = %queue_type, "Semaphore closed, stopping worker");
                            return;
                        }
                    };
                    let state = app_state.clone();
                    let pool = redis_pool.clone();
                    let cfg = config.clone();
                    inflight.spawn(async move {
                        let _permit = permit; // released even on panic
                        process_received_message(message, &cfg, state, &pool).await;
                    });
                }
            }
            Err(status) => {
                error!(
                    queue_type = %queue_type,
                    error = %status,
                    "Pub/Sub pull failed; backing off"
                );
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                    _ = shutdown_rx.changed() => break,
                }
            }
        }
    }

    // Graceful drain: stop pulling, let in-flight handlers finish (bounded);
    // leave any un-acked work for redelivery — never ack incomplete work.
    if !inflight.is_empty() {
        info!(
            queue_type = %queue_type,
            count = inflight.len(),
            "Draining in-flight Pub/Sub handlers before shutdown"
        );
        let drain = async { while inflight.join_next().await.is_some() {} };
        if tokio::time::timeout(DRAIN_TIMEOUT, drain).await.is_err() {
            warn!(
                queue_type = %queue_type,
                "Drain timeout; aborting remaining handlers (left un-acked for redelivery)"
            );
            inflight.abort_all();
        }
    }
}

/// Extends the lease, runs the handler under the 600s bound, and settles the
/// message (ack / re-enqueue-for-retry / leave un-acked).
async fn process_received_message(
    message: ReceivedMessage,
    config: &WorkerConfig,
    app_state: Arc<ThinData<DefaultAppState>>,
    redis_pool: &Arc<Pool>,
) {
    let queue_type = config.queue_type;
    let retry_attempt = retry_attempt_from_attrs(&message.message.attributes);
    let correlation_id = job_correlation_id(&message.message.data);

    // Secure the 600s lease BEFORE running the handler: a handler running past
    // the subscription's (short) default ack deadline would be redelivered and
    // run concurrently on another worker. If we can't extend the lease after a
    // few tries, release the message (nack) instead of processing it under a
    // deadline we know is too short — it is redelivered and retried with a fresh
    // lease. This trades a rare bounce (only on persistent extend failure) for
    // never risking a concurrent double-execution.
    if !extend_lease(&message, queue_type, &correlation_id).await {
        if let Err(e) = message.nack().await {
            warn!(
                queue_type = %queue_type,
                correlation_id = %correlation_id,
                error = %e,
                "Failed to nack after ack-deadline extension failure; relying on lease expiry"
            );
        }
        return;
    }

    // Observe pickup latency on the first delivery only (retries are republished
    // with retry_attempt > 0). publish_time ~= due time, so this excludes
    // intentional scheduling delay.
    if retry_attempt == 0 {
        if let Some(publish_secs) = message.message.publish_time.as_ref().map(|t| t.seconds) {
            let now = chrono::Utc::now().timestamp();
            observe_queue_pickup_latency(
                queue_type.queue_name(),
                "pubsub",
                (now - publish_secs).max(0) as f64,
            );
        }
    }

    debug!(
        queue_type = %queue_type,
        correlation_id = %correlation_id,
        retry_attempt = retry_attempt,
        "Processing Pub/Sub message"
    );

    match run_handler_with_timeout(
        &message.message.data,
        queue_type,
        app_state,
        retry_attempt,
        correlation_id.clone(),
    )
    .await
    {
        HandlerOutcome::Success => {
            debug!(queue_type = %queue_type, correlation_id = %correlation_id, "Handler succeeded; acking");
            ack(&message, queue_type, &correlation_id).await;
        }
        HandlerOutcome::Permanent(e) => {
            // Terminal state already persisted by the handler; drop the message.
            error!(
                queue_type = %queue_type,
                correlation_id = %correlation_id,
                error = %e,
                "Permanent handler failure; acking/dropping (terminal state persisted)"
            );
            ack(&message, queue_type, &correlation_id).await;
        }
        HandlerOutcome::Retryable(e) => {
            settle_retry(
                &message,
                config,
                redis_pool,
                retry_attempt,
                &correlation_id,
                &e,
            )
            .await;
        }
    }
}

/// Re-enqueues a retryable failure to the Redis scheduled set (scored
/// `now + backoff`) with an incremented attempt, then acks the original — or, if
/// the budget is exhausted on a bounded queue, drops it (terminal state is the
/// durable record). On re-enqueue failure the original is left un-acked.
///
/// At-least-once: the retry copy is written to Redis *before* the original
/// Pub/Sub message is acked, and the two stores are not updated atomically. A
/// crash or a failed ack in between can leave both the scheduled retry and the
/// original eligible for delivery, so a job may run more than once. This is the
/// queue subsystem's intended trade-off (favor no-loss over no-duplicates),
/// matching the Redis/Apalis and SQS backends; handlers must be idempotent.
/// Transport-level Pub/Sub message IDs are deliberately not used as a dedup
/// boundary — the meaningful idempotency boundary is the job/transaction layer
/// (e.g. nonce management), shared across all backends.
async fn settle_retry(
    message: &ReceivedMessage,
    config: &WorkerConfig,
    redis_pool: &Arc<Pool>,
    retry_attempt: usize,
    correlation_id: &str,
    err: &str,
) {
    let queue_type = config.queue_type;
    let next_attempt = retry_attempt.saturating_add(1);

    // Bounded queues stop at max_retries; status checks are unbounded (usize::MAX).
    if is_retry_exhausted(config.max_retries, retry_attempt) {
        error!(
            queue_type = %queue_type,
            correlation_id = %correlation_id,
            retry_attempt = retry_attempt,
            max_retries = config.max_retries,
            error = %err,
            "Retry budget exhausted; dropping (terminal state persisted, no dead-letter)"
        );
        ack(message, queue_type, correlation_id).await;
        return;
    }

    let delay = retry_delay_for_queue(queue_type, &message.message.data, retry_attempt);

    let body = match String::from_utf8(message.message.data.clone()) {
        Ok(b) => b,
        Err(e) => {
            // Body isn't UTF-8 JSON — unrecoverable; drop (can't re-enqueue).
            error!(
                queue_type = %queue_type,
                correlation_id = %correlation_id,
                error = %e,
                "Message body is not valid UTF-8; dropping"
            );
            ack(message, queue_type, correlation_id).await;
            return;
        }
    };

    let scheduled = ScheduledJob {
        body,
        retry_attempt: next_attempt,
    };
    let run_at = chrono::Utc::now().timestamp() + delay as i64;

    match zadd_scheduled(
        redis_pool,
        &config.key_prefix,
        queue_type,
        &scheduled,
        run_at,
    )
    .await
    {
        Ok(()) => {
            debug!(
                queue_type = %queue_type,
                correlation_id = %correlation_id,
                retry_attempt = next_attempt,
                delay_secs = delay,
                error = %err,
                "Re-enqueued for retry; acking original"
            );
            ack(message, queue_type, correlation_id).await;
        }
        Err(e) => {
            // Leave the original un-acked → Pub/Sub redelivers (no loss).
            error!(
                queue_type = %queue_type,
                correlation_id = %correlation_id,
                error = %e,
                "Failed to re-enqueue retry; leaving message un-acked for redelivery"
            );
        }
    }
}

/// Extends a message's lease to the full 600s bound, retrying a few times with a
/// short backoff. Returns `false` if every attempt fails — the caller then
/// releases the message rather than processing it under a too-short lease.
async fn extend_lease(
    message: &ReceivedMessage,
    queue_type: QueueType,
    correlation_id: &str,
) -> bool {
    for attempt in 1..=ACK_EXTEND_ATTEMPTS {
        match message.modify_ack_deadline(ACK_DEADLINE_SECS).await {
            Ok(()) => return true,
            Err(e) => {
                warn!(
                    queue_type = %queue_type,
                    correlation_id = %correlation_id,
                    attempt,
                    max_attempts = ACK_EXTEND_ATTEMPTS,
                    error = %e,
                    "Failed to extend Pub/Sub ack deadline"
                );
                if attempt < ACK_EXTEND_ATTEMPTS {
                    tokio::time::sleep(ACK_EXTEND_BACKOFF).await;
                }
            }
        }
    }
    error!(
        queue_type = %queue_type,
        correlation_id = %correlation_id,
        "Could not extend ack deadline after retries; releasing message for redelivery"
    );
    false
}

/// Acks a message (best-effort). On failure the message is redelivered and
/// reprocessed; handlers are idempotent.
async fn ack(message: &ReceivedMessage, queue_type: QueueType, correlation_id: &str) {
    if let Err(e) = message.ack().await {
        warn!(
            queue_type = %queue_type,
            correlation_id = %correlation_id,
            error = %e,
            "Failed to ack Pub/Sub message; will be redelivered (idempotent)"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lease_and_timeout_constants() {
        // 600s lease == handler bound (SQS capped model at a far looser cap).
        assert_eq!(ACK_DEADLINE_SECS, 600);
        assert_eq!(
            crate::queues::worker_shared::HANDLER_TIMEOUT,
            Duration::from_secs(600)
        );
        // The lease must be secured with at least one retry before we fall back
        // to releasing the message, and the backoff must stay well under any
        // sane subscription default ack deadline.
        assert!(ACK_EXTEND_ATTEMPTS >= 1);
        assert!(ACK_EXTEND_BACKOFF < Duration::from_secs(1));
    }
}

// ── Gated emulator integration tests ─────────────────────────────
//
// Require the Pub/Sub emulator (PUBSUB_EMULATOR_HOST) and, for the flow test, a
// running Redis on localhost:6379. Run with:
//   PUBSUB_EMULATOR_HOST=localhost:8085 \
//   cargo test --lib queues::pubsub::worker::emulator_tests -- --ignored
// The full transaction -> final-state end-to-end is validated by the
// quickstart example and the pre-release regression gate.
#[cfg(test)]
mod emulator_tests {
    use std::sync::Arc;
    use std::time::Duration;

    use deadpool_redis::Pool;
    use gcloud_pubsub::client::{Client, ClientConfig};
    use gcloud_pubsub::subscription::SubscriptionConfig;

    use super::ACK_DEADLINE_SECS;
    use crate::queues::pubsub::backend::{message_from_body, retry_attempt_from_attrs};
    use crate::queues::pubsub::schedule::{claim_due, zadd_scheduled, ScheduledJob};
    use crate::queues::QueueType;

    async fn emulator_client() -> Client {
        assert!(
            std::env::var("PUBSUB_EMULATOR_HOST").is_ok(),
            "set PUBSUB_EMULATOR_HOST to run the emulator tests"
        );
        let config = ClientConfig {
            project_id: Some("test-project".to_string()),
            ..ClientConfig::default()
        };
        Client::new(config).await.expect("emulator client")
    }

    fn redis_pool() -> Arc<Pool> {
        Arc::new(
            deadpool_redis::Config::from_url("redis://127.0.0.1:6379")
                .builder()
                .expect("pool builder")
                .max_size(8)
                .runtime(deadpool_redis::Runtime::Tokio1)
                .build()
                .expect("pool build"),
        )
    }

    /// A message whose lease is extended to 600s is NOT redelivered
    /// after the subscription's (short) default ack deadline elapses — proving
    /// a slow (<= 60s) handler is never redelivered mid-run.
    #[tokio::test]
    #[ignore]
    async fn integration_lease_held_past_default_deadline() {
        let client = emulator_client().await;
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let topic_id = format!("lease-topic-{suffix}");
        let sub_id = format!("lease-sub-{suffix}");

        let topic = client
            .create_topic(&topic_id, None, None)
            .await
            .expect("create topic");
        // 10s is the crate's minimum default ack deadline; the worker extends to 600s.
        let cfg = SubscriptionConfig {
            ack_deadline_seconds: 10,
            ..Default::default()
        };
        let subscription = client
            .create_subscription(&sub_id, &topic_id, cfg, None)
            .await
            .expect("create subscription");

        let publisher = topic.new_publisher(None);
        publisher
            .publish(message_from_body(b"{\"message_id\":\"lease\"}".to_vec(), 0))
            .await
            .get()
            .await
            .expect("publish");

        let pulled = subscription.pull(1, None).await.expect("pull");
        assert_eq!(pulled.len(), 1, "should receive the published message");
        pulled[0]
            .modify_ack_deadline(ACK_DEADLINE_SECS)
            .await
            .expect("extend lease to 600s");

        // Past the 10s default; the extended lease must still hold the message.
        tokio::time::sleep(Duration::from_secs(13)).await;
        let again = subscription.pull(1, None).await.expect("second pull");
        assert!(
            again.is_empty(),
            "extended lease must prevent mid-run redelivery"
        );

        pulled[0].ack().await.expect("ack");
    }

    /// The publish -> pull -> (retry re-enqueue) -> due-sweep -> re-pull
    /// spine, and that the topic only ever carries already-due jobs (a future
    /// scheduled job is never published until due). Requires emulator + Redis.
    #[tokio::test]
    #[ignore]
    async fn integration_flow_topic_only_carries_due_jobs() {
        let client = emulator_client().await;
        let pool = redis_pool();
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let prefix = format!("test-pubsub-{suffix}");
        let queue = QueueType::StatusCheckEvm;
        let topic_id = format!("flow-topic-{suffix}");
        let sub_id = format!("flow-sub-{suffix}");

        let topic = client
            .create_topic(&topic_id, None, None)
            .await
            .expect("create topic");
        let subscription = client
            .create_subscription(&sub_id, &topic_id, SubscriptionConfig::default(), None)
            .await
            .expect("create subscription");
        let publisher = topic.new_publisher(None);

        let now = chrono::Utc::now().timestamp();

        // A far-future job sits in Redis and is never claimed/published.
        let future = ScheduledJob {
            body: r#"{"message_id":"future"}"#.to_string(),
            retry_attempt: 0,
        };
        zadd_scheduled(&pool, &prefix, queue, &future, now + 3600)
            .await
            .expect("zadd future");
        let claimed = claim_due(&pool, &prefix, queue, now, 256)
            .await
            .expect("claim");
        assert!(claimed.is_empty(), "future job must not be due yet");

        // A due retry (retry_attempt = 2) is claimed and published; the pulled
        // message carries the incremented logical attempt.
        let due = ScheduledJob {
            body: r#"{"message_id":"due"}"#.to_string(),
            retry_attempt: 2,
        };
        zadd_scheduled(&pool, &prefix, queue, &due, now - 1)
            .await
            .expect("zadd due");
        for job in claim_due(&pool, &prefix, queue, now, 256)
            .await
            .expect("claim due")
        {
            publisher
                .publish(message_from_body(job.body.into_bytes(), job.retry_attempt))
                .await
                .get()
                .await
                .expect("publish due");
        }

        let pulled = subscription.pull(10, None).await.expect("pull");
        assert_eq!(pulled.len(), 1, "only the due job should be on the topic");
        assert_eq!(
            retry_attempt_from_attrs(&pulled[0].message.attributes),
            2,
            "logical retry_attempt must survive the schedule round-trip"
        );
        pulled[0].ack().await.expect("ack");

        // Cleanup the Redis key.
        let mut conn = pool.get().await.unwrap();
        let key = crate::queues::schedule::scheduled_set_key(&prefix, "pubsub", queue);
        let _: () = redis::cmd("DEL")
            .arg(&key)
            .query_async(&mut conn)
            .await
            .unwrap();
    }

    /// Shutdown no-loss: a message that is pulled but
    /// never acked (the worker's shutdown path: drain/abort without acking) is
    /// redelivered once its lease lapses — nothing is lost or acked-incomplete.
    #[tokio::test]
    #[ignore]
    async fn integration_unacked_work_is_redelivered_after_lease() {
        let client = emulator_client().await;
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let topic_id = format!("noloss-topic-{suffix}");
        let sub_id = format!("noloss-sub-{suffix}");

        let topic = client
            .create_topic(&topic_id, None, None)
            .await
            .expect("create topic");
        // Short lease so the test is fast; the worker would extend to 600s, but
        // here we simulate a shutdown that drops the message WITHOUT acking.
        let cfg = SubscriptionConfig {
            ack_deadline_seconds: 10,
            ..Default::default()
        };
        let subscription = client
            .create_subscription(&sub_id, &topic_id, cfg, None)
            .await
            .expect("create subscription");

        topic
            .new_publisher(None)
            .publish(message_from_body(
                b"{\"message_id\":\"noloss\"}".to_vec(),
                0,
            ))
            .await
            .get()
            .await
            .expect("publish");

        {
            let pulled = subscription.pull(1, None).await.expect("pull");
            assert_eq!(pulled.len(), 1);
            // Simulate shutdown: drop the message without acking (never ack
            // incomplete work). No ack/nack is sent.
        }

        // After the lease lapses the message becomes available again.
        tokio::time::sleep(Duration::from_secs(13)).await;
        let redelivered = subscription.pull(1, None).await.expect("second pull");
        assert_eq!(
            redelivered.len(),
            1,
            "un-acked work must be redelivered, never lost"
        );
        redelivered[0].ack().await.expect("ack");
    }
}
