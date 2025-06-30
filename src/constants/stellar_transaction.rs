//! Constants for Stellar transaction processing.
//!
//! This module contains default values used throughout the Stellar transaction
//! handling logic, including fees and retry delays.

pub const STELLAR_DEFAULT_TRANSACTION_FEE: u32 = 100;
pub const STELLAR_DEFAULT_STATUS_RETRY_DELAY_SECONDS: i64 = 5;
/// Default maximum fee for fee-bump transactions (0.1 XLM = 1,000,000 stroops)
pub const STELLAR_DEFAULT_MAX_FEE: i64 = 1_000_000;
