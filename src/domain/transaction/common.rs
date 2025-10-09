//! Common transaction utilities shared across all blockchain networks.
//!
//! This module contains utility functions and constants that are used
//! across multiple blockchain domains (EVM, Solana, Stellar) to avoid
//! cross-domain dependencies.

use crate::constants::FINAL_TRANSACTION_STATUSES;
use crate::models::TransactionStatus;

/// Checks if a transaction is in a final state (confirmed, failed, canceled, or expired).
///
/// Final states are terminal states where no further status updates are expected.
/// This is used across all blockchain implementations to determine if a transaction
/// has completed processing.
///
/// # Arguments
///
/// * `tx_status` - The transaction status to check
///
/// # Returns
///
/// `true` if the transaction is in a final state, `false` otherwise
pub fn is_final_state(tx_status: &TransactionStatus) -> bool {
    FINAL_TRANSACTION_STATUSES.contains(tx_status)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_final_state() {
        // Final states should return true
        assert!(is_final_state(&TransactionStatus::Confirmed));
        assert!(is_final_state(&TransactionStatus::Failed));
        assert!(is_final_state(&TransactionStatus::Expired));
        assert!(is_final_state(&TransactionStatus::Canceled));

        // Non-final states should return false
        assert!(!is_final_state(&TransactionStatus::Pending));
        assert!(!is_final_state(&TransactionStatus::Sent));
        assert!(!is_final_state(&TransactionStatus::Submitted));
        assert!(!is_final_state(&TransactionStatus::Mined));
    }
}
