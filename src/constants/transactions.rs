//! Transaction-related constants
//!
//! This module contains generic transaction constants that apply across all chain types.
//! Chain-specific constants are defined in their respective modules:
//! - EVM constants: `crate::constants::evm_transaction`
//! - Stellar constants: `crate::constants::stellar_transaction`

use crate::models::TransactionStatus;

/// Transaction statuses that are considered final states.
/// These states indicate that a transaction has reached its terminal state
/// and will not be processed further.
pub const FINAL_TRANSACTION_STATUSES: &[TransactionStatus] = &[
    TransactionStatus::Canceled,
    TransactionStatus::Confirmed,
    TransactionStatus::Failed,
    TransactionStatus::Expired,
];

pub const PENDING_TRANSACTION_STATUSES: &[TransactionStatus] = &[
    TransactionStatus::Pending,
    TransactionStatus::Sent,
    TransactionStatus::Submitted,
];

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

    #[test]
    fn test_pending_transaction_statuses_contains_expected_values() {
        assert_eq!(PENDING_TRANSACTION_STATUSES.len(), 3);
        assert!(PENDING_TRANSACTION_STATUSES.contains(&TransactionStatus::Pending));
        assert!(PENDING_TRANSACTION_STATUSES.contains(&TransactionStatus::Sent));
        assert!(PENDING_TRANSACTION_STATUSES.contains(&TransactionStatus::Submitted));
    }

    #[test]
    fn test_pending_transaction_statuses_excludes_final_states() {
        assert!(!PENDING_TRANSACTION_STATUSES.contains(&TransactionStatus::Canceled));
        assert!(!PENDING_TRANSACTION_STATUSES.contains(&TransactionStatus::Confirmed));
        assert!(!PENDING_TRANSACTION_STATUSES.contains(&TransactionStatus::Failed));
        assert!(!PENDING_TRANSACTION_STATUSES.contains(&TransactionStatus::Expired));
        assert!(!PENDING_TRANSACTION_STATUSES.contains(&TransactionStatus::Mined));
    }
}
