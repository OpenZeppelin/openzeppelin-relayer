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
