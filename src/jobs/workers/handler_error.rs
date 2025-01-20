use thiserror::Error;

use crate::{models::TransactionError, ApiError};

#[derive(Debug, Error)]
pub enum HandlerError {
    #[error("API Error: {source}")]
    ApiError {
        #[from]
        source: ApiError,
    },

    #[error("Transaction Error: {source}")]
    TransactionError {
        #[from]
        source: TransactionError,
    },
}
