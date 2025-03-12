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
    services::{
        gas::{EvmGasPriceService, EvmGasPriceServiceTrait},
        provider::evm::EvmProviderTrait,
    },
};

type GasPriceCapResult = (Option<u128>, Option<u128>, Option<u128>);

// Using 10^9 precision (similar to Gwei)
const PRECISION: u128 = 1_000_000_000;
const MINUTE_AND_HALF_MS: u128 = 90000;
const BASE_FEE_INCREASE_FACTOR_PERCENT: u128 = 125; // 12.5% increase per block (as percentage * 10)
const MAX_BASE_FEE_MULTIPLIER: u128 = 10 * PRECISION; // 10.0 * PRECISION
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

/// Calculate base fee multiplier for EIP1559 transactions using fixed-point arithmetic
/// with a simplified approach that avoids complex exponentiation
fn get_base_fee_multiplier(network: &EvmNetwork) -> u128 {
    let block_interval_ms = network.average_blocktime().map(|d| d.as_millis()).unwrap();

    // Calculate number of blocks (as integer)
    let n_blocks_int = MINUTE_AND_HALF_MS / block_interval_ms;

    // Calculate number of blocks (fractional part in thousandths)
    let n_blocks_frac = ((MINUTE_AND_HALF_MS % block_interval_ms) * 1000) / block_interval_ms;

    // Calculate multiplier using compound interest formula: (1 + r)^n
    // For integer part: (1 + 0.125)^n_blocks_int
    let mut multiplier = PRECISION;

    // Calculate (1.125)^n_blocks_int using repeated multiplication
    for _ in 0..n_blocks_int {
        multiplier = (multiplier
            * (PRECISION + (PRECISION * BASE_FEE_INCREASE_FACTOR_PERCENT) / 1000))
            / PRECISION;
    }

    // Handle fractional part with linear approximation
    // For fractional part: approximately 1 + (fraction * 0.125)
    if n_blocks_frac > 0 {
        let frac_increase =
            (n_blocks_frac * BASE_FEE_INCREASE_FACTOR_PERCENT * PRECISION) / (1000 * 1000);
        multiplier = (multiplier * (PRECISION + frac_increase)) / PRECISION;
    }

    // Apply maximum cap
    std::cmp::min(multiplier, MAX_BASE_FEE_MULTIPLIER)
}

