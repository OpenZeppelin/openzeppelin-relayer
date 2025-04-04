use async_trait::async_trait;

use crate::models::{EvmTransactionData, TransactionError, U256};

#[async_trait]
pub trait NetworkGasModifierServiceTrait {
    async fn modify_gas_price(
        &self,
        tx_data: &EvmTransactionData,
        eip1559: bool,
    ) -> Result<U256, TransactionError>;
}
