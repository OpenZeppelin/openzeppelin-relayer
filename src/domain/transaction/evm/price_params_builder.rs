use crate::{
    models::{
        EvmTransactionData, EvmTransactionDataTrait, RelayerRepoModel, TransactionError, U256,
    },
    services::{EvmGasPriceService, EvmGasPriceServiceTrait, EvmProvider, EvmProviderTrait},
};

/// Parameters for determining the price of a transaction.
#[derive(Debug)]
pub struct TransactionPriceParams {
    /// The gas price for the transaction.
    pub gas_price: Option<u128>,
    /// The maximum priority fee per gas.
    pub max_priority_fee_per_gas: Option<u128>,
    /// The maximum fee per gas.
    pub max_fee_per_gas: Option<u128>,
    /// The balance available for the transaction.
    pub balance: Option<U256>,
}

pub struct PriceParamsBuilder<'a> {
    tx_data: &'a EvmTransactionData,
    relayer: &'a RelayerRepoModel,
    gas_price_service: &'a EvmGasPriceService,
    provider: &'a EvmProvider,
    gas_price: Option<u128>,
    max_fee_per_gas: Option<u128>,
    max_priority_fee_per_gas: Option<u128>,
    balance: Option<U256>,
}

impl<'a> PriceParamsBuilder<'a> {
    pub fn new(
        tx_data: &'a EvmTransactionData,
        relayer: &'a RelayerRepoModel,
        gas_price_service: &'a EvmGasPriceService,
        provider: &'a EvmProvider,
    ) -> Self {
        Self {
            tx_data,
            relayer,
            gas_price_service,
            provider,
            gas_price: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            balance: None,
        }
    }

    pub async fn calculate_prices(mut self) -> Result<Self, TransactionError> {
        match () {
            _ if self.tx_data.is_legacy() => self.calculate_legacy_prices().await?,
            _ if self.tx_data.is_eip1559() => self.calculate_eip1559_prices().await?,
            _ if self.tx_data.is_speed() => self.calculate_speed_based_prices().await?,
            _ => {
                return Err(TransactionError::NotSupported(
                    "Invalid transaction type".to_string(),
                ))
            }
        };

        Ok(self)
    }

    pub async fn apply_gas_price_cap(mut self) -> Result<Self, TransactionError> {
        let gas_price_cap = self
            .relayer
            .policies
            .get_evm_policy()
            .gas_price_cap
            .unwrap_or(u128::MAX);

        let is_eip1559 = self.max_fee_per_gas.is_some() && self.max_priority_fee_per_gas.is_some();

        if is_eip1559 {
            let max_fee = self.max_fee_per_gas.unwrap();
            let max_priority_fee = self.max_priority_fee_per_gas.unwrap();

            // Cap the maxFeePerGas
            let capped_max_fee = std::cmp::min(gas_price_cap, max_fee);

            // Ensure maxPriorityFeePerGas < maxFeePerGas to avoid client errors
            let capped_max_priority_fee = std::cmp::min(capped_max_fee, max_priority_fee);

            self.max_fee_per_gas = Some(capped_max_fee);
            self.max_priority_fee_per_gas = Some(capped_max_priority_fee);
            self.gas_price = None;
        } else {
            // Handle legacy transaction
            self.gas_price = Some(std::cmp::min(
                self.gas_price.unwrap_or_default(),
                gas_price_cap,
            ));
            self.max_fee_per_gas = None;
            self.max_priority_fee_per_gas = None;
        }

        Ok(self)
    }

    pub async fn fetch_balance(mut self) -> Result<Self, TransactionError> {
        self.balance = Some(
            self.provider
                .get_balance(&self.tx_data.from)
                .await
                .map_err(|e| TransactionError::UnexpectedError(e.to_string()))?,
        );
        Ok(self)
    }

    pub fn build(self) -> TransactionPriceParams {
        TransactionPriceParams {
            gas_price: self.gas_price,
            max_fee_per_gas: self.max_fee_per_gas,
            max_priority_fee_per_gas: self.max_priority_fee_per_gas,
            balance: self.balance,
        }
    }

    async fn calculate_legacy_prices(&mut self) -> Result<(), TransactionError> {
        self.gas_price = Some(
            self.tx_data
                .gas_price
                .ok_or(TransactionError::NotSupported(
                    "Gas price is required for legacy transactions".to_string(),
                ))?,
        );
        Ok(())
    }

    async fn calculate_eip1559_prices(&mut self) -> Result<(), TransactionError> {
        self.max_fee_per_gas = Some(self.tx_data.max_fee_per_gas.ok_or(
            TransactionError::NotSupported(
                "Max fee per gas is required for EIP1559 transactions".to_string(),
            ),
        )?);

        self.max_priority_fee_per_gas = Some(self.tx_data.max_priority_fee_per_gas.ok_or(
            TransactionError::NotSupported(
                "Max priority fee per gas is required for EIP1559 transactions".to_string(),
            ),
        )?);
        Ok(())
    }

    async fn calculate_speed_based_prices(&mut self) -> Result<(), TransactionError> {
        let speed = self
            .tx_data
            .speed
            .as_ref()
            .ok_or(TransactionError::NotSupported(
                "Speed is required".to_string(),
            ))?;

        if self.relayer.policies.get_evm_policy().eip1559_pricing {
            let prices = self.gas_price_service.get_prices_from_json_rpc().await?;
            let (max_fee, max_priority_fee) = prices
                .into_iter()
                .find(|(s, _, _)| s == speed)
                .map(|(_, max_priority_fee_wei, base_fee_wei)| {
                    let max_fee = calculate_max_fee_per_gas(base_fee_wei, max_priority_fee_wei);
                    (max_fee, max_priority_fee_wei)
                })
                .ok_or(TransactionError::UnexpectedError(
                    "Speed not supported for EIP1559".to_string(),
                ))?;

            self.max_fee_per_gas = Some(max_fee);
            self.max_priority_fee_per_gas = Some(max_priority_fee);
        } else {
            let prices = self
                .gas_price_service
                .get_legacy_prices_from_json_rpc()
                .await?;
            self.gas_price = Some(
                prices
                    .into_iter()
                    .find(|(s, _)| s == speed)
                    .map(|(_, price)| price)
                    .ok_or(TransactionError::NotSupported(
                        "Speed not supported".to_string(),
                    ))?,
            );
        }
        Ok(())
    }
}

// Helper functions moved from price_calculator.rs
const BLOCK_INTERVAL_MS: u64 = 12000; // Mainnet block time
const MINUTE_AND_HALF_MS: f64 = 90000.0;
const BASE_FEE_INCREASE_FACTOR: f64 = 1.125; // 12.5% increase per block
const MAX_BASE_FEE_MULTIPLIER: f64 = 10.0;

fn get_base_fee_multiplier() -> f64 {
    let n_blocks = MINUTE_AND_HALF_MS / BLOCK_INTERVAL_MS as f64;
    f64::min(
        BASE_FEE_INCREASE_FACTOR.powf(n_blocks),
        MAX_BASE_FEE_MULTIPLIER,
    )
}

fn calculate_max_fee_per_gas(base_fee_wei: u128, max_priority_fee_wei: u128) -> u128 {
    let base_fee = base_fee_wei as f64;
    let multiplied_base_fee = (base_fee * get_base_fee_multiplier()) as u128;
    multiplied_base_fee + max_priority_fee_wei
}
