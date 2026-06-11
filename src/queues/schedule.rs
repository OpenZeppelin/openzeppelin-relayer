//! Redis-backed scheduled-job store and due-sweep, shared by the dumb-pipe queue
//! backends (Pub/Sub, RabbitMQ).
//!
//! Deferred and retrying jobs live in a per-queue Redis sorted set
//! `{key_prefix}:{segment}:scheduled:{queue_name}` (member = serialized
//! [`ScheduledJob`], score = target run time in Unix seconds). A due-sweep
//! atomically claims due members (so one fleet instance publishes each) and
//! hands them to a backend-supplied publish callback — the apalis
//! store-and-run-when-due pattern. The backend transport therefore only ever
//! carries already-due jobs.
//!
//! The `segment` parameter keys each backend's scheduled sets apart on a shared
//! Redis: Pub/Sub uses `pubsub`, RabbitMQ uses `rabbitmq`. The Pub/Sub key
//! format `{key_prefix}:pubsub:scheduled:{queue}` is preserved **byte-identical**
//! by `pubsub/schedule.rs` (merged-to-main deployments may hold live jobs).

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use deadpool_redis::Pool;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

use super::{QueueBackendError, QueueType, WorkerHandle};

/// A job awaiting its due time in the Redis scheduled set.
///
/// The `body` is the JSON-serialized `Job<T>` (becomes the published message
/// body); `retry_attempt` is the logical retry counter the backend carries with
/// the published message (a Pub/Sub attribute / an AMQP `x-retry-attempt`
/// header).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ScheduledJob {
    pub body: String,
    pub retry_attempt: usize,
}

/// Redis sorted-set key holding a queue's deferred/retrying jobs for one backend
/// `segment` (`pubsub` / `rabbitmq`).
pub(crate) fn scheduled_set_key(key_prefix: &str, segment: &str, queue_type: QueueType) -> String {
    format!(
        "{key_prefix}:{segment}:scheduled:{}",
        queue_type.queue_name()
    )
}

/// Adds a job to the scheduled set, scored by its target run time (Unix secs).
///
/// The member is the serialized [`ScheduledJob`]; the score is `run_at`.
/// Re-adding a member with the same content updates its score (idempotent).
pub(crate) async fn zadd_scheduled(
    pool: &Arc<Pool>,
    key_prefix: &str,
    segment: &str,
    queue_type: QueueType,
    job: &ScheduledJob,
    run_at: i64,
) -> Result<(), QueueBackendError> {
    let key = scheduled_set_key(key_prefix, segment, queue_type);
    let member = serde_json::to_vec(job)
        .map_err(|e| QueueBackendError::SerializationError(e.to_string()))?;

    let mut conn = pool
        .get()
        .await
        .map_err(|e| QueueBackendError::RedisError(e.to_string()))?;

    let _: () = redis::cmd("ZADD")
        .arg(&key)
        .arg(run_at)
        .arg(member)
        .query_async(&mut conn)
        .await
        .map_err(|e| QueueBackendError::RedisError(e.to_string()))?;

    Ok(())
}

/// Atomically claims up to `max` due members (`score <= now`), removing them so
/// only one fleet instance publishes each.
///
/// Implemented as a single Lua script (`ZRANGEBYSCORE` + `ZREM`) so the
/// range-and-remove is one atomic step — every instance runs the sweep and the
/// atomic claim dedups (no leader election). Corrupt members are skipped (logged
/// and dropped) so one bad entry can't wedge the sweep.
pub(crate) async fn claim_due(
    pool: &Arc<Pool>,
    key_prefix: &str,
    segment: &str,
    queue_type: QueueType,
    now: i64,
    max: usize,
) -> Result<Vec<ScheduledJob>, QueueBackendError> {
    let key = scheduled_set_key(key_prefix, segment, queue_type);

    // Range due members oldest-first (bounded), then remove them in one step.
    let script = redis::Script::new(
        r#"
        local due = redis.call('ZRANGEBYSCORE', KEYS[1], '-inf', ARGV[1], 'LIMIT', 0, ARGV[2])
        if #due > 0 then
            redis.call('ZREM', KEYS[1], unpack(due))
        end
        return due
        "#,
    );

    let mut conn = pool
        .get()
        .await
        .map_err(|e| QueueBackendError::RedisError(e.to_string()))?;

    let raw: Vec<Vec<u8>> = script
        .key(&key)
        .arg(now)
        .arg(max as i64)
        .invoke_async(&mut conn)
        .await
        .map_err(|e| QueueBackendError::RedisError(e.to_string()))?;

    let mut jobs = Vec::with_capacity(raw.len());
    for bytes in raw {
        match serde_json::from_slice::<ScheduledJob>(&bytes) {
            Ok(job) => jobs.push(job),
            Err(e) => warn!(
                queue_type = %queue_type,
                error = %e,
                "Dropping corrupt scheduled-set member"
            ),
        }
    }
    Ok(jobs)
}

/// Maximum due members claimed and published per sweep tick (bounds a burst).
const DUE_SWEEP_BATCH: usize = 256;

/// Per-queue sweep cadence. Status-check queues sweep fast (~1s) to match the
/// proven apalis 2s fast-queue poll and preserve status-check latency; other
/// queues sweep coarser. This cadence is the floor on retry/schedule latency.
pub(crate) fn sweep_interval(queue_type: QueueType) -> Duration {
    if queue_type.is_status_check() {
        Duration::from_secs(1)
    } else {
        Duration::from_secs(5)
    }
}

