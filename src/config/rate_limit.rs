//! This module provides rate limiting functionality using API keys.

use crate::constants::{AUTHORIZATION_HEADER_NAME, PUBLIC_ENDPOINTS};
use actix_governor::governor::clock::{Clock, DefaultClock, QuantaInstant};
use actix_governor::governor::NotUntil;
use actix_governor::{KeyExtractor, SimpleKeyExtractionError};
use actix_web::{
    dev::ServiceRequest,
    http::{header::ContentType, StatusCode},
    HttpResponse, HttpResponseBuilder,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone, Eq, PartialEq)]
pub struct ApiKeyRateLimit;

impl KeyExtractor for ApiKeyRateLimit {
    type Key = String;
    type KeyExtractionError = SimpleKeyExtractionError<&'static str>;

    fn extract(&self, req: &ServiceRequest) -> Result<Self::Key, Self::KeyExtractionError> {
        let path = req.path();
        let is_public_endpoint = PUBLIC_ENDPOINTS
            .iter()
            .any(|prefix| path.starts_with(prefix));

        if is_public_endpoint {
            return Ok("swagger-ui-exempt".to_string());
        }
        req.headers()
            .get(AUTHORIZATION_HEADER_NAME)
            .and_then(|token| token.to_str().ok())
            .map(|token| token.trim().to_owned())
            .ok_or_else(|| {
                Self::KeyExtractionError::new(
					r#"{"success": false, "code": 401, "error": "Unauthorized", "message": "Unauthorized"}"#,
				)
				.set_content_type(ContentType::json())
				.set_status_code(StatusCode::UNAUTHORIZED)
            })
    }

    fn exceed_rate_limit_response(
        &self,
        negative: &NotUntil<QuantaInstant>,
        mut response: HttpResponseBuilder,
    ) -> HttpResponse {
        let wait_time = negative
            .wait_time_from(DefaultClock::default().now())
            .as_secs();
        response.content_type(ContentType::json())
            .body(
                format!(
                    r#"{{ "success": false, "code":429, "error": "TooManyRequests", "message": "Too Many Requests", "after": {wait_time}}}"#
                )
            )
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use actix_governor::governor::{Quota, RateLimiter};
    use actix_web::test::TestRequest;
    use actix_web::{body::MessageBody, http::header::HeaderValue};
    use std::num::NonZeroU32;

    #[tokio::test]
    async fn test_extract_with_valid_api_key() {
        let api_key = "test-api-key";
        let req = TestRequest::default()
            .insert_header((AUTHORIZATION_HEADER_NAME, api_key))
            .to_srv_request();

        let extractor = ApiKeyRateLimit;
        let result = extractor.extract(&req);

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), api_key);
    }

    #[tokio::test]
    async fn test_extract_with_whitespace_in_api_key() {
        let api_key = "  test-api-key-with-spaces  ";
        let expected_key = "test-api-key-with-spaces";
        let req = TestRequest::default()
            .insert_header((AUTHORIZATION_HEADER_NAME, api_key))
            .to_srv_request();

        let extractor = ApiKeyRateLimit;
        let result = extractor.extract(&req);

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), expected_key);
    }

    #[tokio::test]
    async fn test_exceed_rate_limit_response() {
        let extractor = ApiKeyRateLimit;

        // Create a keyed rate limiter
        let quota = Quota::per_second(NonZeroU32::new(1).unwrap());
        let limiter = RateLimiter::keyed(quota);

        // Make two requests to trigger rate limiting
        let _ = limiter.check_key(&"test_key");
        let negative = limiter.check_key(&"test_key").unwrap_err();

        let response_builder = HttpResponse::TooManyRequests();
        let response = extractor.exceed_rate_limit_response(&negative, response_builder);

        // Check status code and content type
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            response
                .headers()
                .get(actix_web::http::header::CONTENT_TYPE),
            Some(&HeaderValue::from_static("application/json"))
        );

        // Check response body
        let body = response.into_body();
        let bytes = body.try_into_bytes().unwrap();
        let body_str = std::str::from_utf8(&bytes).unwrap();

        // Verify JSON structure contains expected fields
        assert!(body_str.contains(r#""success": false"#));
        assert!(body_str.contains(r#""code":429"#));
        assert!(body_str.contains(r#""error": "TooManyRequests""#));
        assert!(body_str.contains(r#""message": "Too Many Requests""#));
        assert!(body_str.contains(r#""after":"#));
    }
}
