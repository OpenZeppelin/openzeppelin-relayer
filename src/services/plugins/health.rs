//! Health monitoring and circuit breaker for the plugin pool.
//!
//! This module provides:
//! - Circuit breaker pattern for automatic degradation under stress
//! - Health status reporting for monitoring
//! - Dead server detection for automatic recovery

use std::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::Arc;

/// Lock-free ring buffer for tracking recent results (sliding window)
pub struct ResultRingBuffer {
    buffer: Vec<AtomicU8>, // 0 = empty, 1 = success, 2 = failure
    index: AtomicUsize,
    size: usize,
}

impl ResultRingBuffer {
    pub fn new(size: usize) -> Self {
        assert!(size > 0, "ResultRingBuffer size must be greater than 0");
        let mut buffer = Vec::with_capacity(size);
        for _ in 0..size {
            buffer.push(AtomicU8::new(0));
        }
        Self {
            buffer,
            index: AtomicUsize::new(0),
            size,
        }
    }

    pub fn record(&self, success: bool) {
        let idx = self.index.fetch_add(1, Ordering::Relaxed) % self.size;
        self.buffer[idx].store(if success { 1 } else { 2 }, Ordering::Relaxed);
    }

    pub fn failure_rate(&self) -> f32 {
        let mut total = 0;
        let mut failures = 0;

        for slot in &self.buffer {
            match slot.load(Ordering::Relaxed) {
                0 => {}          // Empty slot
                1 => total += 1, // Success
                2 => {
                    total += 1;
                    failures += 1;
                }
                _ => {}
            }
        }

        if total < 10 {
            return 0.0; // Not enough data to make decision
        }

        (failures as f32) / (total as f32)
    }
}

/// Circuit breaker state for automatic degradation under stress
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Normal operation - all requests allowed
    Closed,
    /// Degraded - some requests rejected to reduce load
    HalfOpen,
    /// Fully open - most requests rejected, recovery in progress
    Open,
}

/// Indicators that the pool server is dead or unreachable.
/// Using an enum instead of string matching for type safety and documentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeadServerIndicator {
    /// JSON parse error - connection closed mid-message
    EofWhileParsing,
    /// Write to closed socket
    BrokenPipe,
    /// Server not listening
    ConnectionRefused,
    /// Server forcefully closed connection
    ConnectionReset,
    /// Socket not connected
    NotConnected,
    /// Connection establishment failed
    FailedToConnect,
    /// Unix socket file deleted
    SocketFileMissing,
    /// Socket file doesn't exist
    NoSuchFile,
    /// Connection timeout (not execution timeout)
    ConnectionTimedOut,
}

impl DeadServerIndicator {
    /// All patterns that indicate a dead server
    const ALL: &'static [(&'static str, DeadServerIndicator)] = &[
        ("eof while parsing", DeadServerIndicator::EofWhileParsing),
        ("broken pipe", DeadServerIndicator::BrokenPipe),
        ("connection refused", DeadServerIndicator::ConnectionRefused),
        ("connection reset", DeadServerIndicator::ConnectionReset),
        ("not connected", DeadServerIndicator::NotConnected),
        ("failed to connect", DeadServerIndicator::FailedToConnect),
        (
            "socket file missing",
            DeadServerIndicator::SocketFileMissing,
        ),
        ("no such file", DeadServerIndicator::NoSuchFile),
        (
            "connection timed out",
            DeadServerIndicator::ConnectionTimedOut,
        ),
        ("connect timed out", DeadServerIndicator::ConnectionTimedOut),
    ];

    /// Check if an error string matches any dead server indicator
    pub fn from_error_str(error_str: &str) -> Option<DeadServerIndicator> {
        let lower = error_str.to_lowercase();
        Self::ALL
            .iter()
            .find(|(pattern, _)| lower.contains(pattern))
            .map(|(_, indicator)| *indicator)
    }
}

/// Process status for health check decisions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessStatus {
    /// Process is running normally
    Running,
    /// Process has exited
    Exited,
    /// Could not determine status
    Unknown,
    /// No process handle exists
    NoProcess,
}

/// Circuit breaker for managing pool health and automatic recovery.
/// Tracks failure rates and response times to detect GC pressure.
pub struct CircuitBreaker {
    /// Current circuit state (encoded as u8 for atomic access)
    /// 0 = Closed, 1 = HalfOpen, 2 = Open
    state: AtomicU32,
    /// Time when circuit opened (for recovery timing)
    opened_at_ms: AtomicU64,
    /// Consecutive successful requests in half-open state
    recovery_successes: AtomicU32,
    /// Average response time in ms (exponential moving average)
    avg_response_time_ms: AtomicU32,
    /// Number of restart attempts
    restart_attempts: AtomicU32,
    /// Sliding window of recent results (100 most recent)
    recent_results: Arc<ResultRingBuffer>,
}

