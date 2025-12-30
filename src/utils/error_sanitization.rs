//! Error sanitization and mapping utilities for provider errors.
//!
//! This module provides network-agnostic utilities for sanitizing and mapping
//! provider errors to JSON-RPC error codes and user-friendly messages.
//!
//! These utilities are used by all network relayers (EVM, Stellar, Solana) to
//! ensure consistent error handling and prevent exposing sensitive information.

use crate::{
    models::{OpenZeppelinErrorCodes, RpcErrorCodes},
    services::provider::ProviderError,
};

/// Maps provider errors to appropriate JSON-RPC error codes and messages.
///
/// This function translates internal provider errors into standardized
/// JSON-RPC error codes and user-friendly messages that can be returned
/// to clients. It follows JSON-RPC 2.0 specification for standard errors
/// and uses OpenZeppelin-specific codes for extended functionality.
///
/// # Arguments
///
/// * `error` - A reference to the provider error to be mapped
///
/// # Returns
///
/// Returns a tuple containing:
/// - `i32` - The error code (following JSON-RPC 2.0 and OpenZeppelin conventions)
/// - `&'static str` - A static string describing the error type
///
/// # Error Code Mappings
///
/// - `InvalidAddress` → -32602 ("Invalid params")
/// - `NetworkConfiguration` → -33004 ("Network configuration error")
/// - `Timeout` → -33000 ("Request timeout")
/// - `RateLimited` → -33001 ("Rate limited")
/// - `BadGateway` → -33002 ("Bad gateway")
/// - `RequestError` → -33003 ("Request error")
/// - `Other` and unknown errors → -32603 ("Internal error")
pub fn map_provider_error(error: &ProviderError) -> (i32, &'static str) {
    match error {
        ProviderError::InvalidAddress(_) => (RpcErrorCodes::INVALID_PARAMS, "Invalid params"),
        ProviderError::NetworkConfiguration(_) => (
            OpenZeppelinErrorCodes::NETWORK_CONFIGURATION,
            "Network configuration error",
        ),
        ProviderError::Timeout => (OpenZeppelinErrorCodes::TIMEOUT, "Request timeout"),
        ProviderError::RateLimited => (OpenZeppelinErrorCodes::RATE_LIMITED, "Rate limited"),
        ProviderError::BadGateway => (OpenZeppelinErrorCodes::BAD_GATEWAY, "Bad gateway"),
        ProviderError::RequestError { .. } => {
            (OpenZeppelinErrorCodes::REQUEST_ERROR, "Request error")
        }
        ProviderError::Other(_) => (RpcErrorCodes::INTERNAL_ERROR, "Internal error"),
        _ => (RpcErrorCodes::INTERNAL_ERROR, "Internal error"),
    }
}

