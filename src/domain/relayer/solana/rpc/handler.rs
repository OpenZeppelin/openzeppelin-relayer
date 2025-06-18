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
    models::{JsonRpcRequest, JsonRpcResponse},
    models::{NetworkRpcRequest, NetworkRpcResult, SolanaRpcRequest, SolanaRpcResult},
};
use eyre::Result;
use log::info;

pub struct SolanaRpcHandler<T> {
    rpc_methods: T,
}

impl<T: SolanaRpcMethods> SolanaRpcHandler<T> {
    /// Creates a new `SolanaRpcHandler` with the specified RPC methods.
    ///
    /// # Arguments
    ///
    /// * `rpc_methods` - An implementation of the `SolanaRpcMethods` trait that provides the
    ///   necessary methods for handling RPC requests.
    ///
    /// # Returns
    ///
    /// Returns a new instance of `SolanaRpcHandler`
    pub fn new(rpc_methods: T) -> Self {
        Self { rpc_methods }
    }

    /// Handles an incoming JSON-RPC request and dispatches it to the appropriate method.
    ///
    /// This function processes the request by determining the method to call based on
    /// the request's method name, deserializing the parameters, and invoking the corresponding
    /// method on the `rpc_methods` implementation.
    ///
    /// # Arguments
    ///
    /// * `request` - A `JsonRpcRequest` containing the method name and parameters.
    ///
    /// # Returns
    ///
    /// Returns a `Result` containing either a `JsonRpcResponse` with the result of the method call
    /// or a `SolanaRpcError` if an error occurred.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// * The method is unsupported.
    /// * The parameters cannot be deserialized.
    /// * The underlying method call fails.
    pub async fn handle_request(
        &self,
        request: JsonRpcRequest<NetworkRpcRequest>,
    ) -> Result<JsonRpcResponse<NetworkRpcResult>, SolanaRpcError> {
        info!("Received request params: {:?}", request.params);
        // Extract Solana request or return error
        let solana_request = match request.params {
            NetworkRpcRequest::Solana(solana_params) => solana_params,
            _ => {
                return Err(SolanaRpcError::BadRequest(
                    "Expected Solana network request".to_string(),
                ));
            }
        };

        let result = match solana_request {
            SolanaRpcRequest::FeeEstimate(params) => {
                let res = self.rpc_methods.fee_estimate(params).await?;
                SolanaRpcResult::FeeEstimate(res)
            }
            SolanaRpcRequest::TransferTransaction(params) => {
                let res = self.rpc_methods.transfer_transaction(params).await?;
                SolanaRpcResult::TransferTransaction(res)
            }
            SolanaRpcRequest::PrepareTransaction(params) => {
                let res = self.rpc_methods.prepare_transaction(params).await?;
                SolanaRpcResult::PrepareTransaction(res)
            }
            SolanaRpcRequest::SignAndSendTransaction(params) => {
                let res = self.rpc_methods.sign_and_send_transaction(params).await?;
                SolanaRpcResult::SignAndSendTransaction(res)
            }
            SolanaRpcRequest::SignTransaction(params) => {
                let res = self.rpc_methods.sign_transaction(params).await?;
                SolanaRpcResult::SignTransaction(res)
            }
            SolanaRpcRequest::GetSupportedTokens(params) => {
                let res = self.rpc_methods.get_supported_tokens(params).await?;
                SolanaRpcResult::GetSupportedTokens(res)
            }
            SolanaRpcRequest::GetFeaturesEnabled(params) => {
                let res = self.rpc_methods.get_features_enabled(params).await?;
                SolanaRpcResult::GetFeaturesEnabled(res)
            }
        };

        Ok(JsonRpcResponse::result(
            request.id,
            NetworkRpcResult::Solana(result),
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::{
        domain::MockSolanaRpcMethods,
        models::{
            EncodedSerializedTransaction, FeeEstimateRequestParams, FeeEstimateResult,
            GetFeaturesEnabledRequestParams, GetFeaturesEnabledResult, JsonRpcId,
            PrepareTransactionRequestParams, PrepareTransactionResult,
            SignAndSendTransactionRequestParams, SignAndSendTransactionResult,
            SignTransactionRequestParams, SignTransactionResult, TransferTransactionRequestParams,
            TransferTransactionResult,
        },
    };

    use super::*;
    use mockall::predicate::{self};

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
            id: Some(JsonRpcId::Number(1)),
            params: NetworkRpcRequest::Solana(SolanaRpcRequest::FeeEstimate(
                FeeEstimateRequestParams {
                    transaction: EncodedSerializedTransaction::new("test_transaction".to_string()),
                    fee_token: "test_token".to_string(),
                },
            )),
        };

        let response = mock_handler.handle_request(request).await;

        assert!(response.is_ok(), "Expected Ok response, got {:?}", response);
        let json_response = response.unwrap();
        assert_eq!(
            json_response.result,
            Some(NetworkRpcResult::Solana(SolanaRpcResult::FeeEstimate(
                FeeEstimateResult {
                    estimated_fee: "0".to_string(),
                    conversion_rate: "0".to_string(),
                }
            )))
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
            id: Some(JsonRpcId::Number(1)),
            params: NetworkRpcRequest::Solana(SolanaRpcRequest::GetFeaturesEnabled(
                GetFeaturesEnabledRequestParams {},
            )),
        };

        let response = mock_handler.handle_request(request).await;

        assert!(response.is_ok(), "Expected Ok response, got {:?}", response);
        let json_response = response.unwrap();
        assert_eq!(
            json_response.result,
            Some(NetworkRpcResult::Solana(
                SolanaRpcResult::GetFeaturesEnabled(GetFeaturesEnabledResult {
                    features: vec!["gasless".to_string()],
                })
            ))
        );
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
            id: Some(JsonRpcId::Number(1)),
            params: NetworkRpcRequest::Solana(SolanaRpcRequest::SignTransaction(
                SignTransactionRequestParams {
                    transaction: EncodedSerializedTransaction::new("AQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABAAEDAgAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_string()),
                },
            )),
        };

        let response = mock_handler.handle_request(request).await;

        assert!(response.is_ok(), "Expected Ok response, got {:?}", response);
        let json_response = response.unwrap();

        match json_response.result {
            Some(value) => {
                if let NetworkRpcResult::Solana(SolanaRpcResult::SignTransaction(result)) = value {
                    assert_eq!(result.signature, mock_signature);
                } else {
                    panic!("Expected SignTransaction result, got {:?}", value);
                }
            }
            None => panic!("Expected Some result, got None"),
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
            id: Some(JsonRpcId::Number(1)),
            params: NetworkRpcRequest::Solana(SolanaRpcRequest::SignAndSendTransaction(
                SignAndSendTransactionRequestParams {
                    transaction: EncodedSerializedTransaction::new("AQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABAAEDAgAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_string()),
                },
            )),
        };

        let response = handler.handle_request(request).await;

        assert!(response.is_ok());
        let json_response = response.unwrap();
        match json_response.result {
            Some(value) => {
                if let NetworkRpcResult::Solana(SolanaRpcResult::SignAndSendTransaction(result)) =
                    value
                {
                    assert_eq!(result.signature, mock_signature);
                } else {
                    panic!("Expected SignAndSendTransaction result, got {:?}", value);
                }
            }
            None => panic!("Expected Some result, got None"),
        }
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
            id: Some(JsonRpcId::Number(1)),
            params: NetworkRpcRequest::Solana(SolanaRpcRequest::TransferTransaction(
                TransferTransactionRequestParams {
                    source: "C6VBV1EK2Jx7kFgCkCD5wuDeQtEH8ct2hHGUPzEhUSc8".to_string(),
                    destination: "C6VBV1EK2Jx7kFgCkCD5wuDeQtEH8ct2hHGUPzEhUSc8".to_string(),
                    amount: 10,
                    token: "Gh9ZwEmdLJ8DscKNTkTqPbNwLNNBjuSzaG9Vp2KGtKJr".to_string(), // noboost
                },
            )),
        };

        let response = handler.handle_request(request).await;

        assert!(response.is_ok());
        let json_response = response.unwrap();
        match json_response.result {
            Some(value) => {
                if let NetworkRpcResult::Solana(SolanaRpcResult::TransferTransaction(result)) =
                    value
                {
                    assert!(!result.fee_in_lamports.is_empty());
                    assert!(!result.fee_in_spl.is_empty());
                    assert!(!result.fee_token.is_empty());
                    assert!(!result.transaction.into_inner().is_empty());
                    assert!(result.valid_until_blockheight > 0);
                } else {
                    panic!("Expected TransferTransaction result, got {:?}", value);
                }
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
            id: Some(JsonRpcId::Number(1)),
            params: NetworkRpcRequest::Solana(SolanaRpcRequest::PrepareTransaction(
                PrepareTransactionRequestParams {
                    transaction: EncodedSerializedTransaction::new("AQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABAAEDAgAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_string()),
                    fee_token: "Gh9ZwEmdLJ8DscKNTkTqPbNwLNNBjuSzaG9Vp2KGtKJr".to_string(),
                },
            )),
        };

        let response = handler.handle_request(request).await;

        assert!(response.is_ok());
        let json_response = response.unwrap();
        match json_response.result {
            Some(value) => {
                if let NetworkRpcResult::Solana(SolanaRpcResult::PrepareTransaction(result)) = value
                {
                    assert!(!result.fee_in_lamports.is_empty());
                    assert!(!result.fee_in_spl.is_empty());
                    assert!(!result.fee_token.is_empty());
                    assert!(!result.transaction.into_inner().is_empty());
                    assert!(result.valid_until_blockheight > 0);
                } else {
                    panic!("Expected PrepareTransaction result, got {:?}", value);
                }
            }
            None => panic!("Expected Some result, got None"),
        }
    }
}
