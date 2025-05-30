//! This module provides services for estimating gas prices on the Ethereum Virtual Machine (EVM).
//! It includes traits and implementations for calculating gas price multipliers based on
//! transaction speed and fetching gas prices using JSON-RPC.
use crate::{
    models::{evm::Speed, EvmNetwork, EvmTransactionData, TransactionError},
    services::EvmProviderTrait,
};
use alloy::rpc::types::BlockNumberOrTag;
use eyre::Result;
use futures::try_join;
use log::info;

use async_trait::async_trait;
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[cfg(test)]
use mockall::automock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeedPrices {
    pub safe_low: u128,
    pub average: u128,
    pub fast: u128,
    pub fastest: u128,
}

#[cfg(test)]
impl Default for SpeedPrices {
    fn default() -> Self {
        Self {
            safe_low: 20_000_000_000, // 20 Gwei
            average: 30_000_000_000,  // 30 Gwei
            fast: 40_000_000_000,     // 40 Gwei
            fastest: 50_000_000_000,  // 50 Gwei
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GasPrices {
    pub legacy_prices: SpeedPrices,
    pub max_priority_fee_per_gas: SpeedPrices,
    pub base_fee_per_gas: u128,
}

#[cfg(test)]
impl Default for GasPrices {
    fn default() -> Self {
        Self {
            legacy_prices: SpeedPrices::default(),
            max_priority_fee_per_gas: SpeedPrices::default(),
            base_fee_per_gas: 10_000_000_000, // 10 Gwei base fee
        }
    }
}

impl std::cmp::Eq for Speed {}

impl std::hash::Hash for Speed {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        core::mem::discriminant(self).hash(state);
    }
}

const GWEI: f64 = 1e9;

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

impl IntoIterator for SpeedPrices {
    type Item = (Speed, u128);
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        vec![
            (Speed::SafeLow, self.safe_low),
            (Speed::Average, self.average),
            (Speed::Fast, self.fast),
            (Speed::Fastest, self.fastest),
        ]
        .into_iter()
    }
}

#[async_trait]
#[cfg_attr(test, automock(
    type Provider = crate::services::MockEvmProviderTrait;
))]
#[allow(dead_code)]
pub trait EvmGasPriceServiceTrait {
    type Provider: EvmProviderTrait;

    async fn estimate_gas(&self, tx_data: &EvmTransactionData) -> Result<u64, TransactionError>;

    async fn get_legacy_prices_from_json_rpc(&self) -> Result<SpeedPrices, TransactionError>;

    async fn get_prices_from_json_rpc(&self) -> Result<GasPrices, TransactionError>;

    async fn get_current_base_fee(&self) -> Result<u128, TransactionError>;

    fn network(&self) -> &EvmNetwork;
}

pub struct EvmGasPriceService<P: EvmProviderTrait> {
    provider: P,
    network: EvmNetwork,
}

impl<P: EvmProviderTrait> EvmGasPriceService<P> {
    pub fn new(provider: P, network: EvmNetwork) -> Self {
        Self { provider, network }
    }

    pub fn network(&self) -> &EvmNetwork {
        &self.network
    }
}

#[async_trait]
impl<P: EvmProviderTrait> EvmGasPriceServiceTrait for EvmGasPriceService<P> {
    type Provider = P;

    async fn estimate_gas(&self, tx_data: &EvmTransactionData) -> Result<u64, TransactionError> {
        info!("Estimating gas for tx_data: {:?}", tx_data);
        let gas_estimation = self.provider.estimate_gas(tx_data).await.map_err(|err| {
            let msg = format!("Failed to estimate gas: {err}");
            TransactionError::NetworkConfiguration(msg)
        })?;
        Ok(gas_estimation)
    }

    async fn get_legacy_prices_from_json_rpc(&self) -> Result<SpeedPrices, TransactionError> {
        let base = self.provider.get_gas_price().await?;
        let prices: Vec<(Speed, u128)> = Speed::multiplier()
            .into_iter()
            .map(|(speed, multiplier)| {
                let final_gas = (base * multiplier) / 100;
                (speed, final_gas)
            })
            .collect();

        Ok(SpeedPrices {
            safe_low: prices
                .iter()
                .find(|(s, _)| *s == Speed::SafeLow)
                .map(|(_, p)| *p)
                .unwrap_or(0),
            average: prices
                .iter()
                .find(|(s, _)| *s == Speed::Average)
                .map(|(_, p)| *p)
                .unwrap_or(0),
            fast: prices
                .iter()
                .find(|(s, _)| *s == Speed::Fast)
                .map(|(_, p)| *p)
                .unwrap_or(0),
            fastest: prices
                .iter()
                .find(|(s, _)| *s == Speed::Fastest)
                .map(|(_, p)| *p)
                .unwrap_or(0),
        })
    }

