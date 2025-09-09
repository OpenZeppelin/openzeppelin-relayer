use crate::{
    domain::evm::PriceParams,
    models::{evm::EvmTransactionRequest, TransactionError, U256},
};

#[derive(Debug, Clone, Default)]
pub struct MockPriceHandler;

impl MockPriceHandler {
    pub fn new() -> Self {
        Self
    }

    pub async fn handle_price_params(
        &self,
        _tx: &EvmTransactionRequest,
        mut original_params: PriceParams,
    ) -> Result<PriceParams, TransactionError> {
        original_params.extra_fee = Some(U256::from(42u128));
        original_params.total_cost = original_params.total_cost + U256::from(42u128);
        Ok(original_params)
    }
}
