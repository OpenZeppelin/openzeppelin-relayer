//! Network-specific price parameter overrides.
//!
//! This module selects and delegates to handlers that can adjust transaction
//! price parameters for specific EVM networks when required.

#[cfg(test)]
use crate::services::gas::handlers::MockPriceHandler;
use crate::{
    domain::evm::PriceParams,
    models::{evm::EvmTransactionRequest, EvmNetwork, TransactionError},
    services::{gas::handlers::OptimismPriceHandler, EvmProvider},
};
#[derive(Clone)]
pub enum PriceParamsHandler {
    Optimism(OptimismPriceHandler<EvmProvider>),
    #[cfg(test)]
    Mock(MockPriceHandler),
}

impl PriceParamsHandler {
    /// Create a handler for the given network.
    ///
    /// Returns None for networks that don't require custom price calculations.
    pub fn for_network(network: &EvmNetwork, provider: EvmProvider) -> Option<Self> {
        if network.is_optimism() {
            Some(PriceParamsHandler::Optimism(OptimismPriceHandler::new(
                provider,
            )))
        } else {
            None
        }
    }

    /// Handle custom price parameters for a transaction.
    ///
    /// This method receives the original calculated parameters and modifies them
    /// according to the specific network's requirements.
    pub async fn handle_price_params(
        &self,
        tx: &EvmTransactionRequest,
        original_params: PriceParams,
    ) -> Result<PriceParams, TransactionError> {
        match self {
            PriceParamsHandler::Optimism(handler) => {
                handler.handle_price_params(tx, original_params).await
            }
            #[cfg(test)]
            PriceParamsHandler::Mock(handler) => {
                handler.handle_price_params(tx, original_params).await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        constants::OPTIMISM_BASED_TAG,
        models::{RpcConfig, U256},
    };
    use std::env;

    fn create_test_network_with_tags(tags: Vec<&str>) -> EvmNetwork {
        EvmNetwork {
            network: "test-network".to_string(),
            rpc_urls: vec!["https://rpc.example.com".to_string()],
            explorer_urls: None,
            average_blocktime_ms: 12000,
            is_testnet: false,
            tags: tags.into_iter().map(|s| s.to_string()).collect(),
            chain_id: 1,
            required_confirmations: 1,
            features: vec!["eip1559".to_string()],
            symbol: "ETH".to_string(),
            gas_price_cache: None,
        }
    }

    fn setup_test_env() {
        env::set_var("API_KEY", "7EF1CB7C-5003-4696-B384-C72AF8C3E15D");
        env::set_var("REDIS_URL", "redis://localhost:6379");
        env::set_var("RPC_TIMEOUT_MS", "5000");
    }

    #[test]
    fn test_price_params_handler_for_optimism() {
        setup_test_env();
        let rpc_configs = vec![RpcConfig::new("http://localhost:8545".to_string())];
        let provider = EvmProvider::new(rpc_configs, 30).expect("Failed to create EvmProvider");
        let network = create_test_network_with_tags(vec![OPTIMISM_BASED_TAG]);
        let handler = PriceParamsHandler::for_network(&network, provider);
        assert!(handler.is_some());
        assert!(
            matches!(handler, Some(PriceParamsHandler::Optimism(_))),
            "Expected Optimism handler variant"
        );
    }

    #[test]
    fn test_price_params_handler_for_non_l2() {
        setup_test_env();
        let rpc_configs = vec![RpcConfig::new("http://localhost:8545".to_string())];
        let provider = EvmProvider::new(rpc_configs, 30).expect("Failed to create EvmProvider");
        let network = create_test_network_with_tags(vec!["mainnet"]);
        let handler = PriceParamsHandler::for_network(&network, provider);
        assert!(handler.is_none());
    }

    #[tokio::test]
    async fn test_handle_price_params_with_mock_variant() {
        setup_test_env();
        let handler = PriceParamsHandler::Mock(MockPriceHandler::new());

        let tx = EvmTransactionRequest {
            to: Some("0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string()),
            value: U256::from(0u128),
            data: Some("0x".to_string()),
            gas_limit: Some(21000),
            gas_price: Some(1),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            speed: None,
            valid_until: None,
        };

        let original = PriceParams {
            gas_price: Some(1),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            is_min_bumped: None,
            extra_fee: None,
            total_cost: U256::from(0),
        };

        let result = handler.handle_price_params(&tx, original).await.unwrap();
        assert_eq!(result.extra_fee, Some(U256::from(42u128)));
        assert_eq!(result.total_cost, U256::from(42u128));
    }
}
