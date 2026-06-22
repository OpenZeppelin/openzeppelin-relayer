//! Redis-backed scheduled-job store and due-sweep for the Pub/Sub backend.
//!
//! Deferred and retrying jobs live in a per-queue Redis sorted set
//! `{key_prefix}:pubsub:scheduled:{queue_name}` (member = serialized
//! [`ScheduledJob`], score = target run time in Unix seconds). A due-sweep
//! atomically claims due members (so one fleet instance publishes each) and
//! publishes them to the queue's topic — the apalis store-and-run-when-due
//! pattern. The topic therefore only ever carries already-due jobs.

use std::sync::Arc;
use std::time::Duration;

use deadpool_redis::Pool;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

use gcloud_pubsub::publisher::Publisher;

use super::backend::message_from_body;
use super::{QueueBackendError, QueueType, WorkerHandle};

/// A job awaiting its due time in the Redis scheduled set.
///
/// The `body` is the JSON-serialized `Job<T>` (becomes the published message
/// `data`); `retry_attempt` is the logical counter carried as the published
/// message's `retry_attempt` attribute.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ScheduledJob {
    pub body: String,
    pub retry_attempt: usize,
}

/// Redis sorted-set key holding a queue's deferred/retrying jobs.
pub(crate) fn scheduled_set_key(key_prefix: &str, queue_type: QueueType) -> String {
    format!("{key_prefix}:pubsub:scheduled:{}", queue_type.queue_name())
}

