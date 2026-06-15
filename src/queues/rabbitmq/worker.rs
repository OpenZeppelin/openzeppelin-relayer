//! RabbitMQ consumer worker.
//!
//! One consumer loop per queue type: open a channel, set `basic_qos` prefetch to
//! the queue's concurrency, `basic_consume` with manual acks, and process each
//! delivery in a task bounded by a same-sized semaphore. There is **no lease
//! management** — an unacked delivery stays reserved while the channel lives
//! (the broker's `consumer_timeout`, default 30 min, sits far above the 600s
//! handler timeout). The loop **self-heals**: if the delivery stream ends or
//! errors, it re-opens the channel and re-subscribes with capped backoff once
//! the (auto-recovered) connection is available. On shutdown it stops consuming
//! (`basic_cancel`) and drains in-flight handlers **while the channel is still
//! open** so their acks land, then closes it; anything still unacked at the
//! drain timeout is left for the broker to requeue.

use std::sync::Arc;
use std::time::Duration;

use actix_web::web::ThinData;
use deadpool_redis::Pool;
use futures::StreamExt;
use lapin::message::Delivery;
use lapin::options::{
    BasicAckOptions, BasicCancelOptions, BasicConsumeOptions, BasicNackOptions, BasicQosOptions,
};
use lapin::types::FieldTable;
use lapin::{Channel, Connection, Consumer};
use tokio::sync::{watch, Semaphore};
use tokio::task::{JoinHandle, JoinSet};
use tracing::{debug, error, info, warn};

use crate::metrics::observe_queue_pickup_latency;
use crate::models::DefaultAppState;
use crate::queues::schedule::{zadd_scheduled, ScheduledJob};
use crate::queues::worker_shared::{
    get_concurrency_for_queue, is_retry_exhausted, job_correlation_id, retry_delay_for_queue,
    run_handler_with_timeout, HandlerOutcome, DRAIN_TIMEOUT,
};

use super::backend::{retry_attempt_from_headers, RABBITMQ_SEGMENT};
use super::{QueueType, WorkerHandle};

/// Initial reconnect backoff after a consumer stream loss.
const RECONNECT_BACKOFF_INITIAL: Duration = Duration::from_millis(500);
/// Cap for the reconnect backoff.
const RECONNECT_BACKOFF_MAX: Duration = Duration::from_secs(30);
/// Cap for the pre-nack pause when a retry can't be durably recorded (e.g. Redis
/// is down). Without it, nack-with-requeue makes the broker redeliver instantly,
/// which becomes a zero-backoff hot loop; this paces redelivery instead.
const NACK_BACKOFF_MAX: Duration = Duration::from_secs(30);

/// Per-consumer parameters threaded through the loop.
#[derive(Clone)]
struct ConsumerConfig {
    queue_type: QueueType,
    queue_name: String,
    max_retries: usize,
    key_prefix: String,
    /// Redacted endpoint for reconnect logs (never the raw URL).
    redacted_url: String,
}

/// Clamps a concurrency value to the AMQP `prefetch_count` range. A `prefetch` of
/// 0 means UNLIMITED in AMQP (the broker would flood the consumer's backlog into
/// memory), so 0 is clamped to 1; values above `u16::MAX` saturate, never wrap.
fn prefetch_count(concurrency: usize) -> u16 {
    u16::try_from(concurrency.max(1)).unwrap_or(u16::MAX)
}

/// Spawns a consumer loop for one queue.
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_consumer_for_queue(
    connection: Arc<Connection>,
    queue_type: QueueType,
    queue_name: String,
    app_state: Arc<ThinData<DefaultAppState>>,
    redis_pool: Arc<Pool>,
    key_prefix: String,
    redacted_url: String,
    shutdown_rx: watch::Receiver<bool>,
) -> WorkerHandle {
    let concurrency = get_concurrency_for_queue(queue_type);
    let config = ConsumerConfig {
        queue_type,
        queue_name: queue_name.clone(),
        max_retries: queue_type.max_retries(),
        key_prefix,
        redacted_url,
    };

    info!(
        queue_type = %queue_type,
        queue = %queue_name,
        concurrency = concurrency,
        prefetch = prefetch_count(concurrency),
        max_retries = config.max_retries,
        "Spawning RabbitMQ consumer"
    );

    let handle: JoinHandle<()> = tokio::spawn(async move {
        run_consumer_loop(
            connection,
            config,
            concurrency,
            app_state,
            redis_pool,
            shutdown_rx,
        )
        .await;
        info!(queue_type = %queue_type, "RabbitMQ consumer stopped");
    });

    WorkerHandle::Tokio(handle)
}

