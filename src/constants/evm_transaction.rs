use crate::models::evm::Speed;

pub const DEFAULT_TX_VALID_TIMESPAN: i64 = 8 * 60 * 60 * 1000; // 8 hours in milliseconds

pub const DEFAULT_TRANSACTION_SPEED: Speed = Speed::Fast;

/// Minimum gas price bump factor for transaction replacements (10% increase)
pub const MIN_BUMP_FACTOR: f64 = 1.1;

// Maximum number of transaction attempts before considering a NOOP
pub const MAXIMUM_TX_ATTEMPTS: usize = 50;
// Maximum number of NOOP transactions to attempt
pub const MAXIMUM_NOOP_RETRY_ATTEMPTS: u32 = 50;
