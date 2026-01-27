//! Status check context for circuit breaker decisions.
//!
//! This module provides the `StatusCheckContext` struct which carries failure tracking
//! information to network handlers, enabling them to make intelligent decisions about
//! when to force-finalize transactions that have exceeded retry limits.
//!
//! Two thresholds are used for circuit breaker decisions:
//! - **Consecutive failures**: Triggers when RPC is completely down
//! - **Total failures**: Safety net for flaky RPC that succeeds occasionally but keeps failing

use crate::constants::{EVM_MAX_CONSECUTIVE_STATUS_FAILURES, EVM_MAX_TOTAL_STATUS_FAILURES};
use crate::models::NetworkType;

/// Metadata key for tracking consecutive status check failures.
/// Resets to 0 on successful status check (even if transaction not final).
pub const META_CONSECUTIVE_FAILURES: &str = "consecutive_failures";

/// Metadata key for tracking total status check failures.
/// Never resets - useful for monitoring, alerting, and as a safety net circuit breaker.
pub const META_TOTAL_FAILURES: &str = "total_failures";

/// Context for status check circuit breaker decisions.
///
/// This struct is passed to network handlers during status checks to provide
/// failure tracking information. Handlers can use this to decide whether to
/// force-finalize a transaction that has exceeded the maximum retry attempts.
///
/// The circuit breaker triggers when EITHER threshold is exceeded:
/// - `consecutive_failures >= max_consecutive_failures` (RPC completely down)
/// - `total_failures >= max_total_failures` (flaky RPC, safety net)
///
/// # Example
///
/// ```ignore
/// let context = StatusCheckContext::new(
///     consecutive_failures,
///     total_failures,
///     total_retries,
///     max_consecutive_failures,
///     max_total_failures,
///     NetworkType::Stellar,
/// );
///
/// if context.should_force_finalize() {
///     // Mark transaction as Failed with appropriate reason
/// }
/// ```
#[derive(Debug, Clone)]
pub struct StatusCheckContext {
    /// Number of consecutive failures since last successful status check.
    /// Resets to 0 when a status check succeeds (even if transaction not final).
    pub consecutive_failures: u32,

    /// Total number of failures across all status check attempts.
    /// Never resets - serves as safety net for flaky RPC connections.
    pub total_failures: u32,

    /// Total number of retries (from Apalis attempt counter).
    /// Includes both successful and failed attempts.
    pub total_retries: u32,

    /// Maximum consecutive failures allowed before forcing finalization.
    /// Network-specific value from constants.
    pub max_consecutive_failures: u32,

    /// Maximum total failures allowed before forcing finalization.
    /// Safety net for flaky RPC that occasionally succeeds (resetting consecutive counter).
    pub max_total_failures: u32,

    /// The network type for this transaction.
    pub network_type: NetworkType,
}

impl Default for StatusCheckContext {
    fn default() -> Self {
        Self {
            consecutive_failures: 0,
            total_failures: 0,
            total_retries: 0,
            max_consecutive_failures: EVM_MAX_CONSECUTIVE_STATUS_FAILURES,
            max_total_failures: EVM_MAX_TOTAL_STATUS_FAILURES,
            network_type: NetworkType::Evm,
        }
    }
}

impl StatusCheckContext {
    /// Creates a new `StatusCheckContext` with the specified failure counts and limits.
    ///
    /// # Arguments
    ///
    /// * `consecutive_failures` - Current count of consecutive failures
    /// * `total_failures` - Total count of all failures
    /// * `total_retries` - Total Apalis retry attempts (includes successes)
    /// * `max_consecutive_failures` - Network-specific consecutive max before force-finalization
    /// * `max_total_failures` - Network-specific total max (safety net)
    /// * `network_type` - The blockchain network type
    pub fn new(
        consecutive_failures: u32,
        total_failures: u32,
        total_retries: u32,
        max_consecutive_failures: u32,
        max_total_failures: u32,
        network_type: NetworkType,
    ) -> Self {
        Self {
            consecutive_failures,
            total_failures,
            total_retries,
            max_consecutive_failures,
            max_total_failures,
            network_type,
        }
    }

    /// Determines if the circuit breaker should force-finalize the transaction.
    ///
    /// Returns `true` if EITHER threshold is exceeded:
    /// - Consecutive failures reached the network-specific maximum (RPC completely down)
    /// - Total failures reached the network-specific maximum (flaky RPC safety net)
    pub fn should_force_finalize(&self) -> bool {
        self.consecutive_failures >= self.max_consecutive_failures
            || self.total_failures >= self.max_total_failures
    }

    /// Returns true if triggered by consecutive failures threshold.
    pub fn triggered_by_consecutive(&self) -> bool {
        self.consecutive_failures >= self.max_consecutive_failures
    }

