//! RabbitMQ backend implementation.
//!
//! Provides an AMQP 0-9-1 implementation of the `QueueBackend` trait. Each of
//! the 8 queue types maps to one durable classic queue (`{prefix}-{queue_name}`)
//! published via the default exchange. Deferred/retrying jobs are held in the
//! shared Redis sorted-set scheduler and published when due, so a broker queue
//! only ever carries already-due jobs.
//!
//! RabbitMQ semantics shed two of Pub/Sub's hardest parts: no lease/ack-deadline
//! management (an unacked delivery stays reserved while the consumer channel
//! lives) and no external depth read (passive `queue_declare` returns the count
//! over the live connection). New surfaces it adds: publisher confirms,
//! connection auto-recovery, and an idempotent declare / passive-verify startup.

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use actix_web::web::ThinData;
use async_trait::async_trait;
use lapin::options::{BasicPublishOptions, ConfirmSelectOptions, QueueDeclareOptions};
use lapin::types::{AMQPValue, FieldTable, ShortString};
use lapin::uri::AMQPUri;
use lapin::{BasicProperties, Channel, Confirmation, Connection, ConnectionProperties, ErrorKind};
use serde::Serialize;
use tokio::sync::watch;
use tracing::{debug, error, info, warn};

use crate::{
    config::ServerConfig,
    jobs::{
        Job, NotificationSend, RelayerHealthCheck, TokenSwapRequest, TransactionRequest,
        TransactionSend, TransactionStatusCheck,
    },
    models::{DefaultAppState, NetworkType},
    queues::schedule::{self, ScheduledJob},
    queues::{ensure_crypto_provider, QueueBackendType},
    utils::RedisConnections,
};

use super::{QueueBackend, QueueBackendError, QueueHealth, QueueType, WorkerHandle};

/// RabbitMQ 4.x default `max_message_size` (16 MiB) — pre-publish size guard.
pub(crate) const RABBITMQ_MAX_MESSAGE_SIZE_BYTES: usize = 16 * 1024 * 1024;

/// AMQP message header carrying the logical retry counter (single canonical
/// source — the broker's physical `redelivered` flag is deliberately not used,
/// it only signals channel-death redelivery, not handler failures).
pub(crate) const RETRY_ATTEMPT_HEADER: &str = "x-retry-attempt";

/// `content_type` set on every published message.
const CONTENT_TYPE_JSON: &str = "application/json";

/// `delivery_mode = 2` ⇒ persistent (written to disk on a durable queue).
const DELIVERY_MODE_PERSISTENT: u8 = 2;

/// Backend key segment for this backend's Redis scheduled sets
/// (`{key_prefix}:rabbitmq:scheduled:{queue}`), distinct from Pub/Sub's.
pub(crate) const RABBITMQ_SEGMENT: &str = "rabbitmq";

/// All queue types this backend serves (8 durable queues).
pub(crate) const ALL_QUEUE_TYPES: [QueueType; 8] = [
    QueueType::TransactionRequest,
    QueueType::TransactionSubmission,
    QueueType::StatusCheck,
    QueueType::StatusCheckEvm,
    QueueType::StatusCheckStellar,
    QueueType::Notification,
    QueueType::TokenSwapRequest,
    QueueType::RelayerHealthCheck,
];

// ── Credential parsing + redaction ───────────────────────────────────

/// Parses `RABBITMQ_URL` into an `AMQPUri`, returning a STATIC `ConfigError` on
/// failure.
///
/// Constitution I: `RABBITMQ_URL` embeds the broker password. `AMQPUri::from_str`
/// (and the underlying `url` parser) echo the raw URL in their error strings, so
/// the upstream `Err` is dropped and replaced with a fixed message that NEVER
/// includes the input. The classic typo this guards against is a missing `//`
/// after the scheme (e.g. `amqp:user:pass@host/vh`), which the `url` crate treats
/// as a "cannot-be-a-base" URL — under the old string redaction the userinfo
/// landed in the URL path and the password survived "redaction".
fn parse_amqp_url(url: &str) -> Result<AMQPUri, QueueBackendError> {
    AMQPUri::from_str(url).map_err(|_| {
        QueueBackendError::ConfigError(
            "RABBITMQ_URL is not a valid AMQP URL (expected \
             amqp(s)://[user:password@]host[:port][/vhost]; check for a missing \
             '//' after the scheme)"
                .to_string(),
        )
    })
}

/// Builds a redacted endpoint string (`scheme://host:port/vhost`, credentials and
/// query string omitted) from an already-parsed `AMQPUri`, for logs and errors.
///
/// Constitution I: building from the parsed URI's individual fields — never the
/// raw string — structurally guarantees the embedded `user:password` can never
/// reach a log or error message.
pub(crate) fn redact_amqp_uri(uri: &AMQPUri) -> String {
    // The default vhost decodes to "/"; trim the leading slash(es) so it renders
    // as a clean ".../{vhost}" (e.g. "amqp://host:5672/" rather than "...//").
    let vhost = uri.vhost.trim_start_matches('/');
    format!(
        "{}://{}:{}/{}",
        uri.scheme, uri.authority.host, uri.authority.port, vhost
    )
}

