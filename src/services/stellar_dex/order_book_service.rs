//! Stellar Order Book Service implementation
//! Uses Stellar Horizon API `/order_book` endpoint for quote conversion

use super::{StellarDexServiceError, StellarQuoteResponse};
use crate::services::stellar_dex::StellarDexServiceTrait;
use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use std::time::Duration;

/// Stellar Horizon API order book response
#[derive(Debug, Deserialize)]
struct OrderBookResponse {
    bids: Vec<Order>,
    asks: Vec<Order>,
    base: AssetInfo,
    counter: AssetInfo,
}

#[derive(Debug, Deserialize)]
struct Order {
    #[serde(rename = "price_r")]
    price_r: RationalPrice,
    price: String,
    amount: String,
}

#[derive(Debug, Deserialize)]
struct RationalPrice {
    n: u64,
    d: u64,
}

#[derive(Debug, Deserialize)]
struct AssetInfo {
    #[serde(rename = "asset_type")]
    asset_type: String,
    #[serde(rename = "asset_code")]
    asset_code: Option<String>,
    #[serde(rename = "asset_issuer")]
    asset_issuer: Option<String>,
}

/// Service for getting quotes from Stellar Horizon Order Book API
pub struct OrderBookService {
    horizon_base_url: String,
    client: Client,
}

impl OrderBookService {
    /// Create a new OrderBookService instance
    ///
    /// # Arguments
    ///
    /// * `horizon_base_url` - Base URL for Stellar Horizon API (e.g., "https://horizon.stellar.org")
    pub fn new(horizon_base_url: String) -> Result<Self, StellarDexServiceError> {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| StellarDexServiceError::HttpRequestError(e))?;

