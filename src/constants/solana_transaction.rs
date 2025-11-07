//! Constants for Solana transaction processing.
//!
//! This module contains default values used throughout the Solana transaction
//! handling logic, including validation limits, status check delays, and timeout thresholds.

use chrono::Duration;

/// Default transaction valid timespan for Solana when no explicit valid_until is provided (in milliseconds)
/// Set to 30 minutes to balance between:
/// - Preventing infinite resubmission loops
/// - Allowing reasonable time for transaction processing during network congestion
/// - Aligning with Solana's fast finality expectations
pub const SOLANA_DEFAULT_TX_VALID_TIMESPAN: i64 = 30 * 60 * 1000; // 30 minutes in milliseconds

// API request validation limits
/// Maximum number of instructions allowed in a transaction request
pub const REQUEST_MAX_INSTRUCTIONS: usize = 64;

/// Maximum number of accounts allowed per instruction in a request
pub const REQUEST_MAX_ACCOUNTS_PER_INSTRUCTION: usize = 64;

/// Maximum total unique accounts allowed in a transaction request
pub const REQUEST_MAX_TOTAL_ACCOUNTS: usize = 64;

/// Maximum instruction data size in bytes allowed in a request
pub const REQUEST_MAX_INSTRUCTION_DATA_SIZE: usize = 1232;

// Status check scheduling
/// Initial delay before first status check (in seconds)
pub const SOLANA_STATUS_CHECK_INITIAL_DELAY_SECONDS: i64 = 5;

/// Minimum age before checking for resubmit/expiration (in seconds)
/// If transaction is younger than this, we don't check blockhash expiration yet
pub const SOLANA_MIN_AGE_FOR_RESUBMIT_CHECK_SECONDS: i64 = 90;

/// Minimum age before triggering Pending status recovery (in seconds)
/// Only schedule a recovery job if Pending transaction exceeds this age
pub const SOLANA_PENDING_RECOVERY_TRIGGER_SECONDS: i64 = 20;

/// Timeout for Pending status: transaction preparation phase (in minutes)
/// If a transaction stays in Pending for longer than this, mark as Failed
pub const SOLANA_PENDING_TIMEOUT_MINUTES: i64 = 3;

/// Timeout for Sent status: waiting for submission (in minutes)
/// If a transaction stays in Sent for longer than this, mark as Failed
pub const SOLANA_SENT_TIMEOUT_MINUTES: i64 = 3;

/// Maximum number of transaction resubmission attempts before marking as Failed
/// Each attempt creates a new signature (when blockhash expires and tx is resubmitted)
/// Similar to EVM's MAXIMUM_TX_ATTEMPTS but tailored for Solana's resubmission behavior
pub const MAXIMUM_SOLANA_TX_ATTEMPTS: usize = 20;

/// Get status check initial delay duration
pub fn get_solana_status_check_initial_delay() -> Duration {
    Duration::seconds(SOLANA_STATUS_CHECK_INITIAL_DELAY_SECONDS)
}
