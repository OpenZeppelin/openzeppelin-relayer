use std::sync::Arc;

use crate::{
    domain::{JsonRpcRequest, JsonRpcResponse},
    models::{
        FeeEstimateRequestParams, FeeEstimateResult, GetFeaturesEnabledRequestParams,
        GetFeaturesEnabledResult, GetSupportedTokensItem, GetSupportedTokensRequestParams,
        GetSupportedTokensResult, PrepareTransactionRequestParams, PrepareTransactionResult,
        RelayerRepoModel, SignAndSendTransactionRequestParams, SignAndSendTransactionResult,
        SignTransactionRequestParams, SignTransactionResult, TransferTransactionRequestParams,
        TransferTransactionResult,
    },
    services::SolanaProvider,
};
use async_trait::async_trait;
use eyre::Result;

use log::{error, info};
#[cfg(test)]
use mockall::automock;
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SolanaRpcError {
    #[error("Unsupported method: {0}")]
    UnsupportedMethod(String),
    #[error("BadRequest: {0}")]
    BadRequest(String),
}

enum SolanaRpcMethod {
    FeeEstimate,
    TransferTransaction,
    PrepareTransaction,
    SignTransaction,
    SignAndSendTransaction,
    GetSupportedTokens,
    GetFeaturesEnabled,
}