/// Calculate max fee per gas for EIP1559 transactions (all values in wei)
fn calculate_max_fee_per_gas(
    base_fee_wei: u128,
    max_priority_fee_wei: u128,
    network: &EvmNetwork,
) -> u128 {
    // Get multiplier in fixed-point format
    let multiplier = get_base_fee_multiplier(network);

    // Multiply base fee by multiplier (with proper scaling)
    let multiplied_base_fee = (base_fee_wei * multiplier) / PRECISION;

    // Add priority fee
    multiplied_base_fee + max_priority_fee_wei
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        evm::Speed, EvmNamedNetwork, EvmNetwork, EvmTransactionData, RelayerEvmPolicy,
        RelayerRepoModel, U256,
    };
    use crate::services::{
        EvmGasPriceService, GasPrices, MockEvmGasPriceServiceTrait, MockEvmProviderTrait,
        SpeedPrices,
    };
    use futures::FutureExt;

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

    #[tokio::test]
    async fn test_legacy_transaction() {
        let mut provider = MockEvmProviderTrait::new();
        provider
            .expect_get_balance()
            .returning(|_| async { Ok(U256::from(1000000000000000000u128)) }.boxed());

        let relayer = create_mock_relayer();
        let gas_price_service =
            EvmGasPriceService::new(provider, EvmNetwork::from_named(EvmNamedNetwork::Mainnet));

        let tx_data = EvmTransactionData {
            gas_price: Some(20000000000),
            ..Default::default()
        };

        let mut provider = MockEvmProviderTrait::new();
        provider
            .expect_get_balance()
            .returning(|_| async { Ok(U256::from(1000000000000000000u128)) }.boxed());

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
        let mut provider = MockEvmProviderTrait::new();
        provider
            .expect_get_balance()
            .returning(|_| async { Ok(U256::from(1000000000000000000u128)) }.boxed());

        let relayer = create_mock_relayer();
        let gas_price_service =
            EvmGasPriceService::new(provider, EvmNetwork::from_named(EvmNamedNetwork::Mainnet));

        let tx_data = EvmTransactionData {
            gas_price: None,
            max_fee_per_gas: Some(30000000000),
            max_priority_fee_per_gas: Some(2000000000),
            ..Default::default()
        };

        let mut provider = MockEvmProviderTrait::new();
        provider
            .expect_get_balance()
            .returning(|_| async { Ok(U256::from(1000000000000000000u128)) }.boxed());

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
    async fn test_speed_legacy_based_transaction() {
        let mut provider = MockEvmProviderTrait::new();
        provider
            .expect_get_balance()
            .returning(|_| async { Ok(U256::from(1000000000000000000u128)) }.boxed());
        provider
            .expect_get_gas_price()
            .returning(|| async { Ok(20000000000) }.boxed());

        let relayer = create_mock_relayer();
        let gas_price_service =
            EvmGasPriceService::new(provider, EvmNetwork::from_named(EvmNamedNetwork::Celo));

        let tx_data = EvmTransactionData {
            gas_price: None,
            speed: Some(Speed::Fast),
            ..Default::default()
        };

        let mut provider = MockEvmProviderTrait::new();
        provider
            .expect_get_balance()
            .returning(|_| async { Ok(U256::from(1000000000000000000u128)) }.boxed());
        provider
            .expect_get_gas_price()
            .returning(|| async { Ok(20000000000) }.boxed());

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
        let mut provider = MockEvmProviderTrait::new();
        provider
            .expect_get_balance()
            .returning(|_| async { Ok(U256::from(1000000000000000000u128)) }.boxed());

        let relayer = create_mock_relayer();
        let gas_price_service =
            EvmGasPriceService::new(provider, EvmNetwork::from_named(EvmNamedNetwork::Mainnet));

        let tx_data = EvmTransactionData {
            gas_price: None,
            ..Default::default()
        };

        let mut provider = MockEvmProviderTrait::new();
        provider
            .expect_get_balance()
            .returning(|_| async { Ok(U256::from(1000000000000000000u128)) }.boxed());

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
        let mut provider = MockEvmProviderTrait::new();
        provider
            .expect_get_balance()
            .returning(|_| async { Ok(U256::from(1000000000000000000u128)) }.boxed());

        let mut relayer = create_mock_relayer();
        let gas_price_service =
            EvmGasPriceService::new(provider, EvmNetwork::from_named(EvmNamedNetwork::Mainnet));

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

        let mut provider = MockEvmProviderTrait::new();
        provider
            .expect_get_balance()
            .returning(|_| async { Ok(U256::from(1000000000000000000u128)) }.boxed());

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
        assert!(multiplier > 2_300_000_000 && multiplier < 2_500_000_000);

        // Test with Optimism (2s block time)
        let optimism = EvmNetwork::from_named(EvmNamedNetwork::Optimism);
        let multiplier = get_base_fee_multiplier(&optimism);
        // Expected blocks in 90s with 2s block time = 45 blocks
        // Should be capped at MAX_BASE_FEE_MULTIPLIER (10.0)
        assert_eq!(multiplier, MAX_BASE_FEE_MULTIPLIER);
    }

    #[test]
    fn test_calculate_max_fee_per_gas() {
        let network = EvmNetwork::from_named(EvmNamedNetwork::Mainnet);
        let base_fee = 100_000_000_000u128; // 100 Gwei
        let priority_fee = 2_000_000_000u128; // 2 Gwei

        let max_fee = calculate_max_fee_per_gas(base_fee, priority_fee, &network);
        println!("max_fee: {:?}", max_fee);
        // With mainnet's multiplier (~2.4):
        // base_fee * multiplier + priority_fee ≈ 100 * 2.4 + 2 ≈ 242 Gwei
        assert!(max_fee > 240_000_000_000 && max_fee < 245_000_000_000);
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
            // Verify max_priority_fee matches expected value
            assert_eq!(params.max_priority_fee_per_gas, Some(expected_priority_fee));

            // Verify max_fee calculation
            // max_fee = base_fee * multiplier + priority_fee
            // ≈ (50 * 2.4 + priority_fee_in_gwei) Gwei
            let max_fee = params.max_fee_per_gas.unwrap();
            let expected_base_portion = 120_000_000_000; // 50 * 2.4 ≈ 120 Gwei
            println!("max_fee: {:?}", max_fee);
            println!("expected_base_portion: {:?}", expected_base_portion);
            println!("expected_priority_fee: {:?}", expected_priority_fee);
            assert!(max_fee < expected_base_portion + expected_priority_fee + 2_000_000_000);
        }
    }
}
