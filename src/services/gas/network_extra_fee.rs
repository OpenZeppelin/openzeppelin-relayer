use async_trait::async_trait;

use crate::{
    models::{EvmNetwork, EvmTransactionData, TransactionError, U256},
    services::EvmProviderTrait,
};

use super::optimism_extra_fee::OptimismExtraFeeService;

#[cfg(test)]
use mockall::automock;

#[async_trait]
#[cfg_attr(test, automock)]
pub trait NetworkExtraFeeCalculatorServiceTrait {
    /// Get the extra fee for a transaction
    ///
    /// # Arguments
    ///
    /// * `tx_data` - The transaction data to get the extra fee for
    ///
    /// # Returns
    ///
    async fn get_extra_fee(&self, tx_data: &EvmTransactionData) -> Result<U256, TransactionError>;
}

/// Enum of network-specific extra fee calculators
pub enum NetworkExtraFeeCalculator<P: EvmProviderTrait> {
    /// No extra fee calculator
    None,
    /// Optimism extra fee calculator
    Optimism(OptimismExtraFeeService<P>),
    /// Test mock implementation (available only in test builds)
    #[cfg(test)]
    Mock(MockNetworkExtraFeeCalculatorServiceTrait),
}

#[async_trait]
impl<P: EvmProviderTrait + Send + Sync> NetworkExtraFeeCalculatorServiceTrait
    for NetworkExtraFeeCalculator<P>
{
    async fn get_extra_fee(&self, tx_data: &EvmTransactionData) -> Result<U256, TransactionError> {
        match self {
            Self::None => Ok(U256::ZERO),
            Self::Optimism(service) => service.get_extra_fee(tx_data).await,
            #[cfg(test)]
            Self::Mock(mock) => mock.get_extra_fee(tx_data).await,
        }
    }
}

/// Get the network extra fee calculator service
///
/// # Arguments
///
/// * `network` - The network to get the extra fee calculator service for
/// * `provider` - The provider to get the extra fee calculator service for
///
pub fn get_network_extra_fee_calculator_service<P>(
    network: EvmNetwork,
    provider: P,
) -> NetworkExtraFeeCalculator<P>
where
    P: EvmProviderTrait + 'static,
{
    if network.is_optimism() {
        NetworkExtraFeeCalculator::Optimism(OptimismExtraFeeService::new(provider))
    } else {
        NetworkExtraFeeCalculator::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::EvmNetwork;
    use crate::services::MockEvmProviderTrait;
    use alloy::primitives::Bytes;

    fn create_test_evm_network(name: &str, optimism: bool) -> EvmNetwork {
        EvmNetwork {
            network: name.to_string(),
            rpc_urls: vec!["https://optimism-rpc.com".to_string()],
            explorer_urls: None,
            average_blocktime_ms: 12000,
            is_testnet: false,
            tags: vec![if optimism { "optimism" } else { name }.to_string()],
            chain_id: if optimism { 10 } else { 42161 },
            required_confirmations: 1,
            features: vec!["eip1559".to_string()],
            symbol: "ETH".to_string(),
        }
    }

    #[test]
    fn test_get_network_extra_fee_calculator_service_for_optimism() {
        let provider = MockEvmProviderTrait::new();
        let network = create_test_evm_network("optimism", true);
        let service = get_network_extra_fee_calculator_service(network, provider);

        assert!(
            matches!(service, NetworkExtraFeeCalculator::Optimism(_)),
            "Should return an Optimism service for Optimism network"
        );
    }

    #[test]
    fn test_get_network_extra_fee_calculator_service_for_non_optimism() {
        let networks = [
            create_test_evm_network("mainnet", false),
            create_test_evm_network("arbitrum", false),
            create_test_evm_network("polygon", false),
        ];

        for network in networks {
            let provider = MockEvmProviderTrait::new();
            let service = get_network_extra_fee_calculator_service(network, provider);

            assert!(
                matches!(service, NetworkExtraFeeCalculator::None),
                "Should return None service for non-Optimism network"
            );
        }
    }

    #[tokio::test]
    async fn test_integration_with_optimism_extra_fee_service() {
        let mut mock_provider = MockEvmProviderTrait::new();

        mock_provider
            .expect_call_contract()
            .times(6) // All 6 contract calls in get_modifiers
            .returning(|_| {
                let value_bytes = U256::from(1u64).to_be_bytes::<32>();
                Box::pin(async move { Ok(Bytes::from(value_bytes.to_vec())) })
            });

        let network = create_test_evm_network("optimism", true);
        let service = get_network_extra_fee_calculator_service(network, mock_provider);

        let tx_data = EvmTransactionData {
            from: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            to: Some("0xa24Cea55A6171FbA0935c9e171c4Efe5Ba28DF91".to_string()),
            gas_price: Some(20000000000),
            value: U256::from(1000000000),
            data: Some("0x0123".to_string()),
            nonce: Some(1),
            chain_id: 10,
            gas_limit: Some(21000),
            hash: None,
            signature: None,
            speed: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            raw: None,
        };

        let extra_fee_result = service.get_extra_fee(&tx_data).await;

        assert!(
            extra_fee_result.is_ok(),
            "Should calculate extra fee without errors"
        );

        let extra_fee = extra_fee_result.unwrap();
        assert!(
            extra_fee > U256::ZERO,
            "Extra fee should be greater than zero"
        );
    }
}
