//! Default gas price fetcher using standard EVM methods.
//!
//! This module provides the fallback gas price fetcher strategy that works
//! with any EVM-compatible network using the standard `eth_gasPrice` RPC method.

use crate::{
    models::EvmNetwork,
    services::provider::{evm::EvmProviderTrait, ProviderError},
};

/// Universal gas price fetcher using standard EVM RPC methods.
#[derive(Debug, Clone)]
pub struct DefaultGasPriceFetcher;

impl DefaultGasPriceFetcher {
    /// Fetches gas price using `eth_gasPrice`.
    pub async fn fetch_gas_price<P: EvmProviderTrait>(
        &self,
        provider: &P,
        _network: &EvmNetwork,
    ) -> Result<Option<u128>, ProviderError> {
        match provider.get_gas_price().await {
            Ok(gas_price) => Ok(Some(gas_price)),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::provider::evm::MockEvmProviderTrait;
    use futures::FutureExt;

    #[tokio::test]
    async fn test_default_fetcher_success() {
        let mut mock_provider = MockEvmProviderTrait::new();
        mock_provider
            .expect_get_gas_price()
            .times(1)
            .returning(|| async { Ok(20_000_000_000u128) }.boxed());

        let fetcher = DefaultGasPriceFetcher;
        let network = EvmNetwork {
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
        };

        let result = fetcher.fetch_gas_price(&mock_provider, &network).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Some(20_000_000_000u128));
    }

    #[tokio::test]
    async fn test_default_estimator_failure() {
        let mut mock_provider = MockEvmProviderTrait::new();
        mock_provider
            .expect_get_gas_price()
            .times(1)
            .returning(|| async { Err(ProviderError::Timeout) }.boxed());

        let fetcher = DefaultGasPriceFetcher;
        let network = EvmNetwork {
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
        };

        let result = fetcher.fetch_gas_price(&mock_provider, &network).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ProviderError::Timeout));
    }

    #[tokio::test]
    async fn test_default_estimator_network_error() {
        let mut mock_provider = MockEvmProviderTrait::new();
        mock_provider.expect_get_gas_price().times(1).returning(|| {
            async { Err(ProviderError::Other("Connection refused".to_string())) }.boxed()
        });

        let fetcher = DefaultGasPriceFetcher;
        let network = EvmNetwork {
            network: "polygon".to_string(),
            rpc_urls: vec!["https://polygon-rpc.com".to_string()],
            explorer_urls: None,
            average_blocktime_ms: 2000,
            is_testnet: false,
            tags: vec![],
            chain_id: 137,
            required_confirmations: 10,
            features: vec!["eip1559".to_string()],
            symbol: "MATIC".to_string(),
            gas_price_cache: None,
        };

        let result = fetcher.fetch_gas_price(&mock_provider, &network).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ProviderError::Other(_)));
    }

    #[tokio::test]
    async fn test_default_estimator_ethereum() {
        let mut mock_provider = MockEvmProviderTrait::new();
        mock_provider
            .expect_get_gas_price()
            .times(1)
            .returning(|| async { Ok(1_000_000_000u128) }.boxed());

        let fetcher = DefaultGasPriceFetcher;
        let network = EvmNetwork {
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
        };

        let result = fetcher.fetch_gas_price(&mock_provider, &network).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Some(1_000_000_000u128));
    }

    #[tokio::test]
    async fn test_default_estimator_polygon() {
        let mut mock_provider = MockEvmProviderTrait::new();
        mock_provider
            .expect_get_gas_price()
            .times(1)
            .returning(|| async { Ok(137_000_000_000u128) }.boxed());

        let fetcher = DefaultGasPriceFetcher;
        let network = EvmNetwork {
            network: "polygon".to_string(),
            rpc_urls: vec!["https://polygon-rpc.com".to_string()],
            explorer_urls: None,
            average_blocktime_ms: 2000,
            is_testnet: false,
            tags: vec![],
            chain_id: 137,
            required_confirmations: 10,
            features: vec!["eip1559".to_string()],
            symbol: "MATIC".to_string(),
            gas_price_cache: None,
        };

        let result = fetcher.fetch_gas_price(&mock_provider, &network).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Some(137_000_000_000u128));
    }
}
