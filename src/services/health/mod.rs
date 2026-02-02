//! Health check service.
//!
//! This module contains the business logic for performing health checks,
//! including system resource checks, Redis connectivity, queue health, and plugin status.

use std::sync::Arc;
use std::time::{Duration, Instant};

use apalis::prelude::BackendExpose;
use deadpool_redis::Pool;
use tokio::sync::RwLock;

use crate::jobs::{JobProducerTrait, Queue};
use crate::models::health::{
    ComponentStatus, Components, PluginHealth, PoolStatus, QueueHealth, QueueHealthStatus,
    ReadinessResponse, RedisHealth, RedisHealthStatus, SystemHealth,
};
use crate::models::{
    NetworkRepoModel, NotificationRepoModel, RelayerRepoModel, SignerRepoModel, ThinDataAppState,
    TransactionRepoModel,
};
use crate::repositories::{
    ApiKeyRepositoryTrait, NetworkRepository, PluginRepositoryTrait, RelayerRepository, Repository,
    TransactionCounterTrait, TransactionRepository,
};
use crate::services::plugins::get_pool_manager;
use crate::utils::RedisConnections;

// ============================================================================
// Constants
// ============================================================================

/// Timeout for Redis PING operations during health checks.
/// Health check timeout - increased from 500ms to handle high-load scenarios
/// where Redis pool acquisition may take longer due to connection contention.
const PING_TIMEOUT: Duration = Duration::from_millis(2000);

/// Warning file descriptor ratio (70%) - triggers Degraded status.
const WARNING_FD_RATIO: f64 = 0.7;

/// Maximum file descriptor ratio (80%) - triggers Unhealthy status.
const MAX_FD_RATIO: f64 = 0.8;

/// Warning CLOSE_WAIT socket count - triggers Degraded status.
/// Increased from 50 to tolerate Docker/Redis networking artifacts under load.
const WARNING_CLOSE_WAIT: usize = 200;

/// Maximum CLOSE_WAIT socket count - triggers Unhealthy status.
/// Increased from 100 to tolerate Docker/Redis networking artifacts under load.
const MAX_CLOSE_WAIT: usize = 500;

/// Cache TTL - health checks are cached for this duration.
const HEALTH_CACHE_TTL: Duration = Duration::from_secs(10);

// ============================================================================
// Cache
// ============================================================================

/// Cached health check result with timestamp.
struct CachedHealth {
    response: ReadinessResponse,
    checked_at: Instant,
}

/// Global health cache (thread-safe).
static HEALTH_CACHE: std::sync::OnceLock<RwLock<Option<CachedHealth>>> = std::sync::OnceLock::new();

fn get_cache() -> &'static RwLock<Option<CachedHealth>> {
    HEALTH_CACHE.get_or_init(|| RwLock::new(None))
}

/// Check if cached response is still valid and return it.
async fn get_cached_response() -> Option<ReadinessResponse> {
    let cache = get_cache().read().await;
    if let Some(ref cached) = *cache {
        if cached.checked_at.elapsed() < HEALTH_CACHE_TTL {
            return Some(cached.response.clone());
        }
    }
    None
}

/// Store response in cache.
async fn cache_response(response: &ReadinessResponse) {
    let mut cache = get_cache().write().await;
    *cache = Some(CachedHealth {
        response: response.clone(),
        checked_at: Instant::now(),
    });
}

/// Clear the health cache (useful for testing).
#[cfg(test)]
pub async fn clear_cache() {
    let mut cache = get_cache().write().await;
    *cache = None;
}

// ============================================================================
// Redis Health Checks
// ============================================================================

/// Ping a single Redis pool and return its status.
async fn ping_pool(pool: &Arc<Pool>, name: &str) -> PoolStatus {
    let status = pool.status();

    let result = tokio::time::timeout(PING_TIMEOUT, async {
        let mut conn = pool.get().await?;
        redis::cmd("PING")
            .query_async::<String>(&mut conn)
            .await
            .map_err(deadpool_redis::PoolError::Backend)
    })
    .await;

    match result {
        Ok(Ok(_)) => PoolStatus {
            connected: true,
            available: status.available,
            max_size: status.max_size,
            error: None,
        },
        Ok(Err(e)) => {
            tracing::warn!(pool = %name, error = %e, "Redis pool PING failed");
            PoolStatus {
                connected: false,
                available: status.available,
                max_size: status.max_size,
                error: Some(e.to_string()),
            }
        }
        Err(_) => {
            tracing::warn!(pool = %name, "Redis pool PING timed out");
            PoolStatus {
                connected: false,
                available: status.available,
                max_size: status.max_size,
                error: Some("PING timed out".to_string()),
            }
        }
    }
}

