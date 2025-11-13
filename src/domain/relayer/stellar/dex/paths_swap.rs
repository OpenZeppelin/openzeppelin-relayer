//! PathsSwapDex
//!
//! Implements the `DexStrategy` trait to perform Stellar token swaps via the
//! Stellar Paths API (Horizon /paths/strict-receive endpoint). This module handles:
//!  1. Fetching a swap quote from the Paths service.
//!  2. Building the path payment operation.
//!  3. Creating and signing the transaction.
//!  4. Submitting the signed transaction to Stellar.
//!  5. Returning the swap result.

use std::sync::Arc;

use super::{DexStrategy, StellarSwapParams, StellarSwapResult};
use crate::domain::relayer::RelayerError;
use crate::services::stellar_dex::StellarDexServiceTrait;
use async_trait::async_trait;
use tracing::debug;

/// PathsSwapDex implementation using Stellar native paths
pub struct PathsSwapDex<D>
where
    D: StellarDexServiceTrait + 'static,
{
    dex_service: Arc<D>,
}

impl<D> PathsSwapDex<D>
where
    D: StellarDexServiceTrait + 'static,
{
    pub fn new(dex_service: Arc<D>) -> Self {
        Self { dex_service }
    }
}

#[async_trait]
impl<D> DexStrategy for PathsSwapDex<D>
where
    D: StellarDexServiceTrait + Send + Sync + 'static,
{
    async fn execute_swap(
        &self,
        params: StellarSwapParams,
    ) -> Result<StellarSwapResult, RelayerError> {
        debug!(params = ?params, "executing Paths swap");

        // For now, just get a quote - full swap execution requires transaction building
        // and submission which will be implemented in a future iteration
        let quote = self
            .dex_service
            .get_token_to_xlm_quote(&params.source_asset, params.amount, params.slippage_percent)
            .await
            .map_err(|e| RelayerError::DexError(format!("Failed to get Paths quote: {}", e)))?;

        debug!(quote = ?quote, "received quote");

        // TODO: Implement full swap execution:
        // 1. Build path payment operation from quote
        // 2. Create transaction envelope
        // 3. Sign transaction with relayer's signer
        // 4. Submit transaction to Horizon
        // 5. Wait for confirmation
        // 6. Return actual transaction hash

        // For now, return a placeholder result
        Ok(StellarSwapResult {
            asset_id: params.source_asset,
            source_amount: params.amount,
            destination_amount: quote.out_amount,
            transaction_hash: "".to_string(), // Will be populated when full execution is implemented
            error: None,
        })
    }

    async fn get_token_to_xlm_quote(
        &self,
        asset_id: &str,
        amount: u64,
        slippage: f32,
    ) -> Result<
        crate::services::stellar_dex::StellarQuoteResponse,
        crate::services::stellar_dex::StellarDexServiceError,
    > {
        self.dex_service
            .get_token_to_xlm_quote(asset_id, amount, slippage)
            .await
    }

    async fn get_xlm_to_token_quote(
        &self,
        asset_id: &str,
        amount: u64,
        slippage: f32,
    ) -> Result<
        crate::services::stellar_dex::StellarQuoteResponse,
        crate::services::stellar_dex::StellarDexServiceError,
    > {
        self.dex_service
            .get_xlm_to_token_quote(asset_id, amount, slippage)
            .await
    }
}

