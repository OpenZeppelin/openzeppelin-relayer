//! Handles incoming Solana RPC requests.
//!
//! This module defines the `SolanaRpcHandler` struct that dispatches RPC requests
//! to the appropriate methods. It uses the trait defined in the `methods`
//! module to process specific operations such as fee estimation, transaction
//! preparation, signing, sending, and token retrieval.
//!
//! The handler converts JSON-RPC requests into concrete call parameters and then
//! invokes the respective methods of the underlying implementation.
use super::{SolanaRpcError, SolanaRpcMethods};
use crate::{
    domain::{JsonRpcRequest, JsonRpcResponse},
    models::{
        FeeEstimateRequestParams, GetFeaturesEnabledRequestParams, GetSupportedTokensRequestParams,
        PrepareTransactionRequestParams, SignAndSendTransactionRequestParams,
        SignTransactionRequestParams, SolanaRpcMethod, SolanaRpcResult,
        TransferTransactionRequestParams,
    },
};
use eyre::Result;
use log::{error, info};

pub struct SolanaRpcHandler<T> {
    rpc_methods: T,
}

impl<T: SolanaRpcMethods> SolanaRpcHandler<T> {
    pub fn new(rpc_methods: T) -> Self {
        Self { rpc_methods }
    }

    fn handle_error<E>(result: Result<E, serde_json::Error>) -> Result<E, SolanaRpcError> {
        result.map_err(|e| SolanaRpcError::BadRequest(e.to_string()))
    }

    pub async fn handle_request(
        &self,
        request: JsonRpcRequest,
    ) -> Result<JsonRpcResponse, SolanaRpcError> {
        info!("Received request with method: {}", request.method);
        let method = SolanaRpcMethod::from_str(request.method.as_str()).ok_or_else(|| {
            error!("Unsupported method: {}", request.method);
            SolanaRpcError::UnsupportedMethod(request.method.clone())
        })?;

        let result = match method {
            SolanaRpcMethod::FeeEstimate => {
                let params = Self::handle_error(
                    serde_json::from_value::<FeeEstimateRequestParams>(request.params),
                )?;
                let res = self.rpc_methods.fee_estimate(params).await?;
                SolanaRpcResult::FeeEstimate(res)
            }
            SolanaRpcMethod::TransferTransaction => {
                let params = Self::handle_error(serde_json::from_value::<
                    TransferTransactionRequestParams,
                >(request.params))?;
                let res = self.rpc_methods.transfer_transaction(params).await?;
                SolanaRpcResult::TransferTransaction(res)
            }
            SolanaRpcMethod::PrepareTransaction => {
                let params = Self::handle_error(serde_json::from_value::<
                    PrepareTransactionRequestParams,
                >(request.params))?;
                let res = self.rpc_methods.prepare_transaction(params).await?;
                SolanaRpcResult::PrepareTransaction(res)
            }
            SolanaRpcMethod::SignTransaction => {
                let params = Self::handle_error(serde_json::from_value::<
                    SignTransactionRequestParams,
                >(request.params))?;
                let res = self.rpc_methods.sign_transaction(params).await?;
                SolanaRpcResult::SignTransaction(res)
            }
            SolanaRpcMethod::SignAndSendTransaction => {
                let params = Self::handle_error(serde_json::from_value::<
                    SignAndSendTransactionRequestParams,
                >(request.params))?;
                let res = self.rpc_methods.sign_and_send_transaction(params).await?;
                SolanaRpcResult::SignAndSendTransaction(res)
            }
            SolanaRpcMethod::GetSupportedTokens => {
                let params = Self::handle_error(serde_json::from_value::<
                    GetSupportedTokensRequestParams,
                >(request.params))?;
                let res = self.rpc_methods.get_supported_tokens(params).await?;
                SolanaRpcResult::GetSupportedTokens(res)
            }
            SolanaRpcMethod::GetFeaturesEnabled => {
                let params = Self::handle_error(serde_json::from_value::<
                    GetFeaturesEnabledRequestParams,
                >(request.params))?;
                let res = self.rpc_methods.get_features_enabled(params).await?;
                SolanaRpcResult::GetFeaturesEnabled(res)
            }
        };

        Ok(JsonRpcResponse::result(request.id, result))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::{
        domain::MockSolanaRpcMethods,
        models::{
            EncodedSerializedTransaction, FeeEstimateResult, GetFeaturesEnabledResult,
            PrepareTransactionResult, SignAndSendTransactionResult, SignTransactionResult,
            TransferTransactionResult,
        },
    };

    use super::*;
    use mockall::predicate::{self};
    use serde_json::json;

    #[tokio::test]
    async fn test_handle_request_fee_estimate() {
        let mut mock_rpc_methods = MockSolanaRpcMethods::new();
        mock_rpc_methods
            .expect_fee_estimate()
            .with(predicate::eq(FeeEstimateRequestParams {
                transaction: EncodedSerializedTransaction::new("test_transaction".to_string()),
                fee_token: "test_token".to_string(),
            }))
            .returning(|_| {
                Ok(FeeEstimateResult {
                    estimated_fee: "0".to_string(),
                    conversion_rate: "0".to_string(),
                })
            })
            .times(1);
        let mock_handler = Arc::new(SolanaRpcHandler::new(mock_rpc_methods));
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: 1,
            method: "feeEstimate".to_string(),
            params: json!({
                "transaction": "test_transaction",
                "fee_token": "test_token"
            }),
        };

        let response = mock_handler.handle_request(request).await;

        assert!(response.is_ok(), "Expected Ok response, got {:?}", response);
        let json_response = response.unwrap();
        assert_eq!(
            json_response.result,
            Some(json!({
                "estimated_fee": "0",
                "conversion_rate": "0"
            }))
        );
    }