// ── Wire codec: Job<T> ⇄ AMQP message ────────────────────────────────

/// Errors if an encoded body exceeds RabbitMQ's 16 MiB message-size limit.
pub(crate) fn check_message_size(len: usize) -> Result<(), QueueBackendError> {
    if len > RABBITMQ_MAX_MESSAGE_SIZE_BYTES {
        return Err(QueueBackendError::SerializationError(format!(
            "Message body size ({len} bytes) exceeds RabbitMQ limit ({RABBITMQ_MAX_MESSAGE_SIZE_BYTES} bytes)"
        )));
    }
    Ok(())
}

/// Builds the AMQP properties for a published message: persistent
/// (`delivery_mode = 2`), `content_type = application/json`, a publish
/// `timestamp` (so the consumer can observe pickup latency), and the logical
/// retry counter as an `x-retry-attempt` long header.
pub(crate) fn message_properties(retry_attempt: usize) -> BasicProperties {
    let mut headers = FieldTable::default();
    headers.insert(
        ShortString::from(RETRY_ATTEMPT_HEADER),
        AMQPValue::LongLongInt(retry_attempt as i64),
    );
    BasicProperties::default()
        .with_delivery_mode(DELIVERY_MODE_PERSISTENT)
        .with_content_type(ShortString::from(CONTENT_TYPE_JSON))
        .with_timestamp(chrono::Utc::now().timestamp().max(0) as u64)
        .with_headers(headers)
}

/// Reads the logical retry attempt from a message's headers.
///
/// Defaults to 0 when the header is absent or non-integer (the first delivery of
/// a freshly produced job). The broker's `redelivered` flag is deliberately NOT
/// consulted.
pub(crate) fn retry_attempt_from_headers(properties: &BasicProperties) -> usize {
    properties
        .headers()
        .as_ref()
        .and_then(|h| h.inner().get(RETRY_ATTEMPT_HEADER))
        .and_then(amqp_value_as_usize)
        .unwrap_or(0)
}

/// Coerces any AMQP integer value to `usize` (the header is written as a long,
/// but be liberal in what we accept). Negative values fall back to `None` → 0.
fn amqp_value_as_usize(value: &AMQPValue) -> Option<usize> {
    match value {
        AMQPValue::LongLongInt(i) => usize::try_from(*i).ok(),
        AMQPValue::LongInt(i) => usize::try_from(*i).ok(),
        AMQPValue::ShortInt(i) => usize::try_from(*i).ok(),
        AMQPValue::ShortShortInt(i) => usize::try_from(*i).ok(),
        AMQPValue::LongUInt(i) => Some(*i as usize),
        AMQPValue::ShortUInt(i) => Some(*i as usize),
        AMQPValue::ShortShortUInt(i) => Some(*i as usize),
        _ => None,
    }
}

/// Serializes a `Job<T>` to its UTF-8 JSON body (the same serialization all
/// backends use), enforcing the 16 MiB pre-publish size limit.
pub(crate) fn job_to_body<T: Serialize>(job: &Job<T>) -> Result<Vec<u8>, QueueBackendError> {
    let body = serde_json::to_vec(job).map_err(|e| {
        error!(error = %e, "Failed to serialize job to RabbitMQ message");
        QueueBackendError::SerializationError(e.to_string())
    })?;
    check_message_size(body.len())?;
    Ok(body)
}

/// Publishes a body to a queue via the **default exchange** (`routing_key =
/// queue name`), persistent, and **awaits the publisher confirm** — returning
/// only after the broker has accepted the message (FR-009: a durable+persistent
/// message confirmed by the broker is on disk). A broker nack or transport error
/// is a `QueueBackendError`.
///
/// Published with `mandatory: true` so an UNROUTABLE message (e.g. the target
/// queue was deleted at runtime) is RETURNED by the broker rather than silently
/// discarded-then-acked. lapin surfaces a returned message as
/// `Confirmation::Ack(Some(_))`; only `Ack(None)` means the message was actually
/// routed and persisted, so a returned message is reported as an error (the job
/// was not enqueued) instead of a false success.
pub(crate) async fn publish_confirmed(
    channel: &Channel,
    queue: &str,
    body: &[u8],
    retry_attempt: usize,
) -> Result<(), QueueBackendError> {
    let confirm = channel
        .basic_publish(
            ShortString::from(""), // default exchange
            ShortString::from(queue),
            BasicPublishOptions {
                mandatory: true,
                ..BasicPublishOptions::default()
            },
            body,
            message_properties(retry_attempt),
        )
        .await
        .map_err(|e| QueueBackendError::QueueError(format!("RabbitMQ publish failed: {e}")))?
        .await
        .map_err(|e| {
            QueueBackendError::QueueError(format!("RabbitMQ publish confirm failed: {e}"))
        })?;

    match confirm {
        // Routed to a queue and confirmed by the broker.
        Confirmation::Ack(None) => Ok(()),
        // mandatory=true returned the message: it was NOT enqueued (no queue
        // accepted it). Surface the AMQP reason (code + text only — never the
        // body) as an error rather than reporting a false success.
        Confirmation::Ack(Some(returned)) => Err(QueueBackendError::QueueError(format!(
            "RabbitMQ returned publish to '{queue}' as unroutable (AMQP {} {}); message not enqueued",
            returned.reply_code,
            returned.reply_text.as_str()
        ))),
        Confirmation::Nack(_) => Err(QueueBackendError::QueueError(format!(
            "RabbitMQ broker nacked publish to '{queue}'"
        ))),
        Confirmation::NotRequested => Err(QueueBackendError::QueueError(format!(
            "RabbitMQ publish to '{queue}' returned no confirmation (confirms not enabled)"
        ))),
    }
}

