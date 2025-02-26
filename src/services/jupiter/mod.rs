//! Jupiter API service module
//! Jupiter API service is used to get quotes for token swaps
//! Jupiter is not supported on devnet/testnet, so a mock service is used instead
//! The mock service returns a quote with the same input and output amount
use crate::{
    constants::{JUPITER_API_URL, SOL_MINT},
    utils::field_as_string,
};
use async_trait::async_trait;
use eyre::Result;
#[cfg(test)]
use mockall::automock;
use reqwest::Client;
use serde::{Deserialize, Serialize};

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

#[derive(Debug, Deserialize)]
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
}

#[async_trait]
#[cfg_attr(test, automock)]
pub trait JupiterServiceTrait: Send + Sync {
    async fn get_quote(&self, request: QuoteRequest) -> Result<QuoteResponse>;
    async fn get_sol_to_token_quote(
        &self,
        input_mint: &str,
        amount: u64,
        slippage: f32,
    ) -> Result<QuoteResponse>;
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
            base_url: JUPITER_API_URL.to_string(),
        }
    }
}

#[async_trait]
impl JupiterServiceTrait for MainnetJupiterService {
    /// Get a quote for a given input and output mint
    async fn get_quote(&self, request: QuoteRequest) -> Result<QuoteResponse> {
        let slippage_bps: u32 = request.slippage as u32 * 100;
        let url = format!("{}/quote", self.base_url);

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

        let quote: QuoteResponse = response.json().await.unwrap();
        Ok(quote)
    }

    /// Get a quote for a SOL to a given token
    async fn get_sol_to_token_quote(
        &self,
        output_mint: &str,
        amount: u64,
        slippage: f32,
    ) -> Result<QuoteResponse> {
        let request = QuoteRequest {
            input_mint: SOL_MINT.to_string(),
            output_mint: output_mint.to_string(),
            amount,
            slippage,
        };

        self.get_quote(request).await
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

#[async_trait]
impl JupiterServiceTrait for MockJupiterService {
    async fn get_quote(&self, request: QuoteRequest) -> Result<QuoteResponse> {
        let quote = QuoteResponse {
            input_mint: request.input_mint.clone(),
            output_mint: request.output_mint.clone(),
            in_amount: request.amount,
            out_amount: request.amount,
            other_amount_threshold: 0,
            price_impact_pct: 0.0,
        };
        Ok(quote)
    }

    /// Get a quote for a SOL to a given token
    async fn get_sol_to_token_quote(
        &self,
        output_mint: &str,
        amount: u64,
        slippage: f32,
    ) -> Result<QuoteResponse> {
        let request = QuoteRequest {
            input_mint: SOL_MINT.to_string(),
            output_mint: output_mint.to_string(),
            amount,
            slippage,
        };

        self.get_quote(request).await
    }
}

#[async_trait]
impl JupiterServiceTrait for JupiterService {
    async fn get_sol_to_token_quote(
        &self,
        output_mint: &str,
        amount: u64,
        slippage: f32,
    ) -> Result<QuoteResponse> {
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

    async fn get_quote(&self, request: QuoteRequest) -> Result<QuoteResponse> {
        match self {
            JupiterService::Mock(service) => service.get_quote(request).await,
            JupiterService::Mainnet(service) => service.get_quote(request).await,
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
}
