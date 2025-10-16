//! Polygon zkEVM specialized gas price fetcher.
//!
//! This module provides enhanced gas price fetching for Polygon zkEVM networks
//! using their custom `zkevm_estimateGasPrice` RPC method, with automatic fallback
//! to standard methods when the custom method is unavailable.

use crate::{
    models::EvmNetwork,
    services::provider::{evm::EvmProviderTrait, ProviderError},
};
use tracing::{debug, error, info, warn};

/// Specialized fetcher for Polygon zkEVM networks.
#[derive(Debug, Clone)]
pub struct PolygonZkEvmGasPriceFetcher;

impl PolygonZkEvmGasPriceFetcher {
    /// Fetches gas price using zkEVM-specific methods with fallback.
    pub async fn fetch_gas_price<P: EvmProviderTrait>(
        &self,
        provider: &P,
        network: &EvmNetwork,
    ) -> Result<Option<u128>, ProviderError> {
        if let Some(zkevm_price) = self.try_zkevm_fetch(provider, network).await? {
            return Ok(Some(zkevm_price));
        }
        self.fallback_to_standard(provider, network).await
    }

    /// Attempts zkEVM gas price fetch.
    async fn try_zkevm_fetch<P: EvmProviderTrait>(
        &self,
        provider: &P,
        network: &EvmNetwork,
    ) -> Result<Option<u128>, ProviderError> {
        let result = provider
            .raw_request_dyn("zkevm_estimateGasPrice", serde_json::Value::Array(vec![]))
            .await;

        match result {
            Ok(response) => self.parse_zkevm_response(response, network.chain_id),
            Err(ProviderError::RpcErrorCode { code, .. }) if code == -32601 || code == -32004 => {
                debug!(
                    "zkEVM gas price method not available for chain_id {} (error code: {})",
                    network.chain_id, code
                );
                Ok(None)
            }
            Err(e) => {
                debug!(
                    "zkEVM gas price estimation failed for chain_id {}: {}",
                    network.chain_id, e
                );
                Ok(None)
            }
        }
    }

    /// Parses zkEVM response into gas price.
    fn parse_zkevm_response(
        &self,
        response: serde_json::Value,
        chain_id: u64,
    ) -> Result<Option<u128>, ProviderError> {
        let Some(gas_price_hex) = response.as_str() else {
            warn!(
                "Invalid zkEVM gas price response format for chain_id {}",
                chain_id
            );
            return Ok(None);
        };

        match u128::from_str_radix(gas_price_hex.trim_start_matches("0x"), 16) {
            Ok(gas_price) => {
                info!(
                    "zkEVM gas price estimated for chain_id {}: {} wei",
                    chain_id, gas_price
                );
                Ok(Some(gas_price))
            }
            Err(e) => {
                warn!(
                    "Failed to parse zkEVM gas price response for chain_id {}: {}",
                    chain_id, e
                );
                Ok(None)
            }
        }
    }

    /// Falls back to standard gas price methods.
    async fn fallback_to_standard<P: EvmProviderTrait>(
        &self,
        provider: &P,
        network: &EvmNetwork,
    ) -> Result<Option<u128>, ProviderError> {
        match provider.get_gas_price().await {
            Ok(standard_price) => {
                info!(
                    "Using standard gas price fallback for zkEVM chain_id {}: {} wei",
                    network.chain_id, standard_price
                );
                Ok(Some(standard_price))
            }
            Err(e) => {
                error!(
                    "Both zkEVM and standard gas price methods failed for chain_id {}",
                    network.chain_id
                );
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{constants::POLYGON_ZKEVM_TAG, services::provider::evm::MockEvmProviderTrait};
    use futures::FutureExt;
    use mockall::predicate::*;

    fn create_zkevm_network() -> EvmNetwork {
        EvmNetwork {
            network: "polygon-zkevm".to_string(),
            rpc_urls: vec!["https://zkevm-rpc.com".to_string()],
            explorer_urls: None,
            average_blocktime_ms: 2000,
            is_testnet: false,
            tags: vec![POLYGON_ZKEVM_TAG.to_string()],
            chain_id: 1101,
            required_confirmations: 1,
            features: vec!["eip1559".to_string()],
            symbol: "ETH".to_string(),
            gas_price_cache: None,
        }
    }

    #[tokio::test]
    async fn test_zkevm_fetcher_success() {
        let mut mock_provider = MockEvmProviderTrait::new();
        mock_provider
            .expect_raw_request_dyn()
            .with(
                eq("zkevm_estimateGasPrice"),
                eq(serde_json::Value::Array(vec![])),
            )
            .times(1)
            .returning(|_, _| {
                async { Ok(serde_json::Value::String("0x174876e800".to_string())) }.boxed()
            });

        let fetcher = PolygonZkEvmGasPriceFetcher;
        let network = create_zkevm_network();

        let result = fetcher.fetch_gas_price(&mock_provider, &network).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Some(100_000_000_000u128));
    }

    #[tokio::test]
    async fn test_zkevm_estimator_method_not_available() {
        let mut mock_provider = MockEvmProviderTrait::new();
        mock_provider
            .expect_raw_request_dyn()
            .with(
                eq("zkevm_estimateGasPrice"),
                eq(serde_json::Value::Array(vec![])),
            )
            .times(1)
            .returning(|_, _| {
                async {
                    Err(ProviderError::RpcErrorCode {
                        code: -32601,
                        message: "Method not found".to_string(),
                    })
                }
                .boxed()
            });
        mock_provider
            .expect_get_gas_price()
            .times(1)
            .returning(|| async { Ok(20_000_000_000u128) }.boxed());

        let fetcher = PolygonZkEvmGasPriceFetcher;
        let network = create_zkevm_network();

        let result = fetcher.fetch_gas_price(&mock_provider, &network).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Some(20_000_000_000u128));
    }

    #[tokio::test]
    async fn test_zkevm_estimator_invalid_response() {
        let mut mock_provider = MockEvmProviderTrait::new();
        mock_provider
            .expect_raw_request_dyn()
            .with(
                eq("zkevm_estimateGasPrice"),
                eq(serde_json::Value::Array(vec![])),
            )
            .times(1)
            .returning(|_, _| {
                async { Ok(serde_json::Value::Number(serde_json::Number::from(123))) }.boxed()
            });
        mock_provider
            .expect_get_gas_price()
            .times(1)
            .returning(|| async { Ok(15_000_000_000u128) }.boxed());

        let fetcher = PolygonZkEvmGasPriceFetcher;
        let network = create_zkevm_network();

        let result = fetcher.fetch_gas_price(&mock_provider, &network).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Some(15_000_000_000u128));
    }

