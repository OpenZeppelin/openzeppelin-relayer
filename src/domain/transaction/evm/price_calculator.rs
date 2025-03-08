use super::TransactionPriceParams;
use crate::{
    models::{
        evm::Speed, EvmNetwork, EvmTransactionData, EvmTransactionDataTrait, RelayerRepoModel,
        TransactionError,
    },
    services::{EvmGasPriceService, EvmGasPriceServiceTrait, EvmProviderTrait},
};

type GasPriceCapResult = (Option<u128>, Option<u128>, Option<u128>);

/// EIP1559 gas price calculation constants
const MINUTE_AND_HALF_MS: f64 = 90000.0;
const BASE_FEE_INCREASE_FACTOR: f64 = 1.125; // 12.5% increase per block
const MAX_BASE_FEE_MULTIPLIER: f64 = 10.0;

/// Calculates gas prices for different transaction types:
/// - Legacy transactions (gas_price)
/// - EIP1559 transactions (max_fee_per_gas, max_priority_fee_per_gas)
/// - Speed-based transactions (automatically chooses between legacy and EIP1559)
pub struct PriceCalculator;

impl PriceCalculator {
    pub async fn get_transaction_price_params<P: EvmProviderTrait>(
        tx_data: &EvmTransactionData,
        relayer: &RelayerRepoModel,
        gas_price_service: &EvmGasPriceService<P>,
        provider: &P,
    ) -> Result<TransactionPriceParams, TransactionError> {
        let price_params;

        if tx_data.is_legacy() {
            price_params = Self::handle_legacy_transaction(tx_data)?;
        } else if tx_data.is_eip1559() {
            price_params = Self::handle_eip1559_transaction(tx_data)?;
        } else if tx_data.is_speed() {
            price_params =
                Self::handle_speed_transaction(tx_data, relayer, gas_price_service).await?;
        } else {
            return Err(TransactionError::NotSupported(
                "Invalid transaction type".to_string(),
            ));
        }

        let (gas_price_capped, max_fee_per_gas_capped, max_priority_fee_per_gas_capped) =
            Self::apply_gas_price_cap(
                price_params.gas_price.unwrap_or_default(),
                price_params.max_fee_per_gas,
                price_params.max_priority_fee_per_gas,
                relayer,
            )?;

        let balance = provider
            .get_balance(&tx_data.from)
            .await
            .map_err(|e| TransactionError::UnexpectedError(e.to_string()))?;

        Ok(TransactionPriceParams {
            gas_price: gas_price_capped,
            max_fee_per_gas: max_fee_per_gas_capped,
            max_priority_fee_per_gas: max_priority_fee_per_gas_capped,
            balance: Some(balance),
        })
    }

    fn handle_legacy_transaction(
        tx_data: &EvmTransactionData,
    ) -> Result<PriceParams, TransactionError> {
        let gas_price = tx_data.gas_price.ok_or(TransactionError::NotSupported(
            "Gas price is required for legacy transactions".to_string(),
        ))?;

        Ok(PriceParams {
            gas_price: Some(gas_price),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
        })
    }

    fn handle_eip1559_transaction(
        tx_data: &EvmTransactionData,
    ) -> Result<PriceParams, TransactionError> {
        let max_fee = tx_data
            .max_fee_per_gas
            .ok_or(TransactionError::NotSupported(
                "Max fee per gas is required for EIP1559 transactions".to_string(),
            ))?;

        let max_priority_fee =
            tx_data
                .max_priority_fee_per_gas
                .ok_or(TransactionError::NotSupported(
                    "Max priority fee per gas is required for EIP1559 transactions".to_string(),
                ))?;

        Ok(PriceParams {
            gas_price: None,
            max_fee_per_gas: Some(max_fee),
            max_priority_fee_per_gas: Some(max_priority_fee),
        })
    }

