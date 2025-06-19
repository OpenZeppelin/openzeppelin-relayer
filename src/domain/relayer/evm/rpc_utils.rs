//! Utilities for EVM JSON-RPC response handling and error mapping.
//!
//! This module provides helper functions for creating standardized JSON-RPC 2.0 responses
//! and mapping internal provider errors to appropriate error codes and messages.
//!
//! # Main Features
//!
//! - Create success and error responses following JSON-RPC 2.0 specification
//! - Map provider errors to standardized JSON-RPC error codes
//! - Handle EVM-specific result types and error formatting

use crate::{
    models::{EvmRpcResult, NetworkRpcResult, OpenZeppelinErrorCodes, RpcErrorCodes},
    models::{JsonRpcError, JsonRpcId, JsonRpcResponse},
    services::ProviderError,
};
use serde_json;

/// Creates an error response following the JSON-RPC 2.0 specification.
///
/// # Arguments
///
/// * `id` - The request ID from the original JSON-RPC request
/// * `code` - The error code (should follow JSON-RPC error code conventions)
/// * `message` - A short, human-readable error message
/// * `description` - A more detailed description of the error
///
/// # Returns
///
/// Returns a `JsonRpcResponse<NetworkRpcResult>` containing the error details
/// and no result data.
pub fn create_error_response(
    id: Option<JsonRpcId>,
    code: i32,
    message: &str,
    description: &str,
) -> JsonRpcResponse<NetworkRpcResult> {
    JsonRpcResponse {
        id,
        jsonrpc: "2.0".to_string(),
        result: None,
        error: Some(JsonRpcError {
            code,
            message: message.to_string(),
            description: description.to_string(),
        }),
    }
}

