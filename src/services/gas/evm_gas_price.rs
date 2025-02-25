use crate::{
    models::{evm::Speed, EvmTransactionData, TransactionError, U256},
    services::EvmProvider,
};
use eyre::Result;
use log::info;

use async_trait::async_trait;

// calculate the multiplier for the gas estimation
impl Speed {
    pub fn multiplier() -> [(Speed, f64); 4] {
        [
            (Speed::SafeLow, 1.0),
            (Speed::Average, 1.25),
            (Speed::Fast, 1.5),
            (Speed::Fastest, 2.0),
        ]
    }
}
#[async_trait]
#[allow(dead_code)]
pub trait EvmGasPriceServiceTrait {
    async fn estimate_gas(&self, tx_data: &EvmTransactionData) -> Result<U256, TransactionError>;

    async fn get_legacy_prices_from_json_rpc(&self)
        -> Result<Vec<(Speed, U256)>, TransactionError>;
}

pub struct EvmGasPriceService {
    provider: EvmProvider,
}

impl EvmGasPriceService {
    pub fn new(provider: EvmProvider) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl EvmGasPriceServiceTrait for EvmGasPriceService {
    async fn estimate_gas(&self, tx_data: &EvmTransactionData) -> Result<U256, TransactionError> {
        info!("Estimating gas for tx_data: {:?}", tx_data);
        let gas_estimation = self.provider.estimate_gas(tx_data).await.map_err(|err| {
            let msg = format!("Failed to estimate gas: {err}");
            TransactionError::NetworkConfiguration(msg)
        })?;
        Ok(U256::from(gas_estimation))
    }

    async fn get_legacy_prices_from_json_rpc(
        &self,
    ) -> Result<Vec<(Speed, U256)>, TransactionError> {
        let base = self.provider.get_gas_price().await?;
        let base_u128: u128 = base.to::<u128>();
        Ok(Speed::multiplier()
            .into_iter()
            .map(|(speed, multiplier)| {
                let final_gas = ((base_u128 as f64) * multiplier).round() as u128;
                (speed, U256::from(final_gas))
            })
            .collect())
    }
}