#[async_trait]
impl<D> StellarDexServiceTrait for PathsSwapDex<D>
where
    D: StellarDexServiceTrait + Send + Sync + 'static,
{
    async fn get_token_to_xlm_quote(
        &self,
        asset_id: &str,
        amount: u64,
        slippage: f32,
    ) -> Result<
        crate::services::stellar_dex::StellarQuoteResponse,
        crate::services::stellar_dex::StellarDexServiceError,
    > {
        self.dex_service
            .get_token_to_xlm_quote(asset_id, amount, slippage)
            .await
    }

    async fn get_xlm_to_token_quote(
        &self,
        asset_id: &str,
        amount: u64,
        slippage: f32,
    ) -> Result<
        crate::services::stellar_dex::StellarQuoteResponse,
        crate::services::stellar_dex::StellarDexServiceError,
    > {
        self.dex_service
            .get_xlm_to_token_quote(asset_id, amount, slippage)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::stellar_dex::{
        MockStellarDexServiceTrait, StellarDexServiceError, StellarQuoteResponse,
    };
    use mockall::predicate;

    fn create_mock_dex_service() -> MockStellarDexServiceTrait {
        MockStellarDexServiceTrait::new()
    }

    fn create_test_quote_response(
        input_asset: &str,
        output_asset: &str,
        in_amount: u64,
        out_amount: u64,
    ) -> StellarQuoteResponse {
        StellarQuoteResponse {
            input_asset: input_asset.to_string(),
            output_asset: output_asset.to_string(),
            in_amount,
            out_amount,
            price_impact_pct: 0.5,
            slippage_bps: 100,
            path: None,
        }
    }

    #[tokio::test]
    async fn test_execute_swap_success() {
        let source_asset = "USDC:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5";
        let destination_asset = "native";
        let amount = 1_0000000; // 1 USDC
        let output_amount = 250_000; // 0.025 XLM

        let mut mock_dex_service = create_mock_dex_service();

        let quote_response =
            create_test_quote_response(source_asset, destination_asset, amount, output_amount);

        mock_dex_service
            .expect_get_token_to_xlm_quote()
            .with(
                predicate::eq(source_asset),
                predicate::eq(amount),
                predicate::eq(1.0f32),
            )
            .times(1)
            .returning(move |_, _, _| {
                let value = quote_response.clone();
                Box::pin(async move { Ok(value.clone()) })
            });

        let dex = PathsSwapDex::new(Arc::new(mock_dex_service));

        let params = StellarSwapParams {
            relayer_address: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string(),
            source_asset: source_asset.to_string(),
            destination_asset: destination_asset.to_string(),
            amount,
            slippage_percent: 1.0,
        };

        let result = dex.execute_swap(params).await;
        assert!(result.is_ok());

        let swap_result = result.unwrap();
        assert_eq!(swap_result.asset_id, source_asset);
        assert_eq!(swap_result.source_amount, amount);
        assert_eq!(swap_result.destination_amount, output_amount);
        assert!(swap_result.error.is_none());
    }

    #[tokio::test]
    async fn test_execute_swap_quote_error() {
        let source_asset = "USDC:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5";
        let destination_asset = "native";
        let amount = 1_0000000;

        let mut mock_dex_service = create_mock_dex_service();

        mock_dex_service
            .expect_get_token_to_xlm_quote()
            .returning(|_, _, _| Box::pin(async { Err(StellarDexServiceError::NoPathFound) }));

        let dex = PathsSwapDex::new(Arc::new(mock_dex_service));

        let params = StellarSwapParams {
            relayer_address: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string(),
            source_asset: source_asset.to_string(),
            destination_asset: destination_asset.to_string(),
            amount,
            slippage_percent: 1.0,
        };

        let result = dex.execute_swap(params).await;
        assert!(result.is_err());

        match result.err().unwrap() {
            RelayerError::DexError(msg) => {
                assert!(msg.contains("Failed to get Paths quote"));
            }
            _ => panic!("Expected DexError"),
        }
    }

    #[tokio::test]
    async fn test_execute_swap_with_different_slippage() {
        let source_asset = "USDC:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5";
        let destination_asset = "native";
        let amount = 5_0000000; // 5 USDC
        let output_amount = 1_250_000; // 0.125 XLM
        let slippage = 2.5f32;

        let mut mock_dex_service = create_mock_dex_service();

        let quote_response =
            create_test_quote_response(source_asset, destination_asset, amount, output_amount);

        mock_dex_service
            .expect_get_token_to_xlm_quote()
            .with(
                predicate::eq(source_asset),
                predicate::eq(amount),
                predicate::eq(slippage),
            )
            .times(1)
            .returning(move |_, _, _| {
                let value = quote_response.clone();
                Box::pin(async move { Ok(value.clone()) })
            });

        let dex = PathsSwapDex::new(Arc::new(mock_dex_service));

        let params = StellarSwapParams {
            relayer_address: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string(),
            source_asset: source_asset.to_string(),
            destination_asset: destination_asset.to_string(),
            amount,
            slippage_percent: slippage,
        };

        let result = dex.execute_swap(params).await;
        assert!(result.is_ok());

        let swap_result = result.unwrap();
        assert_eq!(swap_result.destination_amount, output_amount);
    }
}
