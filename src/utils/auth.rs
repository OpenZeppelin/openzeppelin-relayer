use actix_web::dev::ServiceRequest;
use actix_web::HttpRequest;
use std::collections::HashMap;
use tracing::info;

use crate::{
    constants::{
        has_permission, has_permission_with_params, AUTHORIZATION_HEADER_NAME,
        AUTHORIZATION_HEADER_VALUE_PREFIX,
    },
    models::{ApiError, SecretString},
    repositories::ApiKeyRepositoryTrait,
};

/// Checks if the authorization header in the request matches the expected API key.
///
/// This function extracts the authorization header from the request and verifies that it starts
/// with the expected prefix (e.g., "Bearer ").
pub fn check_authorization_header(req: &ServiceRequest, _expected_key: &SecretString) -> bool {
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

        info!("Token: {}", token);
        return true;
    }
    false
}

/// Helper to validate API key and check multiple permissions (AND logic - all required)
pub async fn validate_api_key_permissions(
    req: &HttpRequest,
    api_key_repo: &dyn ApiKeyRepositoryTrait,
    required_permissions: &[&str],
) -> Result<(), ApiError> {
    // Extract API key from Authorization header
    let headers: Vec<_> = req.headers().get_all(AUTHORIZATION_HEADER_NAME).collect();
    if headers.len() != 1 {
        return Err(ApiError::Unauthorized(
            "Missing or invalid Authorization header".to_string(),
        ));
    }

    let auth_header = match headers[0].to_str() {
        Ok(header) => header,
        Err(_) => {
            return Err(ApiError::Unauthorized(
                "Invalid Authorization header".to_string(),
            ));
        }
    };

    if !auth_header.starts_with(AUTHORIZATION_HEADER_VALUE_PREFIX) {
        return Err(ApiError::Unauthorized(
            "Invalid Authorization header format".to_string(),
        ));
    }

    let token = &auth_header[AUTHORIZATION_HEADER_VALUE_PREFIX.len()..];
    if token.is_empty() {
        return Err(ApiError::Unauthorized("Empty API key".to_string()));
    }

    // Look up the API key
    let api_key = match api_key_repo.get_by_value(token).await {
        Ok(Some(key)) => key,
        Ok(None) => {
            return Err(ApiError::Unauthorized("Invalid API key".to_string()));
        }
        Err(_) => {
            return Err(ApiError::InternalError(
                "Failed to validate API key".to_string(),
            ));
        }
    };

    // Check permissions with AND logic - ALL permissions must be satisfied
    for required_permission in required_permissions {
        if !has_permission(&api_key.permissions, required_permission) {
            let required_list = required_permissions.join(" AND ");
            return Err(ApiError::ForbiddenError(format!(
                "Insufficient permissions. Required: {}",
                required_list
            )));
        }
    }

    Ok(())
}

/// Enhanced helper to validate API key with parameter substitution support
pub async fn validate_api_key_permissions_with_params(
    req: &HttpRequest,
    api_key_repo: &dyn ApiKeyRepositoryTrait,
    required_permissions: &[&str],
    param_values: &HashMap<String, String>,
) -> Result<(), ApiError> {
    // Extract API key from Authorization header (same as above)
    let headers: Vec<_> = req.headers().get_all(AUTHORIZATION_HEADER_NAME).collect();
    if headers.len() != 1 {
        return Err(ApiError::Unauthorized(
            "Missing or invalid Authorization header".to_string(),
        ));
    }

    let auth_header = match headers[0].to_str() {
        Ok(header) => header,
        Err(_) => {
            return Err(ApiError::Unauthorized(
                "Invalid Authorization header".to_string(),
            ));
        }
    };

    if !auth_header.starts_with(AUTHORIZATION_HEADER_VALUE_PREFIX) {
        return Err(ApiError::Unauthorized(
            "Invalid Authorization header format".to_string(),
        ));
    }

    let token = &auth_header[AUTHORIZATION_HEADER_VALUE_PREFIX.len()..];
    if token.is_empty() {
        return Err(ApiError::Unauthorized("Empty API key".to_string()));
    }

    // Look up the API key
    let api_key = match api_key_repo.get_by_value(token).await {
        Ok(Some(key)) => key,
        Ok(None) => {
            return Err(ApiError::Unauthorized("Invalid API key".to_string()));
        }
        Err(_) => {
            return Err(ApiError::InternalError(
                "Failed to validate API key".to_string(),
            ));
        }
    };

    // Check permissions with parameter substitution
    for required_permission in required_permissions {
        if !has_permission_with_params(&api_key.permissions, required_permission, param_values) {
            let required_list = required_permissions.join(" AND ");
            return Err(ApiError::ForbiddenError(format!(
                "Insufficient permissions. Required: {}",
                required_list
            )));
        }
    }

    Ok(())
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