/// Check health of Redis connections (primary and reader pools).
///
/// PINGs both pools concurrently with a 500ms timeout.
/// Primary pool failure = Unhealthy, reader pool failure = Degraded.
async fn check_redis_health(connections: &Arc<RedisConnections>) -> RedisHealthStatus {
    let (primary_status, reader_status) = tokio::join!(
        ping_pool(connections.primary(), "primary"),
        ping_pool(connections.reader(), "reader")
    );

    // Healthy if primary is connected (reader is optional/degraded mode)
    let healthy = primary_status.connected;

    let error = if !primary_status.connected {
        Some(format!(
            "Redis primary pool: {}",
            primary_status
                .error
                .as_deref()
                .unwrap_or("connection failed")
        ))
    } else {
        None
    };

    RedisHealthStatus {
        healthy,
        primary_pool: primary_status,
        reader_pool: reader_status,
        error,
    }
}

/// Convert RedisHealthStatus to RedisHealth with proper ComponentStatus.
fn redis_status_to_health(status: RedisHealthStatus) -> RedisHealth {
    let component_status = if status.healthy {
        if status.reader_pool.connected {
            ComponentStatus::Healthy
        } else {
            ComponentStatus::Degraded // Reader down but primary OK
        }
    } else {
        ComponentStatus::Unhealthy
    };

    RedisHealth {
        status: component_status,
        primary_pool: status.primary_pool,
        reader_pool: status.reader_pool,
        error: status.error,
    }
}

// ============================================================================
// Queue Health Checks
// ============================================================================

/// Check health of Queue's Redis connection.
///
/// Uses stats() to verify the queue is responsive within the timeout.
async fn check_queue_health(queue: &Queue) -> QueueHealthStatus {
    let result = tokio::time::timeout(PING_TIMEOUT, async {
        queue.relayer_health_check_queue.stats().await
    })
    .await;

    // result is Result<Result<Stats, Error>, Elapsed>
    // Must check both: timeout didn't expire AND stats() succeeded
    match result {
        Ok(Ok(_)) => QueueHealthStatus {
            healthy: true,
            error: None,
        },
        Ok(Err(e)) => {
            // Stats call failed (but didn't timeout)
            tracing::warn!(error = %e, "Queue stats check failed");
            QueueHealthStatus {
                healthy: false,
                error: Some(format!("Queue connection: {e}")),
            }
        }
        Err(_) => {
            // Timeout expired
            tracing::warn!("Queue stats check timed out");
            QueueHealthStatus {
                healthy: false,
                error: Some("Queue connection: Stats check timed out".to_string()),
            }
        }
    }
}

/// Convert QueueHealthStatus to QueueHealth.
fn queue_status_to_health(status: QueueHealthStatus) -> QueueHealth {
    QueueHealth {
        status: if status.healthy {
            ComponentStatus::Healthy
        } else {
            ComponentStatus::Unhealthy
        },
        error: status.error,
    }
}

/// Create unhealthy Redis and Queue health when queue is unavailable.
fn create_unavailable_health() -> (RedisHealth, QueueHealth) {
    let error_msg = "Queue unavailable - cannot check Redis or Queue health";
    let unhealthy_pool = PoolStatus {
        connected: false,
        available: 0,
        max_size: 0,
        error: Some(error_msg.to_string()),
    };

    let redis = RedisHealth {
        status: ComponentStatus::Unhealthy,
        primary_pool: unhealthy_pool.clone(),
        reader_pool: unhealthy_pool,
        error: Some(error_msg.to_string()),
    };

    let queue = QueueHealth {
        status: ComponentStatus::Unhealthy,
        error: Some("Failed to get queue from job producer".to_string()),
    };

    (redis, queue)
}

// ============================================================================
// System Health Checks
// ============================================================================

/// Get file descriptor count for current process.
fn get_fd_count() -> Result<usize, std::io::Error> {
    let pid = std::process::id();

    #[cfg(target_os = "linux")]
    {
        let fd_dir = format!("/proc/{pid}/fd");
        std::fs::read_dir(fd_dir).map(|entries| entries.count())
    }

    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        let output = Command::new("lsof")
            .args(["-p", &pid.to_string()])
            .output()?;
        let count = String::from_utf8_lossy(&output.stdout)
            .lines()
            .count()
            .saturating_sub(1); // Subtract header line
        Ok(count)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Ok(0) // Unsupported platform
    }
}

/// Get soft file descriptor limit for current process.
fn get_fd_limit() -> Result<usize, std::io::Error> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        use std::process::Command;
        let output = Command::new("sh").args(["-c", "ulimit -n"]).output()?;
        let limit = String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .unwrap_or(1024);
        Ok(limit)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Ok(1024) // Default fallback
    }
}

/// Get CLOSE_WAIT socket count.
fn get_close_wait_count() -> Result<usize, std::io::Error> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        use std::process::Command;
        let output = Command::new("sh")
            .args(["-c", "netstat -an | grep CLOSE_WAIT | wc -l"])
            .output()?;
        let count = String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .unwrap_or(0);
        Ok(count)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Ok(0) // Unsupported platform
    }
}