/// Adds a job to the scheduled set, scored by its target run time (Unix secs).
///
/// The member is the serialized [`ScheduledJob`]; the score is `run_at`.
/// Re-adding a member with the same content updates its score (idempotent).
pub(crate) async fn zadd_scheduled(
    pool: &Arc<Pool>,
    key_prefix: &str,
    queue_type: QueueType,
    job: &ScheduledJob,
    run_at: i64,
) -> Result<(), QueueBackendError> {
    let key = scheduled_set_key(key_prefix, queue_type);
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
    queue_type: QueueType,
    now: i64,
    max: usize,
) -> Result<Vec<ScheduledJob>, QueueBackendError> {
    let key = scheduled_set_key(key_prefix, queue_type);

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
/// claim due jobs and publish them to the topic.
///
/// Every fleet instance runs the sweep; the atomic claim (`claim_due`) dedups so
/// each due job is published once. A rare double-publish is harmless (at-least-
/// once + idempotent handlers). Fully interruptible by `shutdown_rx`.
pub(crate) fn spawn_due_sweep(
    queue_type: QueueType,
    publisher: Publisher,
    pool: Arc<Pool>,
    key_prefix: String,
    mut shutdown_rx: watch::Receiver<bool>,
) -> WorkerHandle {
    let interval = sweep_interval(queue_type);
    info!(
        queue_type = %queue_type,
        sweep_interval_secs = interval.as_secs(),
        "Spawning Pub/Sub due-sweep"
    );

    let handle: JoinHandle<()> = tokio::spawn(async move {
        loop {
            if *shutdown_rx.borrow() {
                break;
            }

            let now = chrono::Utc::now().timestamp();
            match claim_due(&pool, &key_prefix, queue_type, now, DUE_SWEEP_BATCH).await {
                Ok(jobs) => {
                    for job in jobs {
                        publish_scheduled(&publisher, &pool, &key_prefix, queue_type, &job).await;
                    }
                }
                Err(e) => warn!(
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
        info!(queue_type = %queue_type, "Pub/Sub due-sweep stopped");
    });

    WorkerHandle::Tokio(handle)
}

/// Publishes one claimed scheduled job to its topic.
///
/// `claim_due` has already removed the job from Redis, so on publish failure the
/// job is re-queued into the scheduled set (scored for immediate re-sweep) to
/// avoid silently dropping a deferred or retrying job on a transient Pub/Sub
/// error. A re-publish of a job that actually succeeded is harmless: Pub/Sub is
/// at-least-once and the handlers are idempotent.
async fn publish_scheduled(
    publisher: &Publisher,
    pool: &Arc<Pool>,
    key_prefix: &str,
    queue_type: QueueType,
    job: &ScheduledJob,
) {
    let message = message_from_body(job.body.clone().into_bytes(), job.retry_attempt);
    let awaiter = publisher.publish(message).await;
    match awaiter.get().await {
        Ok(message_id) => debug!(
            queue_type = %queue_type,
            message_id = %message_id,
            retry_attempt = job.retry_attempt,
            "Published due job from scheduled set"
        ),
        Err(e) => {
            error!(
                queue_type = %queue_type,
                error = %e,
                "Failed to publish due job; re-queuing to the scheduled set"
            );
            let now = chrono::Utc::now().timestamp();
            if let Err(re) = zadd_scheduled(pool, key_prefix, queue_type, job, now).await {
                error!(
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
    fn test_scheduled_set_key_format() {
        assert_eq!(
            scheduled_set_key("oz-relayer", QueueType::StatusCheckEvm),
            "oz-relayer:pubsub:scheduled:status-check-evm"
        );
        assert_eq!(
            scheduled_set_key("custom", QueueType::TransactionRequest),
            "custom:pubsub:scheduled:transaction-request"
        );
    }

    #[test]
    fn test_scheduled_set_key_distinct_per_queue() {
        let prefix = "oz-relayer";
        let keys: std::collections::HashSet<String> = super::super::backend::ALL_QUEUE_TYPES
            .iter()
            .map(|&qt| scheduled_set_key(prefix, qt))
            .collect();
        assert_eq!(keys.len(), 8, "each queue type must have a distinct key");
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

    // ── due-sweep at-most-once (gated; requires running Redis) ───────
    //
    // Run with: cargo test --lib queues::pubsub::schedule -- --ignored
    // Requires Redis on localhost:6379.

    fn test_pool() -> Option<Arc<Pool>> {
        let pool = deadpool_redis::Config::from_url("redis://127.0.0.1:6379")
            .builder()
            .ok()?
            .max_size(8)
            .runtime(deadpool_redis::Runtime::Tokio1)
            .build()
            .ok()?;
        Some(Arc::new(pool))
    }

    #[tokio::test]
    #[ignore]
    async fn integration_claim_due_at_most_once_under_concurrency() {
        let pool = test_pool().expect("Redis required for this test");
        // Unique prefix per run so concurrent CI runs don't collide.
        let prefix = format!("test-pubsub-{}", uuid::Uuid::new_v4());
        let queue = QueueType::StatusCheckEvm;
        let key = scheduled_set_key(&prefix, queue);

        // One due member (run_at in the past).
        let job = ScheduledJob {
            body: r#"{"message_id":"only-one"}"#.to_string(),
            retry_attempt: 0,
        };
        let now = chrono::Utc::now().timestamp();
        zadd_scheduled(&pool, &prefix, queue, &job, now - 5)
            .await
            .expect("zadd");

        // Two concurrent sweepers race to claim the single due member.
        let (a, b) = tokio::join!(
            claim_due(&pool, &prefix, queue, now, 256),
            claim_due(&pool, &prefix, queue, now, 256),
        );
        let a = a.expect("claim a");
        let b = b.expect("claim b");

        // Exactly one sweeper got it; the atomic claim deduped.
        let total = a.len() + b.len();
        assert_eq!(
            total, 1,
            "due member must be claimed exactly once, got {total}"
        );
        assert!(a.contains(&job) ^ b.contains(&job));

        // The set is now empty (claimed member removed).
        let leftover = claim_due(&pool, &prefix, queue, now, 256)
            .await
            .expect("claim leftover");
        assert!(leftover.is_empty(), "claimed member must be removed");

        // Cleanup.
        let mut conn = pool.get().await.unwrap();
        let _: () = redis::cmd("DEL")
            .arg(&key)
            .query_async(&mut conn)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore]
    async fn integration_claim_due_excludes_future_members() {
        // A far-future member is never claimed (topic only carries due jobs).
        let pool = test_pool().expect("Redis required for this test");
        let prefix = format!("test-pubsub-{}", uuid::Uuid::new_v4());
        let queue = QueueType::TransactionRequest;
        let key = scheduled_set_key(&prefix, queue);

        let now = chrono::Utc::now().timestamp();
        let future = ScheduledJob {
            body: r#"{"message_id":"future"}"#.to_string(),
            retry_attempt: 0,
        };
        zadd_scheduled(&pool, &prefix, queue, &future, now + 3600)
            .await
            .expect("zadd");

        let claimed = claim_due(&pool, &prefix, queue, now, 256)
            .await
            .expect("claim");
        assert!(claimed.is_empty(), "future member must not be claimed yet");

        // Cleanup.
        let mut conn = pool.get().await.unwrap();
        let _: () = redis::cmd("DEL")
            .arg(&key)
            .query_async(&mut conn)
            .await
            .unwrap();
    }
}