impl Default for CircuitBreaker {
    fn default() -> Self {
        Self::new()
    }
}

impl CircuitBreaker {
    pub fn new() -> Self {
        Self {
            state: AtomicU32::new(0), // Closed
            opened_at_ms: AtomicU64::new(0),
            recovery_successes: AtomicU32::new(0),
            avg_response_time_ms: AtomicU32::new(0),
            restart_attempts: AtomicU32::new(0),
            recent_results: Arc::new(ResultRingBuffer::new(100)),
        }
    }

    pub fn state(&self) -> CircuitState {
        match self.state.load(Ordering::Relaxed) {
            0 => CircuitState::Closed,
            1 => CircuitState::HalfOpen,
            _ => CircuitState::Open,
        }
    }

    pub fn set_state(&self, state: CircuitState) {
        let val = match state {
            CircuitState::Closed => 0,
            CircuitState::HalfOpen => 1,
            CircuitState::Open => 2,
        };
        self.state.store(val, Ordering::Relaxed);
    }

    /// Record a successful request with response time
    pub fn record_success(&self, response_time_ms: u32) {
        self.recent_results.record(true);

        // Update exponential moving average (alpha = 0.1)
        let current = self.avg_response_time_ms.load(Ordering::Relaxed);
        let new_avg = if current == 0 {
            response_time_ms
        } else {
            (current * 9 + response_time_ms) / 10
        };
        self.avg_response_time_ms.store(new_avg, Ordering::Relaxed);

        // Handle state transitions on success
        match self.state() {
            CircuitState::HalfOpen => {
                let successes = self.recovery_successes.fetch_add(1, Ordering::Relaxed) + 1;
                // Require 10 consecutive successes to close circuit
                if successes >= 10 {
                    tracing::info!("Circuit breaker closing - recovery successful");
                    self.set_state(CircuitState::Closed);
                    self.recovery_successes.store(0, Ordering::Relaxed);
                    self.restart_attempts.store(0, Ordering::Relaxed);
                }
            }
            CircuitState::Open => {
                // Check if enough time has passed to try half-open
                self.maybe_transition_to_half_open();
            }
            CircuitState::Closed => {}
        }
    }

    /// Record a failed request
    pub fn record_failure(&self) {
        self.recent_results.record(false);
        self.recovery_successes.store(0, Ordering::Relaxed);

        let failure_rate = self.recent_results.failure_rate();

        match self.state() {
            CircuitState::Closed => {
                // Open circuit if failure rate > 50% (ring buffer requires at least 10 samples)
                if failure_rate > 0.5 {
                    tracing::warn!(
                        failure_rate = %format!("{:.1}%", failure_rate * 100.0),
                        "Circuit breaker opening - high failure rate"
                    );
                    self.open_circuit();
                }
            }
            CircuitState::HalfOpen => {
                // Any failure in half-open sends back to open
                tracing::warn!("Circuit breaker reopening - failure during recovery");
                self.open_circuit();
            }
            CircuitState::Open => {
                // Already open, just check for transition
                self.maybe_transition_to_half_open();
            }
        }
    }

    fn open_circuit(&self) {
        self.set_state(CircuitState::Open);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        self.opened_at_ms.store(now, Ordering::Relaxed);
        self.recovery_successes.store(0, Ordering::Relaxed);
    }