    async fn handle_speed_transaction<P: EvmProviderTrait>(
        tx_data: &EvmTransactionData,
        relayer: &RelayerRepoModel,
        gas_price_service: &EvmGasPriceService<P>,
    ) -> Result<PriceParams, TransactionError> {
        let speed = tx_data
            .speed
            .as_ref()
            .ok_or(TransactionError::NotSupported(
                "Speed is required".to_string(),
            ))?;

        if relayer.policies.get_evm_policy().eip1559_pricing {
            Self::handle_eip1559_speed(speed, gas_price_service).await
        } else {
            Self::handle_legacy_speed(speed, gas_price_service).await
        }
    }

    async fn handle_eip1559_speed<P: EvmProviderTrait>(
        speed: &Speed,
        gas_price_service: &EvmGasPriceService<P>,
    ) -> Result<PriceParams, TransactionError> {
        let prices = gas_price_service.get_prices_from_json_rpc().await?;
        let (max_fee, max_priority_fee) = prices
            .into_iter()
            .find(|(s, _, _)| s == speed)
            .map(|(_, max_priority_fee_wei, base_fee_wei)| {
                let network = gas_price_service.network();
                let max_fee =
                    calculate_max_fee_per_gas(base_fee_wei, max_priority_fee_wei, network);
                (max_fee, max_priority_fee_wei)
            })
            .ok_or(TransactionError::UnexpectedError(
                "Speed not supported for EIP1559".to_string(),
            ))?;

        Ok(PriceParams {
            gas_price: None,
            max_fee_per_gas: Some(max_fee),
            max_priority_fee_per_gas: Some(max_priority_fee),
        })
    }

    async fn handle_legacy_speed<P: EvmProviderTrait>(
        speed: &Speed,
        gas_price_service: &EvmGasPriceService<P>,
    ) -> Result<PriceParams, TransactionError> {
        let prices = gas_price_service.get_legacy_prices_from_json_rpc().await?;
        let gas_price = prices
            .into_iter()
            .find(|(s, _)| s == speed)
            .map(|(_, price)| price)
            .ok_or(TransactionError::NotSupported(
                "Speed not supported".to_string(),
            ))?;

        Ok(PriceParams {
            gas_price: Some(gas_price),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
        })
    }

    fn apply_gas_price_cap(
        gas_price: u128,
        max_fee_per_gas: Option<u128>,
        max_priority_fee_per_gas: Option<u128>,
        relayer: &RelayerRepoModel,
    ) -> Result<GasPriceCapResult, TransactionError> {
        let gas_price_cap = relayer
            .policies
            .get_evm_policy()
            .gas_price_cap
            .unwrap_or(u128::MAX);

        let is_eip1559 = max_fee_per_gas.is_some() && max_priority_fee_per_gas.is_some();

        if is_eip1559 {
            let max_fee = max_fee_per_gas.unwrap();
            let max_priority_fee: u128 = max_priority_fee_per_gas.unwrap();

            // Cap the maxFeePerGas
            let capped_max_fee = std::cmp::min(gas_price_cap, max_fee);

            // Ensure maxPriorityFeePerGas < maxFeePerGas to avoid client errors
            let capped_max_priority_fee = std::cmp::min(capped_max_fee, max_priority_fee);

            Ok((None, Some(capped_max_fee), Some(capped_max_priority_fee)))
        } else {
            // Handle legacy transaction
            Ok((Some(std::cmp::min(gas_price, gas_price_cap)), None, None))
        }
    }
}

struct PriceParams {
    gas_price: Option<u128>,
    max_fee_per_gas: Option<u128>,
    max_priority_fee_per_gas: Option<u128>,
}

/// Calculate base fee multiplier for EIP1559 transactions
fn get_base_fee_multiplier(network: &EvmNetwork) -> f64 {
    let block_interval_ms = network
        .average_blocktime()
        .map(|d| d.as_millis() as f64)
        .unwrap_or(12000.0); // Fallback to mainnet block time if not specified

    let n_blocks = MINUTE_AND_HALF_MS / block_interval_ms;
    f64::min(
        BASE_FEE_INCREASE_FACTOR.powf(n_blocks),
        MAX_BASE_FEE_MULTIPLIER,
    )
}

