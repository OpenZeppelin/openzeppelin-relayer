//! This module provides rate limiting functionality using API keys.

use actix_governor::{KeyExtractor, SimpleKeyExtractionError};
use actix_web::{
    dev::ServiceRequest,
    http::{header::ContentType, StatusCode},
    HttpResponse, HttpResponseBuilder,
};
use governor::clock::{Clock, DefaultClock};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone, Eq, PartialEq)]
pub struct ApiKeyRateLimit;

impl KeyExtractor for ApiKeyRateLimit {
    type Key = String;
    type KeyExtractionError = SimpleKeyExtractionError<&'static str>;

    fn extract(&self, req: &ServiceRequest) -> Result<Self::Key, Self::KeyExtractionError> {
        req.headers()
            .get("x-api-key")
            .and_then(|token| token.to_str().ok())
            .map(|token| token.trim().to_owned())
            .ok_or_else(|| {
                Self::KeyExtractionError::new(
                    r#"{ "success": false, "code": 401, "error": "Unauthorized", "message": "Unauthorized}"#,
                )
                .set_content_type(ContentType::json())
                .set_status_code(StatusCode::UNAUTHORIZED)
            })
    }

    fn exceed_rate_limit_response(
        &self,
        negative: &governor::NotUntil<governor::clock::QuantaInstant>,
        mut response: HttpResponseBuilder,
    ) -> HttpResponse {
        let wait_time = negative
            .wait_time_from(DefaultClock::default().now())
            .as_secs();
        response.content_type(ContentType::json())
            .body(
                format!(
                    r#"{{ "success": false, "code":429, "error": "TooManyRequests, "message": "TooManyRequests", "after": {wait_time}}}"#
                )
            )
    }
}