/// Evaluate system metrics and return appropriate status with optional error.
///
/// Returns (status, optional_error_message).
fn evaluate_system_metrics(
    fd_ratio: f64,
    fd_count: usize,
    fd_limit: usize,
    close_wait_count: usize,
) -> (ComponentStatus, Option<String>) {
    let mut errors: Vec<String> = Vec::new();
    let mut is_degraded = false;

    // Check file descriptor usage
    if fd_ratio > MAX_FD_RATIO {
        let fd_percent = fd_ratio * 100.0;
        let max_percent = MAX_FD_RATIO * 100.0;
        errors.push(format!(
            "File descriptor limit critical: {fd_count}/{fd_limit} ({fd_percent:.1}% > {max_percent:.1}%)"
        ));
    } else if fd_ratio > WARNING_FD_RATIO {
        is_degraded = true;
    }

    // Check CLOSE_WAIT sockets
    if close_wait_count > MAX_CLOSE_WAIT {
        errors.push(format!(
            "Too many CLOSE_WAIT sockets: {close_wait_count} > {MAX_CLOSE_WAIT}"
        ));
    } else if close_wait_count > WARNING_CLOSE_WAIT {
        is_degraded = true;
    }

    let status = if !errors.is_empty() {
        ComponentStatus::Unhealthy
    } else if is_degraded {
        ComponentStatus::Degraded
    } else {
        ComponentStatus::Healthy
    };

    let error = if errors.is_empty() {
        None
    } else {
        Some(errors.join("; "))
    };

    (status, error)
}

/// Check system resources and return health status.
///
/// Monitors file descriptor usage and CLOSE_WAIT socket count.
/// - Below 70% FD usage and <200 CLOSE_WAIT: Healthy
/// - 70-80% FD usage or 200-500 CLOSE_WAIT: Degraded
/// - Above 80% FD usage or >500 CLOSE_WAIT: Unhealthy
pub fn check_system_health() -> SystemHealth {
    let fd_count = get_fd_count().unwrap_or(0);
    let fd_limit = get_fd_limit().unwrap_or(1024);
    let close_wait_count = get_close_wait_count().unwrap_or(0);

    let fd_ratio = if fd_limit > 0 {
        fd_count as f64 / fd_limit as f64
    } else {
        0.0
    };
    let fd_usage_percent = (fd_ratio * 100.0) as u32;

    let (status, error) = evaluate_system_metrics(fd_ratio, fd_count, fd_limit, close_wait_count);

    SystemHealth {
        status,
        fd_count,
        fd_limit,
        fd_usage_percent,
        close_wait_count,
        error,
    }
}

// ============================================================================
// Plugin Health Checks
// ============================================================================

/// Determine plugin ComponentStatus from health check result.
fn determine_plugin_status(healthy: bool, circuit_state: Option<&str>) -> ComponentStatus {
    if healthy {
        match circuit_state {
            Some("closed") | None => ComponentStatus::Healthy,
            Some("half_open") | Some("open") => ComponentStatus::Degraded,
            _ => ComponentStatus::Healthy,
        }
    } else {
        ComponentStatus::Degraded
    }
}

/// Check plugin health using the global pool manager.
pub async fn check_plugin_health() -> Option<PluginHealth> {
    let pool_manager = get_pool_manager();

    if !pool_manager.is_initialized().await {
        return None;
    }

    match pool_manager.health_check().await {
        Ok(plugin_status) => {
            let status = determine_plugin_status(
                plugin_status.healthy,
                plugin_status.circuit_state.as_deref(),
            );

            Some(PluginHealth {
                status,
                enabled: true,
                circuit_state: plugin_status.circuit_state,
                error: if plugin_status.healthy {
                    None
                } else {
                    Some(plugin_status.status)
                },
                uptime_ms: plugin_status.uptime_ms,
                memory: plugin_status.memory,
                pool_completed: plugin_status.pool_completed,
                pool_queued: plugin_status.pool_queued,
                success_rate: plugin_status.success_rate,
                avg_response_time_ms: plugin_status.avg_response_time_ms,
                recovering: plugin_status.recovering,
                recovery_percent: plugin_status.recovery_percent,
                shared_socket_available_slots: plugin_status.shared_socket_available_slots,
                shared_socket_active_connections: plugin_status.shared_socket_active_connections,
                shared_socket_registered_executions: plugin_status
                    .shared_socket_registered_executions,
                connection_pool_available_slots: plugin_status.connection_pool_available_slots,
                connection_pool_active_connections: plugin_status
                    .connection_pool_active_connections,
            })
        }
        Err(e) => Some(PluginHealth {
            status: ComponentStatus::Degraded,
            enabled: true,
            circuit_state: None,
            error: Some(e.to_string()),
            uptime_ms: None,
            memory: None,
            pool_completed: None,
            pool_queued: None,
            success_rate: None,
            avg_response_time_ms: None,
            recovering: None,
            recovery_percent: None,
            shared_socket_available_slots: None,
            shared_socket_active_connections: None,
            shared_socket_registered_executions: None,
            connection_pool_available_slots: None,
            connection_pool_active_connections: None,
        }),
    }
}

