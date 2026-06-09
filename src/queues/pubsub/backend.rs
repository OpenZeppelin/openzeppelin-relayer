//! GCP Pub/Sub backend implementation.
//!
//! Provides a Pub/Sub-backed implementation of the `QueueBackend` trait. Each of
//! the 8 queue types maps to a topic (publish) and a subscription (consume).
//! Deferred/retrying jobs are held in Redis sorted sets and published when due
//! (store-and-run-when-due), so the topic only ever carries already-due jobs.
//!
//! Anchored to the proven Redis/Apalis semantics; uses `src/queues/sqs/` only as
//! a reference for the dumb-pipe plumbing.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use gcloud_googleapis::pubsub::v1::PubsubMessage;
use gcloud_pubsub::client::{Client, ClientConfig};
use gcloud_pubsub::publisher::Publisher;
use parking_lot::RwLock;
use rustls::crypto::{aws_lc_rs, CryptoProvider};
use serde::Serialize;
use token_source::TokenSource;
use tokio::sync::watch;
use tracing::{debug, error, info, warn};

use crate::{
    config::ServerConfig,
    jobs::{
        Job, NotificationSend, RelayerHealthCheck, TokenSwapRequest, TransactionRequest,
        TransactionSend, TransactionStatusCheck,
    },
    models::{DefaultAppState, NetworkType},
    queues::QueueBackendType,
    utils::RedisConnections,
};
use actix_web::web::ThinData;

use super::schedule::{self, ScheduledJob};
use super::{monitoring, QueueBackend, QueueBackendError, QueueHealth, QueueType, WorkerHandle};

/// How often the backlog-depth snapshot is refreshed from Cloud Monitoring.
const DEPTH_REFRESH_INTERVAL_SECS: u64 = 60;

/// Cached backlog-depth snapshot. `available = false` means depth is unavailable
/// (the read failed, or we're under the emulator) — never reported as 0.
#[derive(Clone, Default)]
struct DepthSnapshot {
    available: bool,
    depths: HashMap<QueueType, u64>,
}

/// Holds what the low-frequency Cloud Monitoring depth read needs.
#[derive(Clone)]
struct MonitoringReader {
    http: reqwest::Client,
    token_source: Arc<dyn TokenSource>,
    /// Reverse of `subscription_names`: subscription id → queue type.
    subscription_to_queue: HashMap<String, QueueType>,
}

/// Backlog depth for a queue from the cached snapshot, for the health endpoint.
///
/// `Some(n)` is a real count (including a genuine `Some(0)` empty queue); `None`
/// is **unavailable** (read hasn't succeeded / emulator) — never conflated with
/// empty.
fn depth_for(snapshot: &DepthSnapshot, queue_type: QueueType) -> Option<u64> {
    if snapshot.available {
        Some(snapshot.depths.get(&queue_type).copied().unwrap_or(0))
    } else {
        None
    }
}

/// Pub/Sub maximum message size (10 MB).
pub(crate) const PUBSUB_MAX_MESSAGE_SIZE_BYTES: usize = 10 * 1024 * 1024;

/// Message attribute carrying the logical retry counter (single canonical
/// source — Pub/Sub's physical `delivery_attempt` is deliberately not used).
pub(crate) const RETRY_ATTEMPT_ATTR: &str = "retry_attempt";

/// All queue types this backend serves (8 topics + 8 subscriptions).
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

// ── Wire codec: Job<T> ⇄ PubsubMessage ──────────────────────────────

/// Errors if an encoded body exceeds Pub/Sub's 10 MB message-size limit.
pub(crate) fn check_message_size(len: usize) -> Result<(), QueueBackendError> {
    if len > PUBSUB_MAX_MESSAGE_SIZE_BYTES {
        return Err(QueueBackendError::SerializationError(format!(
            "Message body size ({len} bytes) exceeds Pub/Sub limit ({PUBSUB_MAX_MESSAGE_SIZE_BYTES} bytes)"
        )));
    }
    Ok(())
}