        Ok(Self {
            horizon_base_url,
            client,
        })
    }

    /// Convert asset identifier to Horizon API format
    ///
    /// # Arguments
    ///
    /// * `asset_id` - Asset identifier (e.g., "native", "USDC:GA5Z...", or "native")
    ///
    /// # Returns
    ///
    /// Tuple of (asset_type, asset_code, asset_issuer)
    fn parse_asset_id(
        &self,
        asset_id: &str,
    ) -> Result<(String, Option<String>, Option<String>), StellarDexServiceError> {
        if asset_id == "native" || asset_id.is_empty() {
            return Ok(("native".to_string(), None, None));
        }

        // Try to parse as "CODE:ISSUER" format
        if let Some(colon_pos) = asset_id.find(':') {
            let code = asset_id[..colon_pos].to_string();
            let issuer = asset_id[colon_pos + 1..].to_string();

            if code.len() <= 4 {
                Ok(("credit_alphanum4".to_string(), Some(code), Some(issuer)))
            } else if code.len() <= 12 {
                Ok(("credit_alphanum12".to_string(), Some(code), Some(issuer)))
            } else {
                Err(StellarDexServiceError::InvalidAssetIdentifier(format!(
                    "Asset code too long: {}",
                    code
                )))
            }
        } else {
            Err(StellarDexServiceError::InvalidAssetIdentifier(format!(
                "Invalid asset format. Expected 'native' or 'CODE:ISSUER', got: {}",
                asset_id
            )))
        }
    }

    /// Build Horizon API URL for order_book endpoint
    fn build_order_book_url(
        &self,
        selling_asset_type: &str,
        selling_asset_code: Option<&str>,
        selling_asset_issuer: Option<&str>,
        buying_asset_type: &str,
        buying_asset_code: Option<&str>,
        buying_asset_issuer: Option<&str>,
    ) -> String {
        let mut url = format!("{}/order_book", self.horizon_base_url);

        // Build query parameters
        let mut params = vec![];

        // Selling asset (base)
        params.push(format!("selling_asset_type={}", selling_asset_type));
        if let Some(code) = selling_asset_code {
            params.push(format!("selling_asset_code={}", code));
        }
        if let Some(issuer) = selling_asset_issuer {
            params.push(format!("selling_asset_issuer={}", issuer));
        }

        // Buying asset (counter)
        params.push(format!("buying_asset_type={}", buying_asset_type));
        if let Some(code) = buying_asset_code {
            params.push(format!("buying_asset_code={}", code));
        }
        if let Some(issuer) = buying_asset_issuer {
            params.push(format!("buying_asset_issuer={}", issuer));
        }

        url.push('?');
        url.push_str(&params.join("&"));

        url
    }

    /// Fetch order book from Horizon API
    async fn fetch_order_book(
        &self,
        selling_asset_type: &str,
        selling_asset_code: Option<&str>,
        selling_asset_issuer: Option<&str>,
        buying_asset_type: &str,
        buying_asset_code: Option<&str>,
        buying_asset_issuer: Option<&str>,
    ) -> Result<OrderBookResponse, StellarDexServiceError> {
        let url = self.build_order_book_url(
            selling_asset_type,
            selling_asset_code,
            selling_asset_issuer,
            buying_asset_type,
            buying_asset_code,
            buying_asset_issuer,
        );

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(StellarDexServiceError::HttpRequestError)?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(StellarDexServiceError::ApiError {
                message: format!("Horizon API returned error {}: {}", status, error_text),
            });
        }

        response.json().await.map_err(|e| {
            StellarDexServiceError::UnknownError(format!("Failed to deserialize response: {}", e))
        })
    }

    // Calculate output amount by walking through order book
    ///
    /// IMPORTANT: Understanding Stellar Order Books
    /// - base asset = asset being SOLD in orders
    /// - counter asset = asset being BOUGHT in orders
    /// - asks = orders selling base for counter (price in counter/base)
    /// - bids = orders buying base with counter (price in counter/base)
    ///
    /// # Arguments
    ///
    /// * `orders` - Order book entries (bids or asks)
    /// * `input_amount` - Amount to convert (in stroops/smallest unit)
    /// * `is_ask` - If true, using asks (selling base), otherwise bids (buying base)
    ///
    /// # Returns
    ///
    /// Tuple of (output_stroops, avg_price, best_price)
    fn calculate_quote_from_orders(
        &self,
        orders: &[Order],
        input_amount: u64,
        is_ask: bool,
    ) -> Result<(u64, f64, f64), StellarDexServiceError> {
        if orders.is_empty() {
            return Err(StellarDexServiceError::NoPathFound);
        }

        let best_price: f64 = orders[0].price.parse().map_err(|e| {
            StellarDexServiceError::UnknownError(format!("Failed to parse price: {}", e))
        })?;

        let mut remaining_stroops = input_amount as i128;
        let mut total_output_stroops: i128 = 0;
        let mut total_input_stroops_used: i128 = 0;

        for order in orders {
            if remaining_stroops <= 0 {
                break;
            }

            // Parse amount (in base asset units as decimal string)
            let amount: f64 = order.amount.parse().map_err(|e| {
                StellarDexServiceError::UnknownError(format!("Failed to parse amount: {}", e))
            })?;

            // Convert to stroops
            let amount_stroops = (amount * 10_000_000.0) as i128;

            // Use rational price for precise calculation
            let price_n = order.price_r.n as i128;
            let price_d = order.price_r.d as i128;

            if is_ask {
                // Selling base asset to get counter asset
                // Output (counter) = base_amount * (price_n / price_d)

                if remaining_stroops <= amount_stroops {
                    // Partially fill this order
                    let output = (remaining_stroops * price_n) / price_d;
                    total_output_stroops += output;
                    total_input_stroops_used += remaining_stroops;
                    remaining_stroops = 0;
                } else {
                    // Use entire order
                    let output = (amount_stroops * price_n) / price_d;
                    total_output_stroops += output;
                    total_input_stroops_used += amount_stroops;
                    remaining_stroops -= amount_stroops;
                }
            } else {
                // Buying base asset with counter asset
                // To get base_amount, pay: base_amount * (price_n / price_d) in counter
                // We have counter, want to know how much base we can get

                let counter_cost = (amount_stroops * price_n) / price_d;

                if remaining_stroops <= counter_cost {
                    // Partially fill this order
                    // Solve: base * (price_n / price_d) = remaining_stroops
                    // base = remaining_stroops * price_d / price_n
                    let base_received = (remaining_stroops * price_d) / price_n;
                    total_output_stroops += base_received;
                    total_input_stroops_used += remaining_stroops;
                    remaining_stroops = 0;
                } else {
                    // Use entire order
                    total_output_stroops += amount_stroops;
                    total_input_stroops_used += counter_cost;
                    remaining_stroops -= counter_cost;
                }
            }
        }

        // Check if we could fulfill the entire amount (allow 1 stroop tolerance)
        if remaining_stroops > 1 {
            return Err(StellarDexServiceError::NoPathFound);
        }

        // Calculate weighted average price (for price impact calculation)
        let avg_price = if is_ask {
            // Selling: avg_price = output / input
            total_output_stroops as f64 / total_input_stroops_used as f64
        } else {
            // Buying: avg_price = input / output
            total_input_stroops_used as f64 / total_output_stroops as f64
        };

        Ok((total_output_stroops as u64, avg_price, best_price))
    }
}

