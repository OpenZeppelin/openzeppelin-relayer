//! Constants for Solana transaction processing.
//!
//! This module contains default values used throughout the Solana transaction
//! handling logic, including validation limits, status check delays, and timeout thresholds.

use chrono::Duration;

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
/// Set to 8s to allow time for transaction propagation on Solana
pub const SOLANA_STATUS_CHECK_INITIAL_DELAY_SECONDS: i64 = 5;

/// Minimum age before checking for resubmit/expiration (in seconds)
/// If transaction is younger than this, we don't check blockhash expiration yet
pub const SOLANA_MIN_AGE_FOR_RESUBMIT_CHECK_SECONDS: i64 = 60;

/// Timeout for Pending status: transaction preparation phase (in minutes)
/// If a transaction stays in Pending for longer than this, mark as Failed
pub const SOLANA_PENDING_TIMEOUT_MINUTES: i64 = 3;

/// Timeout for Sent status: waiting for submission (in minutes)
/// If a transaction stays in Sent for longer than this, mark as Failed
pub const SOLANA_SENT_TIMEOUT_MINUTES: i64 = 3;

/// Timeout for Submitted status: waiting for on-chain confirmation (in minutes)
/// If a transaction stays in Submitted for longer than this and blockhash expired, mark as Failed
pub const SOLANA_SUBMITTED_TIMEOUT_MINUTES: i64 = 3;

/// Get status check initial delay duration
pub fn get_solana_status_check_initial_delay() -> Duration {
    Duration::seconds(SOLANA_STATUS_CHECK_INITIAL_DELAY_SECONDS)
}