/// Builds a `PubsubMessage` from an already-serialized body + logical retry
/// attempt. The `retry_attempt` travels as a string attribute and `ordering_key`
/// is left unset (ordering off in v1). No size check: the
/// body was validated by the producer (see [`check_message_size`]).
pub(crate) fn message_from_body(data: Vec<u8>, retry_attempt: usize) -> PubsubMessage {
    let mut attributes = HashMap::new();
    attributes.insert(RETRY_ATTEMPT_ATTR.to_string(), retry_attempt.to_string());
    PubsubMessage {
        data,
        attributes,
        ordering_key: String::new(),
        ..Default::default()
    }
}

/// Serializes a `Job<T>` into a `PubsubMessage`.
///
/// The body is UTF-8 JSON (the same serialization the SQS/Redis backends use).
/// Returns a serialization/size error if the encoded body exceeds Pub/Sub's
/// 10 MB limit.
pub(crate) fn job_to_message<T: Serialize>(
    job: &Job<T>,
    retry_attempt: usize,
) -> Result<PubsubMessage, QueueBackendError> {
    let data = serde_json::to_vec(job).map_err(|e| {
        error!(error = %e, "Failed to serialize job to Pub/Sub message");
        QueueBackendError::SerializationError(e.to_string())
    })?;
    check_message_size(data.len())?;
    Ok(message_from_body(data, retry_attempt))
}

/// Reads the logical retry attempt from a message's attributes.
///
/// Defaults to 0 when the attribute is absent or unparsable (the first
/// delivery of a freshly produced job). Pub/Sub's physical `delivery_attempt`
/// is deliberately NOT consulted (absent without a dead-letter policy; also
/// counts non-failure redeliveries).
pub(crate) fn retry_attempt_from_attrs(attributes: &HashMap<String, String>) -> usize {
    attributes
        .get(RETRY_ATTEMPT_ATTR)
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0)
}

// ── Resource naming + status routing ────────────────────────────────

/// Topic name for a queue type: `{prefix}-{queue_name}`.
pub(crate) fn topic_name(prefix: &str, queue_type: QueueType) -> String {
    format!("{prefix}-{}", queue_type.queue_name())
}

/// Subscription name for a queue type: `{prefix}-{queue_name}-sub`.
pub(crate) fn subscription_name(prefix: &str, queue_type: QueueType) -> String {
    format!("{prefix}-{}-sub", queue_type.queue_name())
}

/// Builds the startup fail-fast error naming every missing/inaccessible
/// topic/subscription. Each entry already says exactly what is missing.
fn missing_resources_error(missing: &[String]) -> QueueBackendError {
    QueueBackendError::ConfigError(format!(
        "Pub/Sub backend initialization failed. Missing/inaccessible resources: {}",
        missing.join("; ")
    ))
}

/// Selects the status-check queue for a network type, mirroring the SQS backend.
///
/// EVM and Stellar use dedicated queues (independent concurrency pools +
/// network-tuned backoff); all other/unknown network types use the generic
/// status-check queue. Status checks MUST NOT collapse onto a single queue.
pub(crate) fn status_check_queue_type(network_type: Option<&NetworkType>) -> QueueType {
    match network_type {
        Some(NetworkType::Evm) => QueueType::StatusCheckEvm,
        Some(NetworkType::Stellar) => QueueType::StatusCheckStellar,
        _ => QueueType::StatusCheck,
    }
}

// ── Backend ──────────────────────────────────────────────────────────