#[async_trait]
impl StellarDexServiceTrait for OrderBookService {
    async fn get_token_to_xlm_quote(
        &self,
        asset_id: &str,
        amount: u64,
        slippage: f32,
    ) -> Result<StellarQuoteResponse, StellarDexServiceError> {
        if asset_id == "native" || asset_id.is_empty() {
            return Ok(StellarQuoteResponse {
                input_asset: "native".to_string(),
                output_asset: "native".to_string(),
                in_amount: amount,
                out_amount: amount,
                price_impact_pct: 0.0,
                slippage_bps: (slippage * 100.0) as u32,
                path: None,
            });
        }

        let (token_type, token_code, token_issuer) = self.parse_asset_id(asset_id)?;

        // Fetch order book: base=TOKEN, counter=XLM
        // We want to SELL token (base), so we use ASKS
        let order_book = self
            .fetch_order_book(
                &token_type,
                token_code.as_deref(),
                token_issuer.as_deref(),
                "native",
                None,
                None,
            )
            .await?;

        if order_book.asks.is_empty() {
            return Err(StellarDexServiceError::NoPathFound);
        }

        // Use asks: people selling token for XLM
        let (out_amount, avg_price, best_price) =
            self.calculate_quote_from_orders(&order_book.asks, amount, true)?;

        // Calculate price impact
        // When selling, we get LESS as we go deeper (worse price)
        // avg_price < best_price, so (best - avg) / best is positive
        let price_impact_pct = if best_price > 1e-10 {
            ((best_price - avg_price) / best_price * 100.0).max(0.0)
        } else {
            0.0
        };

        Ok(StellarQuoteResponse {
            input_asset: asset_id.to_string(),
            output_asset: "native".to_string(),
            in_amount: amount,
            out_amount,
            price_impact_pct,
            slippage_bps: (slippage * 100.0) as u32,
            path: None,
        })
    }