// ── Resource naming + status routing ────────────────────────────────

/// Queue name for a queue type: `{prefix}-{queue_name}`.
pub(crate) fn queue_name(prefix: &str, queue_type: QueueType) -> String {
    format!("{prefix}-{}", queue_type.queue_name())
}

/// Selects the status-check queue for a network type, identical to the
/// SQS/Pub/Sub rule. EVM and Stellar use dedicated queues; all other/unknown
/// network types use the generic status-check queue. Status checks MUST NOT
/// collapse onto a single queue.
pub(crate) fn status_check_queue_type(network_type: Option<&NetworkType>) -> QueueType {
    match network_type {
        Some(NetworkType::Evm) => QueueType::StatusCheckEvm,
        Some(NetworkType::Stellar) => QueueType::StatusCheckStellar,
        _ => QueueType::StatusCheck,
    }
}

// ── Health & depth ───────────────────────────────────────────────────

/// Maps an observed ready-message `depth` to a `QueueHealth`. `Some(count)` is a
/// real ready-message count (including a genuine `Some(0)` empty queue) and the
/// queue is healthy; `None` means the depth is **unavailable** (disconnected or
/// the passive declare failed) — never a fake `0` — and the queue is unhealthy.
/// `messages_in_flight`/`messages_dlq` are 0: passive declare does not expose
/// unacked counts and the relayer declares no dead-letter queue.
fn build_queue_health(queue_type: QueueType, depth: Option<u64>) -> QueueHealth {
    QueueHealth {
        queue_type,
        messages_visible: depth,
        messages_in_flight: 0,
        messages_dlq: 0,
        backend: "rabbitmq".to_string(),
        is_healthy: depth.is_some(),
    }
}

// ── Startup declare / verify ─────────────────────────────────────────

/// The `queue_declare` options for the active resource policy: an idempotent
/// durable declare by default, or a passive (verify-only) declare in passive
/// mode. Both are durable, non-exclusive, non-auto-delete, no extra arguments.
fn declare_options(passive: bool) -> QueueDeclareOptions {
    if passive {
        QueueDeclareOptions::durable().passive()
    } else {
        QueueDeclareOptions::durable()
    }
}

/// Pure mapping from a queue name + optional AMQP reply code to an actionable
/// declare-failure message (testable without a live broker).
fn declare_failure_message(queue: &str, amqp_code: Option<u16>, detail: &str) -> String {
    match amqp_code {
        // Passive mode, queue missing.
        Some(404) => format!(
            "queue '{queue}' does not exist (RABBITMQ_PASSIVE_QUEUES=true verifies only, never creates) — create it, or unset RABBITMQ_PASSIVE_QUEUES to let the relayer declare it"
        ),
        // Pre-exists with conflicting arguments (non-durable, or a quorum queue
        // declared non-passively, etc.).
        Some(406) => format!(
            "queue '{queue}' exists with inequivalent arguments (e.g. non-durable, or a different queue type such as quorum) — reconcile its properties, or pre-provision it and set RABBITMQ_PASSIVE_QUEUES=true"
        ),
        // Declare needs 'configure' permission the broker user lacks.
        Some(403) => format!(
            "queue '{queue}' declare was access-refused (broker user lacks 'configure' permission) — pre-provision the queue and set RABBITMQ_PASSIVE_QUEUES=true"
        ),
        Some(code) => format!("queue '{queue}' declare failed (AMQP {code}): {detail}"),
        None => format!("queue '{queue}' declare failed: {detail}"),
    }
}

/// Classifies a `queue_declare` error into an actionable message.
fn declare_failure_reason(queue: &str, err: &lapin::Error) -> String {
    match err.kind() {
        ErrorKind::ProtocolError(amqp) => {
            declare_failure_message(queue, Some(amqp.get_id()), &amqp.to_string())
        }
        _ => declare_failure_message(queue, None, &err.to_string()),
    }
}

/// Builds the startup fail-fast error naming every queue setup problem.
fn queue_setup_error(failures: &[String], endpoint: &str) -> QueueBackendError {
    QueueBackendError::ConfigError(format!(
        "RabbitMQ backend initialization failed for {endpoint}. Queue setup problems: {}",
        failures.join("; ")
    ))
}

// ── Backend ──────────────────────────────────────────────────────────

