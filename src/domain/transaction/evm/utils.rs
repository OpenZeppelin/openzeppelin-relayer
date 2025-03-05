use super::TransactionPriceParams;
use crate::{
    models::{EvmTransactionData, EvmTransactionDataTrait, RelayerRepoModel, TransactionError},
    services::{EvmGasPriceService, EvmGasPriceServiceTrait, EvmProvider, EvmProviderTrait},
};

type GasPriceCapResult = (Option<u128>, Option<u128>, Option<u128>);

/// Get the price params for the transaction
pub async fn get_transaction_price_params(
    tx_data: &EvmTransactionData,
    relayer: &RelayerRepoModel,
    gas_price_service: &EvmGasPriceService,
    provider: &EvmProvider,
) -> Result<TransactionPriceParams, TransactionError> {
    let (gas_price, max_fee_per_gas, max_priority_fee_per_gas) = if tx_data.is_legacy() {
        let gas_price = tx_data.gas_price.ok_or(TransactionError::NotSupported(
            "Gas price is required for legacy transactions".to_string(),
        ))?;
        (Some(gas_price), None, None)
    } else if tx_data.is_eip1559() {
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
        (None, Some(max_fee), Some(max_priority_fee))
    } else if tx_data.is_speed() {
        match &tx_data.speed {
            Some(speed) => {
                let eip1559_pricing = relayer.policies.get_evm_policy().eip1559_pricing;

                if eip1559_pricing {
                    let prices = gas_price_service.get_eip1559_prices_from_json_rpc().await?;
                    let (max_fee, max_priority_fee) = prices
                        .into_iter()
                        .find(|(s, _, _)| s == speed)
                        .map(|(_, mf, mpf)| (mf, mpf))
                        .ok_or(TransactionError::UnexpectedError(
                            "Speed not supported for EIP1559".to_string(),
                        ))?;

                    (None, Some(max_fee), Some(max_priority_fee))
                } else {
                    let prices = gas_price_service.get_legacy_prices_from_json_rpc().await?;

                    let gas_price = prices
                        .into_iter()
                        .find(|(s, _)| s == speed)
                        .map(|(_, price)| price)
                        .ok_or(TransactionError::NotSupported(
                            "Speed not supported".to_string(),
                        ))?;

                    (Some(gas_price), None, None)
                }
            }
            None => {
                return Err(TransactionError::NotSupported(
                    "Speed is required".to_string(),
                ));
            }
        }
    } else {
        return Err(TransactionError::NotSupported(
            "Invalid transaction type".to_string(),
        ));
    };

    let (gas_price, max_fee_per_gas, max_priority_fee_per_gas) = apply_gas_price_cap(
        gas_price.unwrap_or_default(),
        max_fee_per_gas,
        max_priority_fee_per_gas,
        relayer,
    )?;

    let balance = provider
        .get_balance(&tx_data.from)
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
    // Get gas price cap from relayer policies, default to u128::MAX if None
    let gas_price_cap = relayer
        .policies
        .get_evm_policy()
        .gas_price_cap
        .unwrap_or(u128::MAX);

    let is_eip1559 = max_fee_per_gas.is_some() && max_priority_fee_per_gas.is_some();

    if is_eip1559 {
        let max_fee = max_fee_per_gas.unwrap();
        let max_priority_fee = max_priority_fee_per_gas.unwrap();

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
