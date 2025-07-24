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
//! ```rust, ignore
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
use crate::{
    constants::{DEFAULT_GAS_LIMIT, DEFAULT_TRANSACTION_SPEED},
    models::{
        evm::Speed, EvmNetwork, EvmTransactionData, EvmTransactionDataTrait, RelayerRepoModel,
        TransactionError, U256,
    },
    services::{
        gas::{EvmGasPriceServiceTrait, NetworkExtraFeeCalculatorServiceTrait},
        GasPrices, NetworkExtraFeeCalculator,
    },
};

#[cfg(test)]
use mockall::automock;

#[cfg(test)]
use crate::services::gas::MockNetworkExtraFeeCalculatorServiceTrait;

#[async_trait::async_trait]
#[cfg_attr(test, automock)]
pub trait PriceCalculatorTrait: Send + Sync {
    async fn get_transaction_price_params(
        &self,
        tx_data: &EvmTransactionData,
        relayer: &RelayerRepoModel,
    ) -> Result<PriceParams, TransactionError>;

    async fn calculate_bumped_gas_price(
        &self,
        tx_data: &EvmTransactionData,
        relayer: &RelayerRepoModel,
    ) -> Result<PriceParams, TransactionError>;
}

type GasPriceCapResult = (Option<u128>, Option<u128>, Option<u128>);

const PRECISION: u128 = 1_000_000_000; // 10^9 (similar to Gwei)
const MINUTE_AND_HALF_MS: u128 = 90000;
const BASE_FEE_INCREASE_FACTOR_PERCENT: u128 = 125; // 12.5% increase per block (as percentage * 10)
const MAX_BASE_FEE_MULTIPLIER: u128 = 10 * PRECISION; // 10.0 * PRECISION

#[derive(Debug, Clone)]
pub struct PriceParams {
    pub gas_price: Option<u128>,
    pub max_fee_per_gas: Option<u128>,
    pub max_priority_fee_per_gas: Option<u128>,
    pub is_min_bumped: Option<bool>,
    pub extra_fee: Option<u128>,
    pub total_cost: U256,
}

impl PriceParams {
    pub fn calculate_total_cost(&self, is_eip1559: bool, gas_limit: u64, value: U256) -> U256 {
        match is_eip1559 {
            true => {
                U256::from(self.max_fee_per_gas.unwrap_or(0)) * U256::from(gas_limit)
                    + value
                    + U256::from(self.extra_fee.unwrap_or(0))
            }
            false => {
                U256::from(self.gas_price.unwrap_or(0)) * U256::from(gas_limit)
                    + value
                    + U256::from(self.extra_fee.unwrap_or(0))
            }
        }
    }
}

/// Safely calculates the minimum required gas price for a replacement transaction.
/// Uses saturating arithmetic to prevent overflow and maintains precision.
///
/// # Arguments
///
/// * `base_price` - The original gas price to calculate bump from
///
/// # Returns
///
/// The minimum required price for replacement, or `u128::MAX` if overflow would occur.
pub fn calculate_min_bump(base_price: u128) -> u128 {
    // Convert MIN_BUMP_FACTOR to a rational representation to avoid floating point precision issues
    // MIN_BUMP_FACTOR = 1.1 = 11/10
    const BUMP_NUMERATOR: u128 = 11;
    const BUMP_DENOMINATOR: u128 = 10;

    let bumped_price = base_price
        .saturating_mul(BUMP_NUMERATOR)
        .saturating_div(BUMP_DENOMINATOR);

    // Ensure we always bump by at least 1 wei to guarantee replacement
    std::cmp::max(bumped_price, base_price.saturating_add(1))
}

/// Primary struct for calculating gas prices with an injected `EvmGasPriceServiceTrait`.
pub struct PriceCalculator<G: EvmGasPriceServiceTrait> {
    gas_price_service: G,
    network_extra_fee_calculator_service: NetworkExtraFeeCalculator<G::Provider>,
}

#[async_trait::async_trait]
impl<G: EvmGasPriceServiceTrait + Send + Sync> PriceCalculatorTrait for PriceCalculator<G> {
    async fn get_transaction_price_params(
        &self,
        tx_data: &EvmTransactionData,
        relayer: &RelayerRepoModel,
    ) -> Result<PriceParams, TransactionError> {
        self.get_transaction_price_params(tx_data, relayer).await
    }

    async fn calculate_bumped_gas_price(
        &self,
        tx_data: &EvmTransactionData,
        relayer: &RelayerRepoModel,
    ) -> Result<PriceParams, TransactionError> {
        self.calculate_bumped_gas_price(tx_data, relayer).await
    }
}

impl<G: EvmGasPriceServiceTrait> PriceCalculator<G> {
    pub fn new(
        gas_price_service: G,
        network_extra_fee_calculator_service: NetworkExtraFeeCalculator<G::Provider>,
    ) -> Self {
        Self {
            gas_price_service,
            network_extra_fee_calculator_service,
        }
    }

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
    /// * `Result<PriceParams, TransactionError>` - Calculated price parameters or error
    pub async fn get_transaction_price_params(
        &self,
        tx_data: &EvmTransactionData,
        relayer: &RelayerRepoModel,
    ) -> Result<PriceParams, TransactionError> {
        let price_params = self
            .fetch_price_params_based_on_tx_type(tx_data, relayer)
            .await?;
        let (gas_price_capped, max_fee_per_gas_capped, max_priority_fee_per_gas_capped) = self
            .apply_gas_price_cap(
                price_params.gas_price.unwrap_or_default(),
                price_params.max_fee_per_gas,
                price_params.max_priority_fee_per_gas,
                relayer,
            )?;

        let mut final_params = PriceParams {
            gas_price: gas_price_capped,
            max_fee_per_gas: max_fee_per_gas_capped,
            max_priority_fee_per_gas: max_priority_fee_per_gas_capped,
            is_min_bumped: None,
            extra_fee: None,
            total_cost: U256::ZERO,
        };

        match &self.network_extra_fee_calculator_service {
            NetworkExtraFeeCalculator::None => {}
            _ => {
                let new_tx = tx_data.clone().with_price_params(final_params.clone());
                let extra_fee = self
                    .network_extra_fee_calculator_service
                    .get_extra_fee(&new_tx)
                    .await?;
                final_params.extra_fee = Some(extra_fee.try_into().unwrap_or(0));
            }
        }

        final_params.total_cost = final_params.calculate_total_cost(
            tx_data.is_eip1559(),
            tx_data.gas_limit.unwrap_or(DEFAULT_GAS_LIMIT), // Use default gas limit if not provided
            U256::from(tx_data.value),
        );

        Ok(final_params)
    }

