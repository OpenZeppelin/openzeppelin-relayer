use crate::constants::{MAXIMUM_NOOP_RETRY_ATTEMPTS, MAXIMUM_TX_ATTEMPTS};
use crate::models::{
    EvmTransactionData, TransactionError, TransactionRepoModel, TransactionStatus, U256,
};
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

/// Checks if a transaction is already a NOOP transaction
pub fn is_noop(evm_data: &EvmTransactionData) -> bool {
    evm_data.value == U256::from(0u64)
        && evm_data.data.as_ref().is_some_and(|data| data == "0x")
        && evm_data.to.as_ref() == Some(&evm_data.from)
        && evm_data.speed.is_some()
}

/// Checks if a transaction has too many attempts
pub fn too_many_attempts(tx: &TransactionRepoModel) -> bool {
    tx.hashes.len() > MAXIMUM_TX_ATTEMPTS
}

/// Checks if a transaction has too many NOOP attempts
pub fn too_many_noop_attempts(tx: &TransactionRepoModel) -> bool {
    tx.noop_count.unwrap_or(0) > MAXIMUM_NOOP_RETRY_ATTEMPTS
}

pub fn is_pending_transaction(tx_status: &TransactionStatus) -> bool {
    tx_status == &TransactionStatus::Pending
        || tx_status == &TransactionStatus::Sent
        || tx_status == &TransactionStatus::Submitted
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{evm::Speed, NetworkTransactionData};

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

    #[test]
    fn test_is_noop() {
        // Create a NOOP transaction
        let noop_tx = EvmTransactionData {
            from: "0x1234567890123456789012345678901234567890".to_string(),
            to: Some("0x1234567890123456789012345678901234567890".to_string()), // Same as from
            value: U256::from(0u64),
            data: Some("0x".to_string()),
            gas_limit: 21000,
            gas_price: Some(10_000_000_000),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            nonce: Some(42),
            signature: None,
            hash: None,
            speed: Some(Speed::Fast),
            chain_id: 1,
            raw: None,
        };
        assert!(is_noop(&noop_tx));

        // Test non-NOOP transactions
        let mut non_noop = noop_tx.clone();
        non_noop.value = U256::from(1000000000000000000u64); // 1 ETH
        assert!(!is_noop(&non_noop));

        let mut non_noop = noop_tx.clone();
        non_noop.data = Some("0x123456".to_string());
        assert!(!is_noop(&non_noop));

        let mut non_noop = noop_tx.clone();
        non_noop.to = Some("0x9876543210987654321098765432109876543210".to_string());
        assert!(!is_noop(&non_noop));

        let mut non_noop = noop_tx;
        non_noop.speed = None;
        assert!(!is_noop(&non_noop));
    }

    #[test]
    fn test_too_many_attempts() {
        let mut tx = TransactionRepoModel {
            id: "test-tx".to_string(),
            relayer_id: "test-relayer".to_string(),
            status: TransactionStatus::Pending,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            sent_at: None,
            confirmed_at: None,
            valid_until: None,
            network_type: crate::models::NetworkType::Evm,
            network_data: NetworkTransactionData::Evm(EvmTransactionData {
                from: "0x1234".to_string(),
                to: Some("0x5678".to_string()),
                value: U256::from(0u64),
                data: Some("0x".to_string()),
                gas_limit: 21000,
                gas_price: Some(10_000_000_000),
                max_fee_per_gas: None,
                max_priority_fee_per_gas: None,
                nonce: Some(42),
                signature: None,
                hash: None,
                speed: Some(Speed::Fast),
                chain_id: 1,
                raw: None,
            }),
            priced_at: None,
            hashes: vec![], // Start with no attempts
            noop_count: None,
            is_canceled: Some(false),
        };

        // Test with no attempts
        assert!(!too_many_attempts(&tx));

        // Test with maximum attempts
        tx.hashes = vec!["hash".to_string(); MAXIMUM_TX_ATTEMPTS];
        assert!(!too_many_attempts(&tx));

        // Test with too many attempts
        tx.hashes = vec!["hash".to_string(); MAXIMUM_TX_ATTEMPTS + 1];
        assert!(too_many_attempts(&tx));
    }

    #[test]
    fn test_too_many_noop_attempts() {
        let mut tx = TransactionRepoModel {
            id: "test-tx".to_string(),
            relayer_id: "test-relayer".to_string(),
            status: TransactionStatus::Pending,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            sent_at: None,
            confirmed_at: None,
            valid_until: None,
            network_type: crate::models::NetworkType::Evm,
            network_data: NetworkTransactionData::Evm(EvmTransactionData {
                from: "0x1234".to_string(),
                to: Some("0x5678".to_string()),
                value: U256::from(0u64),
                data: Some("0x".to_string()),
                gas_limit: 21000,
                gas_price: Some(10_000_000_000),
                max_fee_per_gas: None,
                max_priority_fee_per_gas: None,
                nonce: Some(42),
                signature: None,
                hash: None,
                speed: Some(Speed::Fast),
                chain_id: 1,
                raw: None,
            }),
            priced_at: None,
            hashes: vec![],
            noop_count: None,
            is_canceled: Some(false),
        };

        // Test with no NOOP attempts
        assert!(!too_many_noop_attempts(&tx));

        // Test with maximum NOOP attempts
        tx.noop_count = Some(MAXIMUM_NOOP_RETRY_ATTEMPTS);
        assert!(!too_many_noop_attempts(&tx));

        // Test with too many NOOP attempts
        tx.noop_count = Some(MAXIMUM_NOOP_RETRY_ATTEMPTS + 1);
        assert!(too_many_noop_attempts(&tx));
    }
}