/// Opens a consumer channel, applies `basic_qos`, and subscribes with manual
/// acks. A recognizable consumer tag identifies this consumer in the broker and
/// logs; shutdown cancels it via `consumer.tag()`.
async fn establish_consumer(
    connection: &Arc<Connection>,
    config: &ConsumerConfig,
    concurrency: usize,
) -> Result<(Channel, Consumer), lapin::Error> {
    let channel = connection.create_channel().await?;
    channel
        .basic_qos(prefetch_count(concurrency), BasicQosOptions::default())
        .await?;
    let consumer_tag = format!("relayer-{}", config.queue_name);
    let consumer = channel
        .basic_consume(
            config.queue_name.as_str().into(),
            consumer_tag.as_str().into(),
            BasicConsumeOptions::default(), // manual ack (no_ack = false)
            FieldTable::default(),
        )
        .await?;
    Ok((channel, consumer))
}

async fn run_consumer_loop(
    connection: Arc<Connection>,
    config: ConsumerConfig,
    concurrency: usize,
    app_state: Arc<ThinData<DefaultAppState>>,
    redis_pool: Arc<Pool>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let queue_type = config.queue_type;
    let semaphore = Arc::new(Semaphore::new(concurrency));
    let mut inflight: JoinSet<()> = JoinSet::new();
    let mut backoff = RECONNECT_BACKOFF_INITIAL;

    'reconnect: loop {
        if *shutdown_rx.borrow() {
            break;
        }

        // (Re)establish the consumer channel + subscription. lapin's connection
        // auto-recovery handles the transport; this loop re-creates the channel
        // and re-subscribes once the connection is back (self-healing).
        let (channel, mut consumer) = match establish_consumer(&connection, &config, concurrency)
            .await
        {
            Ok(cc) => {
                backoff = RECONNECT_BACKOFF_INITIAL;
                info!(queue_type = %queue_type, queue = %config.queue_name, "RabbitMQ consumer subscribed");
                cc
            }
            Err(e) => {
                warn!(
                    queue_type = %queue_type,
                    endpoint = %config.redacted_url,
                    backoff_ms = backoff.as_millis(),
                    error = %e,
                    "Failed to (re)subscribe RabbitMQ consumer; backing off"
                );
                tokio::select! {
                    _ = tokio::time::sleep(backoff) => {}
                    _ = shutdown_rx.changed() => break 'reconnect,
                }
                backoff = (backoff * 2).min(RECONNECT_BACKOFF_MAX);
                continue;
            }
        };

        // Consume until the stream ends/errors (→ reconnect) or shutdown.
        // `shutting_down` distinguishes a shutdown exit (drain this channel) from
        // a stream-loss exit (drop + reconnect).
        let mut shutting_down = false;
        loop {
            // Reap finished handlers so the JoinSet doesn't grow unbounded.
            while inflight.try_join_next().is_some() {}

            if *shutdown_rx.borrow() {
                shutting_down = true;
                break;
            }

            let next = tokio::select! {
                d = consumer.next() => d,
                _ = shutdown_rx.changed() => {
                    shutting_down = true;
                    break;
                }
            };

            match next {
                Some(Ok(delivery)) => {
                    // Race permit acquisition against shutdown so a saturated
                    // consumer doesn't block here while a shutdown is pending;
                    // the un-spawned delivery is left unacked and the broker
                    // requeues it when the channel closes.
                    let permit = tokio::select! {
                        p = semaphore.clone().acquire_owned() => match p {
                            Ok(p) => p,
                            Err(_) => {
                                error!(queue_type = %queue_type, "Semaphore closed, stopping consumer");
                                break 'reconnect;
                            }
                        },
                        _ = shutdown_rx.changed() => {
                            shutting_down = true;
                            break;
                        }
                    };
                    let state = app_state.clone();
                    let pool = redis_pool.clone();
                    let cfg = config.clone();
                    inflight.spawn(async move {
                        let _permit = permit; // released even on panic
                        process_delivery(delivery, &cfg, state, &pool).await;
                    });
                }
                Some(Err(e)) => {
                    warn!(
                        queue_type = %queue_type,
                        endpoint = %config.redacted_url,
                        error = %e,
                        "RabbitMQ delivery stream error; will re-subscribe"
                    );
                    break; // → cancel + reconnect
                }
                None => {
                    warn!(
                        queue_type = %queue_type,
                        "RabbitMQ consumer cancelled / stream ended; will re-subscribe"
                    );
                    break; // → cancel + reconnect
                }
            }
        }

        // Stop the broker delivering on the (still-open) channel before draining
        // or reconnecting: on shutdown this halts new deliveries while in-flight
        // handlers drain; on stream loss it's best-effort (channel may be dead).
        let _ = channel
            .basic_cancel(consumer.tag(), BasicCancelOptions::default())
            .await;

        if shutting_down {
            // Drain in-flight handlers WHILE this channel is still open, so each
            // handler's ack/nack actually reaches the broker, THEN drop the
            // channel. Dropping it first (lapin closes a channel on drop) would
            // requeue every unacked in-flight delivery AND turn the drain's acks
            // into poisoned no-ops — duplicating every in-flight job on a graceful
            // shutdown.
            drain_inflight(&mut inflight, queue_type).await;
            break;
        }
        // Stream loss: drop channel/consumer (closing the old channel) and loop to
        // reconnect. In-flight handlers from this subscription run to completion;
        // their acks on the now-closed channel are no-ops and the broker may
        // redeliver (tolerated — handlers are idempotent).
    }

    // Shutdown observed between subscriptions (e.g. during reconnect backoff):
    // any handlers still in flight here are orphaned — their channel is already
    // gone, so their acks can't land. Abort them rather than waiting (no-op after
    // a clean drain above).
    inflight.abort_all();
}