/// RabbitMQ backend for job queue operations.
///
/// Constructed only after the connection succeeds AND all 8 queues are in place
/// (declared idempotently by default, or passively verified). Deferred-job
/// scheduling and cron locks reuse the relayer's existing Redis.
#[derive(Clone)]
pub struct RabbitMqBackend {
    /// Long-lived connection (auto-recovery enabled), one per process. Shared
    /// across clones; workers open consumer channels from it.
    connection: Arc<Connection>,
    /// Dedicated confirm-mode channel for all `produce_*` / due-sweep publishes.
    producer_channel: Channel,
    /// Broker queue name per queue type (`{prefix}-{queue_name}`).
    queue_names: HashMap<QueueType, String>,
    /// Reused for the scheduled-job sorted sets and cron `DistributedLock`.
    redis_connections: Arc<RedisConnections>,
    /// Redis key prefix for scheduled-set keys (same prefix the repos/locks use).
    key_prefix: String,
    /// From `RABBITMQ_PASSIVE_QUEUES` — verify-only vs declare.
    passive: bool,
    /// Pre-redacted endpoint (scheme+host+port+vhost) for logs/errors — the raw
    /// URL (with credentials) is never stored or logged.
    redacted_url: String,
    /// Broadcast graceful-shutdown signal to all workers and due-sweeps.
    shutdown_tx: Arc<watch::Sender<bool>>,
}

impl std::fmt::Debug for RabbitMqBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RabbitMqBackend")
            .field("backend_type", &"rabbitmq")
            .field("endpoint", &self.redacted_url)
            .field("queue_count", &self.queue_names.len())
            .field("passive", &self.passive)
            .finish()
    }
}

impl RabbitMqBackend {
    /// Creates a new RabbitMQ backend.
    ///
    /// Installs the rustls `CryptoProvider` (for `amqps://`), connects with
    /// auto-recovery enabled, opens a confirm-mode producer channel, and
    /// declares-or-verifies all 8 queues — failing fast with a `ConfigError`
    /// naming every queue setup problem (404 missing in passive mode, 406
    /// inequivalent args, 403 access-refused). All logs/errors use the redacted
    /// endpoint, never the raw `RABBITMQ_URL`.
    ///
    /// # Errors
    /// Returns `ConfigError` if `RABBITMQ_URL` is unset, the broker is
    /// unreachable / auth is refused, or any queue cannot be declared/verified.
    pub async fn new(redis_connections: Arc<RedisConnections>) -> Result<Self, QueueBackendError> {
        info!("Initializing RabbitMQ queue backend");

        // amqps:// TLS uses the process-default rustls CryptoProvider (002 lesson).
        ensure_crypto_provider();

        let url = ServerConfig::get_rabbitmq_url().map_err(QueueBackendError::ConfigError)?;
        let prefix = ServerConfig::get_rabbitmq_queue_prefix();
        let passive = ServerConfig::get_rabbitmq_passive_queues();

        // Parse the URL up front so the raw string (with credentials) never
        // reaches lapin's string-based error paths, and build the redacted
        // endpoint from the parsed fields (Constitution I).
        let uri = parse_amqp_url(&url)?;
        let redacted_url = redact_amqp_uri(&uri);

        // One connection with built-in auto-recovery (FR-010: reconnect after a
        // broker restart without restarting the relayer). lapin's default
        // auto-recover caps TCP reconnects at 16 attempts (~11 min) then gives up
        // permanently, which would brick the backend after a longer outage —
        // configure an unbounded retry with a capped delay so it keeps healing.
        let connection = Connection::connect_uri(
            uri,
            ConnectionProperties::default()
                .enable_auto_recover()
                .configure_backoff(|backoff| {
                    backoff
                        .without_max_times()
                        .with_max_delay(Duration::from_secs(30))
                }),
        )
        .await
        .map_err(|e| {
            QueueBackendError::ConfigError(format!(
                "Failed to connect to RabbitMQ at {redacted_url}: {e}"
            ))
        })?;

        // Dedicated confirm-mode channel: produce returns only after the broker
        // acks the publish (FR-009 durability).
        let producer_channel = connection.create_channel().await.map_err(|e| {
            QueueBackendError::ConfigError(format!(
                "Failed to open RabbitMQ producer channel at {redacted_url}: {e}"
            ))
        })?;
        producer_channel
            .confirm_select(ConfirmSelectOptions::default())
            .await
            .map_err(|e| {
                QueueBackendError::ConfigError(format!(
                    "Failed to enable publisher confirms at {redacted_url}: {e}"
                ))
            })?;

        let queue_names: HashMap<QueueType, String> = ALL_QUEUE_TYPES
            .iter()
            .map(|&qt| (qt, queue_name(&prefix, qt)))
            .collect();

        // Declare-or-verify every queue. A 406 closes the channel, so use a
        // fresh channel per attempt and collect ALL failures before erroring.
        let mut failures: Vec<String> = Vec::new();
        for &qt in ALL_QUEUE_TYPES.iter() {
            let name = &queue_names[&qt];
            let channel = match connection.create_channel().await {
                Ok(c) => c,
                Err(e) => {
                    failures.push(format!("queue '{name}': could not open a channel: {e}"));
                    continue;
                }
            };
            match channel
                .queue_declare(
                    name.as_str().into(),
                    declare_options(passive),
                    FieldTable::default(),
                )
                .await
            {
                Ok(_) => {
                    debug!(queue = %name, passive = passive, "RabbitMQ queue declared/verified")
                }
                Err(e) => failures.push(declare_failure_reason(name, &e)),
            }
        }
        if !failures.is_empty() {
            return Err(queue_setup_error(&failures, &redacted_url));
        }

        let (shutdown_tx, _) = watch::channel(false);
        let key_prefix = ServerConfig::get_redis_key_prefix();

        info!(
            queue_count = queue_names.len(),
            passive = passive,
            endpoint = %redacted_url,
            "RabbitMQ backend initialized"
        );

        Ok(Self {
            connection: Arc::new(connection),
            producer_channel,
            queue_names,
            redis_connections,
            key_prefix,
            passive,
            redacted_url,
            shutdown_tx: Arc::new(shutdown_tx),
        })
    }

