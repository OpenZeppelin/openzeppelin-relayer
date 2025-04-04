use async_trait::async_trait;

use crate::{
    models::{EvmNetwork, EvmTransactionData, TransactionError, U256},
    services::{EvmProviderTrait, OptimismProviderTrait},
};

use super::optimism_extra_fee::OptimismExtraFeeService;

#[async_trait]
pub trait NetworkExtraFeeCalculatorServiceTrait {
    /// Get the extra fee for a transaction
    ///
    /// # Arguments
    ///
    /// * `tx_data` - The transaction data to get the extra fee for
    ///
    /// # Returns
    ///
    async fn get_extra_fee(&self, tx_data: &EvmTransactionData) -> Result<U256, TransactionError>;
}

/// Get the network extra fee calculator service
///
/// # Arguments
///
/// * `network` - The network to get the extra fee calculator service for
/// * `provider` - The provider to get the extra fee calculator service for
///
pub fn get_network_extra_fee_calculator_service<P>(
    network: EvmNetwork,
    provider: P,
) -> Option<Box<dyn NetworkExtraFeeCalculatorServiceTrait + Send + Sync>>
where
    P: EvmProviderTrait + OptimismProviderTrait + 'static,
{
    if network.is_optimism() {
        Some(Box::new(OptimismExtraFeeService::new(provider)))
    } else {
        None
    }
}