    #[tokio::test]
    async fn test_zkevm_estimator_invalid_hex_response() {
        let mut mock_provider = MockEvmProviderTrait::new();
        mock_provider
            .expect_raw_request_dyn()
            .with(
                eq("zkevm_estimateGasPrice"),
                eq(serde_json::Value::Array(vec![])),
            )
            .times(1)
            .returning(|_, _| {
                async { Ok(serde_json::Value::String("invalid_hex".to_string())) }.boxed()
            });
        mock_provider
            .expect_get_gas_price()
            .times(1)
            .returning(|| async { Ok(18_000_000_000u128) }.boxed());

        let fetcher = PolygonZkEvmGasPriceFetcher;
        let network = create_zkevm_network();

        let result = fetcher.fetch_gas_price(&mock_provider, &network).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Some(18_000_000_000u128));
    }

    #[tokio::test]
    async fn test_zkevm_estimator_other_error() {
        let mut mock_provider = MockEvmProviderTrait::new();
        mock_provider
            .expect_raw_request_dyn()
            .with(
                eq("zkevm_estimateGasPrice"),
                eq(serde_json::Value::Array(vec![])),
            )
            .times(1)
            .returning(|_, _| {
                async { Err(ProviderError::Other("Network timeout".to_string())) }.boxed()
            });
        mock_provider
            .expect_get_gas_price()
            .times(1)
            .returning(|| async { Ok(22_000_000_000u128) }.boxed());

        let fetcher = PolygonZkEvmGasPriceFetcher;
        let network = create_zkevm_network();

        let result = fetcher.fetch_gas_price(&mock_provider, &network).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Some(22_000_000_000u128));
    }

    #[tokio::test]
    async fn test_zkevm_estimator_both_methods_fail() {
        let mut mock_provider = MockEvmProviderTrait::new();
        mock_provider
            .expect_raw_request_dyn()
            .with(
                eq("zkevm_estimateGasPrice"),
                eq(serde_json::Value::Array(vec![])),
            )
            .times(1)
            .returning(|_, _| {
                async { Err(ProviderError::Other("zkEVM method failed".to_string())) }.boxed()
            });
        mock_provider
            .expect_get_gas_price()
            .times(1)
            .returning(|| async { Err(ProviderError::Timeout) }.boxed());

        let fetcher = PolygonZkEvmGasPriceFetcher;
        let network = create_zkevm_network();

        let result = fetcher.fetch_gas_price(&mock_provider, &network).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ProviderError::Timeout));
    }

    #[tokio::test]
    async fn test_zkevm_estimator_hex_with_0x_prefix() {
        let mut mock_provider = MockEvmProviderTrait::new();
        mock_provider
            .expect_raw_request_dyn()
            .with(
                eq("zkevm_estimateGasPrice"),
                eq(serde_json::Value::Array(vec![])),
            )
            .times(1)
            .returning(|_, _| {
                async { Ok(serde_json::Value::String("0x3b9aca00".to_string())) }.boxed()
            });

        let fetcher = PolygonZkEvmGasPriceFetcher;
        let network = create_zkevm_network();

        let result = fetcher.fetch_gas_price(&mock_provider, &network).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Some(1_000_000_000u128));
    }

    #[tokio::test]
    async fn test_zkevm_estimator_hex_without_0x_prefix() {
        let mut mock_provider = MockEvmProviderTrait::new();
        mock_provider
            .expect_raw_request_dyn()
            .with(
                eq("zkevm_estimateGasPrice"),
                eq(serde_json::Value::Array(vec![])),
            )
            .times(1)
            .returning(|_, _| {
                async { Ok(serde_json::Value::String("3b9aca00".to_string())) }.boxed()
            });

        let fetcher = PolygonZkEvmGasPriceFetcher;
        let network = create_zkevm_network();

        let result = fetcher.fetch_gas_price(&mock_provider, &network).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Some(1_000_000_000u128));
    }

    #[test]
    fn test_parse_zkevm_response_valid_hex() {
        let fetcher = PolygonZkEvmGasPriceFetcher;
        let response = serde_json::Value::String("0x174876e800".to_string());

        let result = fetcher.parse_zkevm_response(response, 1101);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Some(100_000_000_000u128));
    }

    #[test]
    fn test_parse_zkevm_response_invalid_format() {
        let fetcher = PolygonZkEvmGasPriceFetcher;
        let response = serde_json::Value::Number(serde_json::Number::from(123));

        let result = fetcher.parse_zkevm_response(response, 1101);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), None);
    }

    #[test]
    fn test_parse_zkevm_response_invalid_hex() {
        let fetcher = PolygonZkEvmGasPriceFetcher;
        let response = serde_json::Value::String("not_hex".to_string());

        let result = fetcher.parse_zkevm_response(response, 1101);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), None);
    }
}
