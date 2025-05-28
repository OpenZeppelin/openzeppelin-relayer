//! Jupiter API service module
//! Jupiter API service is used to get quotes for token swaps
//! Jupiter is not supported on devnet/testnet, so a mock service is used instead
//! The mock service returns a quote with the same input and output amount
use crate::{
    constants::{JUPITER_BASE_API_URL, WRAPPED_SOL_MINT},
    utils::field_as_string,
};
use async_trait::async_trait;
#[cfg(test)]
use mockall::automock;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum JupiterServiceError {
    #[error("HTTP request failed: {0}")]
    HttpRequestError(#[from] reqwest::Error),
    #[error("API returned an error: {message}")]
    ApiError { message: String },
    #[error("Failed to deserialize response: {0}")]
    DeserializationError(#[from] serde_json::Error),
    #[error("An unknown error occurred")]
    UnknownError,
}

#[derive(Debug, Serialize)]
pub struct QuoteRequest {
    #[serde(rename = "inputMint")]
    pub input_mint: String,
    #[serde(rename = "outputMint")]
    pub output_mint: String,
    pub amount: u64,
    #[serde(rename = "slippage")]
    pub slippage: f32,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[allow(dead_code)]
pub struct SwapInfo {
    #[serde(rename = "ammKey")]
    pub amm_key: String,
    pub label: String,
    #[serde(rename = "inputMint")]
    pub input_mint: String,
    #[serde(rename = "outputMint")]
    pub output_mint: String,
    #[serde(rename = "inAmount")]
    pub in_amount: String,
    #[serde(rename = "outAmount")]
    pub out_amount: String,
    #[serde(rename = "feeAmount")]
    pub fee_amount: String,
    #[serde(rename = "feeMint")]
    pub fee_mint: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[allow(dead_code)]
pub struct RoutePlan {
    pub percent: u32,
    #[serde(rename = "swapInfo")]
    pub swap_info: SwapInfo,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[allow(dead_code)]
pub struct QuoteResponse {
    #[serde(rename = "inputMint")]
    pub input_mint: String,
    #[serde(rename = "outputMint")]
    pub output_mint: String,
    #[serde(rename = "inAmount")]
    #[serde(with = "field_as_string")]
    pub in_amount: u64,
    #[serde(rename = "outAmount")]
    #[serde(with = "field_as_string")]
    pub out_amount: u64,
    #[serde(rename = "otherAmountThreshold")]
    #[serde(with = "field_as_string")]
    pub other_amount_threshold: u64,
    #[serde(rename = "priceImpactPct")]
    #[serde(with = "field_as_string")]
    pub price_impact_pct: f64,
    #[serde(rename = "swapMode")]
    pub swap_mode: String,
    #[serde(rename = "slippageBps")]
    pub slippage_bps: u32,
    #[serde(rename = "routePlan")]
    pub route_plan: Vec<RoutePlan>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrioritizationFeeLamports {
    pub priority_level_with_max_lamports: PriorityLevelWitMaxLamports,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PriorityLevelWitMaxLamports {
    pub priority_level: Option<String>,
    pub max_lamports: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SwapRequest {
    pub quote_response: QuoteResponse,
    pub user_public_key: String,
    pub wrap_and_unwrap_sol: Option<bool>,
    pub fee_account: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compute_unit_price_micro_lamports: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prioritization_fee_lamports: Option<PrioritizationFeeLamports>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dynamic_compute_unit_limit: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SwapResponse {
    pub swap_transaction: String, // base64 encoded transaction
    pub last_valid_block_height: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prioritization_fee_lamports: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compute_unit_limit: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub simulation_error: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UltraOrderRequest {
    #[serde(rename = "inputMint")]
    pub input_mint: String,
    #[serde(rename = "outputMint")]
    pub output_mint: String,
    #[serde(with = "field_as_string")]
    pub amount: u64,
    pub taker: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UltraOrderResponse {
    #[serde(rename = "inputMint")]
    pub input_mint: String,
    #[serde(rename = "outputMint")]
    pub output_mint: String,
    #[serde(rename = "inAmount")]
    #[serde(with = "field_as_string")]
    pub in_amount: u64,
    #[serde(rename = "outAmount")]
    #[serde(with = "field_as_string")]
    pub out_amount: u64,
    #[serde(rename = "otherAmountThreshold")]
    #[serde(with = "field_as_string")]
    pub other_amount_threshold: u64,
    #[serde(rename = "priceImpactPct")]
    #[serde(with = "field_as_string")]
    pub price_impact_pct: f64,
    #[serde(rename = "swapMode")]
    pub swap_mode: String,
    #[serde(rename = "slippageBps")]
    pub slippage_bps: u32,
    #[serde(rename = "routePlan")]
    pub route_plan: Vec<RoutePlan>,
    #[serde(rename = "prioritizationFeeLamports")]
    pub prioritization_fee_lamports: u32,
    pub transaction: Option<String>,
    #[serde(rename = "requestId")]
    pub request_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UltraExecuteRequest {
    #[serde(rename = "signedTransaction")]
    pub signed_transaction: String,
    #[serde(rename = "requestId")]
    pub request_id: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[allow(dead_code)]
pub struct SwapEvents {
    #[serde(rename = "inputMint")]
    pub input_mint: String,
    #[serde(rename = "outputMint")]
    pub output_mint: String,
    #[serde(rename = "inputAmount")]
    pub input_amount: String,
    #[serde(rename = "outputAmount")]
    pub output_amount: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UltraExecuteResponse {
    pub signature: Option<String>,
    pub status: String,
    pub slot: Option<String>,
    pub error: Option<String>,
    pub code: u32,
    #[serde(rename = "totalInputAmount")]
    pub total_input_amount: Option<String>,
    #[serde(rename = "totalOutputAmount")]
    pub total_output_amount: Option<String>,
    #[serde(rename = "inputAmountResult")]
    pub input_amount_result: Option<String>,
    #[serde(rename = "outputAmountResult")]
    pub output_amount_result: Option<String>,
    #[serde(rename = "swapEvents")]
    pub swap_events: Option<Vec<SwapEvents>>,
}

#[async_trait]
#[cfg_attr(test, automock)]
pub trait JupiterServiceTrait: Send + Sync {
    async fn get_quote(&self, request: QuoteRequest) -> Result<QuoteResponse, JupiterServiceError>;
    async fn get_sol_to_token_quote(
        &self,
        input_mint: &str,
        amount: u64,
        slippage: f32,
    ) -> Result<QuoteResponse, JupiterServiceError>;
    async fn get_swap_transaction(
        &self,
        request: SwapRequest,
    ) -> Result<SwapResponse, JupiterServiceError>;
    async fn get_ultra_order(
        &self,
        request: UltraOrderRequest,
    ) -> Result<UltraOrderResponse, JupiterServiceError>;
    async fn execute_ultra_order(
        &self,
        request: UltraExecuteRequest,
    ) -> Result<UltraExecuteResponse, JupiterServiceError>;
}

pub enum JupiterService {
    Mainnet(MainnetJupiterService),
    Mock(MockJupiterService),
}

pub struct MainnetJupiterService {
    client: Client,
    base_url: String,
}

impl MainnetJupiterService {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            base_url: JUPITER_BASE_API_URL.to_string(),
        }
    }
}

impl Default for MainnetJupiterService {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl JupiterServiceTrait for MainnetJupiterService {
    /// Get a quote for a given input and output mint
    async fn get_quote(&self, request: QuoteRequest) -> Result<QuoteResponse, JupiterServiceError> {
        let slippage_bps: u32 = request.slippage as u32 * 100;
        let url = format!("{}/swap/v1/quote", self.base_url);

        let response = self
            .client
            .get(&url)
            .query(&[
                ("inputMint", request.input_mint),
                ("outputMint", request.output_mint),
                ("amount", request.amount.to_string()),
                ("slippageBps", slippage_bps.to_string()),
            ])
            .send()
            .await?
            .error_for_status()?;

        let quote: QuoteResponse = response.json().await?;
        Ok(quote)
    }

    /// Get a quote for a SOL to a given token
    async fn get_sol_to_token_quote(
        &self,
        output_mint: &str,
        amount: u64,
        slippage: f32,
    ) -> Result<QuoteResponse, JupiterServiceError> {
        let request = QuoteRequest {
            input_mint: WRAPPED_SOL_MINT.to_string(),
            output_mint: output_mint.to_string(),
            amount,
            slippage,
        };

        self.get_quote(request).await
    }

    async fn get_swap_transaction(
        &self,
        request: SwapRequest,
    ) -> Result<SwapResponse, JupiterServiceError> {
        let url = format!("{}/swap/v1/swap", self.base_url);
        let response = self.client.post(&url).json(&request).send().await?;

        if response.status().is_success() {
            response
                .json::<SwapResponse>()
                .await
                .map_err(JupiterServiceError::from)
        } else {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            Err(JupiterServiceError::ApiError {
                message: error_text,
            })
        }
    }

    async fn get_ultra_order(
        &self,
        request: UltraOrderRequest,
    ) -> Result<UltraOrderResponse, JupiterServiceError> {
        let url = format!("{}/ultra/v1/order", self.base_url);

        let response = self
            .client
            .get(&url)
            .query(&[
                ("inputMint", request.input_mint),
                ("outputMint", request.output_mint),
                ("amount", request.amount.to_string()),
                ("taker", request.taker),
            ])
            .send()
            .await?
            .error_for_status()?;

        response.json().await.map_err(JupiterServiceError::from)
    }

    async fn execute_ultra_order(
        &self,
        request: UltraExecuteRequest,
    ) -> Result<UltraExecuteResponse, JupiterServiceError> {
        let url = format!("{}/ultra/v1/execute", self.base_url);
        let response = self.client.post(&url).json(&request).send().await?;

        if response.status().is_success() {
            response.json().await.map_err(JupiterServiceError::from)
        } else {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            Err(JupiterServiceError::ApiError {
                message: error_text,
            })
        }
    }
}

// Jupiter Dev Service
// This service is used on testnet/devnets to mock the Jupiter API service
// due to the lack of a testnet API service
pub struct MockJupiterService {}

impl MockJupiterService {
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for MockJupiterService {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl JupiterServiceTrait for MockJupiterService {
    async fn get_quote(&self, request: QuoteRequest) -> Result<QuoteResponse, JupiterServiceError> {
        let quote = QuoteResponse {
            input_mint: request.input_mint.clone(),
            output_mint: request.output_mint.clone(),
            in_amount: request.amount,
            out_amount: request.amount,
            other_amount_threshold: 0,
            price_impact_pct: 0.0,
            swap_mode: "ExactIn".to_string(),
            slippage_bps: 0,
            route_plan: vec![RoutePlan {
                percent: 100,
                swap_info: SwapInfo {
                    amm_key: "mock_amm_key".to_string(),
                    label: "mock_label".to_string(),
                    input_mint: request.input_mint.clone(),
                    output_mint: request.output_mint.to_string(),
                    in_amount: request.amount.to_string(),
                    out_amount: request.amount.to_string(),
                    fee_amount: "0".to_string(),
                    fee_mint: "mock_fee_mint".to_string(),
                },
            }],
        };
        Ok(quote)
    }

    /// Get a quote for a SOL to a given token
    async fn get_sol_to_token_quote(
        &self,
        output_mint: &str,
        amount: u64,
        slippage: f32,
    ) -> Result<QuoteResponse, JupiterServiceError> {
        let request = QuoteRequest {
            input_mint: WRAPPED_SOL_MINT.to_string(),
            output_mint: output_mint.to_string(),
            amount,
            slippage,
        };

        self.get_quote(request).await
    }

    async fn get_swap_transaction(
        &self,
        _request: SwapRequest,
    ) -> Result<SwapResponse, JupiterServiceError> {
        // Provide realistic-looking mock data
        Ok(SwapResponse {
            swap_transaction: "AQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAACAAQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA...".to_string(),
            last_valid_block_height: 279632475,
            prioritization_fee_lamports: Some(9999),
            compute_unit_limit: Some(388876),
            simulation_error: None,
        })
    }

    async fn get_ultra_order(
        &self,
        request: UltraOrderRequest,
    ) -> Result<UltraOrderResponse, JupiterServiceError> {
        Ok(UltraOrderResponse {
            input_mint: request.input_mint.clone(),
            output_mint: request.output_mint.clone(),
            in_amount: 10,
            out_amount: 10,
            other_amount_threshold: 1,
            swap_mode: "ExactIn".to_string(),
            price_impact_pct: 0.0,
            route_plan: vec![RoutePlan {
                percent: 100,
                swap_info: SwapInfo {
                    amm_key: "mock_amm_key".to_string(),
                    label: "mock_label".to_string(),
                    input_mint: request.input_mint,
                    output_mint: request.output_mint.to_string(),
                    in_amount: request.amount.to_string(),
                    out_amount: request.amount.to_string(),
                    fee_amount: "0".to_string(),
                    fee_mint: "mock_fee_mint".to_string(),
                },
            }],
            prioritization_fee_lamports: 0,
            transaction: Some("test_transaction".to_string()),
            request_id: "mock_request_id".to_string(),
            slippage_bps: 0,
        })
    }

    async fn execute_ultra_order(
        &self,
        _request: UltraExecuteRequest,
    ) -> Result<UltraExecuteResponse, JupiterServiceError> {
        Ok(UltraExecuteResponse {
            signature: Some("mock_signature".to_string()),
            status: "success".to_string(),
            slot: Some("123456789".to_string()),
            error: None,
            code: 0,
            total_input_amount: Some("1000000".to_string()),
            total_output_amount: Some("1000000".to_string()),
            input_amount_result: Some("1000000".to_string()),
            output_amount_result: Some("1000000".to_string()),
            swap_events: Some(vec![SwapEvents {
                input_mint: "mock_input_mint".to_string(),
                output_mint: "mock_output_mint".to_string(),
                input_amount: "1000000".to_string(),
                output_amount: "1000000".to_string(),
            }]),
        })
    }
}

#[async_trait]
impl JupiterServiceTrait for JupiterService {
    async fn get_sol_to_token_quote(
        &self,
        output_mint: &str,
        amount: u64,
        slippage: f32,
    ) -> Result<QuoteResponse, JupiterServiceError> {
        match self {
            JupiterService::Mock(service) => {
                service
                    .get_sol_to_token_quote(output_mint, amount, slippage)
                    .await
            }
            JupiterService::Mainnet(service) => {
                service
                    .get_sol_to_token_quote(output_mint, amount, slippage)
                    .await
            }
        }
    }

    async fn get_quote(&self, request: QuoteRequest) -> Result<QuoteResponse, JupiterServiceError> {
        match self {
            JupiterService::Mock(service) => service.get_quote(request).await,
            JupiterService::Mainnet(service) => service.get_quote(request).await,
        }
    }

    async fn get_swap_transaction(
        &self,
        request: SwapRequest,
    ) -> Result<SwapResponse, JupiterServiceError> {
        match self {
            JupiterService::Mock(service) => service.get_swap_transaction(request).await,
            JupiterService::Mainnet(service) => service.get_swap_transaction(request).await,
        }
    }

    async fn get_ultra_order(
        &self,
        request: UltraOrderRequest,
    ) -> Result<UltraOrderResponse, JupiterServiceError> {
        match self {
            JupiterService::Mock(service) => service.get_ultra_order(request).await,
            JupiterService::Mainnet(service) => service.get_ultra_order(request).await,
        }
    }

    async fn execute_ultra_order(
        &self,
        request: UltraExecuteRequest,
    ) -> Result<UltraExecuteResponse, JupiterServiceError> {
        match self {
            JupiterService::Mock(service) => service.execute_ultra_order(request).await,
            JupiterService::Mainnet(service) => service.execute_ultra_order(request).await,
        }
    }
}

impl JupiterService {
    pub fn new_from_network(network: &str) -> Self {
        match network {
            "devnet" | "testnet" => JupiterService::Mock(MockJupiterService::new()),
            "mainnet" => JupiterService::Mainnet(MainnetJupiterService::new()),
            _ => JupiterService::Mainnet(MainnetJupiterService::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{
        matchers::{method, path, query_param},
        Mock, MockServer, ResponseTemplate,
    };

    #[tokio::test]
    async fn test_get_quote() {
        let service = MainnetJupiterService::new();

        // USDC -> SOL quote request
        let request = QuoteRequest {
            input_mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(), // noboost
            output_mint: "So11111111111111111111111111111111111111112".to_string(), // SOL
            amount: 1000000,                                                        // 1 USDC
            slippage: 0.5,                                                          // 0.5%
        };

        let result = service.get_quote(request).await;
        assert!(result.is_ok());

        let quote = result.unwrap();
        assert_eq!(
            quote.input_mint,
            "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
        );
        assert_eq!(
            quote.output_mint,
            "So11111111111111111111111111111111111111112"
        );
        assert!(quote.out_amount > 0);
    }

    #[tokio::test]
    async fn test_get_sol_to_token_quote() {
        let service = MainnetJupiterService::new();

        let result = service
            .get_sol_to_token_quote("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v", 1000000, 0.5)
            .await;
        assert!(result.is_ok());

        let quote = result.unwrap();
        assert_eq!(
            quote.input_mint,
            "So11111111111111111111111111111111111111112"
        );
        assert_eq!(
            quote.output_mint,
            "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
        );
        assert!(quote.out_amount > 0);
    }

    #[tokio::test]
    async fn test_mock_get_quote() {
        let service = MainnetJupiterService::new();

        // USDC -> SOL quote request
        let request = QuoteRequest {
            input_mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(), // USDC
            output_mint: "So11111111111111111111111111111111111111112".to_string(), // SOL
            amount: 1000000,                                                        // 1 USDC
            slippage: 0.5,                                                          // 0.5%
        };

        let result = service.get_quote(request).await;
        assert!(result.is_ok());

        let quote = result.unwrap();
        assert_eq!(
            quote.input_mint,
            "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
        );
        assert_eq!(
            quote.output_mint,
            "So11111111111111111111111111111111111111112"
        );
        assert!(quote.out_amount > 0);
    }

    #[tokio::test]
    async fn test_get_swap_transaction() {
        let mock_server = MockServer::start().await;

        let quote = QuoteResponse {
            input_mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
            output_mint: "So11111111111111111111111111111111111111112".to_string(),
            in_amount: 1000000,
            out_amount: 24860952,
            other_amount_threshold: 24362733,
            price_impact_pct: 0.1,
            swap_mode: "ExactIn".to_string(),
            slippage_bps: 50,
            route_plan: vec![RoutePlan {
                percent: 100,
                swap_info: SwapInfo {
                    amm_key: "test_amm_key".to_string(),
                    label: "test_label".to_string(),
                    input_mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
                    output_mint: "So11111111111111111111111111111111111111112".to_string(),
                    in_amount: "1000000".to_string(),
                    out_amount: "24860952".to_string(),
                    fee_amount: "1000".to_string(),
                    fee_mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
                },
            }],
        };

        let swap_response = SwapResponse {
            swap_transaction:
                "AQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
                    .to_string(),
            last_valid_block_height: 12345678,
            prioritization_fee_lamports: Some(5000),
            compute_unit_limit: Some(200000),
            simulation_error: None,
        };

        Mock::given(method("POST"))
            .and(path("/swap/v1/swap"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&swap_response))
            .expect(1)
            .mount(&mock_server)
            .await;

        let service = MainnetJupiterService {
            client: Client::new(),
            base_url: mock_server.uri(),
        };

        let request = SwapRequest {
            quote_response: quote,
            user_public_key: "test_public_key".to_string(),
            wrap_and_unwrap_sol: Some(true),
            fee_account: None,
            compute_unit_price_micro_lamports: None,
            prioritization_fee_lamports: None,
            dynamic_compute_unit_limit: Some(true),
        };

        let result = service.get_swap_transaction(request).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.last_valid_block_height, 12345678);
        assert_eq!(response.prioritization_fee_lamports, Some(5000));
        assert_eq!(response.compute_unit_limit, Some(200000));
    }

    #[tokio::test]
    async fn test_get_ultra_order() {
        let mock_server = MockServer::start().await;

        let ultra_response = UltraOrderResponse {
            input_mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
            output_mint: "So11111111111111111111111111111111111111112".to_string(),
            in_amount: 1000000,
            out_amount: 24860952,
            other_amount_threshold: 24362733,
            price_impact_pct: 0.1,
            swap_mode: "ExactIn".to_string(),
            slippage_bps: 50,
            route_plan: vec![RoutePlan {
                percent: 100,
                swap_info: SwapInfo {
                    amm_key: "test_amm_key".to_string(),
                    label: "test_label".to_string(),
                    input_mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
                    output_mint: "So11111111111111111111111111111111111111112".to_string(),
                    in_amount: "1000000".to_string(),
                    out_amount: "24860952".to_string(),
                    fee_amount: "1000".to_string(),
                    fee_mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
                },
            }],
            prioritization_fee_lamports: 5000,
            transaction: Some("test_transaction".to_string()),
            request_id: "test_request_id".to_string(),
        };

        Mock::given(method("GET"))
            .and(path("/ultra/v1/order"))
            .and(query_param(
                "inputMint",
                "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
            ))
            .and(query_param(
                "outputMint",
                "So11111111111111111111111111111111111111112",
            ))
            .and(query_param("amount", "1000000"))
            .and(query_param("taker", "test_taker"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&ultra_response))
            .expect(1)
            .mount(&mock_server)
            .await;
        let service = MainnetJupiterService {
            client: Client::new(),
            base_url: mock_server.uri(),
        };

        let request = UltraOrderRequest {
            input_mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
            output_mint: "So11111111111111111111111111111111111111112".to_string(),
            amount: 1000000,
            taker: "test_taker".to_string(),
        };

        let result = service.get_ultra_order(request).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.in_amount, 1000000);
        assert_eq!(response.out_amount, 24860952);
        assert_eq!(response.request_id, "test_request_id");
        assert!(response.transaction.is_some());
    }

    #[tokio::test]
    async fn test_execute_ultra_order() {
        let mock_server = MockServer::start().await;

        let execute_response = UltraExecuteResponse {
            signature: Some("mock_signature".to_string()),
            status: "success".to_string(),
            slot: Some("123456789".to_string()),
            error: None,
            code: 0,
            total_input_amount: Some("1000000".to_string()),
            total_output_amount: Some("1000000".to_string()),
            input_amount_result: Some("1000000".to_string()),
            output_amount_result: Some("1000000".to_string()),
            swap_events: Some(vec![SwapEvents {
                input_mint: "mock_input_mint".to_string(),
                output_mint: "mock_output_mint".to_string(),
                input_amount: "1000000".to_string(),
                output_amount: "1000000".to_string(),
            }]),
        };

        Mock::given(method("POST"))
            .and(path("/ultra/v1/execute"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&execute_response))
            .expect(1)
            .mount(&mock_server)
            .await;

        let service = MainnetJupiterService {
            client: Client::new(),
            base_url: mock_server.uri(),
        };

        let request = UltraExecuteRequest {
            signed_transaction: "signed_transaction_data".to_string(),
            request_id: "test_request_id".to_string(),
        };

        let result = service.execute_ultra_order(request).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.signature, Some("mock_signature".to_string()));
    }

    #[tokio::test]
    async fn test_error_handling_for_api_errors() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/ultra/v1/order"))
            .respond_with(ResponseTemplate::new(400).set_body_string("Invalid request"))
            .expect(1)
            .mount(&mock_server)
            .await;

        let service = MainnetJupiterService {
            client: Client::new(),
            base_url: mock_server.uri(),
        };

        let request = UltraOrderRequest {
            input_mint: "invalid_mint".to_string(),
            output_mint: "invalid_mint".to_string(),
            amount: 1000000,
            taker: "test_taker".to_string(),
        };

        let result = service.get_ultra_order(request).await;

        assert!(result.is_err());
        match result {
            Err(JupiterServiceError::HttpRequestError(err)) => {
                assert!(err
                    .to_string()
                    .contains("HTTP status client error (400 Bad Request)"));
            }
            _ => panic!("Expected ApiError but got different error type"),
        }
    }
}
