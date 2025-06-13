use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

mod solana;
pub use solana::*;

mod stellar;
pub use stellar::*;

mod evm;
pub use evm::*;

mod error;
pub use error::*;

#[derive(Debug, Serialize, Deserialize, ToSchema, PartialEq)]
#[serde(untagged)]
pub enum NetworkRpcResult {
    Solana(SolanaRpcResult),
    Stellar(StellarRpcResult),
    Evm(EvmRpcResult),
}

#[derive(Debug, Serialize, Deserialize, ToSchema, PartialEq)]
#[serde(untagged)]
#[serde(deny_unknown_fields)]
pub enum NetworkRpcRequest {
    Solana(SolanaRpcRequest),
    Stellar(StellarRpcRequest),
    Evm(EvmRpcRequest),
}

/// Converts a raw JSON-RPC request to the internal NetworkRpcRequest format.
///
/// This function takes a generic JSON-RPC request and transforms it into a strongly-typed
/// internal representation based on the specified network type. Each network type (EVM, Solana, Stellar)
/// has different serialization requirements and this function handles the conversion accordingly.
///
/// # Arguments
///
/// * `request` - A raw JSON-RPC request as a `serde_json::Value` containing the method,
///   parameters, and other JSON-RPC fields
/// * `network_type` - The target network type that determines how the request should be parsed
///   and structured
///
/// # Returns
///
/// Returns a `Result` containing:
/// - `Ok(JsonRpcRequest<NetworkRpcRequest>)` - Successfully converted request with the appropriate
///   network-specific format
/// - `Err(ApiError)` - Conversion failed due to missing required fields, invalid format, or
///   serialization errors
///
/// # Examples
///
/// ```rust,ignore
/// use serde_json::json;
/// use crate::models::NetworkType;
///
/// let request = json!({
///     "jsonrpc": "2.0",
///     "method": "eth_getBalance",
///     "params": ["0x742d35Cc6634C0532925a3b844Bc454e4438f44e", "latest"],
///     "id": 1
/// });
///
/// let result = convert_to_internal_rpc_request(request, &NetworkType::Evm)?;
/// ```
pub fn convert_to_internal_rpc_request(
    request: serde_json::Value,
    network_type: &crate::models::NetworkType,
) -> Result<crate::domain::JsonRpcRequest<NetworkRpcRequest>, crate::models::ApiError> {
    let jsonrpc = request
        .get("jsonrpc")
        .and_then(|v| v.as_str())
        .unwrap_or("2.0")
        .to_string();

    let id = request.get("id").and_then(|v| v.as_u64()).unwrap_or(1);

    let method = request
        .get("method")
        .and_then(|v| v.as_str())
        .ok_or_else(|| crate::models::ApiError::BadRequest("Missing 'method' field".to_string()))?;

    match network_type {
        crate::models::NetworkType::Evm => {
            let params = request
                .get("params")
                .cloned()
                .unwrap_or(serde_json::Value::Null);

            Ok(crate::domain::JsonRpcRequest {
                jsonrpc,
                params: NetworkRpcRequest::Evm(crate::models::EvmRpcRequest::RawRpcRequest {
                    method: method.to_string(),
                    params,
                }),
                id,
            })
        }
        crate::models::NetworkType::Solana => {
            let solana_request: crate::models::SolanaRpcRequest =
                serde_json::from_value(request.clone()).map_err(|e| {
                    crate::models::ApiError::BadRequest(format!(
                        "Invalid Solana RPC request: {}",
                        e
                    ))
                })?;

            Ok(crate::domain::JsonRpcRequest {
                jsonrpc,
                params: NetworkRpcRequest::Solana(solana_request),
                id,
            })
        }
        crate::models::NetworkType::Stellar => {
            let stellar_request: crate::models::StellarRpcRequest =
                serde_json::from_value(request.clone()).map_err(|e| {
                    crate::models::ApiError::BadRequest(format!(
                        "Invalid Stellar RPC request: {}",
                        e
                    ))
                })?;

            Ok(crate::domain::JsonRpcRequest {
                jsonrpc,
                params: NetworkRpcRequest::Stellar(stellar_request),
                id,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{EvmRpcRequest, NetworkType, SolanaRpcRequest, StellarRpcRequest};
    use serde_json::json;

    #[test]
    fn test_convert_evm_standard_request() {
        let request = json!({
            "jsonrpc": "2.0",
            "method": "eth_getBalance",
            "params": ["0x742d35Cc6634C0532925a3b844Bc454e4438f44e", "latest"],
            "id": 1
        });

        let result = convert_to_internal_rpc_request(request, &NetworkType::Evm).unwrap();

        assert_eq!(result.jsonrpc, "2.0");
        assert_eq!(result.id, 1);

        match result.params {
            NetworkRpcRequest::Evm(EvmRpcRequest::RawRpcRequest { method, params }) => {
                assert_eq!(method, "eth_getBalance");
                assert_eq!(params[0], "0x742d35Cc6634C0532925a3b844Bc454e4438f44e");
                assert_eq!(params[1], "latest");
            }
            _ => unreachable!("Expected EVM RawRpcRequest"),
        }
    }

    #[test]
    fn test_convert_evm_missing_method_field() {
        let request = json!({
            "jsonrpc": "2.0",
            "params": ["0x742d35Cc6634C0532925a3b844Bc454e4438f44e", "latest"],
            "id": 1
        });

        let result = convert_to_internal_rpc_request(request, &NetworkType::Evm);
        assert!(result.is_err());
    }

    #[test]
    fn test_convert_evm_with_defaults() {
        let request = json!({
            "method": "eth_blockNumber"
        });

        let result = convert_to_internal_rpc_request(request, &NetworkType::Evm).unwrap();

        assert_eq!(result.jsonrpc, "2.0");
        assert_eq!(result.id, 1);

        match result.params {
            NetworkRpcRequest::Evm(EvmRpcRequest::RawRpcRequest { method, params }) => {
                assert_eq!(method, "eth_blockNumber");
                assert_eq!(params, serde_json::Value::Null);
            }
            _ => unreachable!("Expected EVM RawRpcRequest"),
        }
    }

    #[test]
    fn test_convert_evm_with_custom_jsonrpc_and_id() {
        let request = json!({
            "jsonrpc": "1.0",
            "method": "eth_chainId",
            "params": [],
            "id": 42
        });

        let result = convert_to_internal_rpc_request(request, &NetworkType::Evm).unwrap();

        assert_eq!(result.jsonrpc, "1.0");
        assert_eq!(result.id, 42);
    }

    #[test]
    fn test_convert_evm_with_non_numeric_id() {
        let request = json!({
            "jsonrpc": "2.0",
            "method": "eth_chainId",
            "params": [],
            "id": "test-id"
        });

        let result = convert_to_internal_rpc_request(request, &NetworkType::Evm).unwrap();

        // Non-numeric ID should default to 1
        assert_eq!(result.id, 1);
    }

    #[test]
    fn test_convert_evm_with_object_params() {
        let request = json!({
            "jsonrpc": "2.0",
            "method": "eth_getTransactionByHash",
            "params": {
                "hash": "0x123",
                "full": true
            },
            "id": 1
        });

        let result = convert_to_internal_rpc_request(request, &NetworkType::Evm).unwrap();

        match result.params {
            NetworkRpcRequest::Evm(EvmRpcRequest::RawRpcRequest { method, params }) => {
                assert_eq!(method, "eth_getTransactionByHash");
                assert_eq!(params["hash"], "0x123");
                assert_eq!(params["full"], true);
            }
            _ => unreachable!("Expected EVM RawRpcRequest"),
        }
    }

    #[test]
    fn test_convert_solana_fee_estimate_request() {
        let request = json!({
            "jsonrpc": "2.0",
            "method": "feeEstimate",
            "params": {
                "transaction": "base64encodedtransaction",
                "fee_token": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v" // noboost
            },
            "id": 1
        });

        let result = convert_to_internal_rpc_request(request, &NetworkType::Solana).unwrap();

        assert_eq!(result.jsonrpc, "2.0");
        assert_eq!(result.id, 1);

        match result.params {
            NetworkRpcRequest::Solana(solana_request) => {
                // Just verify we got a valid Solana request variant
                match solana_request {
                    SolanaRpcRequest::FeeEstimate(_) => {}
                    _ => unreachable!("Expected FeeEstimate variant"),
                }
            }
            _ => unreachable!("Expected Solana request"),
        }
    }

    #[test]
    fn test_convert_solana_get_supported_tokens_request() {
        let request = json!({
            "jsonrpc": "2.0",
            "method": "getSupportedTokens",
            "params": {},
            "id": 2
        });

        let result = convert_to_internal_rpc_request(request, &NetworkType::Solana).unwrap();

        assert_eq!(result.jsonrpc, "2.0");
        assert_eq!(result.id, 2);

        match result.params {
            NetworkRpcRequest::Solana(solana_request) => match solana_request {
                SolanaRpcRequest::GetSupportedTokens(_) => {}
                _ => unreachable!("Expected GetSupportedTokens variant"),
            },
            _ => unreachable!("Expected Solana request"),
        }
    }

    #[test]
    fn test_convert_solana_transfer_transaction_request() {
        let request = json!({
            "jsonrpc": "2.0",
            "method": "transferTransaction",
            "params": {
                "amount": 1000000,
                "token": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v", // noboost
                "source": "source_address",
                "destination": "destination_address"
            },
            "id": 3
        });

        let result = convert_to_internal_rpc_request(request, &NetworkType::Solana).unwrap();

        match result.params {
            NetworkRpcRequest::Solana(solana_request) => match solana_request {
                SolanaRpcRequest::TransferTransaction(_) => {}
                _ => unreachable!("Expected TransferTransaction variant"),
            },
            _ => unreachable!("Expected Solana request"),
        }
    }

    #[test]
    fn test_convert_solana_invalid_request() {
        let request = json!({
            "jsonrpc": "2.0",
            "method": "invalidMethod",
            "params": {},
            "id": 1
        });

        let result = convert_to_internal_rpc_request(request, &NetworkType::Solana);
        assert!(result.is_err());
    }

    #[test]
    fn test_convert_solana_malformed_request() {
        let request = json!({
            "jsonrpc": "2.0",
            "method": "feeEstimate",
            "params": {
                "invalid_field": "value"
            },
            "id": 1
        });

        let result = convert_to_internal_rpc_request(request, &NetworkType::Solana);
        assert!(result.is_err());
    }

    #[test]
    fn test_convert_solana_with_defaults() {
        let request = json!({
            "method": "getSupportedTokens",
            "params": {}
        });

        let result = convert_to_internal_rpc_request(request, &NetworkType::Solana).unwrap();

        assert_eq!(result.jsonrpc, "2.0");
        assert_eq!(result.id, 1);
    }

    #[test]
    fn test_convert_stellar_generic_request() {
        let request = json!({
            "jsonrpc": "2.0",
            "method": "GenericRpcRequest",
            "params": "test_params",
            "id": 1
        });

        let result = convert_to_internal_rpc_request(request, &NetworkType::Stellar).unwrap();

        assert_eq!(result.jsonrpc, "2.0");
        assert_eq!(result.id, 1);

        match result.params {
            NetworkRpcRequest::Stellar(stellar_request) => match stellar_request {
                StellarRpcRequest::GenericRpcRequest(params) => {
                    assert_eq!(params, "test_params");
                }
            },
            _ => unreachable!("Expected Stellar request"),
        }
    }

    #[test]
    fn test_convert_stellar_invalid_request() {
        let request = json!({
            "jsonrpc": "2.0",
            "method": "InvalidMethod",
            "params": {},
            "id": 1
        });

        let result = convert_to_internal_rpc_request(request, &NetworkType::Stellar);
        assert!(result.is_err());
    }

    #[test]
    fn test_convert_stellar_with_defaults() {
        let request = json!({
            "method": "GenericRpcRequest",
            "params": "default_test"
        });

        let result = convert_to_internal_rpc_request(request, &NetworkType::Stellar).unwrap();

        assert_eq!(result.jsonrpc, "2.0");
        assert_eq!(result.id, 1);
    }

    #[test]
    fn test_convert_empty_request() {
        let request = json!({});

        let result_evm = convert_to_internal_rpc_request(request.clone(), &NetworkType::Evm);
        assert!(result_evm.is_err());

        let result_solana = convert_to_internal_rpc_request(request.clone(), &NetworkType::Solana);
        assert!(result_solana.is_err());

        let result_stellar = convert_to_internal_rpc_request(request, &NetworkType::Stellar);
        assert!(result_stellar.is_err());
    }

    #[test]
    fn test_convert_null_request() {
        let request = serde_json::Value::Null;

        let result_evm = convert_to_internal_rpc_request(request.clone(), &NetworkType::Evm);
        assert!(result_evm.is_err());

        let result_solana = convert_to_internal_rpc_request(request.clone(), &NetworkType::Solana);
        assert!(result_solana.is_err());

        let result_stellar = convert_to_internal_rpc_request(request, &NetworkType::Stellar);
        assert!(result_stellar.is_err());
    }

    #[test]
    fn test_convert_array_request() {
        let request = json!([1, 2, 3]);

        let result_evm = convert_to_internal_rpc_request(request.clone(), &NetworkType::Evm);
        assert!(result_evm.is_err());

        let result_solana = convert_to_internal_rpc_request(request.clone(), &NetworkType::Solana);
        assert!(result_solana.is_err());

        let result_stellar = convert_to_internal_rpc_request(request, &NetworkType::Stellar);
        assert!(result_stellar.is_err());
    }

    #[test]
    fn test_convert_evm_non_string_method() {
        let request = json!({
            "jsonrpc": "2.0",
            "method": 123,
            "params": [],
            "id": 1
        });

        let result = convert_to_internal_rpc_request(request, &NetworkType::Evm);
        assert!(result.is_err());
    }

    #[test]
    fn test_convert_with_large_id() {
        let request = json!({
            "jsonrpc": "2.0",
            "method": "eth_chainId",
            "params": [],
            "id": 18446744073709551615u64  // u64::MAX
        });

        let result = convert_to_internal_rpc_request(request, &NetworkType::Evm).unwrap();
        assert_eq!(result.id, 18446744073709551615u64);
    }

    #[test]
    fn test_convert_with_zero_id() {
        let request = json!({
            "jsonrpc": "2.0",
            "method": "eth_chainId",
            "params": [],
            "id": 0
        });

        let result = convert_to_internal_rpc_request(request, &NetworkType::Evm).unwrap();
        assert_eq!(result.id, 0);
    }

    #[test]
    fn test_convert_evm_empty_method() {
        let request = json!({
            "jsonrpc": "2.0",
            "method": "",
            "params": [],
            "id": 1
        });

        let result = convert_to_internal_rpc_request(request, &NetworkType::Evm).unwrap();

        match result.params {
            NetworkRpcRequest::Evm(EvmRpcRequest::RawRpcRequest { method, params: _ }) => {
                assert_eq!(method, "");
            }
            _ => unreachable!("Expected EVM RawRpcRequest"),
        }
    }

    #[test]
    fn test_convert_evm_very_long_method() {
        let long_method = "a".repeat(1000);
        let request = json!({
            "jsonrpc": "2.0",
            "method": long_method,
            "params": [],
            "id": 1
        });

        let result = convert_to_internal_rpc_request(request, &NetworkType::Evm).unwrap();

        match result.params {
            NetworkRpcRequest::Evm(EvmRpcRequest::RawRpcRequest { method, params: _ }) => {
                assert_eq!(method, long_method);
            }
            _ => unreachable!("Expected EVM RawRpcRequest"),
        }
    }
}
