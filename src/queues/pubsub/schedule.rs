//! Pub/Sub adapter over the shared Redis-backed scheduler
//! (`crate::queues::schedule`).
//!
//! The generic store-and-run-when-due scheduler lives in `queues/schedule.rs`;
//! this module pins the Pub/Sub key segment (`pubsub`, keeping the existing
//! `{key_prefix}:pubsub:scheduled:{queue}` keys **byte-identical** for
//! merged-to-main deployments) and adapts the generic publish callback to a
//! Pub/Sub `Publisher`. The Pub/Sub backend/worker call these thin wrappers
//! unchanged.

use std::sync::Arc;

use deadpool_redis::Pool;
use tokio::sync::watch;

use gcloud_pubsub::publisher::Publisher;

use super::backend::message_from_body;
use super::{QueueBackendError, QueueType, WorkerHandle};

pub(crate) use crate::queues::schedule::ScheduledJob;

/// Backend key segment for Pub/Sub scheduled sets (kept byte-identical).
const SEGMENT: &str = "pubsub";

/// Adds a job to the Pub/Sub scheduled set, scored by run time (Unix secs).
pub(crate) async fn zadd_scheduled(
    pool: &Arc<Pool>,
    key_prefix: &str,
    queue_type: QueueType,
    job: &ScheduledJob,
    run_at: i64,
) -> Result<(), QueueBackendError> {
    crate::queues::schedule::zadd_scheduled(pool, key_prefix, SEGMENT, queue_type, job, run_at)
        .await
}

/// Atomically claims up to `max` due members from the Pub/Sub scheduled set.
///
/// Test-only: production due-sweeps go through `crate::queues::schedule`'s
/// generic `spawn_due_sweep`; this thin wrapper exists for the gated
/// emulator/Redis integration tests.
#[cfg(test)]
pub(crate) async fn claim_due(
    pool: &Arc<Pool>,
    key_prefix: &str,
    queue_type: QueueType,
    now: i64,
    max: usize,
) -> Result<Vec<ScheduledJob>, QueueBackendError> {
    crate::queues::schedule::claim_due(pool, key_prefix, SEGMENT, queue_type, now, max).await
}

/// Spawns the Pub/Sub due-sweep: publishes due jobs to the queue's topic via the
/// supplied `Publisher`, deferring re-queue-on-failure to the shared scheduler.
pub(crate) fn spawn_due_sweep(
    queue_type: QueueType,
    publisher: Publisher,
    pool: Arc<Pool>,
    key_prefix: String,
    shutdown_rx: watch::Receiver<bool>,
    runtime_handle: tokio::runtime::Handle,
) -> WorkerHandle {
    let publish = move |job: ScheduledJob| {
        let publisher = publisher.clone();
        async move {
            let message = message_from_body(job.body.into_bytes(), job.retry_attempt);
            publisher
                .publish(message)
                .await
                .get()
                .await
                .map(|_| ())
                .map_err(|e| QueueBackendError::QueueError(format!("Pub/Sub publish failed: {e}")))
        }
    };
    crate::queues::schedule::spawn_due_sweep(
        SEGMENT,
        queue_type,
        publish,
        pool,
        key_prefix,
        shutdown_rx,
        runtime_handle,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pub/Sub key format helper for tests — production code calls the shared
    /// `scheduled_set_key` with the `"pubsub"` segment directly.
    fn pubsub_key(key_prefix: &str, queue_type: QueueType) -> String {
        crate::queues::schedule::scheduled_set_key(key_prefix, SEGMENT, queue_type)
    }

    #[test]
    fn test_scheduled_set_key_format() {
        assert_eq!(
            pubsub_key("oz-relayer", QueueType::StatusCheckEvm),
            "oz-relayer:pubsub:scheduled:status-check-evm"
        );
        assert_eq!(
            pubsub_key("custom", QueueType::TransactionRequest),
            "custom:pubsub:scheduled:transaction-request"
        );
    }

    #[test]
    fn test_scheduled_set_key_distinct_per_queue() {
        let prefix = "oz-relayer";
        let keys: std::collections::HashSet<String> = super::super::backend::ALL_QUEUE_TYPES
            .iter()
            .map(|&qt| pubsub_key(prefix, qt))
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
        use crate::queues::schedule::sweep_interval;
        use std::time::Duration;
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
        let key = pubsub_key(&prefix, queue);

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
        let key = pubsub_key(&prefix, queue);

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