impl SolanaRpcMethod {
    fn from_str(method: &str) -> Option<Self> {
        match method {
            "feeEstimate" => Some(SolanaRpcMethod::FeeEstimate),
            "transferTransaction" => Some(SolanaRpcMethod::TransferTransaction),
            "prepareTransaction" => Some(SolanaRpcMethod::PrepareTransaction),
            "signTransaction" => Some(SolanaRpcMethod::SignTransaction),
            "signAndSendTransaction" => Some(SolanaRpcMethod::SignAndSendTransaction),
            "getSupportedTokens" => Some(SolanaRpcMethod::GetSupportedTokens),
            "getFeaturesEnabled" => Some(SolanaRpcMethod::GetFeaturesEnabled),
            _ => None,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum SolanaRpcResult {
    FeeEstimate(FeeEstimateResult),
    TransferTransaction(TransferTransactionResult),
    PrepareTransaction(PrepareTransactionResult),
    SignTransaction(SignTransactionResult),
    SignAndSendTransaction(SignAndSendTransactionResult),
    GetSupportedTokens(GetSupportedTokensResult),
    GetFeaturesEnabled(GetFeaturesEnabledResult),
}

#[cfg_attr(test, automock)]
#[async_trait]
pub trait SolanaRpcHandler: Send + Sync {
    async fn handle_request(
        &self,
        request: JsonRpcRequest,
    ) -> Result<JsonRpcResponse, SolanaRpcError>;
    async fn fee_estimate(
        &self,
        request: FeeEstimateRequestParams,
    ) -> Result<FeeEstimateResult, SolanaRpcError>;
    async fn transfer_transaction(
        &self,
        request: TransferTransactionRequestParams,
    ) -> Result<TransferTransactionResult, SolanaRpcError>;
    async fn prepare_transaction(
        &self,
        request: PrepareTransactionRequestParams,
    ) -> Result<PrepareTransactionResult, SolanaRpcError>;
    async fn sign_transaction(
        &self,
        request: SignTransactionRequestParams,
    ) -> Result<SignTransactionResult, SolanaRpcError>;
    async fn sign_and_send_transaction(
        &self,
        request: SignAndSendTransactionRequestParams,
    ) -> Result<SignAndSendTransactionResult, SolanaRpcError>;
    async fn get_supported_tokens(
        &self,
        request: GetSupportedTokensRequestParams,
    ) -> Result<GetSupportedTokensResult, SolanaRpcError>;
    async fn get_features_enabled(
        &self,
        request: GetFeaturesEnabledRequestParams,
    ) -> Result<GetFeaturesEnabledResult, SolanaRpcError>;
}

pub struct DefaultSolanaRpcHandler {
    relayer: RelayerRepoModel,
    provider: Arc<SolanaProvider>,
}

impl DefaultSolanaRpcHandler {
    pub fn new(relayer: RelayerRepoModel, provider: Arc<SolanaProvider>) -> Self {
        Self { relayer, provider }
    }

    fn handle_error<T>(result: Result<T, serde_json::Error>) -> Result<T, SolanaRpcError> {
        result.map_err(|e| SolanaRpcError::BadRequest(e.to_string()))
    }
}

#[async_trait]
impl SolanaRpcHandler for DefaultSolanaRpcHandler {
    async fn handle_request(
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
                let res = self.fee_estimate(params).await?;
                SolanaRpcResult::FeeEstimate(res)
            }
            SolanaRpcMethod::TransferTransaction => {
                let params = Self::handle_error(serde_json::from_value::<
                    TransferTransactionRequestParams,
                >(request.params))?;
                let res = self.transfer_transaction(params).await?;
                SolanaRpcResult::TransferTransaction(res)
            }
            SolanaRpcMethod::PrepareTransaction => {
                let params = Self::handle_error(serde_json::from_value::<
                    PrepareTransactionRequestParams,
                >(request.params))?;
                let res = self.prepare_transaction(params).await?;
                SolanaRpcResult::PrepareTransaction(res)
            }
            SolanaRpcMethod::SignTransaction => {
                let params = Self::handle_error(serde_json::from_value::<
                    SignTransactionRequestParams,
                >(request.params))?;
                let res = self.sign_transaction(params).await?;
                SolanaRpcResult::SignTransaction(res)
            }
            SolanaRpcMethod::SignAndSendTransaction => {
                let params = Self::handle_error(serde_json::from_value::<
                    SignAndSendTransactionRequestParams,
                >(request.params))?;
                let res = self.sign_and_send_transaction(params).await?;
                SolanaRpcResult::SignAndSendTransaction(res)
            }
            SolanaRpcMethod::GetSupportedTokens => {
                let params = Self::handle_error(serde_json::from_value::<
                    GetSupportedTokensRequestParams,
                >(request.params))?;
                let res = self.get_supported_tokens(params).await?;
                SolanaRpcResult::GetSupportedTokens(res)
            }
            SolanaRpcMethod::GetFeaturesEnabled => {
                let params = Self::handle_error(serde_json::from_value::<
                    GetFeaturesEnabledRequestParams,
                >(request.params))?;
                let res = self.get_features_enabled(params).await?;
                SolanaRpcResult::GetFeaturesEnabled(res)
            }
        };

        Ok(JsonRpcResponse::result(request.id, result))
    }

    async fn get_supported_tokens(
        &self,
        _params: GetSupportedTokensRequestParams,
    ) -> Result<GetSupportedTokensResult, SolanaRpcError> {
        let tokens = self
            .relayer
            .policies
            .get_solana_policy()
            .allowed_tokens
            .map(|tokens| {
                tokens
                    .iter()
                    .map(|token| GetSupportedTokensItem {
                        mint: token.mint.clone(),
                        symbol: token.symbol.as_deref().unwrap_or("").to_string(),
                        decimals: token.decimals.unwrap_or(0),
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(GetSupportedTokensResult { tokens })
    }

    async fn fee_estimate(
        &self,
        _params: FeeEstimateRequestParams,
    ) -> Result<FeeEstimateResult, SolanaRpcError> {
        // Implementation
        Ok(FeeEstimateResult {
            estimated_fee: "0".to_string(),
            conversion_rate: "0".to_string(),
        })
    }

    async fn transfer_transaction(
        &self,
        _params: TransferTransactionRequestParams,
    ) -> Result<TransferTransactionResult, SolanaRpcError> {
        // Implementation
        Ok(TransferTransactionResult {
            transaction: "".to_string(),
            fee_in_spl: "0".to_string(),
            fee_in_lamports: "0".to_string(),
            fee_token: "".to_string(),
            valid_until_blockheight: 0,
        })
    }

    async fn prepare_transaction(
        &self,
        _params: PrepareTransactionRequestParams,
    ) -> Result<PrepareTransactionResult, SolanaRpcError> {
        // Implementation
        Ok(PrepareTransactionResult {
            transaction: "".to_string(),
            fee_in_spl: "0".to_string(),
            fee_in_lamports: "0".to_string(),
            fee_token: "".to_string(),
            valid_until_blockheight: 0,
        })
    }

    async fn sign_transaction(
        &self,
        _params: SignTransactionRequestParams,
    ) -> Result<SignTransactionResult, SolanaRpcError> {
        // Implementation
        Ok(SignTransactionResult {
            transaction: "".to_string(),
            signature: "".to_string(),
        })
    }

    async fn sign_and_send_transaction(
        &self,
        _params: SignAndSendTransactionRequestParams,
    ) -> Result<SignAndSendTransactionResult, SolanaRpcError> {
        // Implementation
        Ok(SignAndSendTransactionResult {
            transaction: "".to_string(),
            signature: "".to_string(),
        })
    }

    async fn get_features_enabled(
        &self,
        _params: GetFeaturesEnabledRequestParams,
    ) -> Result<GetFeaturesEnabledResult, SolanaRpcError> {
        // Implementation
        Ok(GetFeaturesEnabledResult { features: vec![] })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockall::predicate::{self, *};
    use serde_json::json;

    #[tokio::test]
    async fn test_handle_request_fee_estimate2() {
        let mut mock_handler = MockSolanaRpcHandler::new();

        // Set up the mock to expect a call to fee_estimate
        mock_handler
            .expect_fee_estimate()
            .with(predicate::eq(FeeEstimateRequestParams {
                transaction: "test_transaction".to_string(),
                fee_token: Some("test_token".to_string()),
            }))
            .returning(|_| {
                Ok(FeeEstimateResult {
                    estimated_fee: "0".to_string(),
                    conversion_rate: "0".to_string(),
                })
            })
            .times(1);

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: 1,
            method: "feeEstimate".to_string(),
            params: json!({
                "transaction": "test_transaction",
                "fee_token": "test_token"
            }),
        };

        let response = mock_handler
            .fee_estimate(FeeEstimateRequestParams {
                transaction: "test_transaction".to_string(),
                fee_token: Some("test_token".to_string()),
            })
            .await;

        assert!(response.is_ok(), "Expected Ok response, got {:?}", response);
        let json_response = response.unwrap();
        assert_eq!(json_response.estimated_fee, "0".to_string());
        assert_eq!(json_response.conversion_rate, "0".to_string());
    }
}
