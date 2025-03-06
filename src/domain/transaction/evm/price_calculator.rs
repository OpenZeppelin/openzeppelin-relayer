use super::price_params_builder::{PriceParamsBuilder, TransactionPriceParams};
use crate::{
    models::{EvmTransactionData, RelayerRepoModel, TransactionError},
    services::{EvmGasPriceService, EvmProvider},
};

/// Calculates gas prices for different transaction types:
/// - Legacy transactions (gas_price)
/// - EIP1559 transactions (max_fee_per_gas, max_priority_fee_per_gas)
/// - Speed-based transactions (automatically chooses between legacy and EIP1559)
pub async fn get_transaction_price_params(
    tx_data: &EvmTransactionData,
    relayer: &RelayerRepoModel,
    gas_price_service: &EvmGasPriceService,
    provider: &EvmProvider,
) -> Result<TransactionPriceParams, TransactionError> {
    Ok(
        PriceParamsBuilder::new(tx_data, relayer, gas_price_service, provider)
            .calculate_prices()
            .await?
            .apply_gas_price_cap()
            .await?
            .fetch_balance()
            .await?
            .build(),
    )
}
