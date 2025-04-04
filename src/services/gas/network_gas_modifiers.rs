use alloy::primitives::map::HashMap;
use async_trait::async_trait;

use crate::{
    models::{EvmNamedNetwork, EvmTransactionData, TransactionError, U256},
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

pub fn gas_price_modifiers_factory<P>(
    provider: P,
) -> HashMap<EvmNamedNetwork, Box<dyn NetworkGasModifierServiceTrait + Send + Sync>>
where
    P: EvmProviderTrait + OptimismProviderTrait + 'static,
{
    let mut modifiers: HashMap<
        EvmNamedNetwork,
        Box<dyn NetworkGasModifierServiceTrait + Send + Sync>,
    > = HashMap::default();
    modifiers.insert(
        EvmNamedNetwork::Optimism,
        Box::new(OptimismGasPriceService::new(provider)),
    );
    modifiers
}
