use crate::{
    constants::OPTIMISM_GAS_PRICE_ORACLE_ADDRESS,
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

#[derive(Debug, Clone)]
pub struct OptimismExtraFeeService<P> {
    provider: P,
    oracle_address: Address,
}

impl<P: EvmProviderTrait> OptimismExtraFeeService<P> {
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
}

impl<P: EvmProviderTrait> OptimismExtraFeeService<P> {
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
}
