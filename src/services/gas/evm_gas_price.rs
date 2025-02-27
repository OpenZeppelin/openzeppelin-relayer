//! This module provides services for estimating gas prices on the Ethereum Virtual Machine (EVM).
//! It includes traits and implementations for calculating gas price multipliers based on
//! transaction speed and fetching gas prices using JSON-RPC.
use crate::{
    models::{evm::Speed, EvmTransactionData, TransactionError},
    services::EvmProvider,
};
use eyre::Result;
use log::info;

use async_trait::async_trait;

// calculate the multiplier for the gas estimation
impl Speed {
    pub fn multiplier() -> [(Speed, u128); 4] {
        [
            (Speed::SafeLow, 100),
            (Speed::Average, 125),
            (Speed::Fast, 150),
            (Speed::Fastest, 200),
        ]
    }
}
#[async_trait]
#[allow(dead_code)]
pub trait EvmGasPriceServiceTrait {
    async fn estimate_gas(&self, tx_data: &EvmTransactionData) -> Result<u64, TransactionError>;

    async fn get_legacy_prices_from_json_rpc(&self)
        -> Result<Vec<(Speed, u128)>, TransactionError>;
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
    async fn estimate_gas(&self, tx_data: &EvmTransactionData) -> Result<u64, TransactionError> {
        info!("Estimating gas for tx_data: {:?}", tx_data);
        let gas_estimation = self.provider.estimate_gas(tx_data).await.map_err(|err| {
            let msg = format!("Failed to estimate gas: {err}");
            TransactionError::NetworkConfiguration(msg)
        })?;
        Ok(gas_estimation)
    }

    async fn get_legacy_prices_from_json_rpc(
        &self,
    ) -> Result<Vec<(Speed, u128)>, TransactionError> {
        let base = self.provider.get_gas_price().await?;
        Ok(Speed::multiplier()
            .into_iter()
            .map(|(speed, multiplier)| {
                let final_gas = (base * multiplier) / 100;
                (speed, final_gas)
            })
            .collect())
    }
}
