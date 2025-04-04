use alloy::consensus::{TxEip1559, TxLegacy};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use solana_sdk::packet::Encode;

use crate::{
    models::{EvmTransactionData, TransactionError, U256},
    services::OptimismProviderTrait,
};

use super::NetworkGasModifierServiceTrait;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimismModifiers {
    pub l1_base_fee: U256,
    pub base_fee: U256,
    pub decimals: U256,
    pub blob_base_fee: U256,
    pub base_fee_scalar: u32,
    pub blob_base_fee_scalar: u32,
}

#[cfg(test)]
impl Default for OptimismModifiers {
    fn default() -> Self {
        Self {
            l1_base_fee: U256::ZERO,
            base_fee: U256::ZERO,
            decimals: U256::ZERO,
            blob_base_fee: U256::ZERO,
            base_fee_scalar: 0,
            blob_base_fee_scalar: 0,
        }
    }
}

pub struct OptimismGasPriceService<P> {
    provider: P,
}

impl<P> OptimismGasPriceService<P> {
    pub fn new(provider: P) -> Self {
        Self { provider }
    }
}

impl<P: OptimismProviderTrait> OptimismGasPriceService<P> {
    pub async fn get_modifiers(&self) -> Result<OptimismModifiers, TransactionError> {
        let l1_base_fee = self.provider.get_l1_base_fee().await?;
        let base_fee = self.provider.get_base_fee().await?;
        let decimals = self.provider.get_decimals().await?;
        let blob_base_fee = self.provider.get_blob_base_fee().await?;
        let base_fee_scalar = self.provider.get_base_fee_scalar().await?;
        let blob_base_fee_scalar = self.provider.get_blob_base_fee_scalar().await?;

        let modifiers = OptimismModifiers {
            l1_base_fee,
            base_fee,
            decimals,
            blob_base_fee,
            base_fee_scalar,
            blob_base_fee_scalar,
        };
        Ok(modifiers)
    }
}

#[async_trait]
impl<P: OptimismProviderTrait> NetworkGasModifierServiceTrait for OptimismGasPriceService<P> {
    async fn modify_gas_price(
        &self,
        tx_data: &EvmTransactionData,
        eip1559: bool,
    ) -> Result<U256, TransactionError> {
        let bytes = if eip1559 {
            let tx_eip1559 = TxEip1559::try_from(tx_data)?;
            let mut bytes = Vec::new();
            tx_eip1559.encode(&mut bytes).map_err(|e| {
                TransactionError::InvalidType(format!("Failed to encode transaction: {}", e))
            })?;
            bytes
        } else {
            let tx_legacy = TxLegacy::try_from(tx_data)?;
            let mut bytes = Vec::new();
            tx_legacy.encode(&mut bytes).map_err(|e| {
                TransactionError::InvalidType(format!("Failed to encode transaction: {}", e))
            })?;
            bytes
        };

        let zero_bytes = U256::from(bytes.iter().filter(|&b| *b == 0).count());
        let non_zero_bytes = U256::from(bytes.len()) - zero_bytes;

        let tx_compressed_size =
            ((zero_bytes * U256::from(4)) + (non_zero_bytes * U256::from(16))) / U256::from(16);

        let gas_modifiers = self.get_modifiers().await?;

        let weighted_gas_price =
            U256::from(16) * U256::from(gas_modifiers.base_fee_scalar) * gas_modifiers.base_fee
                + U256::from(gas_modifiers.blob_base_fee_scalar) * gas_modifiers.blob_base_fee;

        let l1_data_fee = tx_compressed_size * weighted_gas_price;

        Ok(l1_data_fee)
    }
}
