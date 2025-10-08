//! Constants for Stellar transaction processing.
//!
//! This module contains default values used throughout the Stellar transaction
//! handling logic, including fees, retry delays, and timeout thresholds.

pub const STELLAR_DEFAULT_TRANSACTION_FEE: u32 = 100;
/// Default maximum fee for fee-bump transactions (0.1 XLM = 1,000,000 stroops)
pub const STELLAR_DEFAULT_MAX_FEE: i64 = 1_000_000;

// Status check scheduling
/// Default delay (in seconds) for transaction status check job after submission
pub const STELLAR_STATUS_CHECK_JOB_DELAY_SECONDS: i64 = 2;

/// Minimum age of transaction before status check processes it (in seconds)
pub const STELLAR_STATUS_CHECK_MIN_AGE_SECONDS: i64 = 5;

// Other delays
/// Default delay (in seconds) for retrying transaction after bad sequence error
pub const STELLAR_BAD_SEQUENCE_RETRY_DELAY_SECONDS: i64 = 2;