// ============================================================================
// Health Aggregation
// ============================================================================

/// Aggregate component health statuses into overall status and reasons.
///
/// Priority: Unhealthy > Degraded > Healthy
/// Only Unhealthy components contribute to the reason list.
fn aggregate_health(
    system: &SystemHealth,
    redis: &RedisHealth,
    queue: &QueueHealth,
    plugins: &Option<PluginHealth>,
) -> (ComponentStatus, Option<String>) {
    let mut reasons: Vec<String> = Vec::new();
    let mut overall_status = ComponentStatus::Healthy;

    // System check - unhealthy = 503
    if system.status == ComponentStatus::Unhealthy {
        overall_status = ComponentStatus::Unhealthy;
        if let Some(ref err) = system.error {
            reasons.push(err.clone());
        }
    } else if system.status == ComponentStatus::Degraded
        && overall_status == ComponentStatus::Healthy
    {
        overall_status = ComponentStatus::Degraded;
    }

    // Redis check - unhealthy = 503
    if redis.status == ComponentStatus::Unhealthy {
        overall_status = ComponentStatus::Unhealthy;
        if let Some(ref err) = redis.error {
            reasons.push(err.clone());
        }
    } else if redis.status == ComponentStatus::Degraded
        && overall_status == ComponentStatus::Healthy
    {
        overall_status = ComponentStatus::Degraded;
    }

    // Queue check - unhealthy = 503
    if queue.status == ComponentStatus::Unhealthy {
        overall_status = ComponentStatus::Unhealthy;
        if let Some(ref err) = queue.error {
            reasons.push(err.clone());
        }
    } else if queue.status == ComponentStatus::Degraded
        && overall_status == ComponentStatus::Healthy
    {
        overall_status = ComponentStatus::Degraded;
    }

    // Plugin check - degraded only (doesn't cause 503)
    if let Some(ref plugin_health) = plugins {
        if plugin_health.status == ComponentStatus::Degraded
            && overall_status == ComponentStatus::Healthy
        {
            overall_status = ComponentStatus::Degraded;
        }
    }

    let reason = if reasons.is_empty() {
        None
    } else {
        Some(reasons.join("; "))
    };

    (overall_status, reason)
}

/// Build the final ReadinessResponse from components.
fn build_response(
    system: SystemHealth,
    redis: RedisHealth,
    queue: QueueHealth,
    plugins: Option<PluginHealth>,
) -> ReadinessResponse {
    let (overall_status, reason) = aggregate_health(&system, &redis, &queue, &plugins);
    let ready = overall_status != ComponentStatus::Unhealthy;

    ReadinessResponse {
        ready,
        status: overall_status,
        reason,
        components: Components {
            system,
            redis,
            queue,
            plugins,
        },
        timestamp: chrono::Utc::now().to_rfc3339(),
    }
}

// ============================================================================
// Public API
// ============================================================================

