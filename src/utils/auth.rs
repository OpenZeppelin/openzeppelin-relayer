use actix_web::dev::ServiceRequest;

use crate::{
    constants::{AUTHORIZATION_HEADER_NAME, AUTHORIZATION_HEADER_VALUE_PREFIX},
    models::SecretString,
};

/// Checks if the authorization header in the request matches the expected API key.
///
/// This function extracts the authorization header from the request, verifies that it starts
/// with the expected prefix (e.g., "Bearer "), and then compares the remaining part of the header
/// value with the expected API key.
pub fn check_authorization_header(req: &ServiceRequest, expected_key: &SecretString) -> bool {
    if let Some(header_value) = req.headers().get(AUTHORIZATION_HEADER_NAME) {
        if let Ok(key) = header_value.to_str() {
            let key = key.trim();
            // Check exact format: "Bearer <token>"
            if !key.starts_with(AUTHORIZATION_HEADER_VALUE_PREFIX) {
                return false;
            }
            // Split on whitespace and ensure exactly 2 parts
            let parts: Vec<&str> = key.split_whitespace().collect();
            if parts.len() != 2 || parts[0] != "Bearer" {
                return false;
            }
            return &SecretString::new(parts[1]) == expected_key;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::test::TestRequest;

    #[test]
    fn test_check_authorization_header_success() {
        let req = TestRequest::default()
            .insert_header((
                AUTHORIZATION_HEADER_NAME,
                format!("{}{}", AUTHORIZATION_HEADER_VALUE_PREFIX, "test_key"),
            ))
            .to_srv_request();

        assert!(check_authorization_header(
            &req,
            &SecretString::new("test_key")
        ));
    }

    #[test]
    fn test_check_authorization_header_missing_header() {
        let req = TestRequest::default().to_srv_request();

        assert!(!check_authorization_header(
            &req,
            &SecretString::new("test_key")
        ));
    }

    #[test]
    fn test_check_authorization_header_invalid_prefix() {
        let req = TestRequest::default()
            .insert_header((AUTHORIZATION_HEADER_NAME, "InvalidPrefix test_key"))
            .to_srv_request();

        assert!(!check_authorization_header(
            &req,
            &SecretString::new("test_key")
        ));
    }

    #[test]
    fn test_check_authorization_header_invalid_key() {
        let req = TestRequest::default()
            .insert_header((
                AUTHORIZATION_HEADER_NAME,
                format!("{}{}", AUTHORIZATION_HEADER_VALUE_PREFIX, "invalid_key"),
            ))
            .to_srv_request();

        assert!(!check_authorization_header(
            &req,
            &SecretString::new("test_key")
        ));
    }

    #[test]
    fn test_check_authorization_header_multiple_bearer() {
        let req = TestRequest::default()
            .insert_header((
                AUTHORIZATION_HEADER_NAME,
                format!("Bearer Bearer {}", "test_key"),
            ))
            .to_srv_request();

        assert!(!check_authorization_header(
            &req,
            &SecretString::new("test_key")
        ));
    }

    #[test]
    fn test_check_authorization_header_multiple_headers() {
        let req = TestRequest::default()
            .insert_header((
                AUTHORIZATION_HEADER_NAME,
                format!("{}{}", AUTHORIZATION_HEADER_VALUE_PREFIX, "test_key"),
            ))
            .insert_header((
                AUTHORIZATION_HEADER_NAME,
                format!("{}{}", AUTHORIZATION_HEADER_VALUE_PREFIX, "another_key"),
            ))
            .to_srv_request();

        // Should use the first header value only
        assert!(check_authorization_header(
            &req,
            &SecretString::new("test_key")
        ));
    }

    #[test]
    fn test_check_authorization_header_malformed_bearer() {
        // Test with extra spaces
        let req = TestRequest::default()
            .insert_header((AUTHORIZATION_HEADER_NAME, "Bearer    test_key"))
            .to_srv_request();

        assert!(!check_authorization_header(
            &req,
            &SecretString::new("test_key")
        ));

        // Test with Bearer in token
        let req = TestRequest::default()
            .insert_header((AUTHORIZATION_HEADER_NAME, "Bearer Bearer"))
            .to_srv_request();

        assert!(!check_authorization_header(
            &req,
            &SecretString::new("Bearer")
        ));

        // Test with empty token
        let req = TestRequest::default()
            .insert_header((AUTHORIZATION_HEADER_NAME, "Bearer "))
            .to_srv_request();

        assert!(!check_authorization_header(&req, &SecretString::new("")));
    }
}