    #[tokio::test]
    async fn test_handle_request_features_enabled() {
        let mut mock_rpc_methods = MockSolanaRpcMethods::new();
        mock_rpc_methods
            .expect_get_features_enabled()
            .with(predicate::eq(GetFeaturesEnabledRequestParams {}))
            .returning(|_| {
                Ok(GetFeaturesEnabledResult {
                    features: vec!["gasless".to_string()],
                })
            })
            .times(1);
        let mock_handler = Arc::new(SolanaRpcHandler::new(mock_rpc_methods));
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: 1,
            method: "getFeaturesEnabled".to_string(),
            params: json!({}),
        };

        let response = mock_handler.handle_request(request).await;

        assert!(response.is_ok(), "Expected Ok response, got {:?}", response);
        let json_response = response.unwrap();
        assert_eq!(
            json_response.result,
            Some(json!({
                "features": ["gasless"]
            }))
        );
    }

    #[tokio::test]
    async fn test_unsupported_method() {
        let mock_rpc_methods = MockSolanaRpcMethods::new();
        let mock_handler = Arc::new(SolanaRpcHandler::new(mock_rpc_methods));
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: 1,
            method: "unsupported".to_string(),
            params: json!({
                "transaction": "test_transaction",
                "fee_token": "test_token"
            }),
        };

        let response = mock_handler.handle_request(request).await;

        match response {
            Err(SolanaRpcError::UnsupportedMethod(msg)) => {
                // Optionally verify error message
                assert!(
                    msg.contains("unsupported"),
                    "Unexpected error message: {}",
                    msg
                );
            }
            Err(e) => panic!("Expected BadRequest error, but got: {:?}", e),
            Ok(resp) => panic!("Expected error response, got Ok: {:?}", resp),
        }
    }

    #[tokio::test]
    async fn test_unsupported_params() {
        let mock_rpc_methods = MockSolanaRpcMethods::new();
        let mock_handler = Arc::new(SolanaRpcHandler::new(mock_rpc_methods));
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: 1,
            method: "feeEstimate".to_string(),
            params: json!({
                "test": "test_transaction",
            }),
        };

        let response = mock_handler.handle_request(request).await;

        match response {
            Err(SolanaRpcError::BadRequest(msg)) => {
                // Optionally verify error message
                assert!(
                    msg.contains("missing field `transaction`"),
                    "Unexpected error message: {}",
                    msg
                );
            }
            Err(e) => panic!("Expected BadRequest error, but got: {:?}", e),
            Ok(resp) => panic!("Expected error response, got Ok: {:?}", resp),
        }
    }

    #[tokio::test]
    async fn test_handle_request_sign_transaction() {
        let mut mock_rpc_methods = MockSolanaRpcMethods::new();

        // Create mock response
        let mock_signature = "5wHu1qwD4kF3wxjejXkgDYNVnEgB1e8uVvrxNwJYRzHPPxWqRA4nxwE1TU4";
        let mock_transaction = "AQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABAAEDAgAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_string();

        mock_rpc_methods
            .expect_sign_transaction()
            .with(predicate::eq(SignTransactionRequestParams {
                transaction: EncodedSerializedTransaction::new(mock_transaction.clone()),
            }))
            .returning(move |_| {
                Ok(SignTransactionResult {
                    transaction: EncodedSerializedTransaction::new(mock_transaction.clone()),
                    signature: mock_signature.to_string(),
                })
            })
            .times(1);

        let mock_handler = Arc::new(SolanaRpcHandler::new(mock_rpc_methods));

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: 1,
            method: "signTransaction".to_string(),
            params: json!({
                "transaction": "AQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABAAEDAgAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            }),
        };

        let response = mock_handler.handle_request(request).await;

        assert!(response.is_ok(), "Expected Ok response, got {:?}", response);
        let json_response = response.unwrap();

        match json_response.result {
            Some(value) => {
                let result = value.as_object().unwrap();
                assert!(result.contains_key("transaction"));
                assert!(result.contains_key("signature"));
                assert_eq!(result["signature"], mock_signature);
            }
            None => panic!("Expected Some result, got None"),
        }
    }

    #[tokio::test]
    async fn test_handle_request_sign_transaction_invalid_params() {
        let mock_rpc_methods = MockSolanaRpcMethods::new();
        let mock_handler = Arc::new(SolanaRpcHandler::new(mock_rpc_methods));

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: 1,
            method: "signTransaction".to_string(),
            params: json!({
                "invalid_field": "some_value"
            }),
        };

        let response = mock_handler.handle_request(request).await;

        match response {
            Err(SolanaRpcError::BadRequest(msg)) => {
                assert!(
                    msg.contains("missing field `transaction`"),
                    "Unexpected error message: {}",
                    msg
                );
            }
            _ => panic!("Expected BadRequest error"),
        }
    }

    #[tokio::test]
    async fn test_handle_request_sign_and_send_transaction_success() {
        let mut mock_rpc_methods = MockSolanaRpcMethods::new();

        // Create mock data
        let mock_signature = "5wHu1qwD4kF3wxjejXkgDYNVnEgB1e8uVvrxNwJYRzHPPxWqRA4nxwE1TU4";
        let mock_transaction = "AQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABAAEDAgAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_string();

        mock_rpc_methods
            .expect_sign_and_send_transaction()
            .with(predicate::eq(SignAndSendTransactionRequestParams {
                transaction: EncodedSerializedTransaction::new(mock_transaction.clone()),
            }))
            .returning(move |_| {
                Ok(SignAndSendTransactionResult {
                    transaction: EncodedSerializedTransaction::new(mock_transaction.clone()),
                    signature: mock_signature.to_string(),
                })
            })
            .times(1);

        let handler = Arc::new(SolanaRpcHandler::new(mock_rpc_methods));

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: 1,
            method: "signAndSendTransaction".to_string(),
            params: json!({
                "transaction": "AQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABAAEDAgAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            }),
        };

        let response = handler.handle_request(request).await;

        assert!(response.is_ok());
        let json_response = response.unwrap();
        match json_response.result {
            Some(value) => {
                let result = value.as_object().unwrap();
                assert!(result.contains_key("transaction"));
                assert!(result.contains_key("signature"));
                assert_eq!(result["signature"], mock_signature);
            }
            None => panic!("Expected Some result, got None"),
        }
    }

    #[tokio::test]
    async fn test_handle_request_sign_and_send_transaction_invalid_params() {
        let mock_rpc_methods = MockSolanaRpcMethods::new();
        let handler = Arc::new(SolanaRpcHandler::new(mock_rpc_methods));

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: 1,
            method: "signAndSendTransaction".to_string(),
            params: json!({
                "wrong_field": "some_value"
            }),
        };

        let response = handler.handle_request(request).await;

        assert!(matches!(response, Err(SolanaRpcError::BadRequest(_))));
    }

    #[tokio::test]
    async fn test_transfer_transaction_success() {
        let mut mock_rpc_methods = MockSolanaRpcMethods::new();
        let mock_transaction = "AQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABAAEDAgAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_string();

        mock_rpc_methods
            .expect_transfer_transaction()
            .with(predicate::eq(TransferTransactionRequestParams {
                source: "C6VBV1EK2Jx7kFgCkCD5wuDeQtEH8ct2hHGUPzEhUSc8".to_string(),
                destination: "C6VBV1EK2Jx7kFgCkCD5wuDeQtEH8ct2hHGUPzEhUSc8".to_string(),
                amount: 10,
                token: "Gh9ZwEmdLJ8DscKNTkTqPbNwLNNBjuSzaG9Vp2KGtKJr".to_string(), // noboost
            }))
            .returning(move |_| {
                Ok(TransferTransactionResult {
                    fee_in_lamports: "1005000".to_string(),
                    fee_in_spl: "1005000".to_string(),
                    fee_token: "Gh9ZwEmdLJ8DscKNTkTqPbNwLNNBjuSzaG9Vp2KGtKJr".to_string(), // noboost
                    transaction: EncodedSerializedTransaction::new(mock_transaction.clone()),
                    valid_until_blockheight: 351207983,
                })
            })
            .times(1);

        let handler = Arc::new(SolanaRpcHandler::new(mock_rpc_methods));

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: 1,
            method: "transferTransaction".to_string(),
            params: json!({
                "source": "C6VBV1EK2Jx7kFgCkCD5wuDeQtEH8ct2hHGUPzEhUSc8".to_string(),
                "destination": "C6VBV1EK2Jx7kFgCkCD5wuDeQtEH8ct2hHGUPzEhUSc8".to_string(),
                "amount": 10,
                "token": "Gh9ZwEmdLJ8DscKNTkTqPbNwLNNBjuSzaG9Vp2KGtKJr".to_string(), // noboost
            }),
        };

        let response = handler.handle_request(request).await;

        assert!(response.is_ok());
        let json_response = response.unwrap();
        match json_response.result {
            Some(value) => {
                let result = value.as_object().unwrap();
                assert!(result.contains_key("fee_in_lamports"));
                assert!(result.contains_key("fee_in_spl"));
                assert!(result.contains_key("fee_token"));
                assert!(result.contains_key("transaction"));
                assert!(result.contains_key("valid_until_blockheight"));
            }
            None => panic!("Expected Some result, got None"),
        }
    }

    #[tokio::test]
    async fn test_prepare_transaction_success() {
        let mut mock_rpc_methods = MockSolanaRpcMethods::new();
        let mock_transaction = "AQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABAAEDAgAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_string();

        mock_rpc_methods
            .expect_prepare_transaction()
            .with(predicate::eq(PrepareTransactionRequestParams {
                transaction: EncodedSerializedTransaction::new(mock_transaction.clone()),
                fee_token: "Gh9ZwEmdLJ8DscKNTkTqPbNwLNNBjuSzaG9Vp2KGtKJr".to_string(),
            }))
            .returning(move |_| {
                Ok(PrepareTransactionResult {
                    fee_in_lamports: "1005000".to_string(),
                    fee_in_spl: "1005000".to_string(),
                    fee_token: "Gh9ZwEmdLJ8DscKNTkTqPbNwLNNBjuSzaG9Vp2KGtKJr".to_string(),
                    transaction: EncodedSerializedTransaction::new(mock_transaction.clone()),
                    valid_until_blockheight: 351207983,
                })
            })
            .times(1);

        let handler = Arc::new(SolanaRpcHandler::new(mock_rpc_methods));

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: 1,
            method: "prepareTransaction".to_string(),
            params: json!({
                "transaction": "AQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABAAEDAgAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
                "fee_token": "Gh9ZwEmdLJ8DscKNTkTqPbNwLNNBjuSzaG9Vp2KGtKJr".to_string(), // noboost
            }),
        };

        let response = handler.handle_request(request).await;

        assert!(response.is_ok());
        let json_response = response.unwrap();
        match json_response.result {
            Some(value) => {
                let result = value.as_object().unwrap();
                assert!(result.contains_key("fee_in_lamports"));
                assert!(result.contains_key("fee_in_spl"));
                assert!(result.contains_key("fee_token"));
                assert!(result.contains_key("transaction"));
                assert!(result.contains_key("valid_until_blockheight"));
            }
            None => panic!("Expected Some result, got None"),
        }
    }
}