    async fn get_xlm_to_token_quote(
        &self,
        asset_id: &str,
        amount: u64,
        slippage: f32,
    ) -> Result<StellarQuoteResponse, StellarDexServiceError> {
        if asset_id == "native" || asset_id.is_empty() {
            return Ok(StellarQuoteResponse {
                input_asset: "native".to_string(),
                output_asset: "native".to_string(),
                in_amount: amount,
                out_amount: amount,
                price_impact_pct: 0.0,
                slippage_bps: (slippage * 100.0) as u32,
                path: None,
            });
        }

        let (token_type, token_code, token_issuer) = self.parse_asset_id(asset_id)?;

        // Fetch order book: base=TOKEN, counter=XLM
        // We want to BUY token with XLM, so we use BIDS
        let order_book = self
            .fetch_order_book(
                &token_type,
                token_code.as_deref(),
                token_issuer.as_deref(),
                "native",
                None,
                None,
            )
            .await?;

        if order_book.bids.is_empty() {
            return Err(StellarDexServiceError::NoPathFound);
        }

        // Use bids: people buying token (we're taking the other side, paying XLM)
        let (out_amount, avg_price, best_price) =
            self.calculate_quote_from_orders(&order_book.bids, amount, false)?;

        // Calculate price impact
        // When buying, we pay MORE as we go deeper (worse price)
        // avg_price > best_price, so (avg - best) / best is positive
        let price_impact_pct = if best_price > 1e-10 {
            ((avg_price - best_price) / best_price * 100.0).max(0.0)
        } else {
            0.0
        };

        Ok(StellarQuoteResponse {
            input_asset: "native".to_string(),
            output_asset: asset_id.to_string(),
            in_amount: amount,
            out_amount,
            price_impact_pct,
            slippage_bps: (slippage * 100.0) as u32,
            path: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use tracing::debug;

    use super::*;

    const HORIZON_MAINNET_URL: &str = "https://horizon.stellar.org";
    const USDC_ASSET_ID: &str = "USDC:GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN";

    fn create_test_service() -> OrderBookService {
        OrderBookService::new(HORIZON_MAINNET_URL.to_string()).unwrap()
    }

    #[tokio::test]
    async fn test_fetch_order_book_usdc_to_xlm() {
        let service = create_test_service();
        let order_book = service
            .fetch_order_book(
                "credit_alphanum4",
                Some("USDC"),
                Some("GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN"),
                "native",
                None,
                None,
            )
            .await;

        assert!(
            order_book.is_ok(),
            "Failed to fetch order book: {:?}",
            order_book
        );
        let order_book = order_book.unwrap();

        // Verify response structure
        assert!(!order_book.bids.is_empty(), "Order book should have bids");
        assert!(!order_book.asks.is_empty(), "Order book should have asks");
        assert_eq!(order_book.base.asset_type, "credit_alphanum4");
        assert_eq!(order_book.base.asset_code, Some("USDC".to_string()));
        assert_eq!(order_book.counter.asset_type, "native");

        // Verify order structure
        let first_ask = &order_book.asks[0];
        assert!(!first_ask.price.is_empty());
        assert!(!first_ask.amount.is_empty());
        assert!(first_ask.price_r.n > 0);
        assert!(first_ask.price_r.d > 0);
    }

    #[tokio::test]
    async fn test_get_token_to_xlm_quote_usdc() {
        let service = create_test_service();
        // Test with 100 USDC (100 * 10^7 stroops)
        let amount = 100_000_000u64;
        let slippage = 1.0;

        let result = service
            .get_token_to_xlm_quote(USDC_ASSET_ID, amount, slippage)
            .await;

        assert!(result.is_ok(), "Failed to get quote: {:?}", result);
        let quote = result.unwrap();

        assert_eq!(quote.input_asset, USDC_ASSET_ID);
        assert_eq!(quote.output_asset, "native");
        assert_eq!(quote.in_amount, amount);
        assert!(quote.out_amount > 0, "Output amount should be positive");
        assert!(
            quote.price_impact_pct >= 0.0,
            "Price impact should be non-negative"
        );
        assert_eq!(quote.slippage_bps, 100); // 1.0% = 100 bps
        assert!(quote.path.is_none(), "Order book doesn't provide path");
    }

    #[tokio::test]
    async fn test_get_xlm_to_token_quote_usdc() {
        let service = create_test_service();
        // Test with 100 XLM (100 * 10^7 stroops)
        let amount = 100_000_000u64;
        let slippage = 1.0;

        let result = service
            .get_xlm_to_token_quote(USDC_ASSET_ID, amount, slippage)
            .await;

        assert!(result.is_ok(), "Failed to get quote: {:?}", result);
        let quote = result.unwrap();

        assert_eq!(quote.input_asset, "native");
        assert_eq!(quote.output_asset, USDC_ASSET_ID);
        assert_eq!(quote.in_amount, amount);
        assert!(quote.out_amount > 0, "Output amount should be positive");
        assert!(
            quote.price_impact_pct >= 0.0,
            "Price impact should be non-negative"
        );
        assert_eq!(quote.slippage_bps, 100); // 1.0% = 100 bps
        assert!(quote.path.is_none(), "Order book doesn't provide path");
    }

    #[tokio::test]
    async fn test_get_token_to_xlm_quote_native() {
        let service = create_test_service();
        let amount = 100_000_000u64;
        let slippage = 1.0;

        let result = service
            .get_token_to_xlm_quote("native", amount, slippage)
            .await;

        assert!(result.is_ok());
        let quote = result.unwrap();

        assert_eq!(quote.input_asset, "native");
        assert_eq!(quote.output_asset, "native");
        assert_eq!(quote.in_amount, amount);
        assert_eq!(quote.out_amount, amount);
        assert_eq!(quote.price_impact_pct, 0.0);
    }

    #[tokio::test]
    async fn test_get_xlm_to_token_quote_native() {
        let service = create_test_service();
        let amount = 100_000_000u64;
        let slippage = 1.0;

        let result = service
            .get_xlm_to_token_quote("native", amount, slippage)
            .await;

        assert!(result.is_ok());
        let quote = result.unwrap();

        assert_eq!(quote.input_asset, "native");
        assert_eq!(quote.output_asset, "native");
        assert_eq!(quote.in_amount, amount);
        assert_eq!(quote.out_amount, amount);
        assert_eq!(quote.price_impact_pct, 0.0);
    }

    #[tokio::test]
    async fn test_parse_asset_id() {
        let service = create_test_service();

        // Test native
        let result = service.parse_asset_id("native");
        assert!(result.is_ok());
        let (asset_type, code, issuer) = result.unwrap();
        assert_eq!(asset_type, "native");
        assert_eq!(code, None);
        assert_eq!(issuer, None);

        // Test credit_alphanum4
        let result = service.parse_asset_id(USDC_ASSET_ID);
        assert!(result.is_ok());
        let (asset_type, code, issuer) = result.unwrap();
        assert_eq!(asset_type, "credit_alphanum4");
        assert_eq!(code, Some("USDC".to_string()));
        assert_eq!(
            issuer,
            Some("GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN".to_string())
        );

        // Test invalid format
        let result = service.parse_asset_id("INVALID");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_calculate_quote_small_amount() {
        let service = create_test_service();
        // Test with a small amount (1 USDC = 10^7 stroops)
        let amount = 10_000_000u64;
        let slippage = 0.5;

        let result = service
            .get_token_to_xlm_quote(USDC_ASSET_ID, amount, slippage)
            .await;

        assert!(result.is_ok());
        let quote = result.unwrap();
        assert!(quote.out_amount > 0);
        // For small amounts, price impact should be minimal
        assert!(
            quote.price_impact_pct < 5.0,
            "Price impact should be low for small amounts"
        );
    }

    #[tokio::test]
    async fn test_calculate_quote_large_amount() {
        let service = create_test_service();
        // Test with a larger amount (1000 USDC = 1000 * 10^7 stroops)
        let amount = 1_000_000_000u64;
        let slippage = 1.0;

        let result = service
            .get_token_to_xlm_quote(USDC_ASSET_ID, amount, slippage)
            .await;

        // This might fail if there's insufficient liquidity, which is expected
        if result.is_ok() {
            let quote = result.unwrap();
            assert!(quote.out_amount > 0);
            // Large amounts may have higher price impact
            assert!(quote.price_impact_pct >= 0.0);
        }
    }
}