    fn maybe_transition_to_half_open(&self) {
        let opened_at = self.opened_at_ms.load(Ordering::Relaxed);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        // Wait at least 5 seconds before trying recovery
        // Exponential backoff: 5s, 10s, 20s, 40s, max 60s
        let attempts = self.restart_attempts.load(Ordering::Relaxed);
        let backoff_ms = (5000u64 * (1 << attempts.min(4))).min(60000);

        if now - opened_at >= backoff_ms {
            tracing::info!(
                backoff_ms = backoff_ms,
                "Circuit breaker transitioning to half-open"
            );
            self.set_state(CircuitState::HalfOpen);
            self.restart_attempts.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Check if request should be allowed based on circuit state.
    /// If recovery_allowance is provided, use it in HalfOpen state instead of default 10%.
    pub fn should_allow_request(&self, recovery_allowance: Option<u32>) -> bool {
        match self.state() {
            CircuitState::Closed => true,
            CircuitState::HalfOpen => {
                // Use recovery allowance if provided, otherwise default to 10%
                let allowance = recovery_allowance.unwrap_or(10);
                (rand::random::<u32>() % 100) < allowance
            }
            CircuitState::Open => {
                // Check if we should transition to half-open
                self.maybe_transition_to_half_open();
                // Recheck state after potential transition
                matches!(self.state(), CircuitState::HalfOpen)
            }
        }
    }

    /// Get current response time average for monitoring
    pub fn avg_response_time(&self) -> u32 {
        self.avg_response_time_ms.load(Ordering::Relaxed)
    }

    /// Force circuit to closed state (for manual recovery)
    pub fn force_close(&self) {
        self.set_state(CircuitState::Closed);
        self.recovery_successes.store(0, Ordering::Relaxed);
        self.restart_attempts.store(0, Ordering::Relaxed);
    }

    /// Access recovery_successes for testing
    #[cfg(test)]
    pub fn recovery_successes(&self) -> &AtomicU32 {
        &self.recovery_successes
    }
}

/// Health status information from the pool server
#[derive(Debug, Clone)]
pub struct HealthStatus {
    pub healthy: bool,
    pub status: String,
    pub uptime_ms: Option<u64>,
    pub memory: Option<u64>,
    pub pool_completed: Option<u64>,
    pub pool_queued: Option<u64>,
    pub success_rate: Option<f64>,
    /// Circuit breaker state (Closed/HalfOpen/Open)
    pub circuit_state: Option<String>,
    /// Average response time in ms
    pub avg_response_time_ms: Option<u32>,
    /// Whether recovery mode is active
    pub recovering: Option<bool>,
    /// Current recovery allowance percentage
    pub recovery_percent: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // DeadServerIndicator Tests
    // =========================================================================

    #[test]
    fn test_dead_server_indicator_eof_while_parsing() {
        let result = DeadServerIndicator::from_error_str("eof while parsing a value");
        assert_eq!(result, Some(DeadServerIndicator::EofWhileParsing));
    }

    #[test]
    fn test_dead_server_indicator_broken_pipe() {
        let result = DeadServerIndicator::from_error_str("write failed: Broken pipe");
        assert_eq!(result, Some(DeadServerIndicator::BrokenPipe));
    }

    #[test]
    fn test_dead_server_indicator_connection_refused() {
        let result = DeadServerIndicator::from_error_str("Connection refused (os error 61)");
        assert_eq!(result, Some(DeadServerIndicator::ConnectionRefused));
    }

    #[test]
    fn test_dead_server_indicator_connection_reset() {
        let result = DeadServerIndicator::from_error_str("Connection reset by peer");
        assert_eq!(result, Some(DeadServerIndicator::ConnectionReset));
    }

    #[test]
    fn test_dead_server_indicator_not_connected() {
        let result = DeadServerIndicator::from_error_str("Socket is not connected");
        assert_eq!(result, Some(DeadServerIndicator::NotConnected));
    }

    #[test]
    fn test_dead_server_indicator_failed_to_connect() {
        let result = DeadServerIndicator::from_error_str("Failed to connect to server");
        assert_eq!(result, Some(DeadServerIndicator::FailedToConnect));
    }

    #[test]
    fn test_dead_server_indicator_socket_file_missing() {
        let result = DeadServerIndicator::from_error_str("Socket file missing at /tmp/pool.sock");
        assert_eq!(result, Some(DeadServerIndicator::SocketFileMissing));
    }

    #[test]
    fn test_dead_server_indicator_no_such_file() {
        let result = DeadServerIndicator::from_error_str("No such file or directory");
        assert_eq!(result, Some(DeadServerIndicator::NoSuchFile));
    }

    #[test]
    fn test_dead_server_indicator_connection_timed_out() {
        let result = DeadServerIndicator::from_error_str("Connection timed out after 5s");
        assert_eq!(result, Some(DeadServerIndicator::ConnectionTimedOut));
    }

    #[test]
    fn test_dead_server_indicator_connect_timed_out() {
        let result = DeadServerIndicator::from_error_str("Connect timed out");
        assert_eq!(result, Some(DeadServerIndicator::ConnectionTimedOut));
    }

    #[test]
    fn test_dead_server_indicator_case_insensitive() {
        let result = DeadServerIndicator::from_error_str("BROKEN PIPE ERROR");
        assert_eq!(result, Some(DeadServerIndicator::BrokenPipe));

        let result = DeadServerIndicator::from_error_str("EOF While Parsing");
        assert_eq!(result, Some(DeadServerIndicator::EofWhileParsing));
    }

    #[test]
    fn test_dead_server_indicator_no_match() {
        let result = DeadServerIndicator::from_error_str("Plugin execution failed");
        assert_eq!(result, None);

        let result = DeadServerIndicator::from_error_str("Timeout exceeded");
        assert_eq!(result, None);

        let result = DeadServerIndicator::from_error_str("");
        assert_eq!(result, None);
    }

    // =========================================================================
    // ResultRingBuffer Tests
    // =========================================================================

    #[test]
    fn test_result_ring_buffer_empty() {
        let buffer = ResultRingBuffer::new(100);
        assert_eq!(buffer.failure_rate(), 0.0);
    }

    #[test]
    fn test_result_ring_buffer_all_successes() {
        let buffer = ResultRingBuffer::new(100);
        for _ in 0..50 {
            buffer.record(true);
        }
        assert_eq!(buffer.failure_rate(), 0.0);
    }

    #[test]
    fn test_result_ring_buffer_all_failures() {
        let buffer = ResultRingBuffer::new(100);
        for _ in 0..50 {
            buffer.record(false);
        }
        assert_eq!(buffer.failure_rate(), 1.0);
    }

    #[test]
    fn test_result_ring_buffer_mixed_results() {
        let buffer = ResultRingBuffer::new(100);
        for _ in 0..25 {
            buffer.record(true);
        }
        for _ in 0..25 {
            buffer.record(false);
        }
        assert!((buffer.failure_rate() - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_result_ring_buffer_wraps_around() {
        let buffer = ResultRingBuffer::new(100);
        for _ in 0..100 {
            buffer.record(true);
        }
        for _ in 0..50 {
            buffer.record(false);
        }
        assert!((buffer.failure_rate() - 0.5).abs() < 0.01);
    }

    // =========================================================================
    // CircuitBreaker Tests
    // =========================================================================

    #[test]
    fn test_circuit_breaker_initial_state() {
        let cb = CircuitBreaker::new();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(cb.should_allow_request(None));
    }

    #[test]
    fn test_circuit_breaker_stays_closed_on_successes() {
        let cb = CircuitBreaker::new();
        for _ in 0..100 {
            cb.record_success(50);
        }
        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(cb.should_allow_request(None));
    }

    #[test]
    fn test_circuit_breaker_opens_on_high_failure_rate() {
        let cb = CircuitBreaker::new();
        for _ in 0..100 {
            cb.record_failure();
        }
        assert_eq!(cb.state(), CircuitState::Open);
    }

    #[test]
    fn test_circuit_breaker_half_open_allows_some_requests() {
        let cb = CircuitBreaker::new();
        cb.set_state(CircuitState::HalfOpen);

        let mut allowed = 0;
        for _ in 0..100 {
            if cb.should_allow_request(Some(10)) {
                allowed += 1;
            }
        }
        assert!(allowed > 0, "Half-open should allow some requests");
        assert!(allowed < 50, "Half-open should not allow too many requests");
    }

    #[test]
    fn test_circuit_breaker_force_close() {
        let cb = CircuitBreaker::new();
        cb.set_state(CircuitState::Open);
        assert_eq!(cb.state(), CircuitState::Open);

        cb.force_close();
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_circuit_breaker_half_open_to_closed_on_successes() {
        let cb = CircuitBreaker::new();
        cb.set_state(CircuitState::HalfOpen);
        cb.recovery_successes().store(0, Ordering::Relaxed);

        for _ in 0..10 {
            cb.record_success(50);
        }

        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_circuit_breaker_half_open_to_open_on_failure() {
        let cb = CircuitBreaker::new();
        cb.set_state(CircuitState::HalfOpen);
        cb.recovery_successes().store(5, Ordering::Relaxed);

        cb.record_failure();

        assert_eq!(cb.state(), CircuitState::Open);
    }

    #[test]
    fn test_circuit_breaker_response_time_tracking() {
        let cb = CircuitBreaker::new();

        cb.record_success(100);
        cb.record_success(200);
        cb.record_success(150);

        let avg = cb.avg_response_time();
        assert!(avg > 0, "Average should be positive");
        assert!(avg < 300, "Average should be reasonable");
    }

    // =========================================================================
    // CircuitState Tests
    // =========================================================================

    #[test]
    fn test_circuit_state_debug() {
        assert_eq!(format!("{:?}", CircuitState::Closed), "Closed");
        assert_eq!(format!("{:?}", CircuitState::HalfOpen), "HalfOpen");
        assert_eq!(format!("{:?}", CircuitState::Open), "Open");
    }

    #[test]
    fn test_circuit_state_equality() {
        assert_eq!(CircuitState::Closed, CircuitState::Closed);
        assert_ne!(CircuitState::Closed, CircuitState::Open);
        assert_ne!(CircuitState::HalfOpen, CircuitState::Open);
    }

    // =========================================================================
    // ProcessStatus Tests
    // =========================================================================

    #[test]
    fn test_process_status_variants() {
        assert_eq!(ProcessStatus::Running, ProcessStatus::Running);
        assert_eq!(ProcessStatus::Exited, ProcessStatus::Exited);
        assert_eq!(ProcessStatus::Unknown, ProcessStatus::Unknown);
        assert_ne!(ProcessStatus::Running, ProcessStatus::Exited);
    }
}
