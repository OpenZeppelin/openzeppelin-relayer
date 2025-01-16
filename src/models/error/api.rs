use actix_web::{HttpResponse, ResponseError};
use eyre::Report;
use thiserror::Error;

use crate::models::ApiResponse;

#[derive(Error, Debug)]
pub enum ApiError {
    #[error("Internal Server Error: {0}")]
    InternalEyreError(#[from] Report),

    #[error("Internal Server Error: {0}")]
    InternalError(String),

    #[error("Not Found: {0}")]
    NotFound(String),

    #[error("Bad Request: {0}")]
    BadRequest(String),

    #[error("Unauthorized: {0}")]
    Unauthorized(String),

    #[error("Not Supported: {0}")]
    NotSupported(String),
}

impl ResponseError for ApiError {
    fn error_response(&self) -> HttpResponse {
        match self {
            ApiError::InternalError(msg) => {
                HttpResponse::InternalServerError().json(ApiResponse::<()>::error(msg))
            }
            ApiError::NotFound(msg) => HttpResponse::NotFound().json(ApiResponse::<()>::error(msg)),
            ApiError::BadRequest(msg) => {
                HttpResponse::BadRequest().json(ApiResponse::<()>::error(msg))
            }
            ApiError::Unauthorized(msg) => {
                HttpResponse::Unauthorized().json(ApiResponse::<()>::error(msg))
            }
            ApiError::NotSupported(msg) => {
                HttpResponse::NotImplemented().json(ApiResponse::<()>::error(msg))
            }
            ApiError::InternalEyreError(msg) => {
                HttpResponse::InternalServerError().json(ApiResponse::<()>::error(msg.to_string()))
            }
        }
    }
}