    async fn get_current_base_fee(&self) -> Result<u128, TransactionError> {
        let block = self.provider.get_block_by_number().await?;
        let base_fee = block.header.base_fee_per_gas.unwrap_or(0);
        Ok(base_fee.into())
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
                    .map_err(|e| {
                        TransactionError::NetworkConfiguration(format!(
                            "Failed to fetch fee history data: {}",
                            e
                        ))
                    })
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
                                    Some(reward as f64 / GWEI)
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
            safe_low: (max_priority_fees.get(&Speed::SafeLow).unwrap_or(&0.0) * GWEI) as u128,
            average: (max_priority_fees.get(&Speed::Average).unwrap_or(&0.0) * GWEI) as u128,
            fast: (max_priority_fees.get(&Speed::Fast).unwrap_or(&0.0) * GWEI) as u128,
            fastest: (max_priority_fees.get(&Speed::Fastest).unwrap_or(&0.0) * GWEI) as u128,
        };

        Ok(GasPrices {
            legacy_prices,
            max_priority_fee_per_gas: max_priority_fees,
            base_fee_per_gas: base_fee,
        })
    }

    fn network(&self) -> &EvmNetwork {
        &self.network
    }
}

#[cfg(test)]
mod tests {
    use alloy::rpc::types::FeeHistory;

    use crate::services::provider::evm::MockEvmProviderTrait;
    use alloy::rpc::types::{Block as BlockResponse, Header};

    use super::*;

    fn create_test_evm_network() -> EvmNetwork {
        EvmNetwork {
            network: "mainnet".to_string(),
            rpc_urls: vec!["https://mainnet.infura.io/v3/YOUR_INFURA_API_KEY".to_string()],
            explorer_urls: None,
            average_blocktime_ms: 12000,
            is_testnet: false,
            tags: vec!["mainnet".to_string()],
            chain_id: 1,
            required_confirmations: 1,
            features: vec!["eip1559".to_string()],
            symbol: "ETH".to_string(),
        }
    }

    #[test]
    fn test_speed_multiplier() {
        let multipliers = Speed::multiplier();
        assert_eq!(multipliers.len(), 4);
        assert_eq!(multipliers[0], (Speed::SafeLow, 100));
        assert_eq!(multipliers[1], (Speed::Average, 125));
        assert_eq!(multipliers[2], (Speed::Fast, 150));
        assert_eq!(multipliers[3], (Speed::Fastest, 200));
    }

    #[test]
    fn test_gas_prices_into_iterator() {
        let gas_prices = GasPrices {
            legacy_prices: SpeedPrices {
                safe_low: 10,
                average: 20,
                fast: 30,
                fastest: 40,
            },
            max_priority_fee_per_gas: SpeedPrices {
                safe_low: 1,
                average: 2,
                fast: 3,
                fastest: 4,
            },
            base_fee_per_gas: 100,
        };

        let prices: Vec<(Speed, u128, u128)> = gas_prices.into_iter().collect();
        assert_eq!(prices.len(), 4);
        assert_eq!(prices[0], (Speed::SafeLow, 10, 1));
        assert_eq!(prices[1], (Speed::Average, 20, 2));
        assert_eq!(prices[2], (Speed::Fast, 30, 3));
        assert_eq!(prices[3], (Speed::Fastest, 40, 4));
    }

    #[test]
    fn test_speed_prices_into_iterator() {
        let speed_prices = SpeedPrices {
            safe_low: 10,
            average: 20,
            fast: 30,
            fastest: 40,
        };

        let prices: Vec<(Speed, u128)> = speed_prices.into_iter().collect();
        assert_eq!(prices.len(), 4);
        assert_eq!(prices[0], (Speed::SafeLow, 10));
        assert_eq!(prices[1], (Speed::Average, 20));
        assert_eq!(prices[2], (Speed::Fast, 30));
        assert_eq!(prices[3], (Speed::Fastest, 40));
    }

