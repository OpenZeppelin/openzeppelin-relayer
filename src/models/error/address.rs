use serde::Serialize;
use thiserror::Error;

use super::{SignerError, TransactionError};

#[derive(Error, Debug, Serialize)]
pub enum AddressError {
    #[error("Address conversion error: {0}")]
    ConversionError(String),
}

impl From<AddressError> for SignerError {
    fn from(err: AddressError) -> Self {
        SignerError::SigningError(err.to_string())
    }
}

impl From<AddressError> for TransactionError {
    fn from(err: AddressError) -> Self {
        TransactionError::ValidationError(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_address_error_creation() {
        let error = AddressError::ConversionError("Invalid format".to_string());
        assert!(matches!(error, AddressError::ConversionError(_)));
        assert_eq!(
            error.to_string(),
            "Address conversion error: Invalid format"
        );
    }

    #[test]
    fn test_conversion_to_signer_error() {
        let address_error = AddressError::ConversionError("Invalid format".to_string());
        let signer_error: SignerError = address_error.into();

        assert!(matches!(signer_error, SignerError::SigningError(_)));
        assert_eq!(
            signer_error.to_string(),
            "Failed to sign transaction: Address conversion error: Invalid format"
        );
    }

    #[test]
    fn test_conversion_to_transaction_error() {
        let address_error = AddressError::ConversionError("Invalid format".to_string());
        let transaction_error: TransactionError = address_error.into();

        assert!(matches!(
            transaction_error,
            TransactionError::ValidationError(_)
        ));
        assert_eq!(
            transaction_error.to_string(),
            "Transaction validation error: Address conversion error: Invalid format"
        );
    }
}
