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
    // Ensure there is exactly one Authorization header
    let headers: Vec<_> = req.headers().get_all(AUTHORIZATION_HEADER_NAME).collect();
    if headers.len() != 1 {
        return false;
    }

    if let Ok(key) = headers[0].to_str() {
        if !key.starts_with(AUTHORIZATION_HEADER_VALUE_PREFIX) {
            return false;
        }
        let prefix_len = AUTHORIZATION_HEADER_VALUE_PREFIX.len();
        let token = &key[prefix_len..];

        if token.is_empty() || token.contains(' ') {
            return false;
        }

        return &SecretString::new(token) == expected_key;
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

        // Should reject multiple Authorization headers
        assert!(!check_authorization_header(
            &req,
            &SecretString::new("test_key")
        ));
    }

    #[test]
    fn test_check_authorization_header_malformed_bearer() {
        // Test with Bearer in token
        let req = TestRequest::default()
            .insert_header((AUTHORIZATION_HEADER_NAME, "Bearer Bearer token"))
            .to_srv_request();

        assert!(!check_authorization_header(
            &req,
            &SecretString::new("token")
        ));

        // Test with empty token
        let req = TestRequest::default()
            .insert_header((AUTHORIZATION_HEADER_NAME, "Bearer "))
            .to_srv_request();

        assert!(!check_authorization_header(&req, &SecretString::new("")));
    }
}