/// GCP Pub/Sub backend for job queue operations.
///
/// Constructed only after a startup probe confirms every required topic AND
/// subscription exists. Deferred-job scheduling and cron locks reuse the
/// relayer's existing Redis.
#[derive(Clone)]
pub struct PubSubBackend {
    /// Pub/Sub client (cloneable, holds the gRPC connection pool).
    client: Client,
    /// GCP project id (from `PUBSUB_PROJECT_ID`).
    project_id: String,
    /// Topic name per queue type (publish channel).
    topic_names: HashMap<QueueType, String>,
    /// Subscription name per queue type (consume channel).
    subscription_names: HashMap<QueueType, String>,
    /// Cached per-topic publishers (each runs background flush tasks).
    publishers: HashMap<QueueType, Publisher>,
    /// Reused for the scheduled-job sorted sets and cron `DistributedLock`.
    redis_connections: Arc<RedisConnections>,
    /// Redis key prefix for scheduled-set keys (same prefix the repos/locks use).
    key_prefix: String,
    /// True when targeting the emulator (Cloud Monitoring depth is unavailable).
    emulator: bool,
    /// Cached backlog-depth snapshot, refreshed by a low-frequency Cloud
    /// Monitoring read. Shared across clones so `health_check` sees fresh data.
    depth_snapshot: Arc<RwLock<DepthSnapshot>>,
    /// Cloud Monitoring depth reader; `None` under the emulator or when ADC for
    /// monitoring can't be resolved (depth then stays unavailable).
    monitoring: Option<MonitoringReader>,
    /// Broadcast graceful-shutdown signal to all workers and cron tasks.
    shutdown_tx: Arc<watch::Sender<bool>>,
}

impl std::fmt::Debug for PubSubBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PubSubBackend")
            .field("backend_type", &"pubsub")
            .field("project_id", &self.project_id)
            .field("queue_count", &self.topic_names.len())
            .field("emulator", &self.emulator)
            .finish()
    }
}

