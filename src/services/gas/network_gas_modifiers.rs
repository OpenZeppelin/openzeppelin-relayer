use async_trait::async_trait;

use crate::{
    models::{EvmNetwork, EvmTransactionData, TransactionError, U256},
    services::{EvmProviderTrait, OptimismProviderTrait},
};

use super::optimism_gas_modifiers::OptimismGasPriceService;

#[async_trait]
pub trait NetworkGasModifierServiceTrait {
    async fn modify_gas_price(
        &self,
        tx_data: &EvmTransactionData,
    ) -> Result<U256, TransactionError>;
}

pub fn get_network_gas_modifier_service<P>(
    network: EvmNetwork,
    provider: P,
) -> Option<Box<dyn NetworkGasModifierServiceTrait + Send + Sync>>
where
    P: EvmProviderTrait + OptimismProviderTrait + 'static,
{
    if network.is_optimism() {
        Some(Box::new(OptimismGasPriceService::new(provider)))
    } else {
        None
    }
}
