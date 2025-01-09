use actix_web::{HttpResponse, ResponseError};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum RelayerApiError {
    #[error("Internal Server Error")]
    InternalError,

    #[error("Not Found: {0}")]
    NotFound(String),

    #[error("Bad Request: {0}")]
    BadRequest(String),

    #[error("Unauthorized: {0}")]
    Unauthorized(String),

    #[error("Not supported: {0}")]
    NotSupported(String),
}

impl ResponseError for RelayerApiError {
    fn error_response(&self) -> HttpResponse {
        match self {
            RelayerApiError::InternalError => {
                HttpResponse::InternalServerError().json(self.to_string())
            }
            RelayerApiError::NotFound(msg) => HttpResponse::NotFound().json(msg),
            RelayerApiError::BadRequest(msg) => HttpResponse::BadRequest().json(msg),
            RelayerApiError::Unauthorized(msg) => HttpResponse::Unauthorized().json(msg),
            RelayerApiError::NotSupported(msg) => HttpResponse::NotImplemented().json(msg),
        }
    }
}