/// Creates a success response following the JSON-RPC 2.0 specification.
///
/// # Arguments
///
/// * `id` - The request ID from the original JSON-RPC request
/// * `result` - The result data to include in the response as a JSON value
///
/// # Returns
///
/// Returns a `JsonRpcResponse<NetworkRpcResult>` containing the result data
/// wrapped in an EVM-specific result type, with no error information.
pub fn create_success_response(
    id: Option<JsonRpcId>,
    result: serde_json::Value,
) -> JsonRpcResponse<NetworkRpcResult> {
    JsonRpcResponse {
        id,
        jsonrpc: "2.0".to_string(),
        result: Some(NetworkRpcResult::Evm(EvmRpcResult::RawRpcResult(result))),
        error: None,
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{OpenZeppelinErrorCodes, RpcErrorCodes};
    use crate::services::{provider::rpc_selector::RpcSelectorError, SolanaProviderError};
    use serde_json::json;

    #[test]
    fn test_create_error_response_basic() {
        let response = create_error_response(
            Some(JsonRpcId::Number(123)),
            -32602,
            "Invalid params",
            "The provided parameters are invalid",
        );

        assert_eq!(response.id, Some(JsonRpcId::Number(123)));
        assert_eq!(response.jsonrpc, "2.0");
        assert!(response.result.is_none());
        assert!(response.error.is_some());

        let error = response.error.unwrap();
        assert_eq!(error.code, -32602);
        assert!(!error.message.is_empty());
        assert!(!error.description.is_empty());
    }

    #[test]
    fn test_create_error_response_zero_id() {
        let response = create_error_response(
            Some(JsonRpcId::Number(0)),
            -32603,
            "Internal error",
            "Something went wrong",
        );

        assert_eq!(response.id, Some(JsonRpcId::Number(0)));
        assert_eq!(response.jsonrpc, "2.0");
        assert!(response.result.is_none());
        assert!(response.error.is_some());
    }

    #[test]
    fn test_create_error_response_max_id() {
        let response = create_error_response(
            Some(JsonRpcId::Number(u64::MAX as i64)),
            -32700,
            "Parse error",
            "JSON parsing failed",
        );

        assert_eq!(response.id, Some(JsonRpcId::Number(u64::MAX as i64)));
        assert_eq!(response.jsonrpc, "2.0");
        assert!(response.result.is_none());
        assert!(response.error.is_some());
    }

    #[test]
    fn test_create_error_response_empty_message() {
        let response = create_error_response(
            Some(JsonRpcId::Number(42)),
            -32601,
            "",
            "Method not found error",
        );

        assert_eq!(response.id, Some(JsonRpcId::Number(42)));
        let error = response.error.unwrap();
        assert!(error.message.is_empty());
        assert!(!error.description.is_empty());
    }

    #[test]
    fn test_create_error_response_empty_description() {
        let response =
            create_error_response(Some(JsonRpcId::Number(99)), -32600, "Invalid Request", "");

        assert_eq!(response.id, Some(JsonRpcId::Number(99)));
        let error = response.error.unwrap();
        assert!(!error.message.is_empty());
        assert!(error.description.is_empty());
    }

    #[test]
    fn test_create_error_response_preserves_input() {
        let message = "Error with unicode: 🚨 ñáéíóú";
        let description = "Description with symbols: @#$%^&*()";
        let response =
            create_error_response(Some(JsonRpcId::Number(500)), -33000, message, description);

        let error = response.error.unwrap();
        assert!(!error.message.is_empty());
        assert!(!error.description.is_empty());
        assert_eq!(error.code, -33000);
    }

    #[test]
    fn test_create_error_response_long_strings() {
        let long_message = "a".repeat(1000);
        let long_description = "b".repeat(2000);
        let response = create_error_response(
            Some(JsonRpcId::Number(777)),
            -33001,
            &long_message,
            &long_description,
        );

        let error = response.error.unwrap();
        assert_eq!(error.message.len(), 1000);
        assert_eq!(error.description.len(), 2000);
    }

    #[test]
    fn test_create_error_response_custom_openzeppelin_codes() {
        let test_cases = vec![
            OpenZeppelinErrorCodes::TIMEOUT,
            OpenZeppelinErrorCodes::RATE_LIMITED,
            OpenZeppelinErrorCodes::BAD_GATEWAY,
            OpenZeppelinErrorCodes::REQUEST_ERROR,
            OpenZeppelinErrorCodes::NETWORK_CONFIGURATION,
        ];

        for code in test_cases {
            let response = create_error_response(
                Some(JsonRpcId::Number(1)),
                code,
                "Test message",
                "Test description",
            );
            let error = response.error.unwrap();
            assert_eq!(error.code, code);
            assert!(!error.message.is_empty());
            assert!(!error.description.is_empty());
        }
    }

    #[test]
    fn test_create_error_response_standard_json_rpc_codes() {
        let test_cases = vec![
            RpcErrorCodes::PARSE,
            RpcErrorCodes::INVALID_REQUEST,
            RpcErrorCodes::METHOD_NOT_FOUND,
            RpcErrorCodes::INVALID_PARAMS,
            RpcErrorCodes::INTERNAL_ERROR,
        ];

        for code in test_cases {
            let response = create_error_response(
                Some(JsonRpcId::Number(1)),
                code,
                "Test message",
                "Test description",
            );
            let error = response.error.unwrap();
            assert_eq!(error.code, code);
            assert!(!error.message.is_empty());
            assert!(!error.description.is_empty());
        }
    }

    #[test]
    fn test_create_success_response_basic() {
        let result_data = json!({
            "blockNumber": "0x1234",
            "hash": "0xabcd"
        });

        let response = create_success_response(Some(JsonRpcId::Number(456)), result_data.clone());

        assert_eq!(response.id, Some(JsonRpcId::Number(456)));
        assert_eq!(response.jsonrpc, "2.0");
        assert!(response.error.is_none());
        assert!(response.result.is_some());

        match response.result.unwrap() {
            NetworkRpcResult::Evm(EvmRpcResult::RawRpcResult(value)) => {
                assert_eq!(value, result_data);
            }
            _ => unreachable!("Expected EVM RawRpcResult"),
        }
    }

    #[test]
    fn test_create_success_response_zero_id() {
        let result_data = json!(null);
        let response = create_success_response(Some(JsonRpcId::Number(0)), result_data);

        assert_eq!(response.id, Some(JsonRpcId::Number(0)));
        assert_eq!(response.jsonrpc, "2.0");
        assert!(response.error.is_none());
    }

    #[test]
    fn test_create_success_response_max_id() {
        let result_data = json!(42);
        let response =
            create_success_response(Some(JsonRpcId::Number(u64::MAX as i64)), result_data);

        assert_eq!(response.id, Some(JsonRpcId::Number(u64::MAX as i64)));
        assert_eq!(response.jsonrpc, "2.0");
        assert!(response.error.is_none());
    }

    #[test]
    fn test_create_success_response_null_result() {
        let result_data = json!(null);
        let response = create_success_response(Some(JsonRpcId::Number(100)), result_data.clone());

        match response.result.unwrap() {
            NetworkRpcResult::Evm(EvmRpcResult::RawRpcResult(value)) => {
                assert_eq!(value, result_data);
                assert!(value.is_null());
            }
            _ => unreachable!("Expected EVM RawRpcResult"),
        }
    }

    #[test]
    fn test_create_success_response_boolean_result() {
        let result_data = json!(true);
        let response = create_success_response(Some(JsonRpcId::Number(200)), result_data.clone());

        match response.result.unwrap() {
            NetworkRpcResult::Evm(EvmRpcResult::RawRpcResult(value)) => {
                assert_eq!(value, result_data);
                assert!(value.is_boolean());
            }
            _ => unreachable!("Expected EVM RawRpcResult"),
        }
    }

    #[test]
    fn test_create_success_response_number_result() {
        let result_data = json!(12345);
        let response = create_success_response(Some(JsonRpcId::Number(300)), result_data.clone());

        match response.result.unwrap() {
            NetworkRpcResult::Evm(EvmRpcResult::RawRpcResult(value)) => {
                assert_eq!(value, result_data);
                assert!(value.is_number());
            }
            _ => unreachable!("Expected EVM RawRpcResult"),
        }
    }

    #[test]
    fn test_create_success_response_string_result() {
        let result_data = json!("test string");
        let response = create_success_response(Some(JsonRpcId::Number(400)), result_data.clone());

        match response.result.unwrap() {
            NetworkRpcResult::Evm(EvmRpcResult::RawRpcResult(value)) => {
                assert_eq!(value, result_data);
                assert!(value.is_string());
            }
            _ => unreachable!("Expected EVM RawRpcResult"),
        }
    }

    #[test]
    fn test_create_success_response_array_result() {
        let result_data = json!([1, 2, 3, "test", true, null]);
        let response = create_success_response(Some(JsonRpcId::Number(500)), result_data.clone());

        match response.result.unwrap() {
            NetworkRpcResult::Evm(EvmRpcResult::RawRpcResult(value)) => {
                assert_eq!(value, result_data);
                assert!(value.is_array());
                assert_eq!(value.as_array().unwrap().len(), 6);
            }
            _ => unreachable!("Expected EVM RawRpcResult"),
        }
    }

    #[test]
    fn test_create_success_response_complex_object() {
        let result_data = json!({
            "transactions": [
                {"hash": "0x123", "value": "1000000000000000000"},
                {"hash": "0x456", "value": "2000000000000000000"}
            ],
            "blockNumber": "0x1a2b3c",
            "timestamp": 1234567890,
            "gasUsed": "0x5208",
            "metadata": {
                "network": "mainnet",
                "version": "1.0",
                "features": ["eip1559", "eip2930"]
            },
            "isEmpty": false,
            "nullField": null
        });

        let response = create_success_response(Some(JsonRpcId::Number(600)), result_data.clone());

        match response.result.unwrap() {
            NetworkRpcResult::Evm(EvmRpcResult::RawRpcResult(value)) => {
                assert_eq!(value, result_data);
                assert!(value.is_object());
                assert_eq!(value["blockNumber"], "0x1a2b3c");
                assert_eq!(value["transactions"].as_array().unwrap().len(), 2);
                assert!(value["nullField"].is_null());
            }
            _ => unreachable!("Expected EVM RawRpcResult"),
        }
    }

    #[test]
    fn test_create_success_response_large_object() {
        let mut large_object = json!({});
        let object = large_object.as_object_mut().unwrap();

        // Create a large object with many fields
        for i in 0..100 {
            object.insert(format!("field_{}", i), json!(format!("value_{}", i)));
        }

        let response = create_success_response(Some(JsonRpcId::Number(700)), large_object.clone());

        match response.result.unwrap() {
            NetworkRpcResult::Evm(EvmRpcResult::RawRpcResult(value)) => {
                assert_eq!(value, large_object);
                assert_eq!(value.as_object().unwrap().len(), 100);
            }
            _ => unreachable!("Expected EVM RawRpcResult"),
        }
    }

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
    fn test_integration_error_response_with_mapped_provider_error() {
        let provider_error = ProviderError::InvalidAddress("0xinvalid".to_string());
        let (error_code, error_message) = map_provider_error(&provider_error);

        let response = create_error_response(
            Some(JsonRpcId::Number(999)),
            error_code,
            error_message,
            "Invalid address provided",
        );

        assert_eq!(response.id, Some(JsonRpcId::Number(999)));
        assert_eq!(response.jsonrpc, "2.0");
        assert!(response.result.is_none());

        let error = response.error.unwrap();
        assert_eq!(error.code, RpcErrorCodes::INVALID_PARAMS);
        assert!(!error.message.is_empty());
        assert!(!error.description.is_empty());
    }

    #[test]
    fn test_integration_all_provider_errors_to_responses() {
        let test_cases = vec![
            (
                ProviderError::InvalidAddress("test".to_string()),
                RpcErrorCodes::INVALID_PARAMS,
            ),
            (
                ProviderError::NetworkConfiguration("test".to_string()),
                OpenZeppelinErrorCodes::NETWORK_CONFIGURATION,
            ),
            (ProviderError::Timeout, OpenZeppelinErrorCodes::TIMEOUT),
            (
                ProviderError::RateLimited,
                OpenZeppelinErrorCodes::RATE_LIMITED,
            ),
            (
                ProviderError::BadGateway,
                OpenZeppelinErrorCodes::BAD_GATEWAY,
            ),
            (
                ProviderError::RequestError {
                    error: "test".to_string(),
                    status_code: 400,
                },
                OpenZeppelinErrorCodes::REQUEST_ERROR,
            ),
            (
                ProviderError::Other("test".to_string()),
                RpcErrorCodes::INTERNAL_ERROR,
            ),
        ];

        for (provider_error, expected_code) in test_cases {
            let (error_code, error_message) = map_provider_error(&provider_error);
            let response = create_error_response(
                Some(JsonRpcId::Number(1)),
                error_code,
                error_message,
                "Test integration",
            );

            assert_eq!(response.id, Some(JsonRpcId::Number(1)));
            assert_eq!(response.jsonrpc, "2.0");
            assert!(response.result.is_none());

            let error = response.error.unwrap();
            assert_eq!(error.code, expected_code);
            assert!(!error.message.is_empty());
        }
    }

    #[test]
    fn test_response_structure_consistency() {
        // Test that both success and error responses have consistent structure
        let success_response =
            create_success_response(Some(JsonRpcId::Number(100)), json!({"status": "ok"}));
        let error_response = create_error_response(
            Some(JsonRpcId::Number(100)),
            -32603,
            "Internal error",
            "Test error",
        );

        // Both should have same basic structure
        assert_eq!(success_response.id, error_response.id);
        assert_eq!(success_response.jsonrpc, error_response.jsonrpc);

        // Success response should have result, not error
        assert!(success_response.result.is_some());
        assert!(success_response.error.is_none());

        // Error response should have error, not result
        assert!(error_response.result.is_none());
        assert!(error_response.error.is_some());
    }
}