/// Spawns the due-sweep task for one queue: every `sweep_interval`, atomically
/// claim due jobs and hand each to the backend's `publish` callback.
///
/// Every fleet instance runs the sweep; the atomic claim (`claim_due`) dedups so
/// each due job is published once. A rare double-publish is harmless (at-least-
/// once + idempotent handlers). On publish failure the job is re-queued into the
/// scheduled set scored `now` (immediate re-sweep) so a transient transport
/// error never silently drops a deferred/retrying job. Fully interruptible by
/// `shutdown_rx`.
pub(crate) fn spawn_due_sweep<F, Fut>(
    segment: &'static str,
    queue_type: QueueType,
    publish: F,
    pool: Arc<Pool>,
    key_prefix: String,
    mut shutdown_rx: watch::Receiver<bool>,
) -> WorkerHandle
where
    F: Fn(ScheduledJob) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<(), QueueBackendError>> + Send,
{
    let interval = sweep_interval(queue_type);
    info!(
        segment = segment,
        queue_type = %queue_type,
        sweep_interval_secs = interval.as_secs(),
        "Spawning due-sweep"
    );

    let handle: JoinHandle<()> = tokio::spawn(async move {
        loop {
            if *shutdown_rx.borrow() {
                break;
            }

            let now = chrono::Utc::now().timestamp();
            match claim_due(
                &pool,
                &key_prefix,
                segment,
                queue_type,
                now,
                DUE_SWEEP_BATCH,
            )
            .await
            {
                Ok(jobs) => {
                    for job in jobs {
                        publish_scheduled(&publish, &pool, &key_prefix, segment, queue_type, job)
                            .await;
                    }
                }
                Err(e) => warn!(
                    segment = segment,
                    queue_type = %queue_type,
                    error = %e,
                    "Due-sweep claim failed; will retry next tick"
                ),
            }

            tokio::select! {
                _ = tokio::time::sleep(interval) => {}
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        break;
                    }
                }
            }
        }
        info!(segment = segment, queue_type = %queue_type, "Due-sweep stopped");
    });

    WorkerHandle::Tokio(handle)
}

/// Publishes one claimed scheduled job via the backend callback.
///
/// `claim_due` has already removed the job from Redis, so on publish failure the
/// job is re-queued into the scheduled set (scored for immediate re-sweep) to
/// avoid silently dropping a deferred or retrying job on a transient transport
/// error. A re-publish of a job that actually succeeded is harmless: delivery is
/// at-least-once and the handlers are idempotent.
async fn publish_scheduled<F, Fut>(
    publish: &F,
    pool: &Arc<Pool>,
    key_prefix: &str,
    segment: &str,
    queue_type: QueueType,
    job: ScheduledJob,
) where
    F: Fn(ScheduledJob) -> Fut,
    Fut: Future<Output = Result<(), QueueBackendError>>,
{
    let retry_attempt = job.retry_attempt;
    match publish(job.clone()).await {
        Ok(()) => debug!(
            segment = segment,
            queue_type = %queue_type,
            retry_attempt = retry_attempt,
            "Published due job from scheduled set"
        ),
        Err(e) => {
            error!(
                segment = segment,
                queue_type = %queue_type,
                error = %e,
                "Failed to publish due job; re-queuing to the scheduled set"
            );
            let now = chrono::Utc::now().timestamp();
            if let Err(re) = zadd_scheduled(pool, key_prefix, segment, queue_type, &job, now).await
            {
                error!(
                    segment = segment,
                    queue_type = %queue_type,
                    error = %re,
                    "Failed to re-queue due job after publish failure; job dropped this tick"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scheduled_set_key_format_per_segment() {
        // Pub/Sub key format is preserved byte-identical (merged-to-main jobs).
        assert_eq!(
            scheduled_set_key("oz-relayer", "pubsub", QueueType::StatusCheckEvm),
            "oz-relayer:pubsub:scheduled:status-check-evm"
        );
        // RabbitMQ uses its own segment.
        assert_eq!(
            scheduled_set_key("oz-relayer", "rabbitmq", QueueType::StatusCheckEvm),
            "oz-relayer:rabbitmq:scheduled:status-check-evm"
        );
        assert_eq!(
            scheduled_set_key("custom", "rabbitmq", QueueType::TransactionRequest),
            "custom:rabbitmq:scheduled:transaction-request"
        );
    }

    #[test]
    fn test_segments_never_collide_on_shared_redis() {
        // The two backends' scheduled sets must be disjoint on a shared Redis so
        // a mixed-backend fleet never claims the other's jobs.
        let prefix = "oz-relayer";
        for qt in [
            QueueType::TransactionRequest,
            QueueType::StatusCheck,
            QueueType::StatusCheckEvm,
            QueueType::StatusCheckStellar,
            QueueType::Notification,
        ] {
            assert_ne!(
                scheduled_set_key(prefix, "pubsub", qt),
                scheduled_set_key(prefix, "rabbitmq", qt),
                "pubsub and rabbitmq scheduled keys must differ for {qt}"
            );
        }
    }

    #[test]
    fn test_scheduled_job_round_trips() {
        let job = ScheduledJob {
            body: r#"{"message_id":"m1"}"#.to_string(),
            retry_attempt: 3,
        };
        let bytes = serde_json::to_vec(&job).unwrap();
        let decoded: ScheduledJob = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(decoded, job);
    }

    #[test]
    fn test_sweep_interval_status_checks_are_fast() {
        // Status checks sweep at ~1s; others coarser.
        assert_eq!(
            sweep_interval(QueueType::StatusCheck),
            Duration::from_secs(1)
        );
        assert_eq!(
            sweep_interval(QueueType::StatusCheckEvm),
            Duration::from_secs(1)
        );
        assert_eq!(
            sweep_interval(QueueType::StatusCheckStellar),
            Duration::from_secs(1)
        );
        assert!(sweep_interval(QueueType::Notification) > Duration::from_secs(1));
        assert!(sweep_interval(QueueType::TransactionRequest) > Duration::from_secs(1));
    }
}
