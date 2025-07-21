use crate::models::evm::Speed;

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
