use alloy::{
    consensus::{TxEip1559, TxLegacy},
    hex::FromHex,
    primitives::{Address, Bytes},
    rpc::types::{TransactionInput, TransactionRequest},
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use solana_sdk::packet::Encode;

use crate::{
    constants::OPTIMISM_GAS_PRICE_ORACLE_ADDRESS,
    models::{EvmTransactionData, EvmTransactionDataTrait, TransactionError, U256},
    services::EvmProviderTrait,
};

use super::NetworkExtraFeeCalculatorServiceTrait;

// Function selectors as constants
const FN_SELECTOR_L1_BASE_FEE: [u8; 4] = [81, 155, 75, 211]; // bytes4(keccak256("l1BaseFee()"))
const FN_SELECTOR_BASE_FEE: [u8; 4] = [110, 242, 92, 58]; // bytes4(keccak256("baseFee()"))
const FN_SELECTOR_DECIMALS: [u8; 4] = [49, 60, 229, 103]; // bytes4(keccak256("decimals()"))
const FN_SELECTOR_BLOB_BASE_FEE: [u8; 4] = [248, 32, 97, 64]; // bytes4(keccak256("blobBaseFee()"))
const FN_SELECTOR_BASE_FEE_SCALAR: [u8; 4] = [197, 152, 89, 24]; // bytes4(keccak256("baseFeeScalar()"))
const FN_SELECTOR_BLOB_BASE_FEE_SCALAR: [u8; 4] = [104, 213, 220, 166]; // bytes4(keccak256("blobBaseFeeScalar()"))

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

pub struct OptimismExtraFeeService<P> {
    provider: P,
}

impl<P> OptimismExtraFeeService<P> {
    /// Create a new Optimism extra fee service
    ///
    /// # Arguments
    ///
    /// * `provider` - The provider to get the extra fee for
    ///
    pub fn new(provider: P) -> Self {
        Self { provider }
    }
}

impl<P: EvmProviderTrait> OptimismExtraFeeService<P> {
    /// Create a contract call for the given function selector
    ///
    /// # Arguments
    ///
    /// * `bytes_fn_selector` - The function selector to create a contract call for
    ///
    /// # Returns
    fn create_contract_call(
        &self,
        bytes_fn_selector: Vec<u8>,
    ) -> Result<TransactionRequest, TransactionError> {
        let oracle_address = Address::from_hex(OPTIMISM_GAS_PRICE_ORACLE_ADDRESS)
            .map_err(|e| TransactionError::UnexpectedError(e.to_string()))?;

        let fn_selector = Bytes::from(bytes_fn_selector);
        let tx = TransactionRequest::default()
            .to(oracle_address)
            .input(TransactionInput::new(fn_selector));

        Ok(tx)
    }

    /// Get the fee for the given function selector
    ///
    /// # Arguments
    ///
    /// * `fn_selector` - The function selector to get the fee for
    ///
    /// # Returns
    async fn get_fee(&self, fn_selector: Vec<u8>) -> Result<U256, TransactionError> {
        let tx = self.create_contract_call(fn_selector)?;
        let result = self.provider.call_contract(&tx).await?;
        Ok(U256::from_be_slice(result.as_ref()))
    }

    /// Get the price modifiers for optimism
    ///
    /// # Returns
    ///
    /// A `Result` containing the price modifiers or a `TransactionError`.
    pub async fn get_modifiers(&self) -> Result<OptimismModifiers, TransactionError> {
        let (l1_base_fee, base_fee, decimals, blob_base_fee, base_fee_scalar, blob_base_fee_scalar) =
            tokio::try_join!(
                self.get_fee(FN_SELECTOR_L1_BASE_FEE.to_vec()),
                self.get_fee(FN_SELECTOR_BASE_FEE.to_vec()),
                self.get_fee(FN_SELECTOR_DECIMALS.to_vec()),
                self.get_fee(FN_SELECTOR_BLOB_BASE_FEE.to_vec()),
                self.get_fee(FN_SELECTOR_BASE_FEE_SCALAR.to_vec()),
                self.get_fee(FN_SELECTOR_BLOB_BASE_FEE_SCALAR.to_vec()),
            )
            .map_err(|e| TransactionError::UnexpectedError(e.to_string()))?;

        let base_fee_scalar: u32 = base_fee_scalar.try_into().map_err(|e| {
            TransactionError::UnexpectedError(format!("Failed to convert base fee scalar: {}", e))
        })?;

        let blob_base_fee_scalar: u32 = blob_base_fee_scalar.try_into().map_err(|e| {
            TransactionError::UnexpectedError(format!(
                "Failed to convert blob base fee scalar: {}",
                e
            ))
        })?;

        Ok(OptimismModifiers {
            l1_base_fee,
            base_fee,
            decimals,
            blob_base_fee,
            base_fee_scalar,
            blob_base_fee_scalar,
        })
    }
}

#[async_trait]
impl<P: EvmProviderTrait> NetworkExtraFeeCalculatorServiceTrait for OptimismExtraFeeService<P> {
    /// Get the extra fee for the given transaction data
    ///
    /// # Arguments
    ///
    /// * `tx_data` - The transaction data to get the extra fee for
    ///
    async fn get_extra_fee(&self, tx_data: &EvmTransactionData) -> Result<U256, TransactionError> {
        let bytes = if tx_data.is_eip1559() {
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

        // Ecotone L1 Data Fee Calculation
        // https://docs.optimism.io/stack/transactions/fees#ecotone
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::{MockEvmProviderTrait, ProviderError};
    use alloy::primitives::TxKind;

    fn setup_mock_provider_for_modifiers() -> MockEvmProviderTrait {
        let mut mock_provider = MockEvmProviderTrait::new();

        let l1_base_fee_bytes = U256::from(10_000_000_000u64).to_be_bytes::<32>();
        mock_provider
            .expect_call_contract()
            .times(1)
            .returning(move |_| {
                Box::pin(async move { Ok(Bytes::from(l1_base_fee_bytes.to_vec())) })
            });

        let base_fee_bytes = U256::from(1_000_000_000u64).to_be_bytes::<32>();
        mock_provider
            .expect_call_contract()
            .times(1)
            .returning(move |_| Box::pin(async move { Ok(Bytes::from(base_fee_bytes.to_vec())) }));

        let decimals_bytes = U256::from(9u64).to_be_bytes::<32>();
        mock_provider
            .expect_call_contract()
            .times(1)
            .returning(move |_| Box::pin(async move { Ok(Bytes::from(decimals_bytes.to_vec())) }));

        let blob_base_fee_bytes = U256::from(100u64).to_be_bytes::<32>();
        mock_provider
            .expect_call_contract()
            .times(1)
            .returning(move |_| {
                Box::pin(async move { Ok(Bytes::from(blob_base_fee_bytes.to_vec())) })
            });

        let base_fee_scalar_bytes = U256::from(684000u64).to_be_bytes::<32>();
        mock_provider
            .expect_call_contract()
            .times(1)
            .returning(move |_| {
                Box::pin(async move { Ok(Bytes::from(base_fee_scalar_bytes.to_vec())) })
            });

        let blob_base_fee_scalar_bytes = U256::from(50000u64).to_be_bytes::<32>();
        mock_provider
            .expect_call_contract()
            .times(1)
            .returning(move |_| {
                Box::pin(async move { Ok(Bytes::from(blob_base_fee_scalar_bytes.to_vec())) })
            });

        mock_provider
    }

    fn create_test_evm_transaction_data(is_eip1559: bool) -> EvmTransactionData {
        let mut tx_data = EvmTransactionData {
            from: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            to: Some("0xa24Cea55A6171FbA0935c9e171c4Efe5Ba28DF91".to_string()),
            value: U256::from(1000000000),
            data: Some("0x0123".to_string()),
            nonce: Some(1),
            chain_id: 10,
            gas_limit: Some(21000),
            hash: None,
            signature: None,
            speed: None,
            raw: None,
            gas_price: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
        };

        if is_eip1559 {
            tx_data.max_fee_per_gas = Some(30000000000);
            tx_data.max_priority_fee_per_gas = Some(2000000000);
        } else {
            tx_data.gas_price = Some(20000000000);
        }

        tx_data
    }

    #[test]
    fn test_create_contract_call() {
        let mock_provider = MockEvmProviderTrait::new();
        let service = OptimismExtraFeeService::new(mock_provider);

        let result = service.create_contract_call(FN_SELECTOR_L1_BASE_FEE.to_vec());
        assert!(result.is_ok());

        let tx_request = result.unwrap();

        let expected_address = Address::from_hex(OPTIMISM_GAS_PRICE_ORACLE_ADDRESS).unwrap();
        assert_eq!(tx_request.to, Some(TxKind::Call(expected_address)));

        assert!(matches!(tx_request.input, TransactionInput { .. }));
    }

    #[tokio::test]
    async fn test_get_modifiers() {
        let mock_provider = setup_mock_provider_for_modifiers();
        let service = OptimismExtraFeeService::new(mock_provider);

        let modifiers = service.get_modifiers().await;
        assert!(
            modifiers.is_ok(),
            "Failed to get modifiers: {:?}",
            modifiers.err()
        );

        let modifiers = modifiers.unwrap();

        assert_eq!(
            modifiers.l1_base_fee,
            U256::from(10_000_000_000u64),
            "L1 base fee mismatch"
        );
        assert_eq!(
            modifiers.base_fee,
            U256::from(1_000_000_000u64),
            "Base fee mismatch"
        );
        assert_eq!(modifiers.decimals, U256::from(9u64), "Decimals mismatch");
        assert_eq!(
            modifiers.blob_base_fee,
            U256::from(100u64),
            "Blob base fee mismatch"
        );
        assert_eq!(
            modifiers.base_fee_scalar, 684000,
            "Base fee scalar mismatch"
        );
        assert_eq!(
            modifiers.blob_base_fee_scalar, 50000,
            "Blob base fee scalar mismatch"
        );
    }

    #[tokio::test]
    async fn test_get_extra_fee_eip1559_transaction() {
        let mock_provider = setup_mock_provider_for_modifiers();
        let service = OptimismExtraFeeService::new(mock_provider);

        let tx_data = create_test_evm_transaction_data(true);
        let extra_fee = service.get_extra_fee(&tx_data).await;

        assert!(
            extra_fee.is_ok(),
            "Failed to get extra fee: {:?}",
            extra_fee.err()
        );

        let extra_fee = extra_fee.unwrap();
        assert!(
            extra_fee > U256::ZERO,
            "Extra fee should be greater than zero"
        );
    }

    #[tokio::test]
    async fn test_get_extra_fee_legacy_transaction() {
        let mock_provider = setup_mock_provider_for_modifiers();
        let service = OptimismExtraFeeService::new(mock_provider);

        let tx_data = create_test_evm_transaction_data(false);
        let extra_fee = service.get_extra_fee(&tx_data).await;

        assert!(
            extra_fee.is_ok(),
            "Failed to get extra fee: {:?}",
            extra_fee.err()
        );

        let extra_fee = extra_fee.unwrap();
        assert!(
            extra_fee > U256::ZERO,
            "Extra fee should be greater than zero"
        );
    }

    #[tokio::test]
    async fn test_get_modifiers_error_handling() {
        let mut mock_provider = MockEvmProviderTrait::new();

        mock_provider.expect_call_contract().returning(|_| {
            Box::pin(async { Err(ProviderError::Other("Simulated RPC error".to_string())) })
        });

        let service = OptimismExtraFeeService::new(mock_provider);
        let result = service.get_modifiers().await;

        assert!(result.is_err());
        if let Err(e) = result {
            match e {
                TransactionError::UnexpectedError(msg) => {
                    assert!(msg.contains("Simulated RPC error"));
                }
                _ => panic!("Expected UnexpectedError but got {:?}", e),
            }
        }
    }
}
