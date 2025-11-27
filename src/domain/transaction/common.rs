//! Common transaction utilities shared across all blockchain networks.
//!
//! This module contains utility functions and constants that are used
//! across multiple blockchain domains (EVM, Solana, Stellar) to avoid
//! cross-domain dependencies.

use chrono::{DateTime, Duration, Utc};

use crate::constants::FINAL_TRANSACTION_STATUSES;
use crate::models::{TransactionError, TransactionRepoModel, TransactionStatus};

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

pub fn is_pending_transaction(tx_status: &TransactionStatus) -> bool {
    matches!(
        tx_status,
        TransactionStatus::Pending | TransactionStatus::Sent | TransactionStatus::Submitted
    )
}

pub fn is_unsubmitted_transaction(tx_status: &TransactionStatus) -> bool {
    matches!(
        tx_status,
        TransactionStatus::Pending | TransactionStatus::Sent
    )
}

/// Gets the age of a transaction since it was sent.
pub fn get_age_of_sent_at(tx: &TransactionRepoModel) -> Result<Duration, TransactionError> {
    let now = Utc::now();
    let sent_at_str = tx.sent_at.as_ref().ok_or_else(|| {
        TransactionError::UnexpectedError("Transaction sent_at time is missing".to_string())
    })?;
    let sent_time = DateTime::parse_from_rfc3339(sent_at_str)
        .map_err(|_| TransactionError::UnexpectedError("Error parsing sent_at time".to_string()))?
        .with_timezone(&Utc);
    Ok(now.signed_duration_since(sent_time))
}

#[cfg(test)]
mod tests {
    use crate::utils::mocks::mockutils::create_mock_transaction;

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

    #[test]
    fn test_is_pending_transaction() {
        // Test pending status
        assert!(is_pending_transaction(&TransactionStatus::Pending));

        // Test sent status
        assert!(is_pending_transaction(&TransactionStatus::Sent));

        // Test submitted status
        assert!(is_pending_transaction(&TransactionStatus::Submitted));

        // Test non-pending statuses
        assert!(!is_pending_transaction(&TransactionStatus::Confirmed));
        assert!(!is_pending_transaction(&TransactionStatus::Failed));
        assert!(!is_pending_transaction(&TransactionStatus::Canceled));
        assert!(!is_pending_transaction(&TransactionStatus::Mined));
        assert!(!is_pending_transaction(&TransactionStatus::Expired));
    }

    #[test]
    fn test_is_unsubmitted_transaction() {
        // Unsubmitted statuses should return true
        assert!(is_unsubmitted_transaction(&TransactionStatus::Pending));
        assert!(is_unsubmitted_transaction(&TransactionStatus::Sent));

        // Submitted and other statuses should return false
        assert!(!is_unsubmitted_transaction(&TransactionStatus::Submitted));
        assert!(!is_unsubmitted_transaction(&TransactionStatus::Mined));
        assert!(!is_unsubmitted_transaction(&TransactionStatus::Confirmed));
        assert!(!is_unsubmitted_transaction(&TransactionStatus::Failed));
        assert!(!is_unsubmitted_transaction(&TransactionStatus::Canceled));
        assert!(!is_unsubmitted_transaction(&TransactionStatus::Expired));
    }

    #[test]
    fn test_get_age_of_sent_at() {
        let now = Utc::now();

        // Test with valid sent_at timestamp (1 hour ago)
        let sent_at_time = now - Duration::hours(1);
        let mut tx = create_mock_transaction();
        tx.sent_at = Some(sent_at_time.to_rfc3339());

        let age_result = get_age_of_sent_at(&tx);
        assert!(age_result.is_ok());
        let age = age_result.unwrap();
        // Age should be approximately 1 hour (with some tolerance for test execution time)
        assert!(age.num_minutes() >= 59 && age.num_minutes() <= 61);
    }

    #[test]
    fn test_get_age_of_sent_at_missing_sent_at() {
        let mut tx = create_mock_transaction();
        tx.sent_at = None; // Missing sent_at

        let result = get_age_of_sent_at(&tx);
        assert!(result.is_err());
        match result.unwrap_err() {
            TransactionError::UnexpectedError(msg) => {
                assert!(msg.contains("sent_at time is missing"));
            }
            _ => panic!("Expected UnexpectedError for missing sent_at"),
        }
    }

    #[test]
    fn test_get_age_of_sent_at_invalid_timestamp() {
        let mut tx = create_mock_transaction();
        tx.sent_at = Some("invalid-timestamp".to_string()); // Invalid timestamp format

        let result = get_age_of_sent_at(&tx);
        assert!(result.is_err());
        match result.unwrap_err() {
            TransactionError::UnexpectedError(msg) => {
                assert!(msg.contains("Error parsing sent_at time"));
            }
            _ => panic!("Expected UnexpectedError for invalid timestamp"),
        }
    }
}