    /// Enqueues a job: if `scheduled_on` is in the future, store it in the Redis
    /// scheduled set (published when due by the sweep); otherwise publish to the
    /// mapped queue now (confirmed). Returns the job's message id. First enqueue
    /// carries `retry_attempt = 0`.
    async fn enqueue<T: Serialize>(
        &self,
        queue_type: QueueType,
        job: &Job<T>,
        scheduled_on: Option<i64>,
    ) -> Result<String, QueueBackendError> {
        let now = chrono::Utc::now().timestamp();
        // Serialize + size-check once, up front, for both paths.
        let body = job_to_body(job)?;

        match scheduled_on {
            Some(run_at) if run_at > now => {
                // Defer: hold in Redis, published when due (store-and-run-when-due).
                let scheduled = ScheduledJob {
                    body: String::from_utf8(body).map_err(|e| {
                        QueueBackendError::SerializationError(format!(
                            "job body is not valid UTF-8: {e}"
                        ))
                    })?,
                    retry_attempt: 0,
                };
                schedule::zadd_scheduled(
                    self.redis_connections.primary(),
                    &self.key_prefix,
                    RABBITMQ_SEGMENT,
                    queue_type,
                    &scheduled,
                    run_at,
                )
                .await?;
                debug!(
                    queue_type = %queue_type,
                    run_at = run_at,
                    task_id = %job.message_id,
                    "Deferred job to Redis scheduled set"
                );
                Ok(job.message_id.clone())
            }
            // Immediate: publish now, confirmed (retry_attempt = 0).
            _ => {
                let queue = self
                    .queue_names
                    .get(&queue_type)
                    .ok_or_else(|| QueueBackendError::QueueNotFound(format!("{queue_type}")))?;
                publish_confirmed(&self.producer_channel, queue, &body, 0).await?;
                debug!(
                    queue_type = %queue_type,
                    queue = %queue,
                    task_id = %job.message_id,
                    "Published job to RabbitMQ queue (confirmed)"
                );
                Ok(job.message_id.clone())
            }
        }
    }
}

#[async_trait]
impl QueueBackend for RabbitMqBackend {
    async fn produce_transaction_request(
        &self,
        job: Job<TransactionRequest>,
        scheduled_on: Option<i64>,
    ) -> Result<String, QueueBackendError> {
        self.enqueue(QueueType::TransactionRequest, &job, scheduled_on)
            .await
    }

    async fn produce_transaction_submission(
        &self,
        job: Job<TransactionSend>,
        scheduled_on: Option<i64>,
    ) -> Result<String, QueueBackendError> {
        self.enqueue(QueueType::TransactionSubmission, &job, scheduled_on)
            .await
    }

    async fn produce_transaction_status_check(
        &self,
        job: Job<TransactionStatusCheck>,
        scheduled_on: Option<i64>,
    ) -> Result<String, QueueBackendError> {
        // Route by network type to one of three queues (never collapse to one).
        let queue_type = status_check_queue_type(job.data.network_type.as_ref());
        self.enqueue(queue_type, &job, scheduled_on).await
    }

    async fn produce_notification(
        &self,
        job: Job<NotificationSend>,
        scheduled_on: Option<i64>,
    ) -> Result<String, QueueBackendError> {
        self.enqueue(QueueType::Notification, &job, scheduled_on)
            .await
    }

    async fn produce_token_swap_request(
        &self,
        job: Job<TokenSwapRequest>,
        scheduled_on: Option<i64>,
    ) -> Result<String, QueueBackendError> {
        self.enqueue(QueueType::TokenSwapRequest, &job, scheduled_on)
            .await
    }

    async fn produce_relayer_health_check(
        &self,
        job: Job<RelayerHealthCheck>,
        scheduled_on: Option<i64>,
    ) -> Result<String, QueueBackendError> {
        self.enqueue(QueueType::RelayerHealthCheck, &job, scheduled_on)
            .await
    }

