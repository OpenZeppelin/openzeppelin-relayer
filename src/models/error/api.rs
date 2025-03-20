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

    #[error("Forbidden: {0}")]
    ForbiddenError(String),
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
            ApiError::ForbiddenError(msg) => {
                HttpResponse::Forbidden().json(ApiResponse::<()>::error(msg))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::http::StatusCode;

    #[test]
    fn test_api_error_variants() {
        // Test error message formatting for each variant
        let internal_error = ApiError::InternalError("Database connection failed".to_string());
        assert_eq!(
            internal_error.to_string(),
            "Internal Server Error: Database connection failed"
        );

        let not_found = ApiError::NotFound("User not found".to_string());
        assert_eq!(not_found.to_string(), "Not Found: User not found");

        let bad_request = ApiError::BadRequest("Invalid input".to_string());
        assert_eq!(bad_request.to_string(), "Bad Request: Invalid input");

        let unauthorized = ApiError::Unauthorized("Invalid token".to_string());
        assert_eq!(unauthorized.to_string(), "Unauthorized: Invalid token");

        let not_supported = ApiError::NotSupported("Feature not available".to_string());
        assert_eq!(
            not_supported.to_string(),
            "Not Supported: Feature not available"
        );

        let forbidden = ApiError::ForbiddenError("Access denied".to_string());
        assert_eq!(forbidden.to_string(), "Forbidden: Access denied");

        // Test Report conversion
        let report = Report::msg("Something went wrong");
        let internal_eyre_error = ApiError::InternalEyreError(report);
        assert!(internal_eyre_error
            .to_string()
            .starts_with("Internal Server Error:"));
    }

    #[test]
    fn test_response_error_implementation() {
        // Test that each error variant returns the correct status code and response
        let internal_error = ApiError::InternalError("Server error".to_string());
        let response = internal_error.error_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let not_found = ApiError::NotFound("Resource not found".to_string());
        let response = not_found.error_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let bad_request = ApiError::BadRequest("Invalid parameters".to_string());
        let response = bad_request.error_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let unauthorized = ApiError::Unauthorized("Authentication required".to_string());
        let response = unauthorized.error_response();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let not_supported = ApiError::NotSupported("Not implemented".to_string());
        let response = not_supported.error_response();
        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);

        let forbidden = ApiError::ForbiddenError("Permission denied".to_string());
        let response = forbidden.error_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let report = Report::msg("Internal error");
        let internal_eyre_error = ApiError::InternalEyreError(report);
        let response = internal_eyre_error.error_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
