//! Gas price fetcher system for EVM networks.
//!
//! This module provides a flexible gas price fetcher framework that supports
//! network-specific methods while maintaining a consistent interface.
//! The system automatically selects the most appropriate fetcher strategy
//! based on network characteristics.

use crate::{
    models::EvmNetwork,
    services::provider::{evm::EvmProviderTrait, ProviderError},
};
use tracing::{debug, warn};

pub mod default;
pub mod polygon_zkevm;

/// Gas price fetcher strategies for different network types.
///
/// Each variant encapsulates a specific fetcher strategy optimized for
/// particular network characteristics or requirements.
#[derive(Debug, Clone)]
pub enum GasPriceFetcher {
    /// Default EVM gas price fetcher using standard `eth_gasPrice`
    Default(default::DefaultGasPriceFetcher),
    PolygonZkEvm(polygon_zkevm::PolygonZkEvmGasPriceFetcher),
}

impl GasPriceFetcher {
    /// Fetches gas price using the encapsulated strategy.
    pub async fn fetch_gas_price<P: EvmProviderTrait>(
        &self,
        provider: &P,
        network: &EvmNetwork,
    ) -> Result<Option<u128>, ProviderError> {
        match self {
            GasPriceFetcher::Default(fetcher) => fetcher.fetch_gas_price(provider, network).await,
            GasPriceFetcher::PolygonZkEvm(fetcher) => {
                fetcher.fetch_gas_price(provider, network).await
            }
        }
    }
}

/// Factory for creating network-appropriate gas price fetchers.
pub struct GasPriceFetcherFactory;

impl GasPriceFetcherFactory {
    /// Creates the most suitable fetcher for the network.
    pub fn create_for_network(network: &EvmNetwork) -> GasPriceFetcher {
        if network.is_polygon_zkevm() {
            GasPriceFetcher::PolygonZkEvm(polygon_zkevm::PolygonZkEvmGasPriceFetcher)
        } else {
            GasPriceFetcher::Default(default::DefaultGasPriceFetcher)
        }
    }

    /// Fetches gas price using the best available method for the network.
    pub async fn fetch_gas_price<P: EvmProviderTrait>(
        provider: &P,
        network: &EvmNetwork,
    ) -> Result<u128, ProviderError> {
        let gas_price_fetcher = Self::create_for_network(network);

        match gas_price_fetcher.fetch_gas_price(provider, network).await {
            Ok(Some(gas_price)) => {
                debug!(
                    "Gas price fetched for chain_id {}: {} wei",
                    network.chain_id, gas_price
                );
                Ok(gas_price)
            }
            Ok(None) => {
                warn!(
                    "Fetcher returned None for supported network chain_id {}",
                    network.chain_id
                );
                Err(ProviderError::Other(
                    "Fetcher failed to provide gas price for supported network".to_string(),
                ))
            }
            Err(e) => {
                debug!(
                    "Gas price fetch failed for chain_id {}: {}",
                    network.chain_id, e
                );
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::POLYGON_ZKEVM_TAG;
    use crate::services::provider::evm::MockEvmProviderTrait;
    use futures::FutureExt;
    use mockall::predicate::eq;

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

    fn create_default_network() -> EvmNetwork {
        EvmNetwork {
            network: "ethereum".to_string(),
            rpc_urls: vec!["https://mainnet.infura.io".to_string()],
            explorer_urls: None,
            average_blocktime_ms: 12000,
            is_testnet: false,
            tags: vec![],
            chain_id: 1,
            required_confirmations: 12,
            features: vec!["eip1559".to_string()],
            symbol: "ETH".to_string(),
            gas_price_cache: None,
        }
    }

    #[test]
    fn test_factory_selects_zkevm_fetcher() {
        let fetcher = GasPriceFetcherFactory::create_for_network(&create_zkevm_network());
        assert!(matches!(fetcher, GasPriceFetcher::PolygonZkEvm(_)));
    }

    #[test]
    fn test_factory_selects_default_fetcher() {
        let fetcher = GasPriceFetcherFactory::create_for_network(&create_default_network());
        assert!(matches!(fetcher, GasPriceFetcher::Default(_)));
    }

    #[tokio::test]
    async fn test_enum_fetch_gas_price_default() {
        let mut mock_provider = MockEvmProviderTrait::new();
        mock_provider
            .expect_get_gas_price()
            .times(1)
            .returning(|| async { Ok(25_000_000_000u128) }.boxed());

        let fetcher = GasPriceFetcher::Default(
            crate::services::gas::fetchers::default::DefaultGasPriceFetcher,
        );
        let network = create_default_network();

        let result = fetcher.fetch_gas_price(&mock_provider, &network).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Some(25_000_000_000u128));
    }

    #[tokio::test]
    async fn test_enum_fetch_gas_price_zkevm() {
        let mut mock_provider = MockEvmProviderTrait::new();
        mock_provider
            .expect_raw_request_dyn()
            .with(
                eq("zkevm_estimateGasPrice"),
                eq(serde_json::Value::Array(vec![])),
            )
            .times(1)
            .returning(|_, _| {
                async { Ok(serde_json::Value::String("0x2540be400".to_string())) }.boxed()
            });

        let fetcher = GasPriceFetcher::PolygonZkEvm(
            crate::services::gas::fetchers::polygon_zkevm::PolygonZkEvmGasPriceFetcher,
        );
        let network = create_zkevm_network();

        let result = fetcher.fetch_gas_price(&mock_provider, &network).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Some(10_000_000_000u128));
    }

    #[tokio::test]
    async fn test_factory_fetch_gas_price_success() {
        let mut mock_provider = MockEvmProviderTrait::new();
        mock_provider
            .expect_get_gas_price()
            .times(1)
            .returning(|| async { Ok(30_000_000_000u128) }.boxed());

        let network = create_default_network();
        let result = GasPriceFetcherFactory::fetch_gas_price(&mock_provider, &network).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 30_000_000_000u128);
    }

    #[tokio::test]
    async fn test_factory_fetch_gas_price_estimator_returns_none() {
        let mut mock_provider = MockEvmProviderTrait::new();
        mock_provider
            .expect_raw_request_dyn()
            .with(
                eq("zkevm_estimateGasPrice"),
                eq(serde_json::Value::Array(vec![])),
            )
            .times(1)
            .returning(|_, _| async { Ok(serde_json::Value::Null) }.boxed());
        mock_provider
            .expect_get_gas_price()
            .times(1)
            .returning(|| async { Err(ProviderError::Timeout) }.boxed());

        let network = create_zkevm_network();
        let result = GasPriceFetcherFactory::fetch_gas_price(&mock_provider, &network).await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ProviderError::Timeout));
    }

    #[tokio::test]
    async fn test_factory_fetch_gas_price_provider_error() {
        let mut mock_provider = MockEvmProviderTrait::new();
        mock_provider.expect_get_gas_price().times(1).returning(|| {
            async { Err(ProviderError::Other("Connection failed".to_string())) }.boxed()
        });

        let network = create_default_network();
        let result = GasPriceFetcherFactory::fetch_gas_price(&mock_provider, &network).await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ProviderError::Other(_)));
    }
}
