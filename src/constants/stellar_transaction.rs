//! Constants for Stellar transaction processing.
//!
//! This module contains default values used throughout the Stellar transaction
//! handling logic, including fees, retry delays, and timeout thresholds.

use chrono::Duration;

pub const STELLAR_DEFAULT_TRANSACTION_FEE: u32 = 100;
/// Default maximum fee for fee-bump transactions (0.1 XLM = 1,000,000 stroops)
pub const STELLAR_DEFAULT_MAX_FEE: i64 = 1_000_000;
/// Maximum number of operations allowed in a Stellar transaction
pub const STELLAR_MAX_OPERATIONS: usize = 100;

/// Horizon API base URL for Stellar mainnet
pub const STELLAR_HORIZON_MAINNET_URL: &str = "https://horizon.stellar.org";
/// Horizon API base URL for Stellar testnet
pub const STELLAR_HORIZON_TESTNET_URL: &str = "https://horizon-testnet.stellar.org";

// Status check scheduling
/// Initial delay before first status check (in seconds)
/// Set to 2s for faster detection of transaction state changes
pub const STELLAR_STATUS_CHECK_INITIAL_DELAY_SECONDS: i64 = 2;

/// Minimum age before triggering Pending status recovery (in seconds)
/// Only schedule a recovery job if Pending transaction without hash exceeds this age
/// This prevents scheduling a job on every status check
pub const STELLAR_PENDING_RECOVERY_TRIGGER_SECONDS: i64 = 10;

// Transaction validity
/// Default transaction validity duration (in minutes) for sponsored transactions
/// Provides reasonable time for users to review and submit while ensuring transaction doesn't expire too quickly
pub const STELLAR_SPONSORED_TRANSACTION_VALIDITY_MINUTES: i64 = 2;

/// Get status check initial delay duration
pub fn get_stellar_status_check_initial_delay() -> Duration {
    Duration::seconds(STELLAR_STATUS_CHECK_INITIAL_DELAY_SECONDS)
}

/// Get sponsored transaction validity duration
pub fn get_stellar_sponsored_transaction_validity_duration() -> Duration {
    Duration::minutes(STELLAR_SPONSORED_TRANSACTION_VALIDITY_MINUTES)
}

// Recovery thresholds
/// Minimum time before re-queuing submit job for stuck Sent transactions (30 seconds)
/// Gives the original submit job time to complete before attempting recovery.
pub const STELLAR_RESEND_TIMEOUT_SECONDS: i64 = 30;

/// Maximum lifetime for a Sent transaction before marking as Failed (30 minutes)
/// Safety net for transactions without time bounds - prevents infinite retries.
pub const STELLAR_MAX_SENT_LIFETIME_MINUTES: i64 = 15;

/// Maximum lifetime for a Pending transaction before marking as Failed (30 minutes)
/// Safety net for transactions stuck in Pending state - prevents infinite retries.
pub const STELLAR_MAX_PENDING_LIFETIME_MINUTES: i64 = 15;

/// Get resend timeout duration for stuck Sent transactions
pub fn get_stellar_resend_timeout() -> Duration {
    Duration::seconds(STELLAR_RESEND_TIMEOUT_SECONDS)
}

/// Get max sent lifetime duration
pub fn get_stellar_max_sent_lifetime() -> Duration {
    Duration::minutes(STELLAR_MAX_SENT_LIFETIME_MINUTES)
}

/// Get max pending lifetime duration
pub fn get_stellar_max_pending_lifetime() -> Duration {
    Duration::minutes(STELLAR_MAX_PENDING_LIFETIME_MINUTES)
}