impl PubSubBackend {
    /// Creates a new Pub/Sub backend.
    ///
    /// Authenticates via ADC (or targets the emulator when `PUBSUB_EMULATOR_HOST`
    /// is set), builds the topic/subscription maps and cached publishers, and
    /// probes that every required topic AND subscription exists — failing fast
    /// with a `ConfigError` naming what is missing. Dead-letter topics are NOT
    /// probed.
    ///
    /// # Errors
    /// Returns `ConfigError` if `PUBSUB_PROJECT_ID` is unset, ADC cannot be
    /// resolved, or any required topic/subscription is missing/inaccessible.
    pub async fn new(redis_connections: Arc<RedisConnections>) -> Result<Self, QueueBackendError> {
        info!("Initializing Pub/Sub queue backend");

        // rustls 0.23 needs a process-default CryptoProvider when more than one
        // provider is compiled in. Both are present here (aws-lc-rs via the gcloud
        // and AWS SDK trees, ring via aws-config/Solana), so rustls can't select
        // one automatically and gcloud-pubsub's TLS — which uses the process
        // default — panics on the first real-GCP connection. The emulator path is
        // plaintext, so this only surfaces against real GCP. Install once; ignore
        // if a default is already set.
        if CryptoProvider::get_default().is_none() {
            let _ = aws_lc_rs::default_provider().install_default();
        }

        let project_id =
            ServerConfig::get_pubsub_project_id().map_err(QueueBackendError::ConfigError)?;
        let topic_prefix = ServerConfig::get_pubsub_topic_prefix();
        let emulator_host = ServerConfig::get_pubsub_emulator_host();
        let emulator = emulator_host.is_some();

        // ClientConfig::default() picks up PUBSUB_EMULATOR_HOST and selects the
        // Emulator environment; we always pin the project id explicitly. Real
        // GCP additionally resolves ADC via with_auth().
        let config = ClientConfig {
            project_id: Some(project_id.clone()),
            ..ClientConfig::default()
        };
        let config = if emulator {
            info!(
                project_id = %project_id,
                emulator_host = %emulator_host.unwrap_or_default(),
                "Pub/Sub backend targeting emulator (auth skipped)"
            );
            config
        } else {
            config.with_auth().await.map_err(|e| {
                QueueBackendError::ConfigError(format!(
                    "Pub/Sub ADC authentication failed (set GOOGLE_APPLICATION_CREDENTIALS, \
                     use workload identity, or the GCE metadata server): {e}"
                ))
            })?
        };

        let client = Client::new(config).await.map_err(|e| {
            QueueBackendError::ConfigError(format!("Failed to create Pub/Sub client: {e}"))
        })?;

        let topic_names: HashMap<QueueType, String> = ALL_QUEUE_TYPES
            .iter()
            .map(|&qt| (qt, topic_name(&topic_prefix, qt)))
            .collect();
        let subscription_names: HashMap<QueueType, String> = ALL_QUEUE_TYPES
            .iter()
            .map(|&qt| (qt, subscription_name(&topic_prefix, qt)))
            .collect();

        // Probe every topic AND subscription concurrently; fail fast on any
        // missing/inaccessible resource (mirrors the SQS startup probe).
        let probes = ALL_QUEUE_TYPES.iter().map(|&qt| {
            let client = client.clone();
            let topic = topic_names[&qt].clone();
            let sub = subscription_names[&qt].clone();
            async move {
                let topic_exists = client.topic(&topic).exists(None).await;
                let sub_exists = client.subscription(&sub).exists(None).await;
                (qt, topic, sub, topic_exists, sub_exists)
            }
        });
        let probe_results = futures::future::join_all(probes).await;

        let mut missing: Vec<String> = Vec::new();
        for (qt, topic, sub, topic_exists, sub_exists) in probe_results {
            match topic_exists {
                Ok(true) => debug!(queue_type = %qt, topic = %topic, "Pub/Sub topic probe ok"),
                Ok(false) => missing.push(format!("topic '{topic}' (for {qt}) does not exist")),
                Err(e) => missing.push(format!("topic '{topic}' (for {qt}) probe failed: {e}")),
            }
            match sub_exists {
                Ok(true) => {
                    debug!(queue_type = %qt, subscription = %sub, "Pub/Sub subscription probe ok")
                }
                Ok(false) => {
                    missing.push(format!("subscription '{sub}' (for {qt}) does not exist"))
                }
                Err(e) => {
                    missing.push(format!("subscription '{sub}' (for {qt}) probe failed: {e}"))
                }
            }
        }
        if !missing.is_empty() {
            return Err(missing_resources_error(&missing));
        }

        // Cache one publisher per topic (each runs background flush tasks).
        let publishers: HashMap<QueueType, Publisher> = topic_names
            .iter()
            .map(|(&qt, name)| (qt, client.topic(name).new_publisher(None)))
            .collect();

        let key_prefix = ServerConfig::get_redis_key_prefix();
        let (shutdown_tx, _) = watch::channel(false);

        // Cloud Monitoring depth reader (real GCP only). Failure to set up ADC
        // for monitoring is non-fatal — depth simply stays unavailable; the
        // backend's core publish/consume path does not depend on it.
        let monitoring = if emulator {
            None
        } else {
            match monitoring::monitoring_token_source().await {
                Ok(token_source) => {
                    let subscription_to_queue = subscription_names
                        .iter()
                        .map(|(&qt, name)| (name.clone(), qt))
                        .collect();
                    Some(MonitoringReader {
                        http: reqwest::Client::new(),
                        token_source,
                        subscription_to_queue,
                    })
                }
                Err(e) => {
                    warn!(error = %e, "Cloud Monitoring depth read disabled; backlog depth will be unavailable");
                    None
                }
            }
        };

        info!(
            project_id = %project_id,
            queue_count = topic_names.len(),
            emulator = emulator,
            depth_read = monitoring.is_some(),
            "Pub/Sub backend initialized"
        );

        Ok(Self {
            client,
            project_id,
            topic_names,
            subscription_names,
            publishers,
            redis_connections,
            key_prefix,
            emulator,
            depth_snapshot: Arc::new(RwLock::new(DepthSnapshot::default())),
            monitoring,
            shutdown_tx: Arc::new(shutdown_tx),
        })
    }