    async fn initialize_workers(
        &self,
        app_state: Arc<ThinData<DefaultAppState>>,
    ) -> Result<Vec<WorkerHandle>, QueueBackendError> {
        info!(
            queue_count = self.queue_names.len(),
            "Initializing RabbitMQ workers"
        );

        let mut handles = Vec::new();
        let pool = self.redis_connections.primary().clone();

        for &queue_type in ALL_QUEUE_TYPES.iter() {
            let queue = self.queue_names[&queue_type].clone();

            // One consumer loop per queue (consume → handle → settle).
            handles.push(super::worker::spawn_consumer_for_queue(
                self.connection.clone(),
                queue_type,
                queue.clone(),
                app_state.clone(),
                pool.clone(),
                self.key_prefix.clone(),
                self.redacted_url.clone(),
                self.shutdown_tx.subscribe(),
            ));

            // One due-sweep per queue: publishes deferred/retrying jobs to the
            // broker (confirmed) when due. On publish failure the shared
            // scheduler re-ZADDs the job scored `now`.
            let channel = self.producer_channel.clone();
            let queue_name = queue.clone();
            let publish = move |job: ScheduledJob| {
                let channel = channel.clone();
                let queue_name = queue_name.clone();
                async move {
                    publish_confirmed(
                        &channel,
                        &queue_name,
                        job.body.as_bytes(),
                        job.retry_attempt,
                    )
                    .await
                }
            };
            handles.push(schedule::spawn_due_sweep(
                RABBITMQ_SEGMENT,
                queue_type,
                publish,
                pool.clone(),
                self.key_prefix.clone(),
                self.shutdown_tx.subscribe(),
            ));
        }

        // Shared, backend-neutral cron scheduler (cleanup + token-swap crons).
        // Same scheduler/lock keys as the SQS/Pub/Sub backends, so a mixed fleet
        // runs each cron at most once per interval (no rabbitmq-specific keys).
        let cron_scheduler = crate::queues::cron::CronScheduler::new(
            app_state.clone(),
            self.shutdown_tx.subscribe(),
        );
        handles.extend(cron_scheduler.start().await?);

        // SIGINT/SIGTERM → broadcast shutdown to all consumers and due-sweeps.
        {
            let shutdown_tx = self.shutdown_tx.clone();
            let handle = tokio::spawn(async move {
                let mut sigint =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
                        .expect("Failed to create SIGINT handler");
                let mut sigterm =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                        .expect("Failed to create SIGTERM handler");
                tokio::select! {
                    _ = sigint.recv() => info!("RabbitMQ backend: received SIGINT, shutting down"),
                    _ = sigterm.recv() => info!("RabbitMQ backend: received SIGTERM, shutting down"),
                }
                let _ = shutdown_tx.send(true);
            });
            handles.push(WorkerHandle::Tokio(handle));
        }

        info!(
            handle_count = handles.len(),
            "RabbitMQ workers and due-sweeps started"
        );
        Ok(handles)
    }

    async fn health_check(&self) -> Result<Vec<QueueHealth>, QueueBackendError> {
        // Per-queue reachability + ready-message depth via a passive
        // `queue_declare` over the live connection. A passive declare on a
        // missing queue returns a 404 that CLOSES the channel, so each probe
        // uses a fresh channel (never the producer channel). While disconnected
        // or on any probe failure, depth is reported unavailable (`None`) and
        // the queue unhealthy — never a fake `0` (and the gauge is left stale
        // rather than zeroed).
        let connected = self.connection.status().connected();

        let mut health_statuses = Vec::with_capacity(ALL_QUEUE_TYPES.len());
        for &queue_type in ALL_QUEUE_TYPES.iter() {
            let name = &self.queue_names[&queue_type];

            let depth: Option<u64> = if !connected {
                None
            } else {
                match self.connection.create_channel().await {
                    Ok(channel) => match channel
                        .queue_declare(
                            name.as_str().into(),
                            QueueDeclareOptions::durable().passive(),
                            FieldTable::default(),
                        )
                        .await
                    {
                        Ok(queue) => Some(u64::from(queue.message_count())),
                        Err(e) => {
                            warn!(
                                queue_type = %queue_type,
                                queue = %name,
                                error = %e,
                                "RabbitMQ health probe failed; depth unavailable"
                            );
                            None
                        }
                    },
                    Err(e) => {
                        warn!(
                            queue_type = %queue_type,
                            error = %e,
                            "RabbitMQ health channel open failed; depth unavailable"
                        );
                        None
                    }
                }
            };

            // Feed the gauge only when depth is available (never a fake 0 while
            // unavailable — leave the last value stale, matching the Pub/Sub
            // precedent).
            if let Some(d) = depth {
                crate::metrics::set_queue_depth("rabbitmq", queue_type.queue_name(), d as f64);
            }

            health_statuses.push(build_queue_health(queue_type, depth));
        }
        Ok(health_statuses)
    }

    fn backend_type(&self) -> QueueBackendType {
        QueueBackendType::RabbitMq
    }

