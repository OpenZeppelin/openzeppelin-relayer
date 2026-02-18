//! Status check constants for circuit breaker configuration.
//!
//! This module defines network-specific constants for the transaction status check
//! circuit breaker. These values determine when a transaction should be force-finalized
//! after repeated status check failures.
//!
//! Two thresholds are used:
//! - **Consecutive failures**: Quick trigger for complete RPC outage (~5 min window)
//! - **Total failures**: Safety net for flaky RPC (~15 min window), since consecutive
//!   counter resets on any success

use crate::models::NetworkType;

// =============================================================================
// EVM Constants
// =============================================================================

/// Maximum consecutive status check failures before forcing finalization for EVM networks.
///
/// With worker backoff capped at 12s, 25 failures ≈ 5 minutes of monitoring.
/// Quick trigger for complete RPC outage - if RPC is down for 5 min straight, act.
pub const EVM_MAX_CONSECUTIVE_STATUS_FAILURES: u32 = 25;

/// Maximum total status check failures before forcing finalization for EVM networks.
///
/// With worker backoff capped at 12s, 75 failures ≈ 15 minutes of monitoring.
/// Safety net for flaky RPC that occasionally succeeds (resetting consecutive counter).
/// Prevents transactions from being stuck indefinitely due to intermittent failures.
pub const EVM_MAX_TOTAL_STATUS_FAILURES: u32 = 75;

// =============================================================================
// Stellar Constants
// =============================================================================

/// Maximum consecutive status check failures before forcing finalization for Stellar.
///
/// With worker backoff capped at 3s, 100 failures ≈ 5 minutes of monitoring.
/// Quick trigger for complete RPC outage.
pub const STELLAR_MAX_CONSECUTIVE_STATUS_FAILURES: u32 = 100;

/// Maximum total status check failures before forcing finalization for Stellar.
///
/// With worker backoff capped at 3s, 300 failures ≈ 15 minutes of monitoring.
/// Safety net for flaky RPC.
pub const STELLAR_MAX_TOTAL_STATUS_FAILURES: u32 = 300;

// =============================================================================
// Solana Constants
// =============================================================================

/// Maximum consecutive status check failures before forcing finalization for Solana.
///
/// With worker backoff capped at 8s, 38 failures ≈ 5 minutes of monitoring.
/// Quick trigger for complete RPC outage.
pub const SOLANA_MAX_CONSECUTIVE_STATUS_FAILURES: u32 = 38;

/// Maximum total status check failures before forcing finalization for Solana.
///
/// With worker backoff capped at 8s, 115 failures ≈ 15 minutes of monitoring.
/// Safety net for flaky RPC.
pub const SOLANA_MAX_TOTAL_STATUS_FAILURES: u32 = 115;

// =============================================================================
// Helper Functions
// =============================================================================

/// Returns the maximum consecutive status check failures allowed for a given network type.
///
/// This function maps network types to their specific failure limits, allowing
/// network handlers to apply appropriate circuit breaker thresholds.
///
/// # Arguments
///
/// * `network_type` - The blockchain network type
///
/// # Returns
///
/// The maximum number of consecutive failures before force-finalization.
///
/// # Example
///
/// ```ignore
/// let max_failures = get_max_consecutive_status_failures(NetworkType::Stellar);
/// assert_eq!(max_failures, STELLAR_MAX_CONSECUTIVE_STATUS_FAILURES);
/// ```
pub fn get_max_consecutive_status_failures(network_type: NetworkType) -> u32 {
    match network_type {
        NetworkType::Evm => EVM_MAX_CONSECUTIVE_STATUS_FAILURES,
        NetworkType::Stellar => STELLAR_MAX_CONSECUTIVE_STATUS_FAILURES,
        NetworkType::Solana => SOLANA_MAX_CONSECUTIVE_STATUS_FAILURES,
    }
}

