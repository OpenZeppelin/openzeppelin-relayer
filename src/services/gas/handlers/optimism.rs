use crate::{
    constants::{DEFAULT_GAS_LIMIT, OPTIMISM_GAS_PRICE_ORACLE_ADDRESS},
    domain::evm::PriceParams,
    models::{evm::EvmTransactionRequest, TransactionError, U256},
    services::provider::evm::EvmProviderTrait,
};
use alloy::{
    primitives::{Address, Bytes, TxKind},
    rpc::types::{TransactionInput, TransactionRequest},
};

#[derive(Debug, Clone)]
pub struct OptimismFeeData {
    pub l1_base_fee: U256,
    pub base_fee: U256,
    pub decimals: U256,
    pub blob_base_fee: U256,
    pub base_fee_scalar: U256,
    pub blob_base_fee_scalar: U256,
}

/// Price parameter handler for Optimism-based networks
/// This calculates L1 data availability costs and adds them as extra fees
#[derive(Debug, Clone)]
pub struct OptimismPriceHandler<P> {
    provider: P,
    oracle_address: Address,
}

impl<P: EvmProviderTrait> OptimismPriceHandler<P> {
    pub fn new(provider: P) -> Self {
        Self {
            provider,
            oracle_address: OPTIMISM_GAS_PRICE_ORACLE_ADDRESS.parse().unwrap(),
        }
    }

    // Function selectors for Optimism GasPriceOracle
    // bytes4(keccak256("l1BaseFee()"))
    const FN_SELECTOR_L1_BASE_FEE: [u8; 4] = [81, 155, 75, 211];
    // bytes4(keccak256("baseFee()"))
    const FN_SELECTOR_BASE_FEE: [u8; 4] = [110, 242, 92, 58];
    // bytes4(keccak256("decimals()"))
    const FN_SELECTOR_DECIMALS: [u8; 4] = [49, 60, 229, 103];
    // bytes4(keccak256("blobBaseFee()"))
    const FN_SELECTOR_BLOB_BASE_FEE: [u8; 4] = [248, 32, 97, 64];
    // bytes4(keccak256("baseFeeScalar()"))
    const FN_SELECTOR_BASE_FEE_SCALAR: [u8; 4] = [197, 152, 89, 24];
    // bytes4(keccak256("blobBaseFeeScalar()"))
    const FN_SELECTOR_BLOB_BASE_FEE_SCALAR: [u8; 4] = [104, 213, 220, 166];

    fn create_contract_call(&self, selector: [u8; 4]) -> TransactionRequest {
        let mut data = Vec::with_capacity(4);
        data.extend_from_slice(&selector);
        TransactionRequest {
            to: Some(TxKind::Call(self.oracle_address)),
            input: TransactionInput::from(Bytes::from(data)),
            ..Default::default()
        }
    }

    async fn read_u256(&self, selector: [u8; 4]) -> Result<U256, TransactionError> {
        let call = self.create_contract_call(selector);
        let bytes = self
            .provider
            .call_contract(&call)
            .await
            .map_err(|e| TransactionError::UnexpectedError(e.to_string()))?;
        Ok(U256::from_be_slice(bytes.as_ref()))
    }

    fn calculate_compressed_tx_size(tx: &EvmTransactionRequest) -> U256 {
        let data_bytes: Vec<u8> = tx
            .data
            .as_ref()
            .and_then(|hex_str| hex::decode(hex_str.trim_start_matches("0x")).ok())
            .unwrap_or_default();

        let zero_bytes = U256::from(data_bytes.iter().filter(|&b| *b == 0).count());
        let non_zero_bytes = U256::from(data_bytes.len()) - zero_bytes;

        ((zero_bytes * U256::from(4)) + (non_zero_bytes * U256::from(16))) / U256::from(16)
    }

    pub async fn fetch_fee_data(&self) -> Result<OptimismFeeData, TransactionError> {
        let (l1_base_fee, base_fee, decimals, blob_base_fee, base_fee_scalar, blob_base_fee_scalar) =
            tokio::try_join!(
                self.read_u256(Self::FN_SELECTOR_L1_BASE_FEE),
                self.read_u256(Self::FN_SELECTOR_BASE_FEE),
                self.read_u256(Self::FN_SELECTOR_DECIMALS),
                self.read_u256(Self::FN_SELECTOR_BLOB_BASE_FEE),
                self.read_u256(Self::FN_SELECTOR_BASE_FEE_SCALAR),
                self.read_u256(Self::FN_SELECTOR_BLOB_BASE_FEE_SCALAR)
            )
            .map_err(|e| TransactionError::UnexpectedError(e.to_string()))?;

        Ok(OptimismFeeData {
            l1_base_fee,
            base_fee,
            decimals,
            blob_base_fee,
            base_fee_scalar,
            blob_base_fee_scalar,
        })
    }

