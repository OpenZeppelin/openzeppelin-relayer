use actix_web::dev::ServiceRequest;

use crate::constants::{AUTHORIZATION_HEADER_NAME, AUTHORIZATION_HEADER_VALUE_PREFIX};

/// Checks if the authorization header in the request matches the expected API key.
///
/// This function extracts the authorization header from the request, verifies that it starts
/// with the expected prefix (e.g., "Bearer "), and then compares the remaining part of the header
/// value with the expected API key.
pub fn check_authorization_header(req: &ServiceRequest, expected_key: &str) -> bool {
    if let Some(header_value) = req.headers().get(AUTHORIZATION_HEADER_NAME) {
        if let Ok(key) = header_value.to_str() {
            let starts_with_bearer = key.starts_with(AUTHORIZATION_HEADER_VALUE_PREFIX);
            let key_without_bearer = key.trim_start_matches(AUTHORIZATION_HEADER_VALUE_PREFIX);
            return starts_with_bearer && key_without_bearer == expected_key;
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

        assert!(check_authorization_header(&req, "test_key"));
    }

    #[test]
    fn test_check_authorization_header_missing_header() {
        let req = TestRequest::default().to_srv_request();

        assert!(!check_authorization_header(&req, "test_key"));
    }

    #[test]
    fn test_check_authorization_header_invalid_prefix() {
        let req = TestRequest::default()
            .insert_header((AUTHORIZATION_HEADER_NAME, "InvalidPrefix test_key"))
            .to_srv_request();

        assert!(!check_authorization_header(&req, "test_key"));
    }

    #[test]
    fn test_check_authorization_header_invalid_key() {
        let req = TestRequest::default()
            .insert_header((
                AUTHORIZATION_HEADER_NAME,
                format!("{}{}", AUTHORIZATION_HEADER_VALUE_PREFIX, "invalid_key"),
            ))
            .to_srv_request();

        assert!(!check_authorization_header(&req, "test_key"));
    }
}
