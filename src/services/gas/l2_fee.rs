//! This module defines a generic, zero-alloc framework for calculating L2-specific fees
//! without using dynamic dispatch (`dyn`) or heap allocations (`Box`).
use crate::{
    models::{evm::EvmTransactionRequest, EvmNetwork, TransactionError, U256},
    services::{
        gas::optimism_extra_fee::{OptimismExtraFeeService, OptimismFeeData},
        provider::evm::EvmProviderTrait,
    },
};

#[derive(Debug, Clone)]
pub enum L2FeeData {
    Optimism(OptimismFeeData),
}

#[derive(Debug, Clone)]
pub enum L2FeeService<P> {
    Optimism(OptimismExtraFeeService<P>),
}

impl<P: EvmProviderTrait + Clone> L2FeeService<P> {
    pub async fn fetch_fee_data(&self) -> Result<L2FeeData, TransactionError> {
        match self {
            L2FeeService::Optimism(svc) => svc.fetch_fee_data().await.map(L2FeeData::Optimism),
        }
    }

    pub fn calculate_fee(
        &self,
        fee_data: &L2FeeData,
        tx: &EvmTransactionRequest,
    ) -> Result<U256, TransactionError> {
        match (self, fee_data) {
            (L2FeeService::Optimism(svc), L2FeeData::Optimism(data)) => svc.calculate_fee(data, tx),
        }
    }
}

/// Factory function to create an L2-specific fee service using static dispatch.
pub fn l2_fee_service_factory<P: EvmProviderTrait + Clone>(
    network: &EvmNetwork,
    provider: P,
) -> Option<L2FeeService<P>> {
    if network.is_optimism() {
        Some(L2FeeService::Optimism(OptimismExtraFeeService::new(
            provider,
        )))
    } else {
        // Future L2s like Arbitrum can be added here.
        None
    }
}
