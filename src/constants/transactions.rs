//! Transaction-related constants

use crate::models::TransactionStatus;
use chrono::Duration;

/// Transaction statuses that are considered final states.
pub const FINAL_TRANSACTION_STATUSES: &[TransactionStatus] = &[
    TransactionStatus::Canceled,
    TransactionStatus::Confirmed,
    TransactionStatus::Failed,
    TransactionStatus::Expired,
];

// Status check scheduling constants
/// Initial delay before first status check (in seconds)
/// Reduced to 10s to enable faster detection of stuck transactions
pub const STATUS_CHECK_INITIAL_DELAY_SECONDS: i64 = 10;

/// Minimum age of transaction before status check processes it (in seconds)
/// Prevents checking transactions that were just created
pub const STATUS_CHECK_MIN_AGE_SECONDS: i64 = 15;

// Timeout thresholds for failure detection
/// Timeout for preparation phase: Pending → Sent (in minutes)
/// Increased from 1 to 2 minutes to provide wider recovery window
pub const PREPARE_TIMEOUT_MINUTES: i64 = 2;

/// Timeout for submission phase: Sent → Submitted (in minutes)
pub const SUBMIT_TIMEOUT_MINUTES: i64 = 5;

/// Timeout for resend phase: Sent → Submitted (in seconds)
pub const RESEND_TIMEOUT_SECONDS: i64 = 20;

// Recovery trigger thresholds
/// Trigger recovery for stuck Pending transactions (in seconds)
/// Set to 15s to trigger before timeout with sufficient safety margin
pub const PENDING_RECOVERY_TRIGGER_SECONDS: i64 = 15;

/// Timeout for confirmation phase: Submitted → Final (in hours)
pub const CONFIRMATION_TIMEOUT_HOURS: i64 = 1;

/// Get preparation timeout duration
pub fn get_prepare_timeout() -> Duration {
    Duration::minutes(PREPARE_TIMEOUT_MINUTES)
}

/// Get submission timeout duration
pub fn get_submit_timeout() -> Duration {
    Duration::minutes(SUBMIT_TIMEOUT_MINUTES)
}

/// Get confirmation timeout duration
pub fn get_confirmation_timeout() -> Duration {
    Duration::hours(CONFIRMATION_TIMEOUT_HOURS)
}

/// Get resend timeout duration
pub fn get_resend_timeout() -> Duration {
    Duration::seconds(RESEND_TIMEOUT_SECONDS)
}

/// Get pending recovery trigger duration
pub fn get_pending_recovery_trigger_timeout() -> Duration {
    Duration::seconds(PENDING_RECOVERY_TRIGGER_SECONDS)
}

/// Get status check initial delay duration
pub fn get_status_check_initial_delay() -> Duration {
    Duration::seconds(STATUS_CHECK_INITIAL_DELAY_SECONDS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_final_transaction_statuses_contains_expected_values() {
        assert_eq!(FINAL_TRANSACTION_STATUSES.len(), 4);
        assert!(FINAL_TRANSACTION_STATUSES.contains(&TransactionStatus::Canceled));
        assert!(FINAL_TRANSACTION_STATUSES.contains(&TransactionStatus::Confirmed));
        assert!(FINAL_TRANSACTION_STATUSES.contains(&TransactionStatus::Failed));
        assert!(FINAL_TRANSACTION_STATUSES.contains(&TransactionStatus::Expired));
    }

    #[test]
    fn test_final_transaction_statuses_excludes_non_final_states() {
        assert!(!FINAL_TRANSACTION_STATUSES.contains(&TransactionStatus::Pending));
        assert!(!FINAL_TRANSACTION_STATUSES.contains(&TransactionStatus::Sent));
        assert!(!FINAL_TRANSACTION_STATUSES.contains(&TransactionStatus::Submitted));
        assert!(!FINAL_TRANSACTION_STATUSES.contains(&TransactionStatus::Mined));
    }
}
