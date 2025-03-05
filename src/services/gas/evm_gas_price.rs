//! This module provides services for estimating gas prices on the Ethereum Virtual Machine (EVM).
//! It includes traits and implementations for calculating gas price multipliers based on
//! transaction speed and fetching gas prices using JSON-RPC.
use crate::{
    models::{evm::Speed, EvmTransactionData, TransactionError},
    services::{EvmProvider, EvmProviderTrait},
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
pub struct SpeedPrices {
    pub safe_low: u128,
    pub average: u128,
    pub fast: u128,
    pub fastest: u128,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GasPrices {
    pub legacy_prices: SpeedPrices,
    pub max_priority_fee_per_gas: SpeedPrices,
    pub base_fee_per_gas: u128,
}

impl std::cmp::Eq for Speed {}

impl std::hash::Hash for Speed {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        core::mem::discriminant(self).hash(state);
    }
}

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

impl IntoIterator for GasPrices {
    type Item = (Speed, u128, u128);
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        let speeds = [Speed::SafeLow, Speed::Average, Speed::Fast, Speed::Fastest];

        speeds
            .into_iter()
            .map(|speed| {
                let max_fee = match speed {
                    Speed::SafeLow => self.legacy_prices.safe_low,
                    Speed::Average => self.legacy_prices.average,
                    Speed::Fast => self.legacy_prices.fast,
                    Speed::Fastest => self.legacy_prices.fastest,
                };

                let max_priority_fee = match speed {
                    Speed::SafeLow => self.max_priority_fee_per_gas.safe_low,
                    Speed::Average => self.max_priority_fee_per_gas.average,
                    Speed::Fast => self.max_priority_fee_per_gas.fast,
                    Speed::Fastest => self.max_priority_fee_per_gas.fastest,
                };

                (speed, max_fee, max_priority_fee)
            })
            .collect::<Vec<_>>()
            .into_iter()
    }
}

#[async_trait]
#[allow(dead_code)]
pub trait EvmGasPriceServiceTrait {
    async fn estimate_gas(&self, tx_data: &EvmTransactionData) -> Result<u64, TransactionError>;

    async fn get_legacy_prices_from_json_rpc(&self)
        -> Result<Vec<(Speed, u128)>, TransactionError>;

    async fn get_prices_from_json_rpc(&self) -> Result<GasPrices, TransactionError>;

    async fn get_current_base_fee(&self) -> Result<u128, TransactionError>;
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

    async fn get_current_base_fee(&self) -> Result<u128, TransactionError> {
        let block = self.provider.get_block_by_number().await?;
        let base_fee = block.unwrap().header.base_fee_per_gas;
        Ok(base_fee.unwrap_or(0).into())
    }

    async fn get_prices_from_json_rpc(&self) -> Result<GasPrices, TransactionError> {
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

        // Convert max_priority_fees to SpeedPrices
        let max_priority_fees = SpeedPrices {
            safe_low: (max_priority_fees.get(&Speed::SafeLow).unwrap_or(&0.0) * 1e9) as u128,
            average: (max_priority_fees.get(&Speed::Average).unwrap_or(&0.0) * 1e9) as u128,
            fast: (max_priority_fees.get(&Speed::Fast).unwrap_or(&0.0) * 1e9) as u128,
            fastest: (max_priority_fees.get(&Speed::Fastest).unwrap_or(&0.0) * 1e9) as u128,
        };

        // Convert legacy_prices to SpeedPrices
        let legacy_prices = SpeedPrices {
            safe_low: legacy_prices
                .iter()
                .find(|(s, _)| *s == Speed::SafeLow)
                .map(|(_, p)| *p)
                .unwrap_or(0),
            average: legacy_prices
                .iter()
                .find(|(s, _)| *s == Speed::Average)
                .map(|(_, p)| *p)
                .unwrap_or(0),
            fast: legacy_prices
                .iter()
                .find(|(s, _)| *s == Speed::Fast)
                .map(|(_, p)| *p)
                .unwrap_or(0),
            fastest: legacy_prices
                .iter()
                .find(|(s, _)| *s == Speed::Fastest)
                .map(|(_, p)| *p)
                .unwrap_or(0),
        };

        Ok(GasPrices {
            legacy_prices,
            max_priority_fee_per_gas: max_priority_fees,
            base_fee_per_gas: base_fee,
        })
    }
}
