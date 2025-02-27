//! This module provides services for estimating gas prices on the Ethereum Virtual Machine (EVM).
//! It includes traits and implementations for calculating gas price multipliers based on
//! transaction speed and fetching gas prices using JSON-RPC.
use crate::{
    models::{evm::Speed, EvmTransactionData, TransactionError, U256},
    services::EvmProvider,
};
use alloy::rpc::types::BlockNumberOrTag;
use eyre::Result;
use futures::try_join;
use log::info;

use async_trait::async_trait;
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GasPrices {
    pub legacy_prices: Vec<(Speed, U256)>,
    pub max_priority_fee_per_gas: HashMap<Speed, f64>,
    pub base_fee_per_gas: f64,
}

impl std::cmp::Eq for Speed {}

impl std::hash::Hash for Speed {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        core::mem::discriminant(self).hash(state);
    }
}

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

impl IntoIterator for GasPrices {
    type Item = (Speed, U256, U256);
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.legacy_prices
            .into_iter()
            .map(|(speed, max_fee)| {
                let max_priority_fee = U256::from(
                    (self.max_priority_fee_per_gas.get(&speed).unwrap_or(&0.0) * 1e9) as u128,
                );
                (speed, max_fee, max_priority_fee)
            })
            .collect::<Vec<_>>()
            .into_iter()
    }
}

#[async_trait]
#[allow(dead_code)]
pub trait EvmGasPriceServiceTrait {
    async fn estimate_gas(&self, tx_data: &EvmTransactionData) -> Result<U256, TransactionError>;

    async fn get_legacy_prices_from_json_rpc(&self)
        -> Result<Vec<(Speed, U256)>, TransactionError>;

    async fn get_eip1559_prices_from_json_rpc(&self) -> Result<GasPrices, TransactionError>;
}

pub struct EvmGasPriceService {
    provider: EvmProvider,
}

impl EvmGasPriceService {
    pub fn new(provider: EvmProvider) -> Self {
        Self { provider }
    }

    async fn get_current_base_fee(&self) -> Result<f64, TransactionError> {
        let block = self.provider.get_block_by_number().await?;
        let base_fee = block.unwrap().header.base_fee_per_gas;
        Ok(base_fee.unwrap_or(0) as f64)
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

    async fn get_eip1559_prices_from_json_rpc(&self) -> Result<GasPrices, TransactionError> {
        const HISTORICAL_BLOCKS: u64 = 4;

        // Define speed percentiles
        let speed_percentiles: HashMap<Speed, (usize, f64)> = [
            (Speed::SafeLow, (0, 30.0)),
            (Speed::Average, (1, 50.0)),
            (Speed::Fast, (2, 85.0)),
            (Speed::Fastest, (3, 99.0)),
        ]
        .into();

        // Create array of reward percentiles
        let reward_percentiles: Vec<f64> = speed_percentiles
            .values()
            .sorted_by_key(|&(idx, _)| idx)
            .map(|(_, percentile)| *percentile)
            .collect();

        // Get prices in parallel
        let (legacy_prices, base_fee, fee_history) = try_join!(
            self.get_legacy_prices_from_json_rpc(),
            self.get_current_base_fee(),
            async {
                self.provider
                    .get_fee_history(
                        HISTORICAL_BLOCKS,
                        BlockNumberOrTag::Latest,
                        reward_percentiles,
                    )
                    .await
                    .map_err(|e| TransactionError::NetworkConfiguration(e.to_string()))
            }
        )?;

        // Calculate maxPriorityFeePerGas for each speed
        let max_priority_fees: HashMap<Speed, f64> = Speed::multiplier()
            .into_iter()
            .filter_map(|(speed, _)| {
                let (idx, percentile) = speed_percentiles.get(&speed)?;

                // Get rewards for this speed's percentile
                let rewards: Vec<f64> = fee_history
                    .reward
                    .as_ref()
                    .map(|rewards| {
                        rewards
                            .iter()
                            .filter_map(|block_rewards| {
                                let reward = block_rewards[*idx];
                                if reward > 0 {
                                    Some(reward as f64 / 1e9)
                                } else {
                                    None
                                }
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                // Calculate mean of non-zero rewards, or use fallback
                let priority_fee = if rewards.is_empty() {
                    // Fallback: 1 gwei * percentile / 100
                    (1.0 * percentile) / 100.0
                } else {
                    rewards.iter().sum::<f64>() / rewards.len() as f64
                };

                Some((speed, priority_fee))
            })
            .collect();

        Ok(GasPrices {
            legacy_prices,
            max_priority_fee_per_gas: max_priority_fees,
            base_fee_per_gas: base_fee,
        })
    }
}
