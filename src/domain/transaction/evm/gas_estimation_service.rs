use crate::{
    models::{evm::Speed, EvmTransactionData, TransactionError},
    services::EvmProvider,
};
use alloy::primitives::U256;
use eyre::Result;
use log::info;

// calculate the multiplier for the gas estimation
impl Speed {
    pub fn multiplier(&self) -> f64 {
        match self {
            Speed::Slow => 1.0,
            Speed::Average => 1.25,
            Speed::Fast => 1.5,
            Speed::Fastest => 2.0,
        }
    }
}

pub struct GasEstimationService {
    provider: EvmProvider,
}

impl GasEstimationService {
    pub fn new(provider: EvmProvider) -> Self {
        Self { provider }
    }
    pub async fn estimate_gas(
        &self,
        tx_data: &EvmTransactionData,
    ) -> Result<U256, TransactionError> {
        info!("Estimating gas for tx_data: {:?}", tx_data);
        let gas_estimation = self.provider.estimate_gas(tx_data).await.map_err(|err| {
            let msg = format!("Failed to estimate gas: {err}");
            TransactionError::NetworkConfiguration(msg)
        })?;
        Ok(gas_estimation)
    }
    // TODO: This is a temporary implementation for legacy only
    pub async fn estimate_gas_with_speed(
        &self,
        tx_data: &EvmTransactionData,
        speed: Speed,
    ) -> Result<U256, TransactionError> {
        info!("Estimating gas with speed: {:?}", speed);
        let base = self.estimate_gas(tx_data).await?;
        let factor = speed.multiplier();

        // Convert U256 to u128 first, then to f64
        let base_u128: u128 = base.to::<u128>();
        let final_gas = ((base_u128 as f64) * factor).round() as u128;

        Ok(U256::from(final_gas))
    }
}