/// Returns the maximum total status check failures allowed for a given network type.
///
/// This is a safety net for flaky RPC connections that occasionally succeed
/// (resetting the consecutive counter) but continue to fail overall.
///
/// # Arguments
///
/// * `network_type` - The blockchain network type
///
/// # Returns
///
/// The maximum number of total failures before force-finalization.
pub fn get_max_total_status_failures(network_type: NetworkType) -> u32 {
    match network_type {
        NetworkType::Evm => EVM_MAX_TOTAL_STATUS_FAILURES,
        NetworkType::Stellar => STELLAR_MAX_TOTAL_STATUS_FAILURES,
        NetworkType::Solana => SOLANA_MAX_TOTAL_STATUS_FAILURES,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_evm_max_consecutive_failures() {
        assert_eq!(
            get_max_consecutive_status_failures(NetworkType::Evm),
            EVM_MAX_CONSECUTIVE_STATUS_FAILURES
        );
    }

    #[test]
    fn test_evm_max_total_failures() {
        assert_eq!(
            get_max_total_status_failures(NetworkType::Evm),
            EVM_MAX_TOTAL_STATUS_FAILURES
        );
    }

    #[test]
    fn test_stellar_max_consecutive_failures() {
        assert_eq!(
            get_max_consecutive_status_failures(NetworkType::Stellar),
            STELLAR_MAX_CONSECUTIVE_STATUS_FAILURES
        );
    }

    #[test]
    fn test_stellar_max_total_failures() {
        assert_eq!(
            get_max_total_status_failures(NetworkType::Stellar),
            STELLAR_MAX_TOTAL_STATUS_FAILURES
        );
    }

    #[test]
    fn test_solana_max_consecutive_failures() {
        assert_eq!(
            get_max_consecutive_status_failures(NetworkType::Solana),
            SOLANA_MAX_CONSECUTIVE_STATUS_FAILURES
        );
    }

    #[test]
    fn test_solana_max_total_failures() {
        assert_eq!(
            get_max_total_status_failures(NetworkType::Solana),
            SOLANA_MAX_TOTAL_STATUS_FAILURES
        );
    }

    #[test]
    fn test_consecutive_provides_5_min_window() {
        // Consecutive failures = quick trigger for complete RPC outage (~5 min)
        // EVM: 25 failures × 12s backoff = 300s = 5 min
        // Stellar: 100 failures × 3s backoff = 300s = 5 min
        // Solana: 38 failures × 8s backoff = 304s ≈ 5 min
        assert!(EVM_MAX_CONSECUTIVE_STATUS_FAILURES >= 25);
        assert!(STELLAR_MAX_CONSECUTIVE_STATUS_FAILURES >= 100);
        assert!(SOLANA_MAX_CONSECUTIVE_STATUS_FAILURES >= 38);
    }

    #[test]
    fn test_total_provides_15_min_window() {
        // Total failures = safety net for flaky RPC (~15 min)
        // EVM: 75 failures × 12s backoff = 900s = 15 min
        // Stellar: 300 failures × 3s backoff = 900s = 15 min
        // Solana: 115 failures × 8s backoff = 920s ≈ 15 min
        assert!(EVM_MAX_TOTAL_STATUS_FAILURES >= 75);
        assert!(STELLAR_MAX_TOTAL_STATUS_FAILURES >= 300);
        assert!(SOLANA_MAX_TOTAL_STATUS_FAILURES >= 115);
    }

    #[test]
    fn test_total_failures_greater_than_consecutive() {
        // Total failures should always be greater than consecutive (safety net)
        assert!(EVM_MAX_TOTAL_STATUS_FAILURES > EVM_MAX_CONSECUTIVE_STATUS_FAILURES);
        assert!(STELLAR_MAX_TOTAL_STATUS_FAILURES > STELLAR_MAX_CONSECUTIVE_STATUS_FAILURES);
        assert!(SOLANA_MAX_TOTAL_STATUS_FAILURES > SOLANA_MAX_CONSECUTIVE_STATUS_FAILURES);
    }
}
