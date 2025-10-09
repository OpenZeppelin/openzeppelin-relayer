use crate::models::evm::Speed;
use chrono::Duration;

pub const DEFAULT_TX_VALID_TIMESPAN: i64 = 8 * 60 * 60 * 1000; // 8 hours in milliseconds

pub const DEFAULT_TRANSACTION_SPEED: Speed = Speed::Fast;

pub const DEFAULT_GAS_LIMIT: u64 = 21000;
pub const ERC20_TRANSFER_GAS_LIMIT: u64 = 65_000;
pub const ERC721_TRANSFER_GAS_LIMIT: u64 = 80_000;
pub const COMPLEX_GAS_LIMIT: u64 = 200_000;
pub const GAS_TX_CREATE_CONTRACT: u64 = 53000;

pub const GAS_TX_DATA_ZERO: u64 = 4; // Cost per zero byte in data
pub const GAS_TX_DATA_NONZERO: u64 = 16; // Cost per non-zero byte in data

/// Gas limit buffer multiplier for automatic gas limit estimation, 10% increase
pub const GAS_LIMIT_BUFFER_MULTIPLIER: u64 = 110;

/// Minimum gas price bump factor for transaction replacements (10% increase)
pub const MIN_BUMP_FACTOR: f64 = 1.1;

// Maximum number of transaction attempts before considering a NOOP
pub const MAXIMUM_TX_ATTEMPTS: usize = 50;
// Maximum number of NOOP transactions to attempt
pub const MAXIMUM_NOOP_RETRY_ATTEMPTS: u32 = 50;

/// Time to resubmit for Arbitrum networks
pub const ARBITRUM_TIME_TO_RESUBMIT: i64 = 20_000;

// Gas limit for Arbitrum networks (mainly used for NOOP transactions (with no data), covers L1 + L2 costs)
pub const ARBITRUM_GAS_LIMIT: u64 = 50_000;

/// Gas price cache refresh timeout in seconds (5 minutes)
/// Used to cleanup stuck refresh operations that may have failed to complete
pub const GAS_PRICE_CACHE_REFRESH_TIMEOUT_SECS: u64 = 300;

/// Number of historical blocks to fetch for fee history analysis
pub const HISTORICAL_BLOCKS: u64 = 4;

// EVM Status check and timeout constants

/// Initial delay before first status check (in seconds)
/// Reduced to 10s to enable faster detection of stuck transactions
pub const EVM_STATUS_CHECK_INITIAL_DELAY_SECONDS: i64 = 10;

/// Minimum age of transaction before status check processes it (in seconds)
/// Prevents checking transactions that were just created
pub const EVM_STATUS_CHECK_MIN_AGE_SECONDS: i64 = 15;

/// Timeout for preparation phase: Pending → Sent (in minutes)
/// Increased from 1 to 2 minutes to provide wider recovery window
pub const EVM_PREPARE_TIMEOUT_MINUTES: i64 = 2;

/// Timeout for submission phase: Sent → Submitted (in minutes)
pub const EVM_SUBMIT_TIMEOUT_MINUTES: i64 = 5;

/// Timeout for resend phase: Sent → Submitted (in seconds)
pub const EVM_RESEND_TIMEOUT_SECONDS: i64 = 25;

/// Trigger recovery for stuck Pending transactions (in seconds)
/// Set to 15s to trigger before timeout with sufficient safety margin
pub const EVM_PENDING_RECOVERY_TRIGGER_SECONDS: i64 = 20;

/// Get preparation timeout duration
pub fn get_evm_prepare_timeout() -> Duration {
    Duration::minutes(EVM_PREPARE_TIMEOUT_MINUTES)
}

/// Get submission timeout duration
pub fn get_evm_submit_timeout() -> Duration {
    Duration::minutes(EVM_SUBMIT_TIMEOUT_MINUTES)
}

/// Get resend timeout duration
pub fn get_evm_resend_timeout() -> Duration {
    Duration::seconds(EVM_RESEND_TIMEOUT_SECONDS)
}

/// Get pending recovery trigger duration
pub fn get_evm_pending_recovery_trigger_timeout() -> Duration {
    Duration::seconds(EVM_PENDING_RECOVERY_TRIGGER_SECONDS)
}

/// Get status check initial delay duration
pub fn get_evm_status_check_initial_delay() -> Duration {
    Duration::seconds(EVM_STATUS_CHECK_INITIAL_DELAY_SECONDS)
}