    /// Enqueues a job: if `scheduled_on` is in the future, store it in the Redis
    /// scheduled set (published when due by the sweep); otherwise publish to the
    /// mapped topic now. Returns the server message id (immediate) or the job's
    /// message id (deferred). First enqueue carries `retry_attempt = 0`.
    async fn enqueue<T: Serialize>(
        &self,
        queue_type: QueueType,
        job: &Job<T>,
        scheduled_on: Option<i64>,
    ) -> Result<String, QueueBackendError> {
        let now = chrono::Utc::now().timestamp();
        match scheduled_on {
            Some(run_at) if run_at > now => {
                // Defer: hold in Redis, published when due (store-and-run-when-due).
                let body = serde_json::to_vec(job).map_err(|e| {
                    error!(queue_type = %queue_type, error = %e, "Failed to serialize job");
                    QueueBackendError::SerializationError(e.to_string())
                })?;
                check_message_size(body.len())?;
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
            // Immediate: serialize + size-check + publish now (retry_attempt = 0).
            _ => {
                let message = job_to_message(job, 0)?;
                self.publish(queue_type, message).await
            }
        }
    }

    /// Publishes a message to a queue's topic now, returning the server message id.
    async fn publish(
        &self,
        queue_type: QueueType,
        message: PubsubMessage,
    ) -> Result<String, QueueBackendError> {
        let publisher = self
            .publishers
            .get(&queue_type)
            .ok_or_else(|| QueueBackendError::QueueNotFound(format!("{queue_type}")))?;

        let awaiter = publisher.publish(message).await;
        let message_id = awaiter.get().await.map_err(|e| {
            error!(queue_type = %queue_type, error = %e, "Pub/Sub publish failed");
            QueueBackendError::QueueError(format!("Pub/Sub publish failed: {e}"))
        })?;

        debug!(
            queue_type = %queue_type,
            message_id = %message_id,
            "Published message to Pub/Sub topic"
        );
        Ok(message_id)
    }
}

#[async_trait]
impl QueueBackend for PubSubBackend {
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
            queue_count = self.topic_names.len(),
            "Initializing Pub/Sub workers"
        );

        let mut handles = Vec::new();
        let pool = self.redis_connections.primary().clone();

        for &queue_type in ALL_QUEUE_TYPES.iter() {
            let subscription_name = self.subscription_names[&queue_type].clone();

            // One pull-loop worker per subscription (consume → handle → settle).
            handles.push(super::worker::spawn_worker_for_subscription(
                self.client.clone(),
                queue_type,
                subscription_name,
                app_state.clone(),
                pool.clone(),
                self.key_prefix.clone(),
                self.shutdown_tx.subscribe(),
            ));

            // One due-sweep per queue (publishes deferred/retrying jobs when due).
            handles.push(schedule::spawn_due_sweep(
                queue_type,
                self.publishers[&queue_type].clone(),
                pool.clone(),
                self.key_prefix.clone(),
                self.shutdown_tx.subscribe(),
            ));
        }

        // Shared, backend-neutral cron scheduler (cleanup + token-swap crons).
        // Same scheduler/lock keys as the SQS backend, so a mixed fleet runs
        // each cron at most once per interval.
        let cron_scheduler = crate::queues::cron::CronScheduler::new(
            app_state.clone(),
            self.shutdown_tx.subscribe(),
        );
        handles.extend(cron_scheduler.start().await?);

        // Low-frequency, batched Cloud Monitoring depth read → snapshot + gauge.
        // Only on real GCP; under the emulator depth stays unavailable.
        if let Some(reader) = self.monitoring.clone() {
            let snapshot = self.depth_snapshot.clone();
            let project_id = self.project_id.clone();
            let mut shutdown_rx = self.shutdown_tx.subscribe();
            let handle = tokio::spawn(async move {
                let interval = Duration::from_secs(DEPTH_REFRESH_INTERVAL_SECS);
                loop {
                    match monitoring::read_backlog_depths(
                        &reader.http,
                        &reader.token_source,
                        &project_id,
                        &reader.subscription_to_queue,
                    )
                    .await
                    {
                        Ok(depths) => {
                            for (qt, depth) in &depths {
                                crate::metrics::set_queue_depth(
                                    "pubsub",
                                    qt.queue_name(),
                                    *depth as f64,
                                );
                            }
                            *snapshot.write() = DepthSnapshot {
                                available: true,
                                depths,
                            };
                        }
                        Err(e) => {
                            // Mark unavailable (never report a stale/0 depth as real).
                            snapshot.write().available = false;
                            warn!(error = %e, "Cloud Monitoring depth read failed; depth unavailable");
                        }
                    }
                    tokio::select! {
                        _ = tokio::time::sleep(interval) => {}
                        _ = shutdown_rx.changed() => break,
                    }
                }
            });
            handles.push(WorkerHandle::Tokio(handle));
        }

