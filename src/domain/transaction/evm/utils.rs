use crate::models::{EvmTransactionData, TransactionError, TransactionStatus, U256};
use eyre::Result;

/// Updates an existing transaction to be a "noop" transaction (transaction to self with zero value and no data)
/// This is commonly used for cancellation and replacement transactions
pub async fn make_noop(evm_data: &mut EvmTransactionData) -> Result<(), TransactionError> {
    // Update the transaction to be a noop
    evm_data.gas_limit = 21_000;
    evm_data.value = U256::from(0u64);
    evm_data.data = Some("0x".to_string());
    evm_data.to = Some(evm_data.from.clone());

    Ok(())
}

pub fn is_pending_transaction(tx_status: &TransactionStatus) -> bool {
    tx_status == &TransactionStatus::Pending
        || tx_status == &TransactionStatus::Sent
        || tx_status == &TransactionStatus::Submitted
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::evm::Speed;

    #[tokio::test]
    async fn test_make_noop_standard_network() {
        let mut evm_data = EvmTransactionData {
            from: "0x1234567890123456789012345678901234567890".to_string(),
            to: Some("0xoriginal_destination".to_string()),
            value: U256::from(1000000000000000000u64), // 1 ETH
            data: Some("0xoriginal_data".to_string()),
            gas_limit: 50000,
            gas_price: Some(10_000_000_000),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            nonce: Some(42),
            signature: None,
            hash: Some("0xoriginal_hash".to_string()),
            speed: Some(Speed::Fast),
            chain_id: 1,
            raw: Some(vec![1, 2, 3]),
        };

        let result = make_noop(&mut evm_data).await;
        assert!(result.is_ok());

        // Verify the transaction was updated correctly
        assert_eq!(evm_data.gas_limit, 21_000); // Standard gas limit
        assert_eq!(evm_data.to.unwrap(), evm_data.from); // Should send to self
        assert_eq!(evm_data.value, U256::from(0u64)); // Zero value
        assert_eq!(evm_data.data.unwrap(), "0x"); // Empty data
        assert_eq!(evm_data.nonce, Some(42)); // Original nonce preserved
    }
}