    /// Returns true if triggered by total failures threshold (safety net).
    pub fn triggered_by_total(&self) -> bool {
        self.total_failures >= self.max_total_failures
    }
}

/// Reads a counter value from job metadata.
///
/// # Arguments
///
/// * `metadata` - Optional metadata HashMap from the job
/// * `key` - The metadata key to read
///
/// # Returns
///
/// The counter value as u32, or 0 if not present or invalid.
pub fn read_counter_from_metadata(
    metadata: &Option<std::collections::HashMap<String, String>>,
    key: &str,
) -> u32 {
    metadata
        .as_ref()
        .and_then(|m| m.get(key))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_status_check_context_default() {
        let ctx = StatusCheckContext::default();
        assert_eq!(ctx.consecutive_failures, 0);
        assert_eq!(ctx.total_failures, 0);
        assert_eq!(ctx.total_retries, 0);
        assert_eq!(
            ctx.max_consecutive_failures,
            EVM_MAX_CONSECUTIVE_STATUS_FAILURES
        );
        assert_eq!(ctx.max_total_failures, EVM_MAX_TOTAL_STATUS_FAILURES);
        assert_eq!(ctx.network_type, NetworkType::Evm);
    }

    #[test]
    fn test_status_check_context_new() {
        let ctx = StatusCheckContext::new(5, 10, 20, 15, 45, NetworkType::Stellar);
        assert_eq!(ctx.consecutive_failures, 5);
        assert_eq!(ctx.total_failures, 10);
        assert_eq!(ctx.total_retries, 20);
        assert_eq!(ctx.max_consecutive_failures, 15);
        assert_eq!(ctx.max_total_failures, 45);
        assert_eq!(ctx.network_type, NetworkType::Stellar);
    }

    #[test]
    fn test_should_force_finalize_below_both_thresholds() {
        // consecutive: 5 < 15, total: 10 < 45
        let ctx = StatusCheckContext::new(5, 10, 20, 15, 45, NetworkType::Evm);
        assert!(!ctx.should_force_finalize());
    }

    #[test]
    fn test_should_force_finalize_consecutive_at_threshold() {
        // consecutive: 15 >= 15 (triggers), total: 20 < 45
        let ctx = StatusCheckContext::new(15, 20, 30, 15, 45, NetworkType::Evm);
        assert!(ctx.should_force_finalize());
        assert!(ctx.triggered_by_consecutive());
        assert!(!ctx.triggered_by_total());
    }

    #[test]
    fn test_should_force_finalize_total_at_threshold() {
        // consecutive: 5 < 15, total: 45 >= 45 (triggers - safety net)
        let ctx = StatusCheckContext::new(5, 45, 50, 15, 45, NetworkType::Evm);
        assert!(ctx.should_force_finalize());
        assert!(!ctx.triggered_by_consecutive());
        assert!(ctx.triggered_by_total());
    }

    #[test]
    fn test_should_force_finalize_both_exceeded() {
        // Both thresholds exceeded
        let ctx = StatusCheckContext::new(20, 50, 60, 15, 45, NetworkType::Evm);
        assert!(ctx.should_force_finalize());
        assert!(ctx.triggered_by_consecutive());
        assert!(ctx.triggered_by_total());
    }

    #[test]
    fn test_flaky_rpc_scenario() {
        // Simulates flaky RPC: consecutive keeps resetting but total grows
        // consecutive: 3 < 15, total: 100 >= 45 (triggers safety net)
        let ctx = StatusCheckContext::new(3, 100, 150, 15, 45, NetworkType::Evm);
        assert!(ctx.should_force_finalize());
        assert!(!ctx.triggered_by_consecutive());
        assert!(ctx.triggered_by_total());
    }

    #[test]
    fn test_read_counter_from_metadata_present() {
        let mut metadata = HashMap::new();
        metadata.insert(META_CONSECUTIVE_FAILURES.to_string(), "5".to_string());
        let result = read_counter_from_metadata(&Some(metadata), META_CONSECUTIVE_FAILURES);
        assert_eq!(result, 5);
    }

    #[test]
    fn test_read_counter_from_metadata_missing() {
        let metadata: HashMap<String, String> = HashMap::new();
        let result = read_counter_from_metadata(&Some(metadata), META_CONSECUTIVE_FAILURES);
        assert_eq!(result, 0);
    }

    #[test]
    fn test_read_counter_from_metadata_none() {
        let result = read_counter_from_metadata(&None, META_CONSECUTIVE_FAILURES);
        assert_eq!(result, 0);
    }

    #[test]
    fn test_read_counter_from_metadata_invalid() {
        let mut metadata = HashMap::new();
        metadata.insert(
            META_CONSECUTIVE_FAILURES.to_string(),
            "not_a_number".to_string(),
        );
        let result = read_counter_from_metadata(&Some(metadata), META_CONSECUTIVE_FAILURES);
        assert_eq!(result, 0);
    }
}
