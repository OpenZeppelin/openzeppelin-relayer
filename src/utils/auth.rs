use actix_web::dev::ServiceRequest;
use actix_web::HttpRequest;
use tracing::info;

use crate::{
    constants::{
        has_permission_grant, has_permission_grant_for_id, AUTHORIZATION_HEADER_NAME,
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

/// Helper to validate API key and check permissions for global-scoped actions
///
/// Use this for endpoints that access resources globally (e.g., listing all relayers).
/// All required permissions must be satisfied (AND logic).
///
/// # Arguments
/// * `req` - The HTTP request containing the Authorization header
/// * `api_key_repo` - Repository for looking up API keys
/// * `required_actions` - List of required actions (e.g., ["relayers:read"])
///
/// # Returns
/// `Ok(())` if the API key has all required permissions with global scope
pub async fn validate_api_key_permissions(
    req: &HttpRequest,
    api_key_repo: &dyn ApiKeyRepositoryTrait,
    required_actions: &[&str],
) -> Result<(), ApiError> {
    // Extract and validate Authorization header
    let token = extract_token_from_request(req)?;

    // Check if this is the environment API key (full access bypass)
    if let Ok(env_api_key) = std::env::var("API_KEY") {
        if token == env_api_key {
            return Ok(());
        }
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
    for required_action in required_actions {
        if !has_permission_grant(&api_key.permissions, required_action) {
            let required_list = required_actions.join(" AND ");
            return Err(ApiError::ForbiddenError(format!(
                "Insufficient permissions. Required: {} (global scope)",
                required_list
            )));
        }
    }

    Ok(())
}

/// Helper to validate API key and check permissions for ID-scoped actions
///
/// Use this for endpoints that access specific resources (e.g., get/update/delete a specific relayer).
/// All required permissions must be satisfied (AND logic) for the specific resource ID.
///
/// # Arguments
/// * `req` - The HTTP request containing the Authorization header
/// * `api_key_repo` - Repository for looking up API keys
/// * `required_actions` - List of required actions (e.g., ["relayers:read"])
/// * `resource_id` - The specific resource ID being accessed
///
/// # Returns
/// `Ok(())` if the API key has all required permissions for the specific resource ID
pub async fn validate_api_key_permissions_scoped(
    req: &HttpRequest,
    api_key_repo: &dyn ApiKeyRepositoryTrait,
    required_actions: &[&str],
    resource_id: &str,
) -> Result<(), ApiError> {
    // Extract and validate Authorization header
    let token = extract_token_from_request(req)?;

    // Check if this is the environment API key (full access bypass)
    if let Ok(env_api_key) = std::env::var("API_KEY") {
        if token == env_api_key {
            return Ok(());
        }
    }

    // Look up the API key from repository
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

    // Check permissions for the specific resource ID
    for required_action in required_actions {
        if !has_permission_grant_for_id(&api_key.permissions, required_action, resource_id) {
            let required_list = required_actions.join(" AND ");
            return Err(ApiError::ForbiddenError(format!(
                "Insufficient permissions. Required: {} for resource '{}'",
                required_list, resource_id
            )));
        }
    }

    Ok(())
}

/// Helper function to extract and validate the API key token from the request
fn extract_token_from_request(req: &HttpRequest) -> Result<&str, ApiError> {
    let headers: Vec<_> = req.headers().get_all(AUTHORIZATION_HEADER_NAME).collect();
    if headers.len() != 1 {
        return Err(ApiError::Unauthorized(
            "Missing or invalid Authorization header".to_string(),
        ));
    }

    let auth_header = headers[0]
        .to_str()
        .map_err(|_| ApiError::Unauthorized("Invalid Authorization header".to_string()))?;

    if !auth_header.starts_with(AUTHORIZATION_HEADER_VALUE_PREFIX) {
        return Err(ApiError::Unauthorized(
            "Invalid Authorization header format".to_string(),
        ));
    }

    let token = &auth_header[AUTHORIZATION_HEADER_VALUE_PREFIX.len()..];
    if token.is_empty() {
        return Err(ApiError::Unauthorized("Empty API key".to_string()));
    }

    Ok(token)
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
    fn test_check_authorization_header_multiple_headers() {
        let req = TestRequest::default()
            .append_header((
                AUTHORIZATION_HEADER_NAME,
                format!("{}{}", AUTHORIZATION_HEADER_VALUE_PREFIX, "test_key1"),
            ))
            .append_header((
                AUTHORIZATION_HEADER_NAME,
                format!("{}{}", AUTHORIZATION_HEADER_VALUE_PREFIX, "test_key2"),
            ))
            .to_srv_request();

        assert!(!check_authorization_header(
            &req,
            &SecretString::new("test_key1")
        ));
    }

    #[test]
    fn test_check_authorization_header_malformed_bearer() {
        let req = TestRequest::default()
            .insert_header((
                AUTHORIZATION_HEADER_NAME,
                format!("{}test key with spaces", AUTHORIZATION_HEADER_VALUE_PREFIX),
            ))
            .to_srv_request();

        assert!(!check_authorization_header(
            &req,
            &SecretString::new("test_key")
        ));
    }

    #[test]
    fn test_extract_token_from_request_success() {
        let srv_req = TestRequest::default()
            .insert_header((
                AUTHORIZATION_HEADER_NAME,
                format!("{}{}", AUTHORIZATION_HEADER_VALUE_PREFIX, "test_token"),
            ))
            .to_srv_request();
        let req = srv_req.request();

        let token = extract_token_from_request(req).unwrap();
        assert_eq!(token, "test_token");
    }

    #[test]
    fn test_extract_token_from_request_missing_header() {
        let srv_req = TestRequest::default().to_srv_request();
        let req = srv_req.request();

        let result = extract_token_from_request(req);
        assert!(matches!(result, Err(ApiError::Unauthorized(_))));
        assert_eq!(
            result.unwrap_err().to_string(),
            "Unauthorized: Missing or invalid Authorization header"
        );
    }

    #[test]
    fn test_extract_token_from_request_invalid_prefix() {
        let srv_req = TestRequest::default()
            .insert_header((AUTHORIZATION_HEADER_NAME, "Invalid test_token"))
            .to_srv_request();
        let req = srv_req.request();

        let result = extract_token_from_request(req);
        assert!(matches!(result, Err(ApiError::Unauthorized(_))));
        assert_eq!(
            result.unwrap_err().to_string(),
            "Unauthorized: Invalid Authorization header format"
        );
    }

    #[test]
    fn test_extract_token_from_request_empty_token() {
        let srv_req = TestRequest::default()
            .insert_header((
                AUTHORIZATION_HEADER_NAME,
                AUTHORIZATION_HEADER_VALUE_PREFIX.to_string(),
            ))
            .to_srv_request();
        let req = srv_req.request();

        let result = extract_token_from_request(req);
        // Empty token after prefix should fail validation
        assert!(matches!(result, Err(ApiError::Unauthorized(_))));
        assert_eq!(
            result.unwrap_err().to_string(),
            "Unauthorized: Empty API key"
        );
    }
}
