//! Gas price calculation module for Ethereum transactions.
//!
//! This module provides functionality for calculating gas prices for different types of Ethereum transactions:
//! - Legacy transactions (using `gas_price`)
//! - EIP1559 transactions (using `max_fee_per_gas` and `max_priority_fee_per_gas`)
//! - Speed-based transactions (automatically choosing between legacy and EIP1559 based on network support)
//!
//! The module implements various pricing strategies and safety mechanisms:
//! - Gas price caps to protect against excessive fees
//! - Dynamic base fee calculations for EIP1559 transactions
//! - Speed-based multipliers for different transaction priorities (SafeLow, Average, Fast, Fastest)
//! - Network-specific block time considerations for fee estimations
//!
//! # Example
//! ```no_run
//! # use your_crate::{PriceCalculator, EvmTransactionData, RelayerRepoModel, EvmGasPriceService};
//! # async fn example<P: EvmProviderTrait>(
//! #     tx_data: &EvmTransactionData,
//! #     relayer: &RelayerRepoModel,
//! #     gas_price_service: &EvmGasPriceService<P>,
//! #     provider: &P
//! # ) -> Result<(), TransactionError> {
//! let price_params = PriceCalculator::get_transaction_price_params(
//!     tx_data,
//!     relayer,
//!     gas_price_service,
//!     provider
//! ).await?;
//! # Ok(())
//! # }
//! ```
//!
//! The module uses EIP1559-specific constants for calculating appropriate gas fees:
//! - Base fee increase factor: 12.5% per block
//! - Maximum base fee multiplier: 10x
//! - Time window for fee calculation: 90 seconds
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
pub struct PriceCalculator;

impl PriceCalculator {
    /// Calculates transaction price parameters based on the transaction type and network conditions.
    ///
    /// This function determines the appropriate gas pricing strategy based on the transaction type:
    /// - For legacy transactions: calculates gas_price
    /// - For EIP1559 transactions: calculates max_fee_per_gas and max_priority_fee_per_gas
    /// - For speed-based transactions: automatically chooses between legacy and EIP1559 based on network support
    ///
    /// # Arguments
    /// * `tx_data` - Transaction data containing type and pricing information
    /// * `relayer` - Relayer configuration including pricing policies and caps
    /// * `gas_price_service` - Service for fetching current gas prices from the network
    /// * `provider` - Network provider for accessing blockchain data
    ///
    /// # Returns
    /// * `Result<TransactionPriceParams, TransactionError>` - Calculated price parameters or error
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

    /// Handles gas price calculation for legacy transactions.
    ///
    /// # Arguments
    /// * `tx_data` - Transaction data containing the gas price
    ///
    /// # Returns
    /// * `Result<PriceParams, TransactionError>` - Price parameters for legacy transaction
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

    /// Handles gas price calculation for EIP1559 transactions.
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

    /// Handles gas price calculation for speed-based transactions.
    ///
    /// Determines whether to use legacy or EIP1559 pricing based on network configuration
    /// and calculates appropriate gas prices based on the requested speed.
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

