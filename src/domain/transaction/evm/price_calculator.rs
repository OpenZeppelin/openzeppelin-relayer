use super::TransactionPriceParams;
use crate::{
    models::{
        evm::Speed, EvmTransactionData, EvmTransactionDataTrait, RelayerRepoModel, TransactionError,
    },
    services::{EvmGasPriceService, EvmGasPriceServiceTrait, EvmProvider, EvmProviderTrait},
};

type GasPriceCapResult = (Option<u128>, Option<u128>, Option<u128>);

/// EIP1559 gas price calculation constants
const BLOCK_INTERVAL_MS: u64 = 12000; // Mainnet block time
const MINUTE_AND_HALF_MS: f64 = 90000.0;
const BASE_FEE_INCREASE_FACTOR: f64 = 1.125; // 12.5% increase per block
const MAX_BASE_FEE_MULTIPLIER: f64 = 10.0;

/// Calculates gas prices for different transaction types:
/// - Legacy transactions (gas_price)
/// - EIP1559 transactions (max_fee_per_gas, max_priority_fee_per_gas)
/// - Speed-based transactions (automatically chooses between legacy and EIP1559)
pub async fn get_transaction_price_params(
    tx_data: &EvmTransactionData,
    relayer: &RelayerRepoModel,
    gas_price_service: &EvmGasPriceService<EvmProvider>,
    provider: &EvmProvider,
) -> Result<TransactionPriceParams, TransactionError> {
    match () {
        // Legacy transaction
        _ if tx_data.is_legacy() => handle_legacy_transaction(tx_data, relayer, provider).await,

        // EIP1559 transaction
        _ if tx_data.is_eip1559() => handle_eip1559_transaction(tx_data, relayer, provider).await,

        // Speed-based transaction
        _ if tx_data.is_speed() => {
            handle_speed_transaction(tx_data, relayer, gas_price_service, provider).await
        }

        // Invalid transaction type
        _ => Err(TransactionError::NotSupported(
            "Invalid transaction type".to_string(),
        )),
    }
}

/// Handles legacy transaction gas price calculation
async fn handle_legacy_transaction(
    tx_data: &EvmTransactionData,
    relayer: &RelayerRepoModel,
    provider: &EvmProvider,
) -> Result<TransactionPriceParams, TransactionError> {
    let gas_price = tx_data.gas_price.ok_or(TransactionError::NotSupported(
        "Gas price is required for legacy transactions".to_string(),
    ))?;

    get_price_params_with_balance(
        Some(gas_price),
        None,
        None,
        relayer,
        provider,
        &tx_data.from,
    )
    .await
}

/// Handles EIP1559 transaction gas price calculation
async fn handle_eip1559_transaction(
    tx_data: &EvmTransactionData,
    relayer: &RelayerRepoModel,
    provider: &EvmProvider,
) -> Result<TransactionPriceParams, TransactionError> {
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

    get_price_params_with_balance(
        None,
        Some(max_fee),
        Some(max_priority_fee),
        relayer,
        provider,
        &tx_data.from,
    )
    .await
}

/// Handles speed-based transaction gas price calculation
async fn handle_speed_transaction(
    tx_data: &EvmTransactionData,
    relayer: &RelayerRepoModel,
    gas_price_service: &EvmGasPriceService<EvmProvider>,
    provider: &EvmProvider,
) -> Result<TransactionPriceParams, TransactionError> {
    let speed = tx_data
        .speed
        .as_ref()
        .ok_or(TransactionError::NotSupported(
            "Speed is required".to_string(),
        ))?;

    if relayer.policies.get_evm_policy().eip1559_pricing {
        handle_eip1559_speed(speed, gas_price_service, relayer, provider, &tx_data.from).await
    } else {
        handle_legacy_speed(speed, gas_price_service, relayer, provider, &tx_data.from).await
    }
}

/// Handles EIP1559 speed-based transaction gas price calculation
async fn handle_eip1559_speed(
    speed: &Speed,
    gas_price_service: &EvmGasPriceService<EvmProvider>,
    relayer: &RelayerRepoModel,
    provider: &EvmProvider,
    from_address: &str,
) -> Result<TransactionPriceParams, TransactionError> {
    let prices = gas_price_service.get_prices_from_json_rpc().await?;
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

    get_price_params_with_balance(
        None,
        Some(max_fee),
        Some(max_priority_fee),
        relayer,
        provider,
        from_address,
    )
    .await
}

/// Handles legacy speed-based transaction gas price calculation
async fn handle_legacy_speed(
    speed: &Speed,
    gas_price_service: &EvmGasPriceService<EvmProvider>,
    relayer: &RelayerRepoModel,
    provider: &EvmProvider,
    from_address: &str,
) -> Result<TransactionPriceParams, TransactionError> {
    let prices = gas_price_service.get_legacy_prices_from_json_rpc().await?;
    let gas_price = prices
        .into_iter()
        .find(|(s, _)| s == speed)
        .map(|(_, price)| price)
        .ok_or(TransactionError::NotSupported(
            "Speed not supported".to_string(),
        ))?;

    get_price_params_with_balance(Some(gas_price), None, None, relayer, provider, from_address)
        .await
}

/// Calculate base fee multiplier for EIP1559 transactions
fn get_base_fee_multiplier() -> f64 {
    let n_blocks = MINUTE_AND_HALF_MS / BLOCK_INTERVAL_MS as f64;
    f64::min(
        BASE_FEE_INCREASE_FACTOR.powf(n_blocks),
        MAX_BASE_FEE_MULTIPLIER,
    )
}

/// Calculate max fee per gas for EIP1559 transactions (all values in wei)
fn calculate_max_fee_per_gas(base_fee_wei: u128, max_priority_fee_wei: u128) -> u128 {
    let base_fee = base_fee_wei as f64;
    let multiplied_base_fee = (base_fee * get_base_fee_multiplier()) as u128;
    multiplied_base_fee + max_priority_fee_wei
}

async fn get_price_params_with_balance(
    gas_price: Option<u128>,
    max_fee_per_gas: Option<u128>,
    max_priority_fee_per_gas: Option<u128>,
    relayer: &RelayerRepoModel,
    provider: &EvmProvider,
    from_address: &str,
) -> Result<TransactionPriceParams, TransactionError> {
    let (gas_price, max_fee_per_gas, max_priority_fee_per_gas) = apply_gas_price_cap(
        gas_price.unwrap_or_default(),
        max_fee_per_gas,
        max_priority_fee_per_gas,
        relayer,
    )?;

    let balance = provider
        .get_balance(from_address)
        .await
        .map_err(|e| TransactionError::UnexpectedError(e.to_string()))?;

    Ok(TransactionPriceParams {
        gas_price,
        max_fee_per_gas,
        max_priority_fee_per_gas,
        balance: Some(balance),
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