    pub fn calculate_fee(
        &self,
        fee_data: &OptimismFeeData,
        tx: &EvmTransactionRequest,
    ) -> Result<U256, TransactionError> {
        let tx_compressed_size = Self::calculate_compressed_tx_size(tx);

        let weighted_gas_price = U256::from(16)
            .saturating_mul(U256::from(fee_data.base_fee_scalar))
            .saturating_mul(U256::from(fee_data.l1_base_fee))
            + U256::from(fee_data.blob_base_fee_scalar)
                .saturating_mul(U256::from(fee_data.blob_base_fee));

        Ok(tx_compressed_size.saturating_mul(weighted_gas_price))
    }

    pub async fn handle_price_params(
        &self,
        tx: &EvmTransactionRequest,
        mut original_params: PriceParams,
    ) -> Result<PriceParams, TransactionError> {
        // Fetch Optimism fee data and calculate L1 data cost
        let fee_data = self.fetch_fee_data().await?;
        let l1_data_cost = self.calculate_fee(&fee_data, tx)?;

        // Add the L1 data cost as extra fee
        original_params.extra_fee = Some(l1_data_cost);

        // Recalculate total cost with the extra fee
        let gas_limit = tx.gas_limit.unwrap_or(DEFAULT_GAS_LIMIT);
        let value = tx.value;
        let is_eip1559 = original_params.max_fee_per_gas.is_some();

        original_params.total_cost =
            original_params.calculate_total_cost(is_eip1559, gas_limit, value);

        Ok(original_params)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::provider::evm::MockEvmProviderTrait;

    #[tokio::test]
    async fn test_optimism_price_handler() {
        let mut mock_provider = MockEvmProviderTrait::new();

        // Mock all the contract calls for Optimism oracle
        mock_provider.expect_call_contract().returning(|_| {
            // Return mock data for oracle calls
            Box::pin(async { Ok(vec![0u8; 32].into()) })
        });

        let handler = OptimismPriceHandler::new(mock_provider);

        let tx = EvmTransactionRequest {
            to: Some("0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string()),
            value: U256::from(1_000_000_000_000_000_000u128),
            data: Some("0x1234567890abcdef".to_string()),
            gas_limit: Some(21000),
            gas_price: Some(20_000_000_000),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            speed: None,
            valid_until: None,
        };

        let original_params = PriceParams {
            gas_price: Some(20_000_000_000),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            is_min_bumped: None,
            extra_fee: None,
            total_cost: U256::ZERO,
        };

        let result = handler.handle_price_params(&tx, original_params).await;

        assert!(result.is_ok());
        let handled_params = result.unwrap();

        // Gas price should remain unchanged for Optimism (only extra fee is added)
        assert_eq!(handled_params.gas_price, Some(20_000_000_000));

        // Extra fee should be added
        assert!(handled_params.extra_fee.is_some());

        // Total cost should be recalculated
        assert!(handled_params.total_cost > U256::ZERO);
    }

    #[test]
    fn test_calculate_compressed_tx_size() {
        // Test with empty data
        let empty_tx = EvmTransactionRequest {
            to: Some("0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string()),
            value: U256::from(1_000_000_000_000_000_000u128),
            data: None,
            gas_limit: Some(21000),
            gas_price: Some(20_000_000_000),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            speed: None,
            valid_until: None,
        };

        let size =
            OptimismPriceHandler::<MockEvmProviderTrait>::calculate_compressed_tx_size(&empty_tx);
        assert_eq!(size, U256::ZERO);

        // Test with data containing zeros and non-zeros
        let data_tx = EvmTransactionRequest {
            data: Some("0x00001234".to_string()), // 2 zero bytes, 2 non-zero bytes
            ..empty_tx
        };

        let size =
            OptimismPriceHandler::<MockEvmProviderTrait>::calculate_compressed_tx_size(&data_tx);
        // Expected: ((2 * 4) + (2 * 16)) / 16 = (8 + 32) / 16 = 40 / 16 = 2.5 -> 2 (integer division)
        let expected =
            (U256::from(2) * U256::from(4) + U256::from(2) * U256::from(16)) / U256::from(16);
        assert_eq!(size, expected);
    }
}