/// Drains in-flight handlers, bounded by `DRAIN_TIMEOUT`. MUST be called while
/// the consumer channel is still open: a finished handler acks its delivery, and
/// that ack only reaches the broker over a live channel. On timeout the remaining
/// handlers are aborted (left unacked → the broker requeues them).
async fn drain_inflight(inflight: &mut JoinSet<()>, queue_type: QueueType) {
    if inflight.is_empty() {
        return;
    }
    info!(
        queue_type = %queue_type,
        count = inflight.len(),
        "Draining in-flight RabbitMQ handlers before shutdown"
    );
    let drain = async { while inflight.join_next().await.is_some() {} };
    if tokio::time::timeout(DRAIN_TIMEOUT, drain).await.is_err() {
        warn!(
            queue_type = %queue_type,
            "Drain timeout; aborting remaining handlers (left un-acked → broker requeues)"
        );
        inflight.abort_all();
    }
}

/// Runs the handler for one delivery under the 600s timeout + panic guard, then
/// settles it (ack / re-enqueue-for-retry / drop).
async fn process_delivery(
    delivery: Delivery,
    config: &ConsumerConfig,
    app_state: Arc<ThinData<DefaultAppState>>,
    redis_pool: &Arc<Pool>,
) {
    let queue_type = config.queue_type;
    let retry_attempt = retry_attempt_from_headers(&delivery.properties);
    let correlation_id = job_correlation_id(&delivery.data);

    // Observe pickup latency on the first delivery only (retries are republished
    // with retry_attempt > 0). The publish timestamp ~= due time, so this
    // excludes intentional scheduling delay.
    if retry_attempt == 0 {
        if let Some(published_at) = delivery.properties.timestamp() {
            let now = chrono::Utc::now().timestamp();
            observe_queue_pickup_latency(
                queue_type.queue_name(),
                "rabbitmq",
                (now - *published_at as i64).max(0) as f64,
            );
        }
    }

    debug!(
        queue_type = %queue_type,
        correlation_id = %correlation_id,
        retry_attempt = retry_attempt,
        "Processing RabbitMQ delivery"
    );

    match run_handler_with_timeout(
        &delivery.data,
        queue_type,
        app_state,
        retry_attempt,
        correlation_id.clone(),
    )
    .await
    {
        HandlerOutcome::Success => {
            debug!(queue_type = %queue_type, correlation_id = %correlation_id, "Handler succeeded; acking");
            ack(&delivery, queue_type, &correlation_id).await;
        }
        HandlerOutcome::Permanent(e) => {
            // Terminal state already persisted by the handler; drop the message.
            error!(
                queue_type = %queue_type,
                correlation_id = %correlation_id,
                error = %e,
                "Permanent handler failure; acking/dropping (terminal state persisted)"
            );
            ack(&delivery, queue_type, &correlation_id).await;
        }
        HandlerOutcome::Retryable(e) => {
            settle_retry(
                &delivery,
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
/// `now + backoff`) with an incremented attempt, **then** acks the original — or,
/// if the budget is exhausted on a bounded queue, drops it (terminal state is the
/// durable record).
///
/// Settle-retry ordering (at-least-once): the retry copy is written to Redis
/// **before** the original delivery is acked, and the two stores are not updated
/// atomically. A crash between the two yields a duplicate, never a loss; handlers
/// are idempotent. On re-enqueue failure the original is nacked-with-requeue so
/// the broker redelivers it (no loss) rather than holding the prefetch slot.
async fn settle_retry(
    delivery: &Delivery,
    config: &ConsumerConfig,
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
        ack(delivery, queue_type, correlation_id).await;
        return;
    }

    let delay = retry_delay_for_queue(queue_type, &delivery.data, retry_attempt);

    let body = match String::from_utf8(delivery.data.clone()) {
        Ok(b) => b,
        Err(e) => {
            // Body isn't UTF-8 JSON — unrecoverable; drop (can't re-enqueue).
            error!(
                queue_type = %queue_type,
                correlation_id = %correlation_id,
                error = %e,
                "Message body is not valid UTF-8; dropping"
            );
            ack(delivery, queue_type, correlation_id).await;
            return;
        }
    };

    let scheduled = ScheduledJob {
        body,
        retry_attempt: next_attempt,
    };
    let run_at = chrono::Utc::now().timestamp() + delay as i64;

    // ZADD the retry copy FIRST, then ack the original (settle-retry ordering).
    match zadd_scheduled(
        redis_pool,
        &config.key_prefix,
        RABBITMQ_SEGMENT,
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
            ack(delivery, queue_type, correlation_id).await;
        }
        Err(e) => {
            // Couldn't durably record the retry → nack-with-requeue so the broker
            // redelivers (no loss) instead of the slot being held un-acked. But an
            // immediate nack-with-requeue, while the Redis outage that caused this
            // persists, makes the broker redeliver instantly → a zero-backoff hot
            // loop. Pause for the computed retry delay (capped) first so redelivery
            // is paced, then nack.
            let pause = Duration::from_secs(delay.max(0) as u64).min(NACK_BACKOFF_MAX);
            error!(
                queue_type = %queue_type,
                correlation_id = %correlation_id,
                error = %e,
                pause_secs = pause.as_secs(),
                "Failed to re-enqueue retry; pausing before nack-with-requeue"
            );
            tokio::time::sleep(pause).await;
            nack_requeue(delivery, queue_type, correlation_id).await;
        }
    }
}

/// Acks a delivery (best-effort). On failure the broker redelivers on channel
/// close; handlers are idempotent. `ack` returns `Ok(false)` when the acker is
/// already used or poisoned (e.g. the channel closed) — the ack never reached the
/// broker, so it's logged rather than silently treated as success.
async fn ack(delivery: &Delivery, queue_type: QueueType, correlation_id: &str) {
    match delivery.ack(BasicAckOptions::default()).await {
        Ok(true) => {}
        Ok(false) => warn!(
            queue_type = %queue_type,
            correlation_id = %correlation_id,
            "RabbitMQ ack was a no-op (acker poisoned/already used); broker will redeliver on channel close (idempotent)"
        ),
        Err(e) => warn!(
            queue_type = %queue_type,
            correlation_id = %correlation_id,
            error = %e,
            "Failed to ack RabbitMQ delivery; broker will redeliver on channel close (idempotent)"
        ),
    }
}

/// Nacks a delivery with requeue so the broker redelivers it. As with `ack`, an
/// `Ok(false)` (poisoned/used acker) means the nack was a no-op and is logged.
async fn nack_requeue(delivery: &Delivery, queue_type: QueueType, correlation_id: &str) {
    let options = BasicNackOptions {
        multiple: false,
        requeue: true,
    };
    match delivery.nack(options).await {
        Ok(true) => {}
        Ok(false) => warn!(
            queue_type = %queue_type,
            correlation_id = %correlation_id,
            "RabbitMQ nack was a no-op (acker poisoned/already used); broker will redeliver on channel close"
        ),
        Err(e) => warn!(
            queue_type = %queue_type,
            correlation_id = %correlation_id,
            error = %e,
            "Failed to nack RabbitMQ delivery; broker will redeliver on channel close"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prefetch_count_clamps_to_u16() {
        // 0 would mean UNLIMITED prefetch in AMQP, so it's clamped up to 1.
        assert_eq!(prefetch_count(0), 1);
        assert_eq!(prefetch_count(50), 50);
        assert_eq!(prefetch_count(u16::MAX as usize), u16::MAX);
        // Above u16::MAX clamps rather than wrapping.
        assert_eq!(prefetch_count(u16::MAX as usize + 1), u16::MAX);
        assert_eq!(prefetch_count(usize::MAX), u16::MAX);
    }

    #[test]
    fn test_reconnect_backoff_bounds() {
        // Initial backoff is small; the cap is bounded and above the initial.
        assert!(RECONNECT_BACKOFF_INITIAL < RECONNECT_BACKOFF_MAX);
        assert!(RECONNECT_BACKOFF_MAX <= Duration::from_secs(60));
    }

    fn test_config(queue_type: QueueType) -> ConsumerConfig {
        ConsumerConfig {
            queue_type,
            queue_name: format!("relayer-{}", queue_type.queue_name()),
            max_retries: queue_type.max_retries(),
            key_prefix: "test".to_string(),
            redacted_url: "amqp://broker:5672/%2f".to_string(),
        }
    }

    /// A pool that is never actually connected to. The `settle_retry` branches
    /// exercised below ack and return before any Redis write, so the pool is only
    /// a required argument, never used.
    fn unconnected_pool() -> Arc<Pool> {
        let pool = deadpool_redis::Config::from_url("redis://127.0.0.1:6379")
            .builder()
            .expect("pool builder")
            .max_size(1)
            .runtime(deadpool_redis::Runtime::Tokio1)
            .build()
            .expect("build pool");
        Arc::new(pool)
    }

    fn mock_delivery(queue: &str, body: Vec<u8>) -> Delivery {
        Delivery::mock(1, "".into(), queue.into(), false, body)
    }

    #[tokio::test]
    async fn test_ack_and_nack_helpers_handle_mock_acker_states() {
        // A fresh mock acker settles once (Ok(true)); settling it again is the
        // poisoned/already-used no-op (Ok(false)). Both branches must be handled
        // without panicking, with no live channel.
        let d = mock_delivery("relayer-status-check", b"{}".to_vec());
        ack(&d, QueueType::StatusCheck, "cid").await; // Ok(true)
        ack(&d, QueueType::StatusCheck, "cid").await; // Ok(false) → logged no-op

        let d2 = mock_delivery("relayer-status-check", b"{}".to_vec());
        nack_requeue(&d2, QueueType::StatusCheck, "cid").await; // Ok(true)
        nack_requeue(&d2, QueueType::StatusCheck, "cid").await; // Ok(false) → logged
    }

    #[tokio::test]
    async fn test_settle_retry_drops_when_budget_exhausted_without_redis() {
        // A bounded queue at its retry budget drops (acks) and returns BEFORE any
        // Redis write — so this runs without a live Redis.
        let config = test_config(QueueType::TransactionRequest); // bounded
        let delivery = mock_delivery(&config.queue_name, br#"{"message_id":"m1"}"#.to_vec());
        settle_retry(
            &delivery,
            &config,
            &unconnected_pool(),
            config.max_retries,
            "m1",
            "boom",
        )
        .await;
    }

    #[tokio::test]
    async fn test_settle_retry_drops_non_utf8_body_without_redis() {
        // A non-UTF8 body can't be re-enqueued; it's dropped (acked) before the
        // Redis ZADD, so this also runs without a live Redis. Attempt 0 is well
        // under the budget, so it passes the exhaustion check first.
        let config = test_config(QueueType::TransactionRequest);
        let delivery = mock_delivery(&config.queue_name, vec![0xff, 0xfe, 0x00]);
        settle_retry(&delivery, &config, &unconnected_pool(), 0, "m1", "boom").await;
    }
}

// Gated integration tests.
// `settle_retry_*` need only Redis on localhost:6379:
//   cargo test --lib queues::rabbitmq::worker::gated_tests::integration_settle -- --ignored
// The broker spine tests additionally need a real RabbitMQ at RABBITMQ_TEST_URL
// (e.g. amqp://guest:guest@localhost:5672/%2f):
//   RABBITMQ_TEST_URL=amqp://guest:guest@localhost:5672/%2f \
//   cargo test --lib queues::rabbitmq::worker::gated_tests -- --ignored
// The full transaction -> final-state end-to-end is validated by the
// examples/rabbitmq-queue-storage example.
#[cfg(test)]
mod gated_tests {
    use std::sync::Arc;
    use std::time::Duration;

    use deadpool_redis::Pool;
    use futures::StreamExt;
    use lapin::message::Delivery;
    use lapin::options::{
        BasicAckOptions, BasicConsumeOptions, BasicQosOptions, QueueDeclareOptions,
        QueueDeleteOptions,
    };
    use lapin::types::FieldTable;
    use lapin::{Connection, ConnectionProperties};

    use super::{settle_retry, ConsumerConfig};
    use crate::queues::rabbitmq::backend::{
        publish_confirmed, retry_attempt_from_headers, RABBITMQ_SEGMENT,
    };
    use crate::queues::schedule::{claim_due, scheduled_set_key, zadd_scheduled, ScheduledJob};
    use crate::queues::QueueType;

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

    fn broker_url() -> Option<String> {
        std::env::var("RABBITMQ_TEST_URL")
            .ok()
            .filter(|v| !v.is_empty())
    }

    fn config_for(queue_type: QueueType, key_prefix: &str) -> ConsumerConfig {
        ConsumerConfig {
            queue_type,
            queue_name: format!("relayer-{}", queue_type.queue_name()),
            max_retries: queue_type.max_retries(),
            key_prefix: key_prefix.to_string(),
            redacted_url: "amqp://broker:5672/%2f".to_string(),
        }
    }

    async fn drain_key(pool: &Arc<Pool>, prefix: &str, queue_type: QueueType) {
        if let Ok(mut conn) = pool.get().await {
            let key = scheduled_set_key(prefix, RABBITMQ_SEGMENT, queue_type);
            let _: Result<(), _> = redis::cmd("DEL").arg(&key).query_async(&mut conn).await;
        }
    }

    /// A retryable failure on a bounded queue writes the retry copy (attempt+1,
    /// future score) to the Redis scheduled set BEFORE acking the original. The
    /// durable retry record is present regardless of the ack outcome — so a
    /// crash/ack-failure after the ZADD yields a duplicate (tolerated), never a
    /// loss.
    #[tokio::test]
    #[ignore]
    async fn integration_settle_retry_zadd_before_ack() {
        let Some(pool) = test_pool() else {
            return;
        };
        let prefix = format!("test-rabbitmq-{}", uuid::Uuid::new_v4());
        let queue_type = QueueType::TransactionRequest; // bounded
        drain_key(&pool, &prefix, queue_type).await;

        let config = config_for(queue_type, &prefix);
        let delivery = Delivery::mock(
            1,
            "".into(),
            config.queue_name.as_str().into(),
            false,
            br#"{"message_id":"m1","data":{}}"#.to_vec(),
        );

        // Attempt 2 (well below max) → re-enqueue as attempt 3.
        settle_retry(&delivery, &config, &pool, 2, "m1", "boom").await;

        let far = chrono::Utc::now().timestamp() + 1_000_000;
        let claimed = claim_due(&pool, &prefix, RABBITMQ_SEGMENT, queue_type, far, 256)
            .await
            .expect("claim");
        assert_eq!(claimed.len(), 1, "retry copy must be durably enqueued");
        assert_eq!(claimed[0].retry_attempt, 3, "attempt must be incremented");

        drain_key(&pool, &prefix, queue_type).await;
    }

    /// A bounded queue at its retry budget drops (acks) with NO re-enqueue.
    #[tokio::test]
    #[ignore]
    async fn integration_settle_retry_bounded_exhaustion_drops() {
        let Some(pool) = test_pool() else {
            return;
        };
        let prefix = format!("test-rabbitmq-{}", uuid::Uuid::new_v4());
        let queue_type = QueueType::TransactionRequest;
        drain_key(&pool, &prefix, queue_type).await;

        let config = config_for(queue_type, &prefix);
        let delivery = Delivery::mock(
            1,
            "".into(),
            config.queue_name.as_str().into(),
            false,
            br#"{"message_id":"m1","data":{}}"#.to_vec(),
        );

        // retry_attempt == max_retries → next attempt would exceed budget → drop.
        settle_retry(&delivery, &config, &pool, config.max_retries, "m1", "boom").await;

        let far = chrono::Utc::now().timestamp() + 1_000_000;
        let claimed = claim_due(&pool, &prefix, RABBITMQ_SEGMENT, queue_type, far, 256)
            .await
            .expect("claim");
        assert!(
            claimed.is_empty(),
            "exhausted bounded queue must not re-enqueue"
        );

        drain_key(&pool, &prefix, queue_type).await;
    }

    /// Status-check queues are unbounded: they re-enqueue even past any bounded
    /// budget (until the transaction finalizes).
    #[tokio::test]
    #[ignore]
    async fn integration_settle_retry_status_checks_unbounded() {
        let Some(pool) = test_pool() else {
            return;
        };
        let prefix = format!("test-rabbitmq-{}", uuid::Uuid::new_v4());
        let queue_type = QueueType::StatusCheckEvm; // unbounded (usize::MAX)
        drain_key(&pool, &prefix, queue_type).await;

        let config = config_for(queue_type, &prefix);
        let delivery = Delivery::mock(
            1,
            "".into(),
            config.queue_name.as_str().into(),
            false,
            br#"{"message_id":"m1","data":{"network_type":"evm"}}"#.to_vec(),
        );

        // A huge attempt count would exhaust any bounded queue; status checks
        // must still re-enqueue.
        settle_retry(&delivery, &config, &pool, 1_000_000, "m1", "still pending").await;

        let far = chrono::Utc::now().timestamp() + 1_000_000;
        let claimed = claim_due(&pool, &prefix, RABBITMQ_SEGMENT, queue_type, far, 256)
            .await
            .expect("claim");
        assert_eq!(claimed.len(), 1, "status checks must re-enqueue unbounded");
        assert_eq!(claimed[0].retry_attempt, 1_000_001);

        drain_key(&pool, &prefix, queue_type).await;
    }

    async fn connect(url: &str) -> Connection {
        crate::queues::ensure_crypto_provider();
        Connection::connect(url, ConnectionProperties::default())
            .await
            .expect("connect to RABBITMQ_TEST_URL")
    }

    /// publish (confirmed) → consume round-trip against a real broker: the body
    /// and the logical x-retry-attempt header survive the round-trip.
    #[tokio::test]
    #[ignore]
    async fn integration_publish_consume_round_trip() {
        let Some(url) = broker_url() else {
            return;
        };
        let conn = connect(&url).await;
        let queue = format!("relayer-test-{}", uuid::Uuid::new_v4().simple());

        let producer = conn.create_channel().await.unwrap();
        producer.confirm_select(Default::default()).await.unwrap();
        producer
            .queue_declare(
                queue.as_str().into(),
                QueueDeclareOptions::durable(),
                FieldTable::default(),
            )
            .await
            .expect("declare");

        // Confirmed publish carrying retry_attempt = 2.
        publish_confirmed(&producer, &queue, br#"{"message_id":"due"}"#, 2)
            .await
            .expect("publish confirmed");

        let channel = conn.create_channel().await.unwrap();
        channel
            .basic_qos(1, BasicQosOptions::default())
            .await
            .unwrap();
        let mut consumer = channel
            .basic_consume(
                queue.as_str().into(),
                "test-consumer".into(),
                BasicConsumeOptions::default(),
                FieldTable::default(),
            )
            .await
            .expect("consume");

        let delivery = tokio::time::timeout(Duration::from_secs(10), consumer.next())
            .await
            .expect("delivery within 10s")
            .expect("stream item")
            .expect("delivery");
        assert_eq!(delivery.data, br#"{"message_id":"due"}"#);
        assert_eq!(
            retry_attempt_from_headers(&delivery.properties),
            2,
            "logical retry attempt must survive the wire round-trip"
        );
        delivery.ack(BasicAckOptions::default()).await.unwrap();

        // Cleanup the temp queue.
        let _ = producer
            .queue_delete(queue.as_str().into(), QueueDeleteOptions::default())
            .await;
    }

    /// A confirmed publish to a queue that does not exist is RETURNED by the
    /// broker (mandatory=true) and surfaced as an error — never the silent
    /// "broker acked an unroutable publish" success that would lose the job (e.g.
    /// if a queue is deleted at runtime).
    #[tokio::test]
    #[ignore]
    async fn integration_publish_to_missing_queue_errors() {
        let Some(url) = broker_url() else {
            return;
        };
        let conn = connect(&url).await;
        let producer = conn.create_channel().await.unwrap();
        producer.confirm_select(Default::default()).await.unwrap();

        // Never-declared queue → unroutable via the default exchange.
        let missing = format!("relayer-missing-{}", uuid::Uuid::new_v4().simple());
        let err = publish_confirmed(&producer, &missing, br#"{"message_id":"lost"}"#, 0)
            .await
            .expect_err("publish to a non-existent queue must error, not silently succeed");
        let msg = err.to_string();
        assert!(
            msg.contains("unroutable") && msg.contains("not enqueued"),
            "error must explain the publish was returned and not enqueued, got: {msg}"
        );
    }

    /// A future-scheduled job stays in Redis and never reaches the broker until
    /// due; a due job is swept (claimed) and published, then consumed — proving
    /// the broker queue only ever carries already-due jobs.
    #[tokio::test]
    #[ignore]
    async fn integration_broker_only_carries_due_jobs() {
        let Some(url) = broker_url() else {
            return;
        };
        let Some(pool) = test_pool() else {
            return;
        };
        let conn = connect(&url).await;
        let prefix = format!("test-rabbitmq-{}", uuid::Uuid::new_v4());
        let queue_type = QueueType::StatusCheckEvm;
        let queue = format!("relayer-test-{}", uuid::Uuid::new_v4().simple());
        drain_key(&pool, &prefix, queue_type).await;

        let producer = conn.create_channel().await.unwrap();
        producer.confirm_select(Default::default()).await.unwrap();
        producer
            .queue_declare(
                queue.as_str().into(),
                QueueDeclareOptions::durable(),
                FieldTable::default(),
            )
            .await
            .unwrap();

        let now = chrono::Utc::now().timestamp();

        // Far-future job: held in Redis, not claimed/published.
        let future = ScheduledJob {
            body: r#"{"message_id":"future"}"#.to_string(),
            retry_attempt: 0,
        };
        zadd_scheduled(
            &pool,
            &prefix,
            RABBITMQ_SEGMENT,
            queue_type,
            &future,
            now + 3600,
        )
        .await
        .unwrap();
        assert!(
            claim_due(&pool, &prefix, RABBITMQ_SEGMENT, queue_type, now, 256)
                .await
                .unwrap()
                .is_empty(),
            "future job must not be due yet"
        );

        // Due retry (attempt 2): claimed and published to the broker.
        let due = ScheduledJob {
            body: r#"{"message_id":"due"}"#.to_string(),
            retry_attempt: 2,
        };
        zadd_scheduled(&pool, &prefix, RABBITMQ_SEGMENT, queue_type, &due, now - 1)
            .await
            .unwrap();
        for job in claim_due(&pool, &prefix, RABBITMQ_SEGMENT, queue_type, now, 256)
            .await
            .unwrap()
        {
            publish_confirmed(&producer, &queue, job.body.as_bytes(), job.retry_attempt)
                .await
                .unwrap();
        }

        let channel = conn.create_channel().await.unwrap();
        channel
            .basic_qos(8, BasicQosOptions::default())
            .await
            .unwrap();
        let mut consumer = channel
            .basic_consume(
                queue.as_str().into(),
                "test-consumer".into(),
                BasicConsumeOptions::default(),
                FieldTable::default(),
            )
            .await
            .unwrap();
        let delivery = tokio::time::timeout(Duration::from_secs(10), consumer.next())
            .await
            .expect("delivery within 10s")
            .expect("stream item")
            .expect("delivery");
        assert_eq!(delivery.data, br#"{"message_id":"due"}"#);
        assert_eq!(retry_attempt_from_headers(&delivery.properties), 2);
        delivery.ack(BasicAckOptions::default()).await.unwrap();

        // Cleanup.
        let _ = producer
            .queue_delete(queue.as_str().into(), QueueDeleteOptions::default())
            .await;
        drain_key(&pool, &prefix, queue_type).await;
    }

    /// A passive `queue_declare` reports the ready-message count — the depth that
    /// feeds `health_check` and the `queue_depth` gauge. With work queued and no
    /// consumer, the reported depth reflects the backlog (never 0).
    #[tokio::test]
    #[ignore]
    async fn integration_passive_declare_reports_backlog_depth() {
        let Some(url) = broker_url() else {
            return;
        };
        let conn = connect(&url).await;
        let queue = format!("relayer-test-{}", uuid::Uuid::new_v4().simple());

        let producer = conn.create_channel().await.unwrap();
        producer.confirm_select(Default::default()).await.unwrap();
        producer
            .queue_declare(
                queue.as_str().into(),
                QueueDeclareOptions::durable(),
                FieldTable::default(),
            )
            .await
            .unwrap();

        // Enqueue 3 messages, consume none.
        for i in 0..3 {
            publish_confirmed(
                &producer,
                &queue,
                format!(r#"{{"message_id":"m{i}"}}"#).as_bytes(),
                0,
            )
            .await
            .unwrap();
        }

        // A passive declare on a fresh channel reports the backlog as ready depth.
        let channel = conn.create_channel().await.unwrap();
        let q = channel
            .queue_declare(
                queue.as_str().into(),
                QueueDeclareOptions::durable().passive(),
                FieldTable::default(),
            )
            .await
            .expect("passive declare");
        assert!(
            q.message_count() >= 3,
            "passive declare must report the backlog (never 0 while queued), got {}",
            q.message_count()
        );

        let _ = producer
            .queue_delete(queue.as_str().into(), QueueDeleteOptions::default())
            .await;
    }
}