/// Sanitizes provider error descriptions to prevent exposing internal details.
///
/// This function creates a safe, user-friendly error description that doesn't
/// expose sensitive information like API keys, internal URLs, or implementation
/// details. The full error is logged internally for debugging purposes.
///
/// # Arguments
///
/// * `error` - A reference to the provider error to sanitize
///
/// # Returns
///
/// Returns a sanitized error description string that is safe to return to clients.
pub fn sanitize_error_description(error: &ProviderError) -> String {
    match error {
        ProviderError::InvalidAddress(_) => "The provided address is invalid".to_string(),
        ProviderError::NetworkConfiguration(_) => {
            "Network configuration error. Please check your network settings".to_string()
        }
        ProviderError::Timeout => "The request timed out. Please try again later".to_string(),
        ProviderError::RateLimited => "Rate limit exceeded. Please try again later".to_string(),
        ProviderError::BadGateway => {
            "Service temporarily unavailable. Please try again later".to_string()
        }
        ProviderError::RequestError { status_code, .. } => {
            format!("Request failed with status code {}", status_code)
        }
        ProviderError::RpcErrorCode { code, .. } => {
            format!("RPC error occurred (code: {})", code)
        }
        ProviderError::TransportError(_) => {
            "Network error occurred. Please try again later".to_string()
        }
        ProviderError::SolanaRpcError(_) => {
            "RPC request failed. Please try again later".to_string()
        }
        ProviderError::Other(_) => "An internal error occurred. Please try again later".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::provider::{rpc_selector::RpcSelectorError, SolanaProviderError};

    #[test]
    fn test_map_provider_error_invalid_address() {
        let error = ProviderError::InvalidAddress("invalid address".to_string());
        let (code, _message) = map_provider_error(&error);

        assert_eq!(code, RpcErrorCodes::INVALID_PARAMS);
    }

    #[test]
    fn test_map_provider_error_invalid_address_empty() {
        let error = ProviderError::InvalidAddress("".to_string());
        let (code, _message) = map_provider_error(&error);

        assert_eq!(code, RpcErrorCodes::INVALID_PARAMS);
    }

    #[test]
    fn test_map_provider_error_network_configuration() {
        let error = ProviderError::NetworkConfiguration("network config error".to_string());
        let (code, _message) = map_provider_error(&error);

        assert_eq!(code, OpenZeppelinErrorCodes::NETWORK_CONFIGURATION);
    }

    #[test]
    fn test_map_provider_error_network_configuration_empty() {
        let error = ProviderError::NetworkConfiguration("".to_string());
        let (code, _message) = map_provider_error(&error);

        assert_eq!(code, OpenZeppelinErrorCodes::NETWORK_CONFIGURATION);
    }

    #[test]
    fn test_map_provider_error_timeout() {
        let error = ProviderError::Timeout;
        let (code, _message) = map_provider_error(&error);

        assert_eq!(code, OpenZeppelinErrorCodes::TIMEOUT);
    }

    #[test]
    fn test_map_provider_error_rate_limited() {
        let error = ProviderError::RateLimited;
        let (code, _message) = map_provider_error(&error);

        assert_eq!(code, OpenZeppelinErrorCodes::RATE_LIMITED);
    }

    #[test]
    fn test_map_provider_error_bad_gateway() {
        let error = ProviderError::BadGateway;
        let (code, _message) = map_provider_error(&error);

        assert_eq!(code, OpenZeppelinErrorCodes::BAD_GATEWAY);
    }

    #[test]
    fn test_map_provider_error_request_error_400() {
        let error = ProviderError::RequestError {
            error: "Bad request".to_string(),
            status_code: 400,
        };
        let (code, _message) = map_provider_error(&error);

        assert_eq!(code, OpenZeppelinErrorCodes::REQUEST_ERROR);
    }

    #[test]
    fn test_map_provider_error_request_error_500() {
        let error = ProviderError::RequestError {
            error: "Internal server error".to_string(),
            status_code: 500,
        };
        let (code, _message) = map_provider_error(&error);

        assert_eq!(code, OpenZeppelinErrorCodes::REQUEST_ERROR);
    }

    #[test]
    fn test_map_provider_error_request_error_empty_message() {
        let error = ProviderError::RequestError {
            error: "".to_string(),
            status_code: 404,
        };
        let (code, _message) = map_provider_error(&error);

        assert_eq!(code, OpenZeppelinErrorCodes::REQUEST_ERROR);
    }

    #[test]
    fn test_map_provider_error_request_error_zero_status() {
        let error = ProviderError::RequestError {
            error: "No status".to_string(),
            status_code: 0,
        };
        let (code, _message) = map_provider_error(&error);

        assert_eq!(code, OpenZeppelinErrorCodes::REQUEST_ERROR);
    }

    #[test]
    fn test_map_provider_error_other() {
        let error = ProviderError::Other("some other error".to_string());
        let (code, _message) = map_provider_error(&error);

        assert_eq!(code, RpcErrorCodes::INTERNAL_ERROR);
    }

    #[test]
    fn test_map_provider_error_other_empty() {
        let error = ProviderError::Other("".to_string());
        let (code, _message) = map_provider_error(&error);

        assert_eq!(code, RpcErrorCodes::INTERNAL_ERROR);
    }

    #[test]
    fn test_map_provider_error_solana_rpc_error() {
        let solana_error = SolanaProviderError::RpcError("Solana RPC failed".to_string());
        let error = ProviderError::SolanaRpcError(solana_error);
        let (code, _message) = map_provider_error(&error);

        // The SolanaRpcError variant should be caught by the wildcard pattern
        assert_eq!(code, RpcErrorCodes::INTERNAL_ERROR);
    }

    #[test]
    fn test_map_provider_error_solana_invalid_address() {
        let solana_error =
            SolanaProviderError::InvalidAddress("Invalid Solana address".to_string());
        let error = ProviderError::SolanaRpcError(solana_error);
        let (code, _message) = map_provider_error(&error);

        // The SolanaRpcError variant should be caught by the wildcard pattern
        assert_eq!(code, RpcErrorCodes::INTERNAL_ERROR);
    }

    #[test]
    fn test_map_provider_error_solana_selector_error() {
        let selector_error = RpcSelectorError::NoProviders;
        let solana_error = SolanaProviderError::SelectorError(selector_error);
        let error = ProviderError::SolanaRpcError(solana_error);
        let (code, _message) = map_provider_error(&error);

        // The SolanaRpcError variant should be caught by the wildcard pattern
        assert_eq!(code, RpcErrorCodes::INTERNAL_ERROR);
    }

    #[test]
    fn test_map_provider_error_solana_network_configuration() {
        let solana_error =
            SolanaProviderError::NetworkConfiguration("Solana network config error".to_string());
        let error = ProviderError::SolanaRpcError(solana_error);
        let (code, _message) = map_provider_error(&error);

        // The SolanaRpcError variant should be caught by the wildcard pattern
        assert_eq!(code, RpcErrorCodes::INTERNAL_ERROR);
    }

    #[test]
    fn test_map_provider_error_wildcard_pattern() {
        // This test ensures the wildcard pattern works by testing all variations
        // that should fall through to the default case
        let test_cases = vec![
            ProviderError::SolanaRpcError(SolanaProviderError::RpcError("test".to_string())),
            ProviderError::SolanaRpcError(SolanaProviderError::InvalidAddress("test".to_string())),
            ProviderError::SolanaRpcError(SolanaProviderError::NetworkConfiguration(
                "test".to_string(),
            )),
            ProviderError::SolanaRpcError(SolanaProviderError::SelectorError(
                RpcSelectorError::NoProviders,
            )),
        ];

        for error in test_cases {
            let (code, _message) = map_provider_error(&error);
            assert_eq!(code, RpcErrorCodes::INTERNAL_ERROR);
        }
    }

    #[test]
    fn test_sanitize_error_description_invalid_address() {
        let error = ProviderError::InvalidAddress("0xinvalid".to_string());
        let description = sanitize_error_description(&error);
        assert_eq!(description, "The provided address is invalid");
        // Ensure no internal details are exposed
        assert!(!description.contains("0xinvalid"));
    }

    #[test]
    fn test_sanitize_error_description_network_configuration() {
        let error =
            ProviderError::NetworkConfiguration("RPC selector error: No providers".to_string());
        let description = sanitize_error_description(&error);
        assert_eq!(
            description,
            "Network configuration error. Please check your network settings"
        );
        // Ensure no internal details are exposed
        assert!(!description.contains("RPC selector"));
        assert!(!description.contains("No providers"));
    }

    #[test]
    fn test_sanitize_error_description_timeout() {
        let error = ProviderError::Timeout;
        let description = sanitize_error_description(&error);
        assert_eq!(description, "The request timed out. Please try again later");
    }

    #[test]
    fn test_sanitize_error_description_rate_limited() {
        let error = ProviderError::RateLimited;
        let description = sanitize_error_description(&error);
        assert_eq!(description, "Rate limit exceeded. Please try again later");
    }

    #[test]
    fn test_sanitize_error_description_bad_gateway() {
        let error = ProviderError::BadGateway;
        let description = sanitize_error_description(&error);
        assert_eq!(
            description,
            "Service temporarily unavailable. Please try again later"
        );
    }

    #[test]
    fn test_sanitize_error_description_request_error() {
        let error = ProviderError::RequestError {
            error: "API key invalid: abc123".to_string(),
            status_code: 401,
        };
        let description = sanitize_error_description(&error);
        assert_eq!(description, "Request failed with status code 401");
        // Ensure no API key details are exposed
        assert!(!description.contains("API key"));
        assert!(!description.contains("abc123"));
    }

    #[test]
    fn test_sanitize_error_description_rpc_error_code() {
        let error = ProviderError::RpcErrorCode {
            code: -32000,
            message: "Server error: Invalid API key".to_string(),
        };
        let description = sanitize_error_description(&error);
        assert_eq!(description, "RPC error occurred (code: -32000)");
        // Ensure no internal message details are exposed
        assert!(!description.contains("Server error"));
        assert!(!description.contains("API key"));
    }

    #[test]
    fn test_sanitize_error_description_transport_error() {
        let error = ProviderError::TransportError(
            "Connection failed: https://rpc.example.com/api?key=secret".to_string(),
        );
        let description = sanitize_error_description(&error);
        assert_eq!(
            description,
            "Network error occurred. Please try again later"
        );
        // Ensure no URLs or keys are exposed
        assert!(!description.contains("https://"));
        assert!(!description.contains("key="));
        assert!(!description.contains("secret"));
    }

    #[test]
    fn test_sanitize_error_description_solana_rpc_error() {
        let solana_error =
            SolanaProviderError::RpcError("RPC failed: Invalid API key abc123".to_string());
        let error = ProviderError::SolanaRpcError(solana_error);
        let description = sanitize_error_description(&error);
        assert_eq!(description, "RPC request failed. Please try again later");
        // Ensure no internal details are exposed
        assert!(!description.contains("API key"));
        assert!(!description.contains("abc123"));
    }

    #[test]
    fn test_sanitize_error_description_other() {
        let error = ProviderError::Other(
            "Internal error: Failed to connect to https://rpc.example.com with key abc123"
                .to_string(),
        );
        let description = sanitize_error_description(&error);
        assert_eq!(
            description,
            "An internal error occurred. Please try again later"
        );
        // Ensure no internal details are exposed
        assert!(!description.contains("Failed to connect"));
        assert!(!description.contains("https://"));
        assert!(!description.contains("key"));
        assert!(!description.contains("abc123"));
    }
}