    #[tokio::test]
    async fn test_get_legacy_prices_from_json_rpc() {
        let mut mock_provider = MockEvmProviderTrait::new();
        let base_gas_price = 10_000_000_000u128; // 10 gwei base price

        // Mock the provider's get_gas_price method
        mock_provider
            .expect_get_gas_price()
            .times(1)
            .returning(move || Box::pin(async move { Ok(base_gas_price) }));

        // Create the actual service with mocked provider
        let service = EvmGasPriceService::new(mock_provider, create_test_evm_network());

        // Test the actual implementation
        let prices = service.get_legacy_prices_from_json_rpc().await.unwrap();

        // Verify each speed level has correct multiplier applied
        assert_eq!(prices.safe_low, 10_000_000_000); // 10 gwei * 100%
        assert_eq!(prices.average, 12_500_000_000); // 10 gwei * 125%
        assert_eq!(prices.fast, 15_000_000_000); // 10 gwei * 150%
        assert_eq!(prices.fastest, 20_000_000_000); // 10 gwei * 200%

        // Verify against Speed::multiplier()
        let multipliers = Speed::multiplier();
        for (speed, multiplier) in multipliers.iter() {
            let price = match speed {
                Speed::SafeLow => prices.safe_low,
                Speed::Average => prices.average,
                Speed::Fast => prices.fast,
                Speed::Fastest => prices.fastest,
            };
            assert_eq!(
                price,
                base_gas_price * multiplier / 100,
                "Price for {:?} should be {}% of base price",
                speed,
                multiplier
            );
        }
    }

    #[tokio::test]
    async fn test_get_current_base_fee() {
        let mut mock_provider = MockEvmProviderTrait::new();
        let expected_base_fee = 10_000_000_000u128;

        // Mock the provider's get_block_by_number method
        mock_provider
            .expect_get_block_by_number()
            .times(1)
            .returning(move || {
                Box::pin(async move {
                    Ok(BlockResponse {
                        header: Header {
                            inner: alloy::consensus::Header {
                                base_fee_per_gas: Some(expected_base_fee as u64),
                                ..Default::default()
                            },
                            ..Default::default()
                        },
                        ..Default::default()
                    })
                })
            });

        let service = EvmGasPriceService::new(mock_provider, create_test_evm_network());
        let result = service.get_current_base_fee().await.unwrap();
        assert_eq!(result, expected_base_fee);
    }

    #[tokio::test]
    async fn test_get_prices_from_json_rpc() {
        let mut mock_provider = MockEvmProviderTrait::new();
        let base_gas_price = 10_000_000_000u128;
        let base_fee = 5_000_000_000u128;

        // Mock get_gas_price for legacy prices
        mock_provider
            .expect_get_gas_price()
            .times(1)
            .returning(move || Box::pin(async move { Ok(base_gas_price) }));

        // Mock get_block_by_number for base fee
        mock_provider
            .expect_get_block_by_number()
            .times(1)
            .returning(move || {
                Box::pin(async move {
                    Ok(BlockResponse {
                        header: Header {
                            inner: alloy::consensus::Header {
                                base_fee_per_gas: Some(base_fee as u64),
                                ..Default::default()
                            },
                            ..Default::default()
                        },
                        ..Default::default()
                    })
                })
            });

        // Mock get_fee_history
        mock_provider
            .expect_get_fee_history()
            .times(1)
            .returning(|_, _, _| {
                Box::pin(async {
                    Ok(FeeHistory {
                        oldest_block: 100,
                        base_fee_per_gas: vec![5_000_000_000],
                        gas_used_ratio: vec![0.5],
                        reward: Some(vec![vec![
                            1_000_000_000,
                            2_000_000_000,
                            3_000_000_000,
                            4_000_000_000,
                        ]]),
                        base_fee_per_blob_gas: vec![],
                        blob_gas_used_ratio: vec![],
                    })
                })
            });

        let service = EvmGasPriceService::new(mock_provider, create_test_evm_network());
        let prices = service.get_prices_from_json_rpc().await.unwrap();

        // Test legacy prices
        assert_eq!(prices.legacy_prices.safe_low, 10_000_000_000);
        assert_eq!(prices.legacy_prices.average, 12_500_000_000);
        assert_eq!(prices.legacy_prices.fast, 15_000_000_000);
        assert_eq!(prices.legacy_prices.fastest, 20_000_000_000);

        // Test base fee
        assert_eq!(prices.base_fee_per_gas, 5_000_000_000);

        // Test priority fees
        assert_eq!(prices.max_priority_fee_per_gas.safe_low, 1_000_000_000);
        assert_eq!(prices.max_priority_fee_per_gas.average, 2_000_000_000);
        assert_eq!(prices.max_priority_fee_per_gas.fast, 3_000_000_000);
        assert_eq!(prices.max_priority_fee_per_gas.fastest, 4_000_000_000);
    }
}