    /// Computes bumped gas price for transaction resubmission, factoring in network conditions.
    ///
    /// This refactor breaks the logic into smaller helper functions for clarity and testability.
    /// Each helper is commented to show how the final gas parameters are derived.
    ///
    /// 1. Determine if the transaction is EIP1559 or Legacy.
    /// 2. Calculate minimum bump requirements (e.g., +10%).
    /// 3. Compare with current network prices to decide how much to bump.
    /// 4. Apply any relayer gas price caps.
    /// 5. Return the final bumped gas parameters.
    ///
    /// The returned PriceParams includes an is_min_bumped flag that indicates whether
    /// the calculated gas parameters meet the minimum bump requirements.
    pub async fn calculate_bumped_gas_price(
        &self,
        tx_data: &EvmTransactionData,
        relayer: &RelayerRepoModel,
    ) -> Result<PriceParams, TransactionError> {
        // Check if network lacks mempool (e.g., Arbitrum) - skip bump and use current prices
        if self.gas_price_service.network().lacks_mempool()
            || self.gas_price_service.network().is_arbitrum()
        {
            let mut price_params = self.get_transaction_price_params(tx_data, relayer).await?;

            // For mempool-less networks, we don't need to bump - just use current market prices
            price_params.is_min_bumped = Some(true);
            return Ok(price_params);
        }

        let network_gas_prices = self.gas_price_service.get_prices_from_json_rpc().await?;
        let relayer_gas_price_cap = relayer
            .policies
            .get_evm_policy()
            .gas_price_cap
            .unwrap_or(u128::MAX);

        // Decide EIP1559 vs Legacy based on presence of maxFeePerGas / maxPriorityFeePerGas vs gasPrice
        let bumped_price_params = match (
            tx_data.max_fee_per_gas,
            tx_data.max_priority_fee_per_gas,
            tx_data.gas_price,
        ) {
            (Some(max_fee), Some(max_priority_fee), _) => {
                // EIP1559
                self.handle_eip1559_bump(
                    &network_gas_prices,
                    relayer_gas_price_cap,
                    tx_data.speed.as_ref(),
                    max_fee,
                    max_priority_fee,
                )?
            }
            (None, None, Some(gas_price)) => {
                // Legacy
                self.handle_legacy_bump(
                    &network_gas_prices,
                    relayer_gas_price_cap,
                    tx_data.speed.as_ref(),
                    gas_price,
                )?
            }
            _ => {
                return Err(TransactionError::InvalidType(
                    "Transaction missing required gas price parameters".to_string(),
                ))
            }
        };

        // Add extra fee if needed
        let mut final_params = bumped_price_params;
        let value = tx_data.value;
        let gas_limit = tx_data.gas_limit;
        let is_eip1559 = tx_data.is_eip1559();

        match &self.network_extra_fee_calculator_service {
            NetworkExtraFeeCalculator::None => {}
            _ => {
                let new_tx = tx_data.clone().with_price_params(final_params.clone());
                let extra_fee = self
                    .network_extra_fee_calculator_service
                    .get_extra_fee(&new_tx)
                    .await?;
                final_params.extra_fee = Some(extra_fee.try_into().unwrap_or(0));
            }
        }

        final_params.total_cost = final_params.calculate_total_cost(
            is_eip1559,
            gas_limit.unwrap_or(DEFAULT_GAS_LIMIT),
            U256::from(value),
        );

        Ok(final_params)
    }

    /// Computes the bumped gas parameters for an EIP-1559 transaction resubmission.
    ///
    /// The function performs the following steps:
    /// 1. Computes the minimum required fee values by increasing the previous fees by 10%.
    /// 2. Retrieves the current network market priority fee for the transaction's speed.
    /// 3. Chooses the new priority fee as either the current market fee (if it meets the 10% increase)
    ///    or the calculated minimum bump.
    /// 4. Computes the new maximum fee using two approaches:
    ///    - Method A: Uses the current base fee, ensuring it meets the minimum bumped max fee.
    ///    - Method B: Computes a recommended max fee based on a network-specific multiplier plus the new priority fee.
    ///      The higher value between these two methods is chosen.
    /// 5. Applies the relayer's gas price cap to both the new priority fee and the new max fee.
    /// 6. Returns the final capped gas parameters.
    ///
    /// Note: All fee values are expected to be in Wei.
    fn handle_eip1559_bump(
        &self,
        network_gas_prices: &GasPrices,
        gas_price_cap: u128,
        maybe_speed: Option<&Speed>,
        max_fee: u128,
        max_priority_fee: u128,
    ) -> Result<PriceParams, TransactionError> {
        let speed = maybe_speed.unwrap_or(&DEFAULT_TRANSACTION_SPEED);

        // Calculate the minimum required fees (10% increase over previous values)
        let min_bump_max_fee = calculate_min_bump(max_fee);
        let min_bump_max_priority = calculate_min_bump(max_priority_fee);

        // Get the current market priority fee for the given speed.
        let current_market_priority =
            Self::get_market_price_for_speed(network_gas_prices, true, speed);

        // Determine the new maxPriorityFeePerGas:
        // Use the current market fee if it is at least the minimum bumped fee,
        // otherwise use the minimum bumped priority fee.
        let bumped_priority_fee = if current_market_priority >= min_bump_max_priority {
            current_market_priority
        } else {
            min_bump_max_priority
        };

        // Compute the new maxFeePerGas using two methods:
        // Method A: Use the current base fee, but ensure it is not lower than the minimum bumped max fee.
        let base_fee_wei = network_gas_prices.base_fee_per_gas;
        let bumped_max_fee_per_gas = if base_fee_wei >= min_bump_max_fee {
            base_fee_wei
        } else {
            min_bump_max_fee
        };

        // Method B: Calculate a recommended max fee based on the base fee multiplied by a network factor,
        // plus the new priority fee.
        let recommended_max_fee_per_gas = calculate_max_fee_per_gas(
            base_fee_wei,
            bumped_priority_fee,
            self.gas_price_service.network(),
        );

        // Choose the higher value from the two methods to be competitive under current network conditions.
        let final_max_fee = std::cmp::max(bumped_max_fee_per_gas, recommended_max_fee_per_gas);

        // Step 5: Apply the gas price cap to both the new priority fee and the new max fee.
        let capped_priority = Self::cap_gas_price(bumped_priority_fee, gas_price_cap);
        let capped_max_fee = Self::cap_gas_price(final_max_fee, gas_price_cap);

        // Check if the capped values still meet the minimum bump requirements
        let is_min_bumped =
            capped_priority >= min_bump_max_priority && capped_max_fee >= min_bump_max_fee;

        // Step 6: Return the final bumped gas parameters.
        Ok(PriceParams {
            gas_price: None,
            max_priority_fee_per_gas: Some(capped_priority),
            max_fee_per_gas: Some(capped_max_fee),
            is_min_bumped: Some(is_min_bumped),
            extra_fee: None,
            total_cost: U256::ZERO,
        })
    }

    /// Handle Legacy bump logic:
    /// 1) Calculate min bump for gasPrice.
    /// 2) Compare with current market price for the given speed.
    /// 3) Apply final caps.
    fn handle_legacy_bump(
        &self,
        network_gas_prices: &GasPrices,
        gas_price_cap: u128,
        maybe_speed: Option<&Speed>,
        gas_price: u128,
    ) -> Result<PriceParams, TransactionError> {
        let speed = maybe_speed.unwrap_or(&Speed::Fast);

        // Minimum bump
        let min_bump_gas_price = calculate_min_bump(gas_price);

        // Current market gas price for chosen speed
        let current_market_price =
            Self::get_market_price_for_speed(network_gas_prices, false, speed);

        let bumped_gas_price = if current_market_price >= min_bump_gas_price {
            current_market_price
        } else {
            min_bump_gas_price
        };

        // Cap
        let capped_gas_price = Self::cap_gas_price(bumped_gas_price, gas_price_cap);

        // Check if the capped value still meets the minimum bump requirement
        let is_min_bumped = capped_gas_price >= min_bump_gas_price;

        Ok(PriceParams {
            gas_price: Some(capped_gas_price),
            max_priority_fee_per_gas: None,
            max_fee_per_gas: None,
            is_min_bumped: Some(is_min_bumped),
            extra_fee: None,
            total_cost: U256::ZERO,
        })
    }
    /// Fetches price params based on the type of transaction (legacy, EIP1559, speed-based).
    async fn fetch_price_params_based_on_tx_type(
        &self,
        tx_data: &EvmTransactionData,
        relayer: &RelayerRepoModel,
    ) -> Result<PriceParams, TransactionError> {
        if tx_data.is_legacy() {
            self.fetch_legacy_price_params(tx_data)
        } else if tx_data.is_eip1559() {
            self.fetch_eip1559_price_params(tx_data)
        } else if tx_data.is_speed() {
            self.fetch_speed_price_params(tx_data, relayer).await
        } else {
            Err(TransactionError::NotSupported(
                "Invalid transaction type".to_string(),
            ))
        }
    }