    fn shutdown(&self) {
        info!("RabbitMQ backend: broadcasting shutdown signal to all workers");
        let _ = self.shutdown_tx.send(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::{Job, JobType, TransactionStatusCheck};
    use crate::models::NetworkType;

    fn sample_status_job(network: Option<NetworkType>) -> Job<TransactionStatusCheck> {
        let data = TransactionStatusCheck {
            transaction_id: "tx-1".to_string(),
            relayer_id: "relayer-1".to_string(),
            network_type: network,
            metadata: None,
        };
        Job::new(JobType::TransactionStatusCheck, data)
    }

    // ── credential parsing + redaction (Constitution I) ────────────

    #[test]
    fn test_redact_amqp_uri_strips_credentials() {
        // The password must never survive redaction; scheme/host/port/vhost stay.
        // Default vhost "/%2f" decodes to "/" and renders as a trailing slash.
        let uri = AMQPUri::from_str("amqp://user:s3cr3t@broker.example:5672/%2f").unwrap();
        let redacted = redact_amqp_uri(&uri);
        assert!(
            !redacted.contains("s3cr3t"),
            "redaction leaked the password: {redacted}"
        );
        assert!(!redacted.contains("user"), "redaction leaked the user");
        assert_eq!(redacted, "amqp://broker.example:5672/");
    }

    #[test]
    fn test_redact_amqp_uri_tls_custom_vhost_and_drops_query() {
        // amqps + custom vhost, no credentials present.
        let uri = AMQPUri::from_str("amqps://broker:5671/prod").unwrap();
        assert_eq!(redact_amqp_uri(&uri), "amqps://broker:5671/prod");
        // Query string (e.g. heartbeat) is dropped, credentials stripped — assert
        // the exact redacted endpoint (not a substring guard).
        let uri = AMQPUri::from_str("amqps://u:p@h:5671/v?heartbeat=20").unwrap();
        assert_eq!(redact_amqp_uri(&uri), "amqps://h:5671/v");
    }

    #[test]
    fn test_parse_amqp_url_rejects_missing_slashes_without_echoing_secret() {
        // The "cannot-be-a-base" typo (missing `//`) is what leaked the password
        // under the old string redaction. It must be rejected with a STATIC error
        // that never echoes the input (which embeds the password).
        let err =
            parse_amqp_url("amqp:user:s3cr3t@host/vh").expect_err("missing // must be rejected");
        assert!(
            !err.to_string().contains("s3cr3t"),
            "parse error leaked the password: {err}"
        );
        assert!(matches!(err, QueueBackendError::ConfigError(_)));
    }

    #[test]
    fn test_parse_amqp_url_rejects_garbage_without_echoing_input() {
        // A garbage URL (which could itself embed a secret) is rejected with the
        // same static error — never echoed back.
        let err = parse_amqp_url("not a url with maybe-a-secret").expect_err("garbage rejected");
        assert!(!err.to_string().contains("secret"));
        assert!(matches!(err, QueueBackendError::ConfigError(_)));
    }

    // ── wire codec ─────────────────────────────────────────────────

    #[test]
    fn test_retry_header_round_trip() {
        let props = message_properties(3);
        assert_eq!(retry_attempt_from_headers(&props), 3);
        assert_eq!(props.delivery_mode(), &Some(DELIVERY_MODE_PERSISTENT));
        assert_eq!(
            props.content_type().as_ref().map(|s| s.as_str()),
            Some(CONTENT_TYPE_JSON)
        );
    }

    #[test]
    fn test_retry_attempt_missing_header_defaults_to_zero() {
        // No headers at all → 0 (first delivery of a freshly produced job).
        let bare = BasicProperties::default();
        assert_eq!(retry_attempt_from_headers(&bare), 0);

        // Headers present but no x-retry-attempt → 0.
        let other = BasicProperties::default().with_headers(FieldTable::default());
        assert_eq!(retry_attempt_from_headers(&other), 0);

        // Present and valid round-trips.
        assert_eq!(retry_attempt_from_headers(&message_properties(7)), 7);
    }

    #[test]
    fn test_check_message_size_boundary() {
        assert!(check_message_size(RABBITMQ_MAX_MESSAGE_SIZE_BYTES).is_ok());
        let err = check_message_size(RABBITMQ_MAX_MESSAGE_SIZE_BYTES + 1)
            .expect_err("over-limit must error");
        assert!(matches!(err, QueueBackendError::SerializationError(_)));
        assert!(err.to_string().contains("exceeds RabbitMQ limit"));
    }

    #[test]
    fn test_job_to_body_round_trips_and_rejects_oversize() {
        // Normal job serializes to its JSON body.
        let job = sample_status_job(Some(NetworkType::Evm));
        let body = job_to_body(&job).expect("encode");
        let decoded: Job<TransactionStatusCheck> = serde_json::from_slice(&body).expect("decode");
        assert_eq!(decoded.message_id, job.message_id);
        assert_eq!(decoded.data.transaction_id, "tx-1");

        // Oversize payload is rejected pre-publish with a clear size error.
        let big = "x".repeat(RABBITMQ_MAX_MESSAGE_SIZE_BYTES + 1024);
        let data = TransactionStatusCheck {
            transaction_id: "tx".to_string(),
            relayer_id: "r".to_string(),
            network_type: Some(NetworkType::Evm),
            metadata: Some(std::collections::HashMap::from([("blob".to_string(), big)])),
        };
        let oversized = Job::new(JobType::TransactionStatusCheck, data);
        let err = job_to_body(&oversized).expect_err("oversized payload must error");
        assert!(matches!(err, QueueBackendError::SerializationError(_)));
        assert!(err.to_string().contains("exceeds RabbitMQ limit"));
    }

    // ── resource naming + status routing ───────────────────────────

    #[test]
    fn test_queue_names_all_eight() {
        let prefix = "relayer";
        let expected = [
            (QueueType::TransactionRequest, "relayer-transaction-request"),
            (
                QueueType::TransactionSubmission,
                "relayer-transaction-submission",
            ),
            (QueueType::StatusCheck, "relayer-status-check"),
            (QueueType::StatusCheckEvm, "relayer-status-check-evm"),
            (
                QueueType::StatusCheckStellar,
                "relayer-status-check-stellar",
            ),
            (QueueType::Notification, "relayer-notification"),
            (QueueType::TokenSwapRequest, "relayer-token-swap-request"),
            (
                QueueType::RelayerHealthCheck,
                "relayer-relayer-health-check",
            ),
        ];
        for (qt, name) in expected {
            assert_eq!(queue_name(prefix, qt), name);
        }
        // All 8 names are distinct.
        let names: std::collections::HashSet<String> = ALL_QUEUE_TYPES
            .iter()
            .map(|&qt| queue_name(prefix, qt))
            .collect();
        assert_eq!(names.len(), 8, "queue names must be distinct");
    }

    #[test]
    fn test_status_check_routing_four_cases() {
        assert_eq!(
            status_check_queue_type(Some(&NetworkType::Evm)),
            QueueType::StatusCheckEvm
        );
        assert_eq!(
            status_check_queue_type(Some(&NetworkType::Stellar)),
            QueueType::StatusCheckStellar
        );
        assert_eq!(
            status_check_queue_type(Some(&NetworkType::Solana)),
            QueueType::StatusCheck
        );
        assert_eq!(status_check_queue_type(None), QueueType::StatusCheck);
    }

    // ── declare mode selection + error composition (pure parts) ─────

    #[test]
    fn test_declare_options_mode_selection() {
        let active = declare_options(false);
        assert!(active.durable, "default declares must be durable");
        assert!(!active.passive, "default mode must not be passive");
        assert!(!active.exclusive && !active.auto_delete);

        let verify = declare_options(true);
        assert!(verify.passive, "passive mode must verify only");
        assert!(verify.durable, "passive declare still names durable");
    }

    #[test]
    fn test_declare_failure_message_is_actionable_per_code() {
        let missing = declare_failure_message("relayer-status-check", Some(404), "");
        assert!(missing.contains("relayer-status-check"));
        assert!(missing.contains("RABBITMQ_PASSIVE_QUEUES"));

        let conflict = declare_failure_message("relayer-notification", Some(406), "");
        assert!(conflict.contains("relayer-notification"));
        assert!(conflict.contains("inequivalent"));

        let refused = declare_failure_message("relayer-transaction-request", Some(403), "");
        assert!(refused.contains("relayer-transaction-request"));
        assert!(refused.contains("configure"));
        assert!(refused.contains("RABBITMQ_PASSIVE_QUEUES=true"));

        // Unknown code / non-protocol error keep the detail.
        assert!(declare_failure_message("q", Some(500), "boom").contains("boom"));
        assert!(declare_failure_message("q", None, "io error").contains("io error"));
    }

    #[test]
    fn test_queue_setup_error_names_every_problem_and_endpoint() {
        let failures = vec![
            "queue 'relayer-status-check' does not exist".to_string(),
            "queue 'relayer-notification' exists with inequivalent arguments".to_string(),
        ];
        let err = queue_setup_error(&failures, "amqp://broker:5672/%2f");
        assert!(matches!(err, QueueBackendError::ConfigError(_)));
        let msg = err.to_string();
        assert!(msg.contains("relayer-status-check"));
        assert!(msg.contains("relayer-notification"));
        assert!(msg.contains("amqp://broker:5672/%2f"));
        // The redacted endpoint carries no credentials.
        assert!(!msg.contains('@'));
    }

    #[test]
    fn test_all_queue_types_has_eight_distinct() {
        assert_eq!(ALL_QUEUE_TYPES.len(), 8);
        let names: std::collections::HashSet<&str> =
            ALL_QUEUE_TYPES.iter().map(|qt| qt.queue_name()).collect();
        assert_eq!(names.len(), 8);
    }

    // ── health shape: unavailable is never 0 ───────────────────────

    #[test]
    fn test_build_queue_health_available_reports_real_depth() {
        // A real count → healthy + Some(count); a genuine empty queue is Some(0),
        // distinct from the unavailable None.
        let h = build_queue_health(QueueType::StatusCheckEvm, Some(42));
        assert!(h.is_healthy);
        assert_eq!(h.messages_visible, Some(42));
        assert_eq!(h.backend, "rabbitmq");
        assert_eq!(h.messages_in_flight, 0);
        assert_eq!(h.messages_dlq, 0);

        let empty = build_queue_health(QueueType::TransactionRequest, Some(0));
        assert!(empty.is_healthy, "an empty-but-reachable queue is healthy");
        assert_eq!(empty.messages_visible, Some(0));
    }

    #[test]
    fn test_build_queue_health_unavailable_is_none_and_unhealthy() {
        // Disconnected / probe failure → unhealthy + None depth, NEVER a fake 0.
        let h = build_queue_health(QueueType::Notification, None);
        assert!(!h.is_healthy);
        assert_eq!(h.messages_visible, None);
    }
}
