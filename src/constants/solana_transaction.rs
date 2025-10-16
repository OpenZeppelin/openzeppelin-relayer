//! Constants for Solana transaction processing.
//!
//! This module contains default values used throughout the Solana transaction
//! handling logic, including status check delays and timeout thresholds.

use chrono::Duration;

// Status check scheduling
/// Initial delay before first status check (in seconds)
/// Set to 8s to allow time for transaction propagation on Solana
pub const SOLANA_STATUS_CHECK_INITIAL_DELAY_SECONDS: i64 = 5;

/// Get status check initial delay duration
pub fn get_solana_status_check_initial_delay() -> Duration {
    Duration::seconds(SOLANA_STATUS_CHECK_INITIAL_DELAY_SECONDS)
}