/// Get readiness response with caching.
///
/// Checks the cache first (10-second TTL). On cache miss, performs health checks
/// for all components: system resources, Redis pools, queue, and plugins.
///
/// Returns 200 OK if ready (Healthy or Degraded), 503 if Unhealthy.
pub async fn get_readiness<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
    data: ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
) -> ReadinessResponse
where
    J: JobProducerTrait + Send + Sync + 'static,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
    SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PR: PluginRepositoryTrait + Send + Sync + 'static,
    AKR: ApiKeyRepositoryTrait + Send + Sync + 'static,
{
    // Check cache first to avoid unnecessary health checks
    if let Some(cached) = get_cached_response().await {
        return cached;
    }

    // Try to get queue for Redis and Queue health checks
    let queue = match data.job_producer.get_queue().await {
        Ok(queue) => Some(queue),
        Err(e) => {
            tracing::warn!(error = %e, "Failed to get queue from job producer");
            None
        }
    };

    // Perform health checks
    let system = check_system_health();
    let plugins = check_plugin_health().await;

    let (redis, queue_health) = if let Some(ref q) = queue {
        let redis_connections = q.redis_connections();
        let redis_status = check_redis_health(&redis_connections).await;
        let queue_status = check_queue_health(q).await;

        (
            redis_status_to_health(redis_status),
            queue_status_to_health(queue_status),
        )
    } else {
        create_unavailable_health()
    };

    // Build response
    let response = build_response(system, redis, queue_health, plugins);

    // Cache only if we have a working queue (cache is less useful in degraded state)
    if queue.is_some() {
        cache_response(&response).await;
    }

    response
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // Constants Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_constants() {
        assert_eq!(PING_TIMEOUT, Duration::from_millis(2000));
        assert_eq!(WARNING_FD_RATIO, 0.7);
        assert_eq!(MAX_FD_RATIO, 0.8);
        assert_eq!(WARNING_CLOSE_WAIT, 200);
        assert_eq!(MAX_CLOSE_WAIT, 500);
        assert_eq!(HEALTH_CACHE_TTL, Duration::from_secs(10));
    }

    // -------------------------------------------------------------------------
    // System Metrics Evaluation Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_evaluate_system_metrics_healthy() {
        // Low FD usage, low CLOSE_WAIT
        let (status, error) = evaluate_system_metrics(0.5, 500, 1000, 20);
        assert_eq!(status, ComponentStatus::Healthy);
        assert!(error.is_none());
    }

    #[test]
    fn test_evaluate_system_metrics_degraded_fd() {
        // FD at 75% (between warning and max)
        let (status, error) = evaluate_system_metrics(0.75, 750, 1000, 20);
        assert_eq!(status, ComponentStatus::Degraded);
        assert!(error.is_none());
    }

    #[test]
    fn test_evaluate_system_metrics_degraded_close_wait() {
        // CLOSE_WAIT at 300 (between warning 200 and max 500)
        let (status, error) = evaluate_system_metrics(0.5, 500, 1000, 300);
        assert_eq!(status, ComponentStatus::Degraded);
        assert!(error.is_none());
    }

    #[test]
    fn test_evaluate_system_metrics_unhealthy_fd() {
        // FD at 85% (above max)
        let (status, error) = evaluate_system_metrics(0.85, 850, 1000, 20);
        assert_eq!(status, ComponentStatus::Unhealthy);
        assert!(error.is_some());
        assert!(error.unwrap().contains("File descriptor limit critical"));
    }

    #[test]
    fn test_evaluate_system_metrics_unhealthy_close_wait() {
        // CLOSE_WAIT at 600 (above max 500)
        let (status, error) = evaluate_system_metrics(0.5, 500, 1000, 600);
        assert_eq!(status, ComponentStatus::Unhealthy);
        assert!(error.is_some());
        assert!(error.unwrap().contains("CLOSE_WAIT"));
    }

    #[test]
    fn test_evaluate_system_metrics_both_unhealthy() {
        // Both FD and CLOSE_WAIT above max
        let (status, error) = evaluate_system_metrics(0.9, 900, 1000, 600);
        assert_eq!(status, ComponentStatus::Unhealthy);
        assert!(error.is_some());
        let err = error.unwrap();
        assert!(err.contains("File descriptor"));
        assert!(err.contains("CLOSE_WAIT"));
    }

    #[test]
    fn test_evaluate_system_metrics_at_warning_threshold() {
        // Exactly at warning threshold (70% FD, 200 CLOSE_WAIT)
        let (status, _) = evaluate_system_metrics(0.7, 700, 1000, 200);
        // At exactly warning threshold, should still be healthy (> comparison)
        assert_eq!(status, ComponentStatus::Healthy);
    }

    #[test]
    fn test_evaluate_system_metrics_just_above_warning() {
        // Just above warning threshold
        let (status, _) = evaluate_system_metrics(0.71, 710, 1000, 201);
        assert_eq!(status, ComponentStatus::Degraded);
    }

    #[test]
    fn test_evaluate_system_metrics_at_max_threshold() {
        // Exactly at max threshold (80% FD, 500 CLOSE_WAIT)
        let (status, _) = evaluate_system_metrics(0.8, 800, 1000, 500);
        // At exactly max, should be degraded (> comparison for unhealthy)
        assert_eq!(status, ComponentStatus::Degraded);
    }

    #[test]
    fn test_evaluate_system_metrics_zero_limit() {
        // Edge case: zero fd_limit should not cause division by zero
        let (status, _) = evaluate_system_metrics(0.0, 0, 0, 0);
        assert_eq!(status, ComponentStatus::Healthy);
    }

    // -------------------------------------------------------------------------
    // Plugin Status Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_determine_plugin_status_healthy_closed() {
        assert_eq!(
            determine_plugin_status(true, Some("closed")),
            ComponentStatus::Healthy
        );
    }

    #[test]
    fn test_determine_plugin_status_healthy_none() {
        assert_eq!(
            determine_plugin_status(true, None),
            ComponentStatus::Healthy
        );
    }

    #[test]
    fn test_determine_plugin_status_healthy_half_open() {
        assert_eq!(
            determine_plugin_status(true, Some("half_open")),
            ComponentStatus::Degraded
        );
    }

    #[test]
    fn test_determine_plugin_status_healthy_open() {
        assert_eq!(
            determine_plugin_status(true, Some("open")),
            ComponentStatus::Degraded
        );
    }

    #[test]
    fn test_determine_plugin_status_unhealthy() {
        assert_eq!(
            determine_plugin_status(false, Some("closed")),
            ComponentStatus::Degraded
        );
    }

    #[test]
    fn test_determine_plugin_status_unknown_state() {
        assert_eq!(
            determine_plugin_status(true, Some("unknown")),
            ComponentStatus::Healthy
        );
    }

    // -------------------------------------------------------------------------
    // Health Aggregation Tests
    // -------------------------------------------------------------------------

    fn create_healthy_system() -> SystemHealth {
        SystemHealth {
            status: ComponentStatus::Healthy,
            fd_count: 100,
            fd_limit: 1000,
            fd_usage_percent: 10,
            close_wait_count: 5,
            error: None,
        }
    }

    fn create_healthy_redis() -> RedisHealth {
        RedisHealth {
            status: ComponentStatus::Healthy,
            primary_pool: PoolStatus {
                connected: true,
                available: 8,
                max_size: 16,
                error: None,
            },
            reader_pool: PoolStatus {
                connected: true,
                available: 8,
                max_size: 16,
                error: None,
            },
            error: None,
        }
    }

    fn create_healthy_queue() -> QueueHealth {
        QueueHealth {
            status: ComponentStatus::Healthy,
            error: None,
        }
    }

    #[test]
    fn test_aggregate_health_all_healthy() {
        let system = create_healthy_system();
        let redis = create_healthy_redis();
        let queue = create_healthy_queue();

        let (status, reason) = aggregate_health(&system, &redis, &queue, &None);
        assert_eq!(status, ComponentStatus::Healthy);
        assert!(reason.is_none());
    }

    #[test]
    fn test_aggregate_health_system_degraded() {
        let mut system = create_healthy_system();
        system.status = ComponentStatus::Degraded;
        let redis = create_healthy_redis();
        let queue = create_healthy_queue();

        let (status, reason) = aggregate_health(&system, &redis, &queue, &None);
        assert_eq!(status, ComponentStatus::Degraded);
        assert!(reason.is_none());
    }

    #[test]
    fn test_aggregate_health_system_unhealthy() {
        let mut system = create_healthy_system();
        system.status = ComponentStatus::Unhealthy;
        system.error = Some("FD limit exceeded".to_string());
        let redis = create_healthy_redis();
        let queue = create_healthy_queue();

        let (status, reason) = aggregate_health(&system, &redis, &queue, &None);
        assert_eq!(status, ComponentStatus::Unhealthy);
        assert!(reason.is_some());
        assert!(reason.unwrap().contains("FD limit"));
    }

    #[test]
    fn test_aggregate_health_redis_unhealthy() {
        let system = create_healthy_system();
        let mut redis = create_healthy_redis();
        redis.status = ComponentStatus::Unhealthy;
        redis.error = Some("Primary pool down".to_string());
        let queue = create_healthy_queue();

        let (status, reason) = aggregate_health(&system, &redis, &queue, &None);
        assert_eq!(status, ComponentStatus::Unhealthy);
        assert!(reason.unwrap().contains("Primary pool"));
    }

    #[test]
    fn test_aggregate_health_queue_unhealthy() {
        let system = create_healthy_system();
        let redis = create_healthy_redis();
        let mut queue = create_healthy_queue();
        queue.status = ComponentStatus::Unhealthy;
        queue.error = Some("Queue timeout".to_string());

        let (status, reason) = aggregate_health(&system, &redis, &queue, &None);
        assert_eq!(status, ComponentStatus::Unhealthy);
        assert!(reason.unwrap().contains("Queue timeout"));
    }

    #[test]
    fn test_aggregate_health_multiple_unhealthy() {
        let mut system = create_healthy_system();
        system.status = ComponentStatus::Unhealthy;
        system.error = Some("FD limit".to_string());

        let mut redis = create_healthy_redis();
        redis.status = ComponentStatus::Unhealthy;
        redis.error = Some("Redis down".to_string());

        let queue = create_healthy_queue();

        let (status, reason) = aggregate_health(&system, &redis, &queue, &None);
        assert_eq!(status, ComponentStatus::Unhealthy);
        let reason_str = reason.unwrap();
        assert!(reason_str.contains("FD limit"));
        assert!(reason_str.contains("Redis down"));
    }

    #[test]
    fn test_aggregate_health_plugin_degraded_only() {
        let system = create_healthy_system();
        let redis = create_healthy_redis();
        let queue = create_healthy_queue();
        let plugins = Some(PluginHealth {
            status: ComponentStatus::Degraded,
            enabled: true,
            circuit_state: Some("open".to_string()),
            error: Some("Circuit open".to_string()),
            uptime_ms: None,
            memory: None,
            pool_completed: None,
            pool_queued: None,
            success_rate: None,
            avg_response_time_ms: None,
            recovering: None,
            recovery_percent: None,
            shared_socket_available_slots: None,
            shared_socket_active_connections: None,
            shared_socket_registered_executions: None,
            connection_pool_available_slots: None,
            connection_pool_active_connections: None,
        });

        let (status, reason) = aggregate_health(&system, &redis, &queue, &plugins);
        // Plugin degraded doesn't cause 503, just degrades overall status
        assert_eq!(status, ComponentStatus::Degraded);
        assert!(reason.is_none());
    }

    #[test]
    fn test_aggregate_health_unhealthy_overrides_degraded() {
        let mut system = create_healthy_system();
        system.status = ComponentStatus::Degraded;

        let mut redis = create_healthy_redis();
        redis.status = ComponentStatus::Unhealthy;
        redis.error = Some("Redis down".to_string());

        let queue = create_healthy_queue();

        let (status, _) = aggregate_health(&system, &redis, &queue, &None);
        assert_eq!(status, ComponentStatus::Unhealthy);
    }

    // -------------------------------------------------------------------------
    // Build Response Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_build_response_ready() {
        let response = build_response(
            create_healthy_system(),
            create_healthy_redis(),
            create_healthy_queue(),
            None,
        );

        assert!(response.ready);
        assert_eq!(response.status, ComponentStatus::Healthy);
        assert!(response.reason.is_none());
        assert!(!response.timestamp.is_empty());
    }

    #[test]
    fn test_build_response_not_ready() {
        let mut system = create_healthy_system();
        system.status = ComponentStatus::Unhealthy;
        system.error = Some("Critical error".to_string());

        let response = build_response(system, create_healthy_redis(), create_healthy_queue(), None);

        assert!(!response.ready);
        assert_eq!(response.status, ComponentStatus::Unhealthy);
        assert!(response.reason.is_some());
    }

    #[test]
    fn test_build_response_degraded_is_ready() {
        let mut system = create_healthy_system();
        system.status = ComponentStatus::Degraded;

        let response = build_response(system, create_healthy_redis(), create_healthy_queue(), None);

        // Degraded is still ready (returns 200, not 503)
        assert!(response.ready);
        assert_eq!(response.status, ComponentStatus::Degraded);
    }

    // -------------------------------------------------------------------------
    // Redis/Queue Helper Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_redis_status_to_health_healthy() {
        let status = RedisHealthStatus {
            healthy: true,
            primary_pool: PoolStatus {
                connected: true,
                available: 8,
                max_size: 16,
                error: None,
            },
            reader_pool: PoolStatus {
                connected: true,
                available: 8,
                max_size: 16,
                error: None,
            },
            error: None,
        };

        let health = redis_status_to_health(status);
        assert_eq!(health.status, ComponentStatus::Healthy);
    }

    #[test]
    fn test_redis_status_to_health_degraded() {
        let status = RedisHealthStatus {
            healthy: true,
            primary_pool: PoolStatus {
                connected: true,
                available: 8,
                max_size: 16,
                error: None,
            },
            reader_pool: PoolStatus {
                connected: false, // Reader down
                available: 0,
                max_size: 16,
                error: Some("Connection refused".to_string()),
            },
            error: None,
        };

        let health = redis_status_to_health(status);
        assert_eq!(health.status, ComponentStatus::Degraded);
    }

    #[test]
    fn test_redis_status_to_health_unhealthy() {
        let status = RedisHealthStatus {
            healthy: false,
            primary_pool: PoolStatus {
                connected: false,
                available: 0,
                max_size: 16,
                error: Some("PING timeout".to_string()),
            },
            reader_pool: PoolStatus {
                connected: false,
                available: 0,
                max_size: 16,
                error: Some("PING timeout".to_string()),
            },
            error: Some("Primary pool failed".to_string()),
        };

        let health = redis_status_to_health(status);
        assert_eq!(health.status, ComponentStatus::Unhealthy);
    }

    #[test]
    fn test_queue_status_to_health_healthy() {
        let status = QueueHealthStatus {
            healthy: true,
            error: None,
        };

        let health = queue_status_to_health(status);
        assert_eq!(health.status, ComponentStatus::Healthy);
        assert!(health.error.is_none());
    }

    #[test]
    fn test_queue_status_to_health_unhealthy() {
        let status = QueueHealthStatus {
            healthy: false,
            error: Some("Stats timeout".to_string()),
        };

        let health = queue_status_to_health(status);
        assert_eq!(health.status, ComponentStatus::Unhealthy);
        assert!(health.error.is_some());
    }

    #[test]
    fn test_create_unavailable_health() {
        let (redis, queue) = create_unavailable_health();

        assert_eq!(redis.status, ComponentStatus::Unhealthy);
        assert!(!redis.primary_pool.connected);
        assert!(!redis.reader_pool.connected);
        assert!(redis.error.is_some());

        assert_eq!(queue.status, ComponentStatus::Unhealthy);
        assert!(queue.error.is_some());
    }

    // -------------------------------------------------------------------------
    // Serialization Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_pool_status_serialization_without_error() {
        let status = PoolStatus {
            connected: true,
            available: 8,
            max_size: 16,
            error: None,
        };

        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("\"connected\":true"));
        assert!(json.contains("\"available\":8"));
        assert!(json.contains("\"max_size\":16"));
        // error should be omitted when None (skip_serializing_if)
        assert!(!json.contains("error"));
    }

    #[test]
    fn test_pool_status_serialization_with_error() {
        let status = PoolStatus {
            connected: false,
            available: 0,
            max_size: 16,
            error: Some("Connection refused".to_string()),
        };

        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("\"connected\":false"));
        assert!(json.contains("\"error\":\"Connection refused\""));
    }

    // -------------------------------------------------------------------------
    // System Health Integration Test
    // -------------------------------------------------------------------------

    #[actix_web::test]
    async fn test_check_system_health_returns_valid_data() {
        let health = check_system_health();

        // Should have positive fd_limit
        assert!(health.fd_limit > 0);

        // fd_usage_percent should be reasonable (0-100 under normal conditions)
        assert!(health.fd_usage_percent <= 100);

        // close_wait_count should be non-negative (always true for usize)
        // Status should be one of the valid enum values
        assert!(matches!(
            health.status,
            ComponentStatus::Healthy | ComponentStatus::Degraded | ComponentStatus::Unhealthy
        ));
    }

    // -------------------------------------------------------------------------
    // Nested Result Pattern Tests (documents check_queue_health behavior)
    // -------------------------------------------------------------------------

    /// Represents a timeout error for testing (mirrors tokio::time::error::Elapsed)
    #[derive(Debug)]
    struct TimeoutError;

    /// Helper that mimics the nested Result pattern from timeout + async operation.
    /// This documents the correct handling to prevent regression of the bug where
    /// `result.is_ok()` incorrectly marked failed operations as healthy.
    fn evaluate_nested_result<T, E: std::fmt::Display>(
        result: Result<Result<T, E>, TimeoutError>,
    ) -> (bool, Option<String>) {
        match result {
            Ok(Ok(_)) => (true, None),
            Ok(Err(e)) => (false, Some(format!("Operation failed: {e}"))),
            Err(_) => (false, Some("Operation timed out".to_string())),
        }
    }

    #[test]
    fn test_nested_result_success() {
        // Simulates: timeout didn't expire AND inner operation succeeded
        let result: Result<Result<(), &str>, TimeoutError> = Ok(Ok(()));
        let (healthy, error) = evaluate_nested_result(result);

        assert!(healthy);
        assert!(error.is_none());
    }

    #[test]
    fn test_nested_result_inner_error() {
        // Simulates: timeout didn't expire BUT inner operation failed
        // This is the bug case - previously `result.is_ok()` returned true here!
        let result: Result<Result<(), &str>, TimeoutError> = Ok(Err("connection refused"));
        let (healthy, error) = evaluate_nested_result(result);

        assert!(!healthy, "Inner error should mark as unhealthy");
        assert!(error.is_some());
        assert!(error.unwrap().contains("connection refused"));
    }

    #[test]
    fn test_nested_result_timeout() {
        // Simulates: timeout expired
        let result: Result<Result<(), &str>, TimeoutError> = Err(TimeoutError);
        let (healthy, error) = evaluate_nested_result(result);

        assert!(!healthy);
        assert!(error.is_some());
        assert!(error.unwrap().contains("timed out"));
    }

    #[test]
    fn test_nested_result_is_ok_pitfall() {
        // Documents the pitfall: is_ok() only checks outer Result
        let inner_error: Result<Result<(), &str>, TimeoutError> = Ok(Err("inner error"));

        // This is the WRONG way (the bug)
        let wrong_healthy = inner_error.is_ok();
        assert!(
            wrong_healthy,
            "is_ok() returns true even with inner error - this is the bug!"
        );

        // This is the CORRECT way (the fix)
        let correct_healthy = matches!(inner_error, Ok(Ok(_)));
        assert!(
            !correct_healthy,
            "matches! correctly identifies inner error"
        );
    }

    // -------------------------------------------------------------------------
    // Cache Tests
    // -------------------------------------------------------------------------

    #[actix_web::test]
    async fn test_cache_operations() {
        // Clear any existing cache
        clear_cache().await;

        // Should be empty initially
        let cached = get_cached_response().await;
        assert!(cached.is_none());

        // Cache a response
        let response = build_response(
            create_healthy_system(),
            create_healthy_redis(),
            create_healthy_queue(),
            None,
        );
        cache_response(&response).await;

        // Should now be cached
        let cached = get_cached_response().await;
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().ready, response.ready);

        // Clean up
        clear_cache().await;
    }
}
