//! Health check models and response types.
//!
//! This module contains all data structures used for health check endpoints,
//! including component statuses, health information, and readiness responses.

use serde::Serialize;
use utoipa::ToSchema;

/// Status of an individual Redis connection pool.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PoolStatus {
    /// Whether the pool is connected and responding to PING.
    pub connected: bool,
    /// Number of available connections in the pool.
    pub available: usize,
    /// Maximum configured pool size.
    pub max_size: usize,
    /// Error message if the pool is not healthy.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub error: Option<String>,
}

/// Component health status levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ComponentStatus {
    /// Component is fully operational.
    Healthy,
    /// Component is operational but with reduced capacity or fallback mode.
    Degraded,
    /// Component is not operational.
    Unhealthy,
}

/// System health information (file descriptors, sockets).
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SystemHealth {
    pub status: ComponentStatus,
    pub fd_count: usize,
    pub fd_limit: usize,
    pub fd_usage_percent: u32,
    pub close_wait_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub error: Option<String>,
}

/// Redis health information.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct RedisHealth {
    pub status: ComponentStatus,
    pub primary_pool: PoolStatus,
    pub reader_pool: PoolStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub error: Option<String>,
}

/// Queue health information.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct QueueHealth {
    pub status: ComponentStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub error: Option<String>,
}

/// Plugin health information.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PluginHealth {
    pub status: ComponentStatus,
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub circuit_state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub error: Option<String>,
    /// Plugin uptime in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub uptime_ms: Option<u64>,
    /// Memory usage in bytes
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub memory: Option<u64>,
    /// Number of completed tasks in the pool
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub pool_completed: Option<u64>,
    /// Number of queued tasks in the pool
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub pool_queued: Option<u64>,
    /// Success rate as a percentage (0.0-100.0)
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub success_rate: Option<f64>,
    /// Average response time in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub avg_response_time_ms: Option<u32>,
    /// Whether recovery mode is active
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub recovering: Option<bool>,
    /// Current recovery allowance percentage
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub recovery_percent: Option<u32>,
    /// Shared socket available connection slots
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub shared_socket_available_slots: Option<usize>,
    /// Shared socket active connection count
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub shared_socket_active_connections: Option<usize>,
    /// Shared socket registered execution count
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub shared_socket_registered_executions: Option<usize>,
    /// Connection pool available slots (for pool server connections)
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub connection_pool_available_slots: Option<usize>,
    /// Connection pool active connections (for pool server connections)
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub connection_pool_active_connections: Option<usize>,
}

/// All health check components.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct Components {
    pub system: SystemHealth,
    pub redis: RedisHealth,
    pub queue: QueueHealth,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub plugins: Option<PluginHealth>,
}

/// Complete readiness response.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ReadinessResponse {
    pub ready: bool,
    pub status: ComponentStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub reason: Option<String>,
    pub components: Components,
    pub timestamp: String,
}

/// Health status of Redis connections (primary and reader pools).
///
/// This is an intermediate structure used internally by health check functions
/// before being converted to the public `RedisHealth` model.
#[derive(Debug, Clone)]
pub struct RedisHealthStatus {
    /// Overall health status.
    pub healthy: bool,
    /// Primary pool status.
    pub primary_pool: PoolStatus,
    /// Reader pool status.
    pub reader_pool: PoolStatus,
    /// Error message if unhealthy.
    pub error: Option<String>,
}

/// Health status of Queue's Redis connection.
///
/// This is an intermediate structure used internally by health check functions
/// before being converted to the public `QueueHealth` model.
#[derive(Debug, Clone)]
pub struct QueueHealthStatus {
    /// Overall health status.
    pub healthy: bool,
    /// Error message if unhealthy.
    pub error: Option<String>,
}
