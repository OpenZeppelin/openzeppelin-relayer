//! Constants for Stellar transaction processing.
//!
//! This module contains default values used throughout the Stellar transaction
//! handling logic, including fees, retry delays, and timeout thresholds.

use chrono::Duration;

pub const STELLAR_DEFAULT_TRANSACTION_FEE: u32 = 100;
/// Default maximum fee for fee-bump transactions (0.1 XLM = 1,000,000 stroops)
pub const STELLAR_DEFAULT_MAX_FEE: i64 = 1_000_000;

// Status check scheduling
/// Initial delay before first status check (in seconds)
/// Set to 2s for faster detection of transaction state changes
pub const STELLAR_STATUS_CHECK_INITIAL_DELAY_SECONDS: i64 = 2;

// Other delays
/// Default delay (in seconds) for retrying transaction after bad sequence error
pub const STELLAR_BAD_SEQUENCE_RETRY_DELAY_SECONDS: i64 = 2;

/// Get status check initial delay duration
pub fn get_stellar_status_check_initial_delay() -> Duration {
    Duration::seconds(STELLAR_STATUS_CHECK_INITIAL_DELAY_SECONDS)
}
