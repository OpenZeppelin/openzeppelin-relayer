use crate::constants::{
    DEFAULT_TX_VALID_TIMESPAN, MAXIMUM_NOOP_RETRY_ATTEMPTS, MAXIMUM_TX_ATTEMPTS,
};
use crate::models::{
    EvmNetwork, EvmTransactionData, TransactionError, TransactionRepoModel, TransactionStatus, U256,
};
use chrono::{DateTime, Duration, Utc};
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

/// Helper function to check if a transaction has enough confirmations.
pub fn has_enough_confirmations(
    tx_block_number: u64,
    current_block_number: u64,
    chain_id: u64,
) -> bool {
    let network = EvmNetwork::from_id(chain_id);
    let required_confirmations = network.required_confirmations();
    current_block_number >= tx_block_number + required_confirmations
}

/// Checks if a transaction is still valid based on its valid_until timestamp.
pub fn is_transaction_valid(created_at: &str, valid_until: &Option<String>) -> bool {
    if let Some(valid_until_str) = valid_until {
        match DateTime::parse_from_rfc3339(valid_until_str) {
            Ok(valid_until_time) => return Utc::now() < valid_until_time,
            Err(e) => {
                log::warn!("Failed to parse valid_until timestamp: {}", e);
                return false;
            }
        }
    }
    match DateTime::parse_from_rfc3339(created_at) {
        Ok(created_time) => {
            let default_valid_until =
                created_time + Duration::milliseconds(DEFAULT_TX_VALID_TIMESPAN);
            Utc::now() < default_valid_until
        }
        Err(e) => {
            log::warn!("Failed to parse created_at timestamp: {}", e);
            false
        }
    }
}

/// Gets the age of a transaction since it was sent.
pub fn get_age_of_sent_at(tx: &TransactionRepoModel) -> Result<Duration, TransactionError> {
    let now = Utc::now();
    let sent_at_str = tx.sent_at.as_ref().ok_or_else(|| {
        TransactionError::UnexpectedError("Transaction sent_at time is missing".to_string())
    })?;
    let sent_time = DateTime::parse_from_rfc3339(sent_at_str)
        .map_err(|_| TransactionError::UnexpectedError("Error parsing sent_at time".to_string()))?
        .with_timezone(&Utc);
    Ok(now.signed_duration_since(sent_time))
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

    #[test]
    fn test_has_enough_confirmations() {
        // Test Ethereum Mainnet (requires 12 confirmations)
        let chain_id = 1; // Ethereum Mainnet

        // Not enough confirmations
        let tx_block_number = 100;
        let current_block_number = 110; // Only 10 confirmations
        assert!(!has_enough_confirmations(
            tx_block_number,
            current_block_number,
            chain_id
        ));

        // Exactly enough confirmations
        let current_block_number = 112; // Exactly 12 confirmations
        assert!(has_enough_confirmations(
            tx_block_number,
            current_block_number,
            chain_id
        ));

        // More than enough confirmations
        let current_block_number = 120; // 20 confirmations
        assert!(has_enough_confirmations(
            tx_block_number,
            current_block_number,
            chain_id
        ));
    }

    #[test]
    fn test_is_transaction_valid_with_future_timestamp() {
        let now = Utc::now();
        let valid_until = Some((now + Duration::hours(1)).to_rfc3339());
        let created_at = now.to_rfc3339();

        assert!(is_transaction_valid(&created_at, &valid_until));
    }

    #[test]
    fn test_is_transaction_valid_with_past_timestamp() {
        let now = Utc::now();
        let valid_until = Some((now - Duration::hours(1)).to_rfc3339());
        let created_at = now.to_rfc3339();

        assert!(!is_transaction_valid(&created_at, &valid_until));
    }

    #[test]
    fn test_is_transaction_valid_with_valid_until() {
        // Test with valid_until in the future
        let created_at = Utc::now().to_rfc3339();
        let valid_until = Some((Utc::now() + Duration::hours(1)).to_rfc3339());
        assert!(is_transaction_valid(&created_at, &valid_until));

        // Test with valid_until in the past
        let valid_until = Some((Utc::now() - Duration::hours(1)).to_rfc3339());
        assert!(!is_transaction_valid(&created_at, &valid_until));

        // Test with valid_until exactly at current time (should be invalid)
        let valid_until = Some(Utc::now().to_rfc3339());
        assert!(!is_transaction_valid(&created_at, &valid_until));

        // Test with valid_until very far in the future
        let valid_until = Some((Utc::now() + Duration::days(365)).to_rfc3339());
        assert!(is_transaction_valid(&created_at, &valid_until));

        // Test with invalid valid_until format
        let valid_until = Some("invalid-date-format".to_string());
        assert!(!is_transaction_valid(&created_at, &valid_until));

        // Test with empty valid_until string
        let valid_until = Some("".to_string());
        assert!(!is_transaction_valid(&created_at, &valid_until));
    }

    #[test]
    fn test_is_transaction_valid_without_valid_until() {
        // Test with created_at within the default timespan
        let created_at = Utc::now().to_rfc3339();
        let valid_until = None;
        assert!(is_transaction_valid(&created_at, &valid_until));

        // Test with created_at older than the default timespan (8 hours)
        let old_created_at =
            (Utc::now() - Duration::milliseconds(DEFAULT_TX_VALID_TIMESPAN + 1000)).to_rfc3339();
        assert!(!is_transaction_valid(&old_created_at, &valid_until));

        // Test with created_at exactly at the boundary
        let boundary_created_at =
            (Utc::now() - Duration::milliseconds(DEFAULT_TX_VALID_TIMESPAN)).to_rfc3339();
        assert!(!is_transaction_valid(&boundary_created_at, &valid_until));

        // Test with created_at just within the default timespan
        let within_boundary_created_at =
            (Utc::now() - Duration::milliseconds(DEFAULT_TX_VALID_TIMESPAN - 1000)).to_rfc3339();
        assert!(is_transaction_valid(
            &within_boundary_created_at,
            &valid_until
        ));

        // Test with invalid created_at format
        let invalid_created_at = "invalid-date-format";
        assert!(!is_transaction_valid(invalid_created_at, &valid_until));

        // Test with empty created_at string
        assert!(!is_transaction_valid("", &valid_until));
    }
}
