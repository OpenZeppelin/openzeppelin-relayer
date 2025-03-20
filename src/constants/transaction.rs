use crate::models::evm::Speed;

pub const DEFAULT_TX_VALID_TIMESPAN: i64 = 8 * 60 * 60 * 1000; // 8 hours in milliseconds

pub const DEFAULT_TRANSACTION_SPEED: Speed = Speed::Fast;
