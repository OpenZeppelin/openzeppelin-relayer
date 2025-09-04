use crate::{
    models::{evm::EvmTransactionRequest, EvmNetwork, TransactionError, U256},
    services::{gas::l2_fee::l2_fee_service_factory, EvmProvider},
};

#[cfg_attr(test, mockall::automock)]
#[async_trait::async_trait]
pub trait NetworkExtraFeeCalculatorServiceTrait: Send + Sync {
    async fn get_extra_fee(&self, tx: &EvmTransactionRequest) -> Result<U256, TransactionError>;
}

pub struct NetworkExtraFeeCalculatorService {
    network: EvmNetwork,
    provider: EvmProvider,
}

impl NetworkExtraFeeCalculatorService {
    pub fn new(network: EvmNetwork, provider: EvmProvider) -> Self {
        Self { network, provider }
    }
}

#[async_trait::async_trait]
impl NetworkExtraFeeCalculatorServiceTrait for NetworkExtraFeeCalculatorService {
    async fn get_extra_fee(&self, tx: &EvmTransactionRequest) -> Result<U256, TransactionError> {
        if let Some(l2_fee_service) = l2_fee_service_factory(&self.network, self.provider.clone()) {
            let fee_data = l2_fee_service.fetch_fee_data().await?;
            let fee = l2_fee_service.calculate_fee(&fee_data, tx)?;
            Ok(fee)
        } else {
            Ok(U256::from(0))
        }
    }
}