    /// Calculates EIP1559 gas prices based on the requested speed.
    ///
    /// Uses the gas price service to fetch current network conditions and calculates
    /// appropriate max fee and priority fee based on the speed setting.
    async fn handle_eip1559_speed<P: EvmGasPriceServiceTrait>(
        speed: &Speed,
        gas_price_service: &P,
    ) -> Result<PriceParams, TransactionError> {
        let prices = gas_price_service.get_prices_from_json_rpc().await?;
        let (max_fee, max_priority_fee) = prices
            .clone()
            .into_iter()
            .find(|(s, _, _)| s == speed)
            .map(|(_speed, _max_fee, max_priority_fee_wei)| {
                let network = gas_price_service.network();
                let max_fee = calculate_max_fee_per_gas(
                    prices.base_fee_per_gas,
                    max_priority_fee_wei,
                    network,
                );
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

    /// Calculates legacy gas prices based on the requested speed.
    ///
    /// Uses the gas price service to fetch current gas prices and applies
    /// speed-based multipliers for legacy transactions.
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

    /// Applies gas price caps to the calculated prices.
    ///
    /// Ensures that gas prices don't exceed the configured maximum limits and
    /// maintains proper relationships between different price parameters.
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

#[derive(Debug, Clone)]
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
        .unwrap();

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
    use crate::services::gas::MockEvmGasPriceServiceTrait;
    use crate::services::{EvmProviderTrait, GasPrices, MockEvmProviderTrait, SpeedPrices};
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

    #[test]
    fn test_get_base_fee_multiplier() {
        // Test with mainnet (12s block time)
        let mainnet = EvmNetwork::from_named(EvmNamedNetwork::Mainnet);
        let multiplier = get_base_fee_multiplier(&mainnet);
        // Expected blocks in 90s with 12s block time = 7.5 blocks
        // 1.125^7.5 ≈ 2.4
        assert!(multiplier > 2.3 && multiplier < 2.5);

        // Test with Optimism (2s block time)
        let optimism = EvmNetwork::from_named(EvmNamedNetwork::Optimism);
        let multiplier = get_base_fee_multiplier(&optimism);
        // Expected blocks in 90s with 2s block time = 45 blocks
        // Should be capped at MAX_BASE_FEE_MULTIPLIER (10.0)
        assert_eq!(multiplier, MAX_BASE_FEE_MULTIPLIER);

        // Test with custom network (no block time specified)
        let custom = EvmNetwork::from_id(999999);
        let multiplier = get_base_fee_multiplier(&custom);
        // Should use mainnet default (12s)
        assert!(multiplier > 2.3 && multiplier < 2.5);
    }

    #[test]
    fn test_calculate_max_fee_per_gas() {
        let network = EvmNetwork::from_named(EvmNamedNetwork::Mainnet);
        let base_fee = 100_000_000_000u128; // 100 Gwei
        let priority_fee = 2_000_000_000u128; // 2 Gwei

        let max_fee = calculate_max_fee_per_gas(base_fee, priority_fee, &network);

        // With mainnet's multiplier (~2.4):
        // base_fee * multiplier + priority_fee ≈ 100 * 2.4 + 2 ≈ 242 Gwei
        assert!(max_fee > 240_000_000_000 && max_fee < 244_000_000_000);
    }

    #[tokio::test]
    async fn test_handle_eip1559_speed() {
        let mut mock_gas_price_service = MockEvmGasPriceServiceTrait::new();

        // Mock the gas price service's get_prices_from_json_rpc method
        let test_data = [
            (Speed::SafeLow, 1_000_000_000),
            (Speed::Average, 2_000_000_000),
            (Speed::Fast, 3_000_000_000),
            (Speed::Fastest, 4_000_000_000),
        ];
        // Create mock prices
        let mock_prices = GasPrices {
            legacy_prices: SpeedPrices {
                safe_low: 10_000_000_000,
                average: 12_500_000_000,
                fast: 15_000_000_000,
                fastest: 20_000_000_000,
            },
            max_priority_fee_per_gas: SpeedPrices {
                safe_low: 1_000_000_000,
                average: 2_000_000_000,
                fast: 3_000_000_000,
                fastest: 4_000_000_000,
            },
            base_fee_per_gas: 50_000_000_000,
        };

        // Mock get_prices_from_json_rpc
        mock_gas_price_service
            .expect_get_prices_from_json_rpc()
            .returning(move || {
                let prices = mock_prices.clone();
                Box::pin(async move { Ok(prices) })
            });

        // Mock the network method
        let network = EvmNetwork::from_named(EvmNamedNetwork::Mainnet);
        mock_gas_price_service
            .expect_network()
            .return_const(network);

        for (speed, expected_priority_fee) in test_data {
            let result =
                PriceCalculator::handle_eip1559_speed(&speed, &mock_gas_price_service).await;
            assert!(result.is_ok());
            let params = result.unwrap();
            println!("speed: {:?}", speed);
            println!("params: {:?}", params);
            // Verify max_priority_fee matches expected value
            assert_eq!(params.max_priority_fee_per_gas, Some(expected_priority_fee));

            // Verify max_fee calculation
            // max_fee = base_fee * multiplier + priority_fee
            // ≈ (50 * 2.4 + priority_fee_in_gwei) Gwei
            let max_fee = params.max_fee_per_gas.unwrap();
            let expected_base_portion = 120_000_000_000; // 50 * 2.4 ≈ 120 Gwei
            assert!(max_fee > expected_base_portion + expected_priority_fee - 1_000_000_000); // Allow 1 Gwei margin
            assert!(max_fee < expected_base_portion + expected_priority_fee + 1_000_000_000);

            // Verify no legacy gas price is set
            assert!(params.gas_price.is_none());
        }
    }
}