        // SIGINT/SIGTERM → broadcast shutdown to all workers and due-sweeps.
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
                    _ = sigint.recv() => info!("Pub/Sub backend: received SIGINT, shutting down"),
                    _ = sigterm.recv() => info!("Pub/Sub backend: received SIGTERM, shutting down"),
                }
                let _ = shutdown_tx.send(true);
            });
            handles.push(WorkerHandle::Tokio(handle));
        }

        info!(
            handle_count = handles.len(),
            "Pub/Sub workers and due-sweeps started"
        );
        Ok(handles)
    }

    async fn health_check(&self) -> Result<Vec<QueueHealth>, QueueBackendError> {
        // Reachability per queue type (probe the subscription), plus backlog
        // depth from the cached Cloud Monitoring snapshot. Depth is `None`
        // (unavailable) when the read hasn't succeeded or we're on the emulator
        // — never a hardcoded 0.
        let snapshot = self.depth_snapshot.read().clone();

        let probes = ALL_QUEUE_TYPES.iter().map(|&queue_type| {
            let client = self.client.clone();
            let subscription = self.subscription_names[&queue_type].clone();
            async move {
                let reachable = client
                    .subscription(&subscription)
                    .exists(None)
                    .await
                    .unwrap_or(false);
                (queue_type, reachable)
            }
        });
        let reachability = futures::future::join_all(probes).await;

        let mut health_statuses = Vec::with_capacity(reachability.len());
        for (queue_type, is_healthy) in reachability {
            let messages_visible = depth_for(&snapshot, queue_type);
            health_statuses.push(QueueHealth {
                queue_type,
                messages_visible,
                messages_in_flight: 0,
                // The relayer publishes to no dead-letter destination; an
                // operator's optional native policy is not tracked here.
                messages_dlq: 0,
                backend: "pubsub".to_string(),
                is_healthy,
            });
        }
        Ok(health_statuses)
    }

    fn backend_type(&self) -> QueueBackendType {
        QueueBackendType::PubSub
    }

    fn shutdown(&self) {
        info!("Pub/Sub backend: broadcasting shutdown signal to all workers");
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

    // ── wire codec ─────────────────────────────────────────────────

    #[test]
    fn test_job_to_message_round_trip_and_attribute() {
        let job = sample_status_job(Some(NetworkType::Evm));
        let msg = job_to_message(&job, 3).expect("encode");

        // retry_attempt attribute carried as a string.
        assert_eq!(
            msg.attributes.get(RETRY_ATTEMPT_ATTR),
            Some(&"3".to_string())
        );
        // ordering off in v1.
        assert_eq!(msg.ordering_key, "");

        // Body is the same JSON the other backends use.
        let decoded: Job<TransactionStatusCheck> =
            serde_json::from_slice(&msg.data).expect("decode");
        assert_eq!(decoded.message_id, job.message_id);
        assert_eq!(decoded.data.transaction_id, "tx-1");
    }

    #[test]
    fn test_retry_attempt_from_attrs_defaults_to_zero() {
        // Missing attribute → 0.
        let empty = HashMap::new();
        assert_eq!(retry_attempt_from_attrs(&empty), 0);

        // Present and valid.
        let mut attrs = HashMap::new();
        attrs.insert(RETRY_ATTEMPT_ATTR.to_string(), "7".to_string());
        assert_eq!(retry_attempt_from_attrs(&attrs), 7);

        // Present but unparsable → 0 (defensive default).
        let mut bad = HashMap::new();
        bad.insert(RETRY_ATTEMPT_ATTR.to_string(), "not-a-number".to_string());
        assert_eq!(retry_attempt_from_attrs(&bad), 0);
    }

    #[test]
    fn test_job_to_message_round_trips_retry_attempt() {
        // The attribute the worker writes is the one it reads back.
        let job = sample_status_job(None);
        let msg = job_to_message(&job, 5).expect("encode");
        assert_eq!(retry_attempt_from_attrs(&msg.attributes), 5);
    }

    // ── resource naming + status routing ───────────────────────────

    #[test]
    fn test_topic_and_subscription_names_all_eight() {
        // The prefix carries no trailing separator; the `-` is inserted by the
        // name builders, so the final names are still `relayer-<queue>`.
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
        for (qt, topic) in expected {
            assert_eq!(topic_name(prefix, qt), topic);
            assert_eq!(subscription_name(prefix, qt), format!("{topic}-sub"));
        }
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

    #[test]
    fn test_all_queue_types_has_eight_distinct() {
        assert_eq!(ALL_QUEUE_TYPES.len(), 8);
        let names: std::collections::HashSet<&str> =
            ALL_QUEUE_TYPES.iter().map(|qt| qt.queue_name()).collect();
        assert_eq!(names.len(), 8, "queue names must be distinct");
    }

    #[test]
    fn test_pubsub_backend_type_value() {
        assert_eq!(QueueBackendType::PubSub.as_str(), "pubsub");
        assert_eq!(QueueBackendType::PubSub.to_string(), "pubsub");
    }

    // ── depth observability — unavailable is never 0 ──

    #[test]
    fn test_depth_for_unavailable_is_none_not_zero() {
        // Snapshot unavailable (read never succeeded / emulator) → None, NOT 0.
        let snap = DepthSnapshot::default();
        assert!(!snap.available);
        assert_eq!(depth_for(&snap, QueueType::StatusCheckEvm), None);
    }

    #[test]
    fn test_depth_for_available_reports_real_value() {
        let mut depths = HashMap::new();
        depths.insert(QueueType::StatusCheckEvm, 42);
        let snap = DepthSnapshot {
            available: true,
            depths,
        };
        // Present → real value (never 0 while work is queued).
        assert_eq!(depth_for(&snap, QueueType::StatusCheckEvm), Some(42));
        // Available but absent for this queue → a genuine empty 0 (distinct from
        // the unavailable `None`).
        assert_eq!(depth_for(&snap, QueueType::TransactionRequest), Some(0));
    }

    // ── oversized payload returns a clear size error ────

    #[test]
    fn test_check_message_size_boundary() {
        assert!(check_message_size(PUBSUB_MAX_MESSAGE_SIZE_BYTES).is_ok());
        let err = check_message_size(PUBSUB_MAX_MESSAGE_SIZE_BYTES + 1)
            .expect_err("over-limit must error");
        assert!(matches!(err, QueueBackendError::SerializationError(_)));
        assert!(err.to_string().contains("exceeds Pub/Sub limit"));
    }

    #[test]
    fn test_job_to_message_rejects_oversized_payload() {
        // A job whose JSON body exceeds 10 MB returns a serialization/size error.
        let big = "x".repeat(PUBSUB_MAX_MESSAGE_SIZE_BYTES + 1024);
        let data = TransactionStatusCheck {
            transaction_id: "tx".to_string(),
            relayer_id: "r".to_string(),
            network_type: Some(NetworkType::Evm),
            metadata: Some(std::collections::HashMap::from([("blob".to_string(), big)])),
        };
        let job = Job::new(JobType::TransactionStatusCheck, data);
        let err = job_to_message(&job, 0).expect_err("oversized payload must error");
        assert!(matches!(err, QueueBackendError::SerializationError(_)));
        assert!(err.to_string().contains("exceeds Pub/Sub limit"));
    }

    // ── startup fail-fast wording names what's missing ──────

    #[test]
    fn test_missing_resources_error_names_each_missing_resource() {
        let missing = vec![
            "topic 'relayer-status-check' (for status-check) does not exist".to_string(),
            "subscription 'relayer-notification-sub' (for notification) does not exist".to_string(),
        ];
        let err = missing_resources_error(&missing);
        assert!(matches!(err, QueueBackendError::ConfigError(_)));
        let msg = err.to_string();
        assert!(msg.contains("Missing/inaccessible resources"));
        assert!(msg.contains("relayer-status-check"));
        assert!(msg.contains("relayer-notification-sub"));
    }
}