/// Calculate max fee per gas for EIP1559 transactions (all values in wei)
fn calculate_max_fee_per_gas(
    base_fee_wei: u128,
    max_priority_fee_wei: u128,
    network: &EvmNetwork,
) -> u128 {
    let base_fee = base_fee_wei as f64;
    let multiplied_base_fee = (base_fee * get_base_fee_multiplier(network)) as u128;
    multiplied_base_fee + max_priority_fee_wei
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{EvmNamedNetwork, EvmNetwork, RelayerEvmPolicy, U256};
    use crate::services::{EvmProviderTrait, MockEvmProviderTrait};
    use alloy::rpc::types::{
        Block as BlockResponse, BlockNumberOrTag, FeeHistory, TransactionReceipt,
        TransactionRequest,
    };
    use async_trait::async_trait;
    use futures::FutureExt;
    use std::sync::Arc;

    #[async_trait]
    impl EvmProviderTrait for Arc<MockEvmProviderTrait> {
        async fn get_balance(&self, address: &str) -> eyre::Result<U256> {
            self.as_ref().get_balance(address).await
        }

        async fn get_block_number(&self) -> eyre::Result<u64> {
            self.as_ref().get_block_number().await
        }

        async fn estimate_gas(&self, tx: &EvmTransactionData) -> eyre::Result<u64> {
            self.as_ref().estimate_gas(tx).await
        }

        async fn get_gas_price(&self) -> eyre::Result<u128> {
            self.as_ref().get_gas_price().await
        }

        async fn send_transaction(&self, tx: TransactionRequest) -> eyre::Result<String> {
            self.as_ref().send_transaction(tx).await
        }

        async fn send_raw_transaction(&self, tx: &[u8]) -> eyre::Result<String> {
            self.as_ref().send_raw_transaction(tx).await
        }

        async fn health_check(&self) -> eyre::Result<bool> {
            self.as_ref().health_check().await
        }

        async fn get_transaction_count(&self, address: &str) -> eyre::Result<u64> {
            self.as_ref().get_transaction_count(address).await
        }

        async fn get_fee_history(
            &self,
            block_count: u64,
            newest_block: BlockNumberOrTag,
            reward_percentiles: Vec<f64>,
        ) -> eyre::Result<FeeHistory> {
            self.as_ref()
                .get_fee_history(block_count, newest_block, reward_percentiles)
                .await
        }

        async fn get_block_by_number(&self) -> eyre::Result<BlockResponse> {
            self.as_ref().get_block_by_number().await
        }

        async fn get_transaction_receipt(
            &self,
            tx_hash: &str,
        ) -> eyre::Result<Option<TransactionReceipt>> {
            self.as_ref().get_transaction_receipt(tx_hash).await
        }
    }

    fn create_mock_provider() -> Arc<MockEvmProviderTrait> {
        let mut mock_provider = MockEvmProviderTrait::new();
        mock_provider
            .expect_get_balance()
            .returning(|_| async { Ok(U256::from(1000000000000000000u128)) }.boxed()); // 1 ETH
        Arc::new(mock_provider)
    }

    fn create_mock_relayer() -> RelayerRepoModel {
        RelayerRepoModel {
            id: "test-relayer".to_string(),
            name: "Test Relayer".to_string(),
            network: "mainnet".to_string(),
            network_type: crate::models::NetworkType::Evm,
            address: "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266".to_string(),
            policies: crate::models::RelayerNetworkPolicy::Evm(RelayerEvmPolicy::default()),
            paused: false,
            notification_id: None,
            signer_id: "test-signer".to_string(),
            system_disabled: false,
        }
    }

    fn create_mock_gas_price_service(
        provider: Arc<MockEvmProviderTrait>,
    ) -> EvmGasPriceService<Arc<MockEvmProviderTrait>> {
        EvmGasPriceService::new(provider, EvmNetwork::from_named(EvmNamedNetwork::Mainnet))
    }

    #[tokio::test]
    async fn test_legacy_transaction() {
        let provider = create_mock_provider();
        let relayer = create_mock_relayer();
        let gas_price_service = create_mock_gas_price_service(provider.clone());

        let tx_data = EvmTransactionData {
            gas_price: Some(20000000000),
            ..Default::default()
        };

        let result = PriceCalculator::get_transaction_price_params(
            &tx_data,
            &relayer,
            &gas_price_service,
            &provider,
        )
        .await;
        assert!(result.is_ok());
        let params = result.unwrap();
        assert_eq!(params.gas_price, Some(20000000000));
        assert!(params.max_fee_per_gas.is_none());
        assert!(params.max_priority_fee_per_gas.is_none());
    }

    #[tokio::test]
    async fn test_eip1559_transaction() {
        let provider = create_mock_provider();
        let relayer = create_mock_relayer();
        let gas_price_service = create_mock_gas_price_service(provider.clone());

        let tx_data = EvmTransactionData {
            gas_price: None,
            max_fee_per_gas: Some(30000000000),
            max_priority_fee_per_gas: Some(2000000000),
            ..Default::default()
        };

        let result = PriceCalculator::get_transaction_price_params(
            &tx_data,
            &relayer,
            &gas_price_service,
            &provider,
        )
        .await;
        assert!(result.is_ok());
        let params = result.unwrap();
        assert!(params.gas_price.is_none());
        assert_eq!(params.max_fee_per_gas, Some(30000000000));
        assert_eq!(params.max_priority_fee_per_gas, Some(2000000000));
    }

    #[tokio::test]
    async fn test_speed_based_transaction() {
        let mut mock_provider = MockEvmProviderTrait::new();
        mock_provider
            .expect_get_balance()
            .returning(|_| async { Ok(U256::from(1000000000000000000u128)) }.boxed()); // 1 ETH
        mock_provider
            .expect_get_gas_price()
            .returning(|| async { Ok(20000000000) }.boxed());
        let provider = Arc::new(mock_provider);
        let relayer = create_mock_relayer();
        let gas_price_service = create_mock_gas_price_service(provider.clone());

        let tx_data = EvmTransactionData {
            gas_price: None,
            speed: Some(Speed::Fast),
            ..Default::default()
        };

        let result = PriceCalculator::get_transaction_price_params(
            &tx_data,
            &relayer,
            &gas_price_service,
            &provider,
        )
        .await;
        assert!(result.is_ok());
        let params = result.unwrap();
        assert!(
            params.gas_price.is_some()
                || (params.max_fee_per_gas.is_some() && params.max_priority_fee_per_gas.is_some())
        );
    }

    #[tokio::test]
    async fn test_invalid_transaction_type() {
        let provider = create_mock_provider();
        let relayer = create_mock_relayer();
        let gas_price_service = create_mock_gas_price_service(provider.clone());

        let tx_data = EvmTransactionData {
            gas_price: None,
            ..Default::default()
        };

        let result = PriceCalculator::get_transaction_price_params(
            &tx_data,
            &relayer,
            &gas_price_service,
            &provider,
        )
        .await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TransactionError::NotSupported(_)
        ));
    }

    #[tokio::test]
    async fn test_gas_price_cap() {
        let provider = create_mock_provider();
        let mut relayer = create_mock_relayer();
        let gas_price_service = create_mock_gas_price_service(provider.clone());

        // Update policies with new EVM policy
        let evm_policy = RelayerEvmPolicy {
            gas_price_cap: Some(10000000000),
            eip1559_pricing: true,
            ..RelayerEvmPolicy::default()
        };
        relayer.policies = crate::models::RelayerNetworkPolicy::Evm(evm_policy);

        let tx_data = EvmTransactionData {
            gas_price: Some(20000000000), // Higher than cap
            ..Default::default()
        };

        let result = PriceCalculator::get_transaction_price_params(
            &tx_data,
            &relayer,
            &gas_price_service,
            &provider,
        )
        .await;
        assert!(result.is_ok());
        let params = result.unwrap();
        assert_eq!(params.gas_price, Some(10000000000)); // Should be capped
    }
}