    /// Handles gas price calculation for legacy transactions.
    ///
    /// # Arguments
    /// * `tx_data` - Transaction data containing the gas price
    ///
    /// # Returns
    /// * `Result<PriceParams, TransactionError>` - Price parameters for legacy transaction
    fn fetch_legacy_price_params(
        &self,
        tx_data: &EvmTransactionData,
    ) -> Result<PriceParams, TransactionError> {
        let gas_price = tx_data.gas_price.ok_or(TransactionError::NotSupported(
            "Gas price is required for legacy transactions".to_string(),
        ))?;
        Ok(PriceParams {
            gas_price: Some(gas_price),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            is_min_bumped: None,
            extra_fee: None,
            total_cost: U256::ZERO,
        })
    }

    fn fetch_eip1559_price_params(
        &self,
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
            is_min_bumped: None,
            extra_fee: None,
            total_cost: U256::ZERO,
        })
    }
    /// Handles gas price calculation for speed-based transactions.
    ///
    /// Determines whether to use legacy or EIP1559 pricing based on network configuration
    /// and calculates appropriate gas prices based on the requested speed.
    async fn fetch_speed_price_params(
        &self,
        tx_data: &EvmTransactionData,
        relayer: &RelayerRepoModel,
    ) -> Result<PriceParams, TransactionError> {
        let speed = tx_data
            .speed
            .as_ref()
            .ok_or(TransactionError::NotSupported(
                "Speed is required".to_string(),
            ))?;
        let use_legacy = relayer.policies.get_evm_policy().eip1559_pricing == Some(false)
            || self.gas_price_service.network().is_legacy();

        if use_legacy {
            self.fetch_legacy_speed_params(speed).await
        } else {
            self.fetch_eip1559_speed_params(speed).await
        }
    }

    /// Calculates EIP1559 gas prices based on the requested speed.
    ///
    /// Uses the gas price service to fetch current network conditions and calculates
    /// appropriate max fee and priority fee based on the speed setting.
    async fn fetch_eip1559_speed_params(
        &self,
        speed: &Speed,
    ) -> Result<PriceParams, TransactionError> {
        let prices = self.gas_price_service.get_prices_from_json_rpc().await?;
        let priority_fee = match speed {
            Speed::SafeLow => prices.max_priority_fee_per_gas.safe_low,
            Speed::Average => prices.max_priority_fee_per_gas.average,
            Speed::Fast => prices.max_priority_fee_per_gas.fast,
            Speed::Fastest => prices.max_priority_fee_per_gas.fastest,
        };
        let max_fee = calculate_max_fee_per_gas(
            prices.base_fee_per_gas,
            priority_fee,
            self.gas_price_service.network(),
        );
        Ok(PriceParams {
            gas_price: None,
            max_fee_per_gas: Some(max_fee),
            max_priority_fee_per_gas: Some(priority_fee),
            is_min_bumped: None,
            extra_fee: None,
            total_cost: U256::ZERO,
        })
    }
    /// Calculates legacy gas prices based on the requested speed.
    ///
    /// Uses the gas price service to fetch current gas prices and applies
    /// speed-based multipliers for legacy transactions.
    async fn fetch_legacy_speed_params(
        &self,
        speed: &Speed,
    ) -> Result<PriceParams, TransactionError> {
        let prices = self
            .gas_price_service
            .get_legacy_prices_from_json_rpc()
            .await?;
        let gas_price = match speed {
            Speed::SafeLow => prices.safe_low,
            Speed::Average => prices.average,
            Speed::Fast => prices.fast,
            Speed::Fastest => prices.fastest,
        };
        Ok(PriceParams {
            gas_price: Some(gas_price),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            is_min_bumped: None,
            extra_fee: None,
            total_cost: U256::ZERO,
        })
    }

    /// Applies gas price caps to the calculated prices.
    ///
    /// Ensures that gas prices don't exceed the configured maximum limits and
    /// maintains proper relationships between different price parameters.
    fn apply_gas_price_cap(
        &self,
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

        if let (Some(max_fee), Some(max_priority)) = (max_fee_per_gas, max_priority_fee_per_gas) {
            // Cap the maxFeePerGas
            let capped_max_fee = Self::cap_gas_price(max_fee, gas_price_cap);

            // Ensure maxPriorityFeePerGas < maxFeePerGas to avoid client errors
            let capped_max_priority = Self::cap_gas_price(max_priority, capped_max_fee);
            Ok((None, Some(capped_max_fee), Some(capped_max_priority)))
        } else {
            // Handle legacy transaction
            Ok((
                Some(Self::cap_gas_price(gas_price, gas_price_cap)),
                None,
                None,
            ))
        }
    }

    fn cap_gas_price(price: u128, cap: u128) -> u128 {
        std::cmp::min(price, cap)
    }

    /// Returns the market price for the given speed. If `is_eip1559` is true, use `max_priority_fee_per_gas`,
    /// otherwise use `legacy_prices`.
    fn get_market_price_for_speed(prices: &GasPrices, is_eip1559: bool, speed: &Speed) -> u128 {
        if is_eip1559 {
            match speed {
                Speed::SafeLow => prices.max_priority_fee_per_gas.safe_low,
                Speed::Average => prices.max_priority_fee_per_gas.average,
                Speed::Fast => prices.max_priority_fee_per_gas.fast,
                Speed::Fastest => prices.max_priority_fee_per_gas.fastest,
            }
        } else {
            match speed {
                Speed::SafeLow => prices.legacy_prices.safe_low,
                Speed::Average => prices.legacy_prices.average,
                Speed::Fast => prices.legacy_prices.fast,
                Speed::Fastest => prices.legacy_prices.fastest,
            }
        }
    }
}

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
        evm::Speed, EvmNetwork, EvmTransactionData, NetworkType, RelayerEvmPolicy,
        RelayerNetworkPolicy, RelayerRepoModel, U256,
    };
    use crate::services::{
        EvmGasPriceService, GasPrices, MockEvmGasPriceServiceTrait, MockEvmProviderTrait,
        SpeedPrices,
    };
    use futures::FutureExt;

    fn create_mock_evm_network(name: &str) -> EvmNetwork {
        let average_blocktime_ms = match name {
            "optimism" => 2000, // 2 seconds for optimism to test max cap
            _ => 12000,         // 12 seconds for mainnet and others
        };

        EvmNetwork {
            network: name.to_string(),
            rpc_urls: vec!["https://rpc.example.com".to_string()],
            explorer_urls: None,
            average_blocktime_ms,
            is_testnet: true,
            tags: vec![],
            chain_id: 1337,
            required_confirmations: 1,
            features: vec![],
            symbol: "ETH".to_string(),
        }
    }

    fn create_mock_no_mempool_network(name: &str) -> EvmNetwork {
        let average_blocktime_ms = match name {
            "arbitrum" => 1000, // 1 second for arbitrum
            _ => 12000,         // 12 seconds for others
        };

        EvmNetwork {
            network: name.to_string(),
            rpc_urls: vec!["https://rpc.example.com".to_string()],
            explorer_urls: None,
            average_blocktime_ms,
            is_testnet: true,
            tags: vec!["no-mempool".to_string()], // This makes lacks_mempool() return true
            chain_id: 42161,
            required_confirmations: 1,
            features: vec!["eip1559".to_string()], // This makes it use EIP1559 pricing
            symbol: "ETH".to_string(),
        }
    }

    fn create_mock_relayer() -> RelayerRepoModel {
        RelayerRepoModel {
            id: "test-relayer".to_string(),
            name: "Test Relayer".to_string(),
            network: "mainnet".to_string(),
            network_type: NetworkType::Evm,
            address: "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266".to_string(),
            policies: RelayerNetworkPolicy::Evm(RelayerEvmPolicy::default()),
            paused: false,
            notification_id: None,
            signer_id: "test-signer".to_string(),
            system_disabled: false,
            custom_rpc_urls: None,
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
            EvmGasPriceService::new(provider, create_mock_evm_network("mainnet"));

        let tx_data = EvmTransactionData {
            gas_price: Some(20000000000),
            ..Default::default()
        };

        let mut provider = MockEvmProviderTrait::new();
        provider
            .expect_get_balance()
            .returning(|_| async { Ok(U256::from(1000000000000000000u128)) }.boxed());

        // Create the PriceCalculator with the gas_price_service
        let pc = PriceCalculator::new(gas_price_service, NetworkExtraFeeCalculator::None);

        let result = pc.get_transaction_price_params(&tx_data, &relayer).await;
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
            EvmGasPriceService::new(provider, create_mock_evm_network("mainnet"));

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

        // Create the PriceCalculator
        let pc = PriceCalculator::new(gas_price_service, NetworkExtraFeeCalculator::None);

        let result = pc.get_transaction_price_params(&tx_data, &relayer).await;
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
        let gas_price_service = EvmGasPriceService::new(provider, create_mock_evm_network("celo"));

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

        let pc = PriceCalculator::new(gas_price_service, NetworkExtraFeeCalculator::None);

        let result = pc.get_transaction_price_params(&tx_data, &relayer).await;
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
            EvmGasPriceService::new(provider, create_mock_evm_network("mainnet"));

        let tx_data = EvmTransactionData {
            gas_price: None,
            ..Default::default()
        };

        let mut provider = MockEvmProviderTrait::new();
        provider
            .expect_get_balance()
            .returning(|_| async { Ok(U256::from(1000000000000000000u128)) }.boxed());

        let pc = PriceCalculator::new(gas_price_service, NetworkExtraFeeCalculator::None);

        let result = pc.get_transaction_price_params(&tx_data, &relayer).await;
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
            EvmGasPriceService::new(provider, create_mock_evm_network("mainnet"));

        // Update policies with new EVM policy
        let evm_policy = RelayerEvmPolicy {
            gas_price_cap: Some(10000000000),
            eip1559_pricing: Some(true),
            ..RelayerEvmPolicy::default()
        };
        relayer.policies = RelayerNetworkPolicy::Evm(evm_policy);

        let tx_data = EvmTransactionData {
            gas_price: Some(20000000000), // Higher than cap
            ..Default::default()
        };

        let mut provider = MockEvmProviderTrait::new();
        provider
            .expect_get_balance()
            .returning(|_| async { Ok(U256::from(1000000000000000000u128)) }.boxed());

        let pc = PriceCalculator::new(gas_price_service, NetworkExtraFeeCalculator::None);

        let result = pc.get_transaction_price_params(&tx_data, &relayer).await;
        assert!(result.is_ok());
        let params = result.unwrap();
        assert_eq!(params.gas_price, Some(10000000000)); // Should be capped
    }

    #[test]
    fn test_get_base_fee_multiplier() {
        let mainnet = create_mock_evm_network("mainnet");
        let multiplier = super::get_base_fee_multiplier(&mainnet);
        // 90s with ~12s blocks = ~7.5 blocks => ~2.4 multiplier
        assert!(multiplier > 2_300_000_000 && multiplier < 2_500_000_000);

        let optimism = create_mock_evm_network("optimism");
        let multiplier = super::get_base_fee_multiplier(&optimism);
        // 2s block time => ~45 blocks => capped at 10.0
        assert_eq!(multiplier, MAX_BASE_FEE_MULTIPLIER);
    }

    #[test]
    fn test_calculate_max_fee_per_gas() {
        let network = create_mock_evm_network("mainnet");
        let base_fee = 100_000_000_000u128; // 100 Gwei
        let priority_fee = 2_000_000_000u128; // 2 Gwei

        let max_fee = super::calculate_max_fee_per_gas(base_fee, priority_fee, &network);
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
        let network = create_mock_evm_network("mainnet");
        mock_gas_price_service
            .expect_network()
            .return_const(network);

        // Construct our PriceCalculator with the mocked gas service
        let pc = PriceCalculator::new(mock_gas_price_service, NetworkExtraFeeCalculator::None);

        for (speed, expected_priority_fee) in test_data {
            // Call our internal fetch_eip1559_speed_params, which replaced handle_eip1559_speed
            let result = pc.fetch_eip1559_speed_params(&speed).await;
            assert!(result.is_ok());
            let params = result.unwrap();
            // Verify max_priority_fee matches expected value
            assert_eq!(params.max_priority_fee_per_gas, Some(expected_priority_fee));

            // Verify max_fee calculation
            // max_fee = base_fee * multiplier + priority_fee
            // ≈ (50 * 2.4 + priority_fee_in_gwei) Gwei
            let max_fee = params.max_fee_per_gas.unwrap();
            let expected_base_portion = 120_000_000_000; // ~50 Gwei * 2.4
            assert!(max_fee < expected_base_portion + expected_priority_fee + 2_000_000_000);
        }
    }

    #[tokio::test]
    async fn test_calculate_bumped_gas_price_eip1559_basic() {
        let mut mock_service = MockEvmGasPriceServiceTrait::new();
        let mock_prices = GasPrices {
            legacy_prices: SpeedPrices {
                safe_low: 8_000_000_000,
                average: 10_000_000_000,
                fast: 12_000_000_000,
                fastest: 15_000_000_000,
            },
            max_priority_fee_per_gas: SpeedPrices {
                safe_low: 1_000_000_000,
                average: 2_000_000_000,
                fast: 3_000_000_000,
                fastest: 4_000_000_000,
            },
            base_fee_per_gas: 50_000_000_000,
        };
        mock_service
            .expect_get_prices_from_json_rpc()
            .returning(move || {
                let prices = mock_prices.clone();
                Box::pin(async move { Ok(prices) })
            });
        mock_service
            .expect_network()
            .return_const(create_mock_evm_network("mainnet"));

        let pc = PriceCalculator::new(mock_service, NetworkExtraFeeCalculator::None);
        let mut relayer = create_mock_relayer();
        // Example cap to demonstrate bump capping
        relayer.policies = RelayerNetworkPolicy::Evm(RelayerEvmPolicy {
            gas_price_cap: Some(300_000_000_000u128),
            ..Default::default()
        });

        let tx_data = EvmTransactionData {
            max_fee_per_gas: Some(100_000_000_000),
            max_priority_fee_per_gas: Some(2_000_000_000),
            speed: Some(Speed::Fast),
            ..Default::default()
        };

        let bumped = pc
            .calculate_bumped_gas_price(&tx_data, &relayer)
            .await
            .unwrap();
        assert!(bumped.max_fee_per_gas.unwrap() >= 110_000_000_000); // >= 10% bump
        assert!(bumped.max_priority_fee_per_gas.unwrap() >= 2_200_000_000); // >= 10% bump
    }

    #[tokio::test]
    async fn test_calculate_bumped_gas_price_eip1559_market_lower_than_min_bump() {
        let mut mock_service = MockEvmGasPriceServiceTrait::new();
        let mock_prices = GasPrices {
            legacy_prices: SpeedPrices::default(),
            max_priority_fee_per_gas: SpeedPrices {
                safe_low: 1_500_000_000, // market priority
                average: 2_500_000_000,
                fast: 2_700_000_000,
                fastest: 3_000_000_000,
            },
            base_fee_per_gas: 30_000_000_000,
        };
        mock_service
            .expect_get_prices_from_json_rpc()
            .returning(move || {
                let prices = mock_prices.clone();
                Box::pin(async move { Ok(prices) })
            });
        mock_service
            .expect_network()
            .return_const(create_mock_evm_network("mainnet"));

        let pc = PriceCalculator::new(mock_service, NetworkExtraFeeCalculator::None);
        let relayer = create_mock_relayer();

        // Old max_priority_fee: 2.0 Gwei, new market is 1.5 Gwei (less)
        // Should use min bump (2.2 Gwei) instead
        let tx_data = EvmTransactionData {
            max_fee_per_gas: Some(20_000_000_000),
            max_priority_fee_per_gas: Some(2_000_000_000),
            speed: Some(Speed::SafeLow),
            ..Default::default()
        };

        let bumped = pc
            .calculate_bumped_gas_price(&tx_data, &relayer)
            .await
            .unwrap();
        assert!(bumped.max_priority_fee_per_gas.unwrap() >= 2_200_000_000);
        assert!(bumped.max_fee_per_gas.unwrap() > 20_000_000_000);
    }

    #[tokio::test]
    async fn test_calculate_bumped_gas_price_legacy_basic() {
        let mut mock_service = MockEvmGasPriceServiceTrait::new();
        let mock_prices = GasPrices {
            legacy_prices: SpeedPrices {
                safe_low: 10_000_000_000,
                average: 12_000_000_000,
                fast: 14_000_000_000,
                fastest: 18_000_000_000,
            },
            max_priority_fee_per_gas: SpeedPrices::default(),
            base_fee_per_gas: 0,
        };
        mock_service
            .expect_get_prices_from_json_rpc()
            .returning(move || {
                let prices = mock_prices.clone();
                Box::pin(async move { Ok(prices) })
            });
        mock_service
            .expect_network()
            .return_const(create_mock_evm_network("mainnet"));

        let pc = PriceCalculator::new(mock_service, NetworkExtraFeeCalculator::None);
        let relayer = create_mock_relayer();
        let tx_data = EvmTransactionData {
            gas_price: Some(10_000_000_000),
            speed: Some(Speed::Fast),
            ..Default::default()
        };

        let bumped = pc
            .calculate_bumped_gas_price(&tx_data, &relayer)
            .await
            .unwrap();
        assert!(bumped.gas_price.unwrap() >= 11_000_000_000); // at least 10% bump
    }

    #[tokio::test]
    async fn test_calculate_bumped_gas_price_missing_params() {
        let mut mock_service = MockEvmGasPriceServiceTrait::new();

        // Add the missing expectation for get_prices_from_json_rpc
        mock_service
            .expect_get_prices_from_json_rpc()
            .times(1)
            .returning(|| Box::pin(async { Ok(GasPrices::default()) }));

        // Add the missing expectation for network
        mock_service
            .expect_network()
            .return_const(create_mock_evm_network("mainnet"));

        let pc = PriceCalculator::new(mock_service, NetworkExtraFeeCalculator::None);
        let relayer = create_mock_relayer();
        // Both max_fee_per_gas, max_priority_fee_per_gas, and gas_price absent
        let tx_data = EvmTransactionData {
            gas_price: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            ..Default::default()
        };

        let result = pc.calculate_bumped_gas_price(&tx_data, &relayer).await;
        assert!(result.is_err());
        if let Err(TransactionError::InvalidType(msg)) = result {
            assert!(msg.contains("missing required gas price parameters"));
        } else {
            panic!("Expected InvalidType error");
        }
    }

    #[tokio::test]
    async fn test_calculate_bumped_gas_price_capped() {
        let mut mock_service = MockEvmGasPriceServiceTrait::new();
        let mock_prices = GasPrices {
            legacy_prices: SpeedPrices::default(),
            max_priority_fee_per_gas: SpeedPrices {
                safe_low: 4_000_000_000,
                average: 5_000_000_000,
                fast: 6_000_000_000,
                fastest: 8_000_000_000,
            },
            base_fee_per_gas: 100_000_000_000,
        };
        mock_service
            .expect_get_prices_from_json_rpc()
            .returning(move || {
                let prices = mock_prices.clone();
                Box::pin(async move { Ok(prices) })
            });
        mock_service
            .expect_network()
            .return_const(create_mock_evm_network("mainnet"));

        let pc = PriceCalculator::new(mock_service, NetworkExtraFeeCalculator::None);
        let mut relayer = create_mock_relayer();
        relayer.policies = RelayerNetworkPolicy::Evm(RelayerEvmPolicy {
            gas_price_cap: Some(105_000_000_000),
            ..Default::default()
        });

        let tx_data = EvmTransactionData {
            max_fee_per_gas: Some(90_000_000_000),
            max_priority_fee_per_gas: Some(4_000_000_000),
            speed: Some(Speed::Fastest),
            ..Default::default()
        };

        // Normally, we'd expect ~ (100 Gwei * 2.4) + 8 Gwei > 248 Gwei. We'll cap it at 105 Gwei.
        let bumped = pc
            .calculate_bumped_gas_price(&tx_data, &relayer)
            .await
            .unwrap();
        assert!(bumped.max_fee_per_gas.unwrap() <= 105_000_000_000);
        assert!(bumped.max_priority_fee_per_gas.unwrap() <= 105_000_000_000);
    }

    #[tokio::test]
    async fn test_is_min_bumped_flag_eip1559() {
        let mut mock_service = MockEvmGasPriceServiceTrait::new();
        let mock_prices = GasPrices {
            legacy_prices: SpeedPrices::default(),
            max_priority_fee_per_gas: SpeedPrices {
                safe_low: 1_000_000_000,
                average: 2_000_000_000,
                fast: 3_000_000_000,
                fastest: 4_000_000_000,
            },
            base_fee_per_gas: 40_000_000_000,
        };
        mock_service
            .expect_get_prices_from_json_rpc()
            .returning(move || {
                let prices = mock_prices.clone();
                Box::pin(async move { Ok(prices) })
            });
        mock_service
            .expect_network()
            .return_const(create_mock_evm_network("mainnet"));

        let pc = PriceCalculator::new(mock_service, NetworkExtraFeeCalculator::None);
        let mut relayer = create_mock_relayer();

        // Case 1: Price high enough - should result in is_min_bumped = true
        relayer.policies = RelayerNetworkPolicy::Evm(RelayerEvmPolicy {
            gas_price_cap: Some(200_000_000_000u128),
            ..Default::default()
        });

        let tx_data = EvmTransactionData {
            max_fee_per_gas: Some(50_000_000_000),
            max_priority_fee_per_gas: Some(2_000_000_000),
            speed: Some(Speed::Fast),
            ..Default::default()
        };

        let bumped = pc
            .calculate_bumped_gas_price(&tx_data, &relayer)
            .await
            .unwrap();
        assert_eq!(
            bumped.is_min_bumped,
            Some(true),
            "Should be min bumped when prices are high enough"
        );

        // Case 2: Gas price cap too low - should result in is_min_bumped = false
        relayer.policies = RelayerNetworkPolicy::Evm(RelayerEvmPolicy {
            gas_price_cap: Some(50_000_000_000u128), // Cap is below the min bump for max_fee_per_gas
            ..Default::default()
        });

        let tx_data = EvmTransactionData {
            max_fee_per_gas: Some(50_000_000_000),
            max_priority_fee_per_gas: Some(2_000_000_000),
            speed: Some(Speed::Fast),
            ..Default::default()
        };

        let bumped = pc
            .calculate_bumped_gas_price(&tx_data, &relayer)
            .await
            .unwrap();
        // Since min bump is 10%, original was 50 Gwei, min is 55 Gwei, but cap is 50 Gwei
        assert_eq!(
            bumped.is_min_bumped,
            Some(false),
            "Should not be min bumped when cap is too low"
        );
    }

    #[tokio::test]
    async fn test_is_min_bumped_flag_legacy() {
        let mut mock_service = MockEvmGasPriceServiceTrait::new();
        let mock_prices = GasPrices {
            legacy_prices: SpeedPrices {
                safe_low: 8_000_000_000,
                average: 10_000_000_000,
                fast: 12_000_000_000,
                fastest: 15_000_000_000,
            },
            max_priority_fee_per_gas: SpeedPrices::default(),
            base_fee_per_gas: 0,
        };
        mock_service
            .expect_get_prices_from_json_rpc()
            .returning(move || {
                let prices = mock_prices.clone();
                Box::pin(async move { Ok(prices) })
            });
        mock_service
            .expect_network()
            .return_const(create_mock_evm_network("mainnet"));

        let pc = PriceCalculator::new(mock_service, NetworkExtraFeeCalculator::None);
        let mut relayer = create_mock_relayer();

        // Case 1: Regular case, cap is high enough
        relayer.policies = RelayerNetworkPolicy::Evm(RelayerEvmPolicy {
            gas_price_cap: Some(100_000_000_000u128),
            ..Default::default()
        });

        let tx_data = EvmTransactionData {
            gas_price: Some(10_000_000_000),
            speed: Some(Speed::Fast),
            ..Default::default()
        };

        let bumped = pc
            .calculate_bumped_gas_price(&tx_data, &relayer)
            .await
            .unwrap();
        assert_eq!(
            bumped.is_min_bumped,
            Some(true),
            "Should be min bumped with sufficient cap"
        );

        // Case 2: Cap too low
        relayer.policies = RelayerNetworkPolicy::Evm(RelayerEvmPolicy {
            gas_price_cap: Some(10_000_000_000u128), // Same as original, preventing the 10% bump
            ..Default::default()
        });

        let bumped = pc
            .calculate_bumped_gas_price(&tx_data, &relayer)
            .await
            .unwrap();
        assert_eq!(
            bumped.is_min_bumped,
            Some(false),
            "Should not be min bumped with insufficient cap"
        );
    }

    #[tokio::test]
    async fn test_calculate_bumped_gas_price_with_extra_fee() {
        // Set up mock gas price service
        let mut mock_gas_service = MockEvmGasPriceServiceTrait::new();
        mock_gas_service
            .expect_get_prices_from_json_rpc()
            .returning(|| {
                Box::pin(async {
                    Ok(GasPrices {
                        legacy_prices: SpeedPrices {
                            safe_low: 10_000_000_000,
                            average: 12_000_000_000,
                            fast: 14_000_000_000,
                            fastest: 18_000_000_000,
                        },
                        max_priority_fee_per_gas: SpeedPrices::default(),
                        base_fee_per_gas: 40_000_000_000,
                    })
                })
            });
        mock_gas_service
            .expect_network()
            .return_const(create_mock_evm_network("mainnet"));

        // Create a PriceCalculator without extra fee service first
        let pc = PriceCalculator::new(mock_gas_service, NetworkExtraFeeCalculator::None);

        // Create test transaction and relayer
        let relayer = create_mock_relayer();
        let tx_data = EvmTransactionData {
            max_fee_per_gas: Some(50_000_000_000),
            max_priority_fee_per_gas: Some(2_000_000_000),
            speed: Some(Speed::Fast),
            ..Default::default()
        };

        // Call the method under test
        let result = pc.calculate_bumped_gas_price(&tx_data, &relayer).await;

        // Verify extra fee is None when no extra fee service is used
        assert!(result.is_ok());
        let price_params = result.unwrap();
        assert_eq!(price_params.extra_fee, None);
    }

    #[tokio::test]
    async fn test_calculate_bumped_gas_price_with_mock_extra_fee() {
        // Set up mock gas price service
        let mut mock_gas_service = MockEvmGasPriceServiceTrait::new();
        mock_gas_service
            .expect_get_prices_from_json_rpc()
            .returning(|| {
                Box::pin(async {
                    Ok(GasPrices {
                        legacy_prices: SpeedPrices {
                            safe_low: 10_000_000_000,
                            average: 12_000_000_000,
                            fast: 14_000_000_000,
                            fastest: 18_000_000_000,
                        },
                        max_priority_fee_per_gas: SpeedPrices::default(),
                        base_fee_per_gas: 40_000_000_000,
                    })
                })
            });
        mock_gas_service
            .expect_network()
            .return_const(create_mock_evm_network("mainnet"));

        // Create mock extra fee service
        let mut mock_extra_fee_service = MockNetworkExtraFeeCalculatorServiceTrait::new();

        // Set up expected return value for the extra fee
        let expected_extra_fee = U256::from(123456789u64);
        mock_extra_fee_service
            .expect_get_extra_fee()
            .returning(move |_| Box::pin(async move { Ok(expected_extra_fee) }));

        // Use the Mock variant from NetworkExtraFeeCalculator enum
        let extra_fee_calculator = NetworkExtraFeeCalculator::Mock(mock_extra_fee_service);

        // Create the price calculator with the mock extra fee service
        let pc = PriceCalculator::new(mock_gas_service, extra_fee_calculator);

        // Create test transaction and relayer
        let relayer = create_mock_relayer();
        let tx_data = EvmTransactionData {
            max_fee_per_gas: Some(50_000_000_000),
            max_priority_fee_per_gas: Some(2_000_000_000),
            speed: Some(Speed::Fast),
            ..Default::default()
        };

        // Call the method under test
        let result = pc.calculate_bumped_gas_price(&tx_data, &relayer).await;

        // Verify extra fee was properly included
        assert!(result.is_ok());
        let price_params = result.unwrap();
        assert_eq!(
            price_params.extra_fee,
            Some(expected_extra_fee.try_into().unwrap_or(0))
        );
    }

    #[test]
    fn test_calculate_total_cost_eip1559() {
        let price_params = PriceParams {
            gas_price: None,
            max_fee_per_gas: Some(30_000_000_000),
            max_priority_fee_per_gas: Some(2_000_000_000),
            is_min_bumped: None,
            extra_fee: None,
            total_cost: U256::ZERO,
        };

        let gas_limit = 100_000;
        let value = U256::from(1_000_000_000_000_000_000u128); // 1 ETH
        let is_eip1559 = true;

        let total_cost = price_params.calculate_total_cost(is_eip1559, gas_limit, value);

        // Expected: max_fee_per_gas * gas_limit + value
        let expected = U256::from(30_000_000_000u128) * U256::from(gas_limit) + value;
        assert_eq!(total_cost, expected);
    }

    #[test]
    fn test_calculate_total_cost_legacy() {
        let price_params = PriceParams {
            gas_price: Some(20_000_000_000),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            is_min_bumped: None,
            extra_fee: None,
            total_cost: U256::ZERO,
        };

        let gas_limit = 100_000;
        let value = U256::from(1_000_000_000_000_000_000u128); // 1 ETH
        let is_eip1559 = false;

        let total_cost = price_params.calculate_total_cost(is_eip1559, gas_limit, value);

        // Expected: gas_price * gas_limit + value
        let expected = U256::from(20_000_000_000u128) * U256::from(gas_limit) + value;
        assert_eq!(total_cost, expected);
    }

    #[test]
    fn test_calculate_total_cost_with_extra_fee() {
        let price_params = PriceParams {
            gas_price: Some(20_000_000_000),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            is_min_bumped: None,
            extra_fee: Some(5_000_000_000),
            total_cost: U256::ZERO,
        };

        let gas_limit = 100_000;
        let value = U256::from(1_000_000_000_000_000_000u128); // 1 ETH
        let is_eip1559 = false;

        let total_cost = price_params.calculate_total_cost(is_eip1559, gas_limit, value);

        let expected = U256::from(20_000_000_000u128) * U256::from(gas_limit)
            + value
            + U256::from(5_000_000_000u128);
        assert_eq!(total_cost, expected);
    }

    #[test]
    fn test_calculate_total_cost_zero_values() {
        let price_params = PriceParams {
            gas_price: Some(0),
            max_fee_per_gas: Some(0),
            max_priority_fee_per_gas: Some(0),
            is_min_bumped: None,
            extra_fee: Some(0),
            total_cost: U256::ZERO,
        };

        let gas_limit = 0;
        let value = U256::from(0);

        let legacy_total_cost = price_params.calculate_total_cost(false, gas_limit, value);
        assert_eq!(legacy_total_cost, U256::ZERO);

        let eip1559_total_cost = price_params.calculate_total_cost(true, gas_limit, value);
        assert_eq!(eip1559_total_cost, U256::ZERO);
    }

    #[test]
    fn test_calculate_total_cost_missing_values() {
        let price_params = PriceParams {
            gas_price: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            is_min_bumped: None,
            extra_fee: None,
            total_cost: U256::ZERO,
        };

        let gas_limit = 100_000;
        let value = U256::from(1_000_000_000_000_000_000u128);

        let legacy_total = price_params.calculate_total_cost(false, gas_limit, value);
        assert_eq!(legacy_total, value);

        let eip1559_total = price_params.calculate_total_cost(true, gas_limit, value);
        assert_eq!(eip1559_total, value);
    }

    #[test]
    fn test_calculate_min_bump_normal_cases() {
        let base_price = 20_000_000_000u128; // 20 Gwei
        let expected = 22_000_000_000u128; // 22 Gwei (10% bump)
        assert_eq!(calculate_min_bump(base_price), expected);

        let base_price = 1_000_000_000u128;
        let expected = 1_100_000_000u128; // 1.1 Gwei
        assert_eq!(calculate_min_bump(base_price), expected);

        let base_price = 100_000_000_000u128;
        let expected = 110_000_000_000u128; // 110 Gwei
        assert_eq!(calculate_min_bump(base_price), expected);
    }

    #[test]
    fn test_calculate_min_bump_edge_cases() {
        // Test with zero - should return 1 wei (minimum bump)
        assert_eq!(calculate_min_bump(0), 1);

        // Test with 1 wei - should return at least 2 wei
        let result = calculate_min_bump(1);
        assert!(result >= 2);

        // Test with very small values where 10% bump rounds down to 0
        let base_price = 5u128; // 5 wei
        let result = calculate_min_bump(base_price);
        assert!(
            result > base_price,
            "Result {} should be greater than base_price {}",
            result,
            base_price
        );

        let base_price = 9u128;
        let result = calculate_min_bump(base_price);
        assert_eq!(
            result, 10u128,
            "9 wei should bump to 10 wei (minimum 1 wei increase)"
        );
    }

    #[test]
    fn test_calculate_min_bump_large_values() {
        // Test with large values to ensure no overflow
        let base_price = u128::MAX / 2;
        let result = calculate_min_bump(base_price);
        assert!(result > base_price);

        // Test near maximum value
        let base_price = u128::MAX - 1000;
        let result = calculate_min_bump(base_price);
        // Should not panic and should return a reasonable value
        assert!(result >= base_price.saturating_add(1));
    }

    #[test]
    fn test_calculate_min_bump_overflow_protection() {
        let base_price = u128::MAX;
        let result = calculate_min_bump(base_price);
        assert_eq!(result, u128::MAX);

        let base_price = (u128::MAX / 11) * 10 + 1;
        let result = calculate_min_bump(base_price);
        assert!(result >= base_price);
    }

    #[test]
    fn test_calculate_min_bump_minimum_increase_guarantee() {
        // Test that the function always increases by at least 1 wei
        let test_cases = vec![0, 1, 2, 5, 9, 10, 100, 1000, 10000];

        for base_price in test_cases {
            let result = calculate_min_bump(base_price);
            assert!(
                result > base_price,
                "calculate_min_bump({}) = {} should be greater than base_price",
                base_price,
                result
            );
            assert!(
                result >= base_price.saturating_add(1),
                "calculate_min_bump({}) = {} should be at least base_price + 1",
                base_price,
                result
            );
        }
    }

    #[tokio::test]
    async fn test_calculate_bumped_gas_price_no_mempool_network_eip1559() {
        let mut mock_service = MockEvmGasPriceServiceTrait::new();

        // Mock the network to return a no-mempool network
        mock_service
            .expect_network()
            .return_const(create_mock_no_mempool_network("arbitrum"));

        // Mock get_prices_from_json_rpc for get_transaction_price_params call
        let mock_prices = GasPrices {
            legacy_prices: SpeedPrices {
                safe_low: 1_000_000_000,
                average: 2_000_000_000,
                fast: 3_000_000_000,
                fastest: 4_000_000_000,
            },
            max_priority_fee_per_gas: SpeedPrices {
                safe_low: 1_000_000_000,
                average: 2_000_000_000,
                fast: 3_000_000_000,
                fastest: 4_000_000_000,
            },
            base_fee_per_gas: 50_000_000_000,
        };

        mock_service
            .expect_get_prices_from_json_rpc()
            .returning(move || {
                let prices = mock_prices.clone();
                Box::pin(async move { Ok(prices) })
            });

        // Also mock get_legacy_prices_from_json_rpc in case it's called
        mock_service
            .expect_get_legacy_prices_from_json_rpc()
            .returning(|| {
                Box::pin(async {
                    Ok(SpeedPrices {
                        safe_low: 1_000_000_000,
                        average: 2_000_000_000,
                        fast: 3_000_000_000,
                        fastest: 4_000_000_000,
                    })
                })
            });

        let pc = PriceCalculator::new(mock_service, NetworkExtraFeeCalculator::None);
        let mut relayer = create_mock_relayer();

        // Ensure relayer policy allows EIP1559 pricing
        relayer.policies = RelayerNetworkPolicy::Evm(RelayerEvmPolicy {
            eip1559_pricing: Some(true),
            ..Default::default()
        });

        // Create a speed-based transaction that would normally be bumped
        let tx_data = EvmTransactionData {
            speed: Some(Speed::Fast),
            gas_limit: Some(21000),
            value: U256::from(1_000_000_000_000_000_000u128), // 1 ETH
            ..Default::default()
        };

        let result = pc.calculate_bumped_gas_price(&tx_data, &relayer).await;

        assert!(result.is_ok());
        let price_params = result.unwrap();

        // For no-mempool networks, should use current market prices, not bump
        assert_eq!(price_params.is_min_bumped, Some(true));

        // The no-mempool network should use the current market prices instead of bumping
        // For this test, what matters is that it returns is_min_bumped: true
        // The exact pricing method depends on the network configuration

        // Verify that some pricing was returned (either EIP1559 or legacy)
        assert!(
            price_params.max_priority_fee_per_gas.is_some() || price_params.gas_price.is_some()
        );

        // Should have calculated total cost
        assert!(price_params.total_cost > U256::ZERO);
    }

    #[tokio::test]
    async fn test_calculate_bumped_gas_price_no_mempool_network_legacy() {
        let mut mock_service = MockEvmGasPriceServiceTrait::new();

        // Mock the network to return a no-mempool network
        mock_service
            .expect_network()
            .return_const(create_mock_no_mempool_network("arbitrum"));

        // Mock get_legacy_prices_from_json_rpc for get_transaction_price_params call
        let mock_legacy_prices = SpeedPrices {
            safe_low: 10_000_000_000,
            average: 12_000_000_000,
            fast: 14_000_000_000,
            fastest: 18_000_000_000,
        };

        mock_service
            .expect_get_legacy_prices_from_json_rpc()
            .returning(move || {
                let prices = mock_legacy_prices.clone();
                Box::pin(async move { Ok(prices) })
            });

        let pc = PriceCalculator::new(mock_service, NetworkExtraFeeCalculator::None);
        let mut relayer = create_mock_relayer();

        // Force legacy pricing
        relayer.policies = RelayerNetworkPolicy::Evm(RelayerEvmPolicy {
            eip1559_pricing: Some(false),
            ..Default::default()
        });

        // Create a speed-based transaction that would normally be bumped
        let tx_data = EvmTransactionData {
            speed: Some(Speed::Fast),
            gas_limit: Some(21000),
            value: U256::from(1_000_000_000_000_000_000u128), // 1 ETH
            ..Default::default()
        };

        let result = pc.calculate_bumped_gas_price(&tx_data, &relayer).await;

        assert!(result.is_ok());
        let price_params = result.unwrap();

        // For no-mempool networks, should use current market prices, not bump
        assert_eq!(price_params.is_min_bumped, Some(true));

        // Should return current market prices - verify that some pricing was returned
        assert!(
            price_params.max_priority_fee_per_gas.is_some() || price_params.gas_price.is_some()
        );

        // Should have calculated total cost
        assert!(price_params.total_cost > U256::ZERO);
    }

    #[tokio::test]
    async fn test_calculate_bumped_gas_price_no_mempool_vs_regular_network() {
        // Test EIP1559 transaction on regular network (should bump)
        let mut mock_service_regular = MockEvmGasPriceServiceTrait::new();

        let mock_prices = GasPrices {
            legacy_prices: SpeedPrices::default(),
            max_priority_fee_per_gas: SpeedPrices {
                safe_low: 1_000_000_000,
                average: 2_000_000_000,
                fast: 3_000_000_000,
                fastest: 4_000_000_000,
            },
            base_fee_per_gas: 50_000_000_000,
        };

        mock_service_regular
            .expect_network()
            .return_const(create_mock_evm_network("mainnet"));

        let mock_prices_clone = mock_prices.clone();
        mock_service_regular
            .expect_get_prices_from_json_rpc()
            .returning(move || {
                let prices = mock_prices_clone.clone();
                Box::pin(async move { Ok(prices) })
            });

        let pc_regular =
            PriceCalculator::new(mock_service_regular, NetworkExtraFeeCalculator::None);
        let relayer = create_mock_relayer();

        let tx_data = EvmTransactionData {
            speed: Some(Speed::Fast),
            ..Default::default()
        };

        let result_regular = pc_regular
            .calculate_bumped_gas_price(&tx_data, &relayer)
            .await
            .unwrap();

        // Regular network should return some pricing (either EIP1559 or legacy)
        assert!(
            result_regular.max_priority_fee_per_gas.is_some() || result_regular.gas_price.is_some()
        );

        // Test same transaction on no-mempool network (should not bump)
        let mut mock_service_no_mempool = MockEvmGasPriceServiceTrait::new();

        mock_service_no_mempool
            .expect_network()
            .return_const(create_mock_no_mempool_network("arbitrum"));

        mock_service_no_mempool
            .expect_get_prices_from_json_rpc()
            .returning(move || {
                let prices = mock_prices.clone();
                Box::pin(async move { Ok(prices) })
            });

        let pc_no_mempool =
            PriceCalculator::new(mock_service_no_mempool, NetworkExtraFeeCalculator::None);

        let result_no_mempool = pc_no_mempool
            .calculate_bumped_gas_price(&tx_data, &relayer)
            .await
            .unwrap();

        // No-mempool network should use current market prices
        assert_eq!(result_no_mempool.is_min_bumped, Some(true));

        // Both networks should return some pricing
        assert!(
            result_no_mempool.max_priority_fee_per_gas.is_some()
                || result_no_mempool.gas_price.is_some()
        );

        // The key difference is that no-mempool networks should set is_min_bumped to true
        // Regular networks may or may not set is_min_bumped depending on the actual implementation
        assert_eq!(result_no_mempool.is_min_bumped, Some(true));
    }

    #[tokio::test]
    async fn test_calculate_bumped_gas_price_arbitrum_network() {
        let mut mock_service = MockEvmGasPriceServiceTrait::new();

        // Create a network that returns true for is_arbitrum() but not lacks_mempool()
        let arbitrum_network = EvmNetwork {
            network: "arbitrum-one".to_string(),
            rpc_urls: vec!["https://arb1.arbitrum.io/rpc".to_string()],
            explorer_urls: None,
            average_blocktime_ms: 1000, // 1 second for arbitrum
            is_testnet: false,
            tags: vec!["arbitrum-based".to_string()], // This makes is_arbitrum() return true
            chain_id: 42161,
            required_confirmations: 1,
            features: vec!["eip1559".to_string()],
            symbol: "ETH".to_string(),
        };

        // Mock the network to return our arbitrum network
        mock_service.expect_network().return_const(arbitrum_network);

        // Mock get_prices_from_json_rpc for get_transaction_price_params call
        let mock_prices = GasPrices {
            legacy_prices: SpeedPrices {
                safe_low: 100_000_000, // 0.1 Gwei (typical for Arbitrum)
                average: 200_000_000,
                fast: 300_000_000,
                fastest: 400_000_000,
            },
            max_priority_fee_per_gas: SpeedPrices {
                safe_low: 10_000_000, // 0.01 Gwei
                average: 20_000_000,
                fast: 30_000_000,
                fastest: 40_000_000,
            },
            base_fee_per_gas: 100_000_000, // 0.1 Gwei
        };

        mock_service
            .expect_get_prices_from_json_rpc()
            .returning(move || {
                let prices = mock_prices.clone();
                Box::pin(async move { Ok(prices) })
            });

        let pc = PriceCalculator::new(mock_service, NetworkExtraFeeCalculator::None);
        let relayer = create_mock_relayer();

        // Create a speed-based transaction that would normally be bumped on regular networks
        let tx_data = EvmTransactionData {
            speed: Some(Speed::Fast),
            gas_limit: Some(21000),
            value: U256::from(1_000_000_000_000_000_000u128), // 1 ETH
            ..Default::default()
        };

        let result = pc.calculate_bumped_gas_price(&tx_data, &relayer).await;

        assert!(result.is_ok());
        let price_params = result.unwrap();

        // For Arbitrum networks (is_arbitrum() == true), should skip bump and use current market prices
        assert_eq!(
            price_params.is_min_bumped,
            Some(true),
            "Arbitrum networks should skip bumping and use current market prices"
        );

        // Should return pricing based on current market conditions, not bumped prices
        assert!(
            price_params.max_priority_fee_per_gas.is_some() || price_params.gas_price.is_some(),
            "Should return some form of pricing"
        );

        // Should have calculated total cost
        assert!(
            price_params.total_cost > U256::ZERO,
            "Should have non-zero total cost"
        );

        // Verify that the prices returned are based on current market (Speed::Fast)
        // and not bumped versions of existing transaction prices
        if let Some(priority_fee) = price_params.max_priority_fee_per_gas {
            // For Speed::Fast, should be around 30_000_000 (0.03 Gwei) based on our mock
            assert!(
                priority_fee <= 50_000_000, // Should be reasonable for current market, not a bump
                "Priority fee should be based on current market, not bumped: {}",
                priority_fee
            );
        }
    }
}
