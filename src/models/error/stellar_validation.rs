//! Specific error types for Stellar transaction validation
//!
//! This module provides granular error types for different validation failures,
//! making it easier to handle specific error cases and provide better error messages.

use serde::Serialize;
use thiserror::Error;

/// Specific errors that can occur during Stellar transaction validation
#[derive(Error, Debug, Serialize, Clone, PartialEq)]
#[serde(tag = "type", content = "details")]
pub enum StellarValidationError {
    /// Transaction has no operations
    #[error("Transaction must have at least one operation")]
    EmptyOperations,

    /// Transaction exceeds maximum operation count
    #[error("Transaction has {count} operations, but maximum allowed is {max}")]
    TooManyOperations { count: usize, max: usize },

    /// Multiple Soroban operations in a single transaction
    #[error("Transaction can contain at most one Soroban operation")]
    MultipleSorobanOperations,

    /// Soroban operation mixed with non-Soroban operations
    #[error("Soroban operations must be exclusive - no other operations allowed in the same transaction")]
    SorobanNotExclusive,

    /// Soroban operation with non-None memo
    #[error("Soroban operations cannot have a memo (except MemoNone)")]
    SorobanWithMemo,

    /// Source account mismatch for unsigned XDR
    #[error("XDR source account {actual} does not match relayer account {expected}")]
    SourceAccountMismatch { expected: String, actual: String },

    /// Signed XDR when unsigned was expected
    #[error("Expected unsigned XDR but received signed XDR")]
    UnexpectedSignedXdr,

    /// Unsigned XDR when signed was expected
    #[error("Expected signed XDR but received unsigned XDR")]
    UnexpectedUnsignedXdr,

    /// Invalid max_fee for signed XDR
    #[error("max_fee must be greater than 0 for signed XDR")]
    InvalidMaxFee,

    /// Generic XDR parsing error
    #[error("Invalid XDR: {0}")]
    InvalidXdr(String),
}

/// Convert StellarValidationError to the generic TransactionError
impl From<StellarValidationError> for crate::models::TransactionError {
    fn from(err: StellarValidationError) -> Self {
        crate::models::TransactionError::ValidationError(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_messages() {
        assert_eq!(
            StellarValidationError::EmptyOperations.to_string(),
            "Transaction must have at least one operation"
        );

        assert_eq!(
            StellarValidationError::TooManyOperations {
                count: 150,
                max: 100
            }
            .to_string(),
            "Transaction has 150 operations, but maximum allowed is 100"
        );

        assert_eq!(
            StellarValidationError::SourceAccountMismatch {
                expected: "GAAAA...".to_string(),
                actual: "GBBBB...".to_string()
            }
            .to_string(),
            "XDR source account GBBBB... does not match relayer account GAAAA..."
        );
    }

    #[test]
    fn test_serialization() {
        let error = StellarValidationError::TooManyOperations {
            count: 150,
            max: 100,
        };
        let json = serde_json::to_string(&error).unwrap();
        assert!(json.contains("\"type\":\"TooManyOperations\""));
        assert!(json.contains("\"count\":150"));
        assert!(json.contains("\"max\":100"));
    }

    #[test]
    fn test_conversion_to_transaction_error() {
        let stellar_err = StellarValidationError::SorobanWithMemo;
        let tx_err: crate::models::TransactionError = stellar_err.into();

        match tx_err {
            crate::models::TransactionError::ValidationError(msg) => {
                assert_eq!(
                    msg,
                    "Soroban operations cannot have a memo (except MemoNone)"
                );
            }
            _ => panic!("Expected ValidationError variant"),
        }
    }
}
