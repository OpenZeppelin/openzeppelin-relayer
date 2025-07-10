use crate::constants::{
    COMPLEX_GAS_LIMIT, DEFAULT_GAS_LIMIT, DEFAULT_TRANSACTION_SPEED, ERC20_TRANSFER_GAS_LIMIT,
    ERC721_TRANSFER_GAS_LIMIT,
};
use crate::models::evm::Speed;
use crate::models::EvmTransactionData;
use crate::utils::time::minutes_ms;

/// Gets the resubmit timeout for a given speed
/// Returns the timeout in milliseconds based on the speed:
/// - SafeLow: 10 minutes
/// - Average: 5 minutes
/// - Fast: 3 minutes
/// - Fastest: 2 minutes
///   If no speed is provided, uses the default transaction speed
pub fn get_resubmit_timeout_for_speed(speed: &Option<Speed>) -> i64 {
    let speed_value = speed.clone().unwrap_or(DEFAULT_TRANSACTION_SPEED);

    match speed_value {
        Speed::SafeLow => minutes_ms(10),
        Speed::Average => minutes_ms(5),
        Speed::Fast => minutes_ms(3),
        Speed::Fastest => minutes_ms(2),
    }
}

/// Calculates the resubmit age with exponential backoff
///
/// # Arguments
/// * `timeout` - The base timeout in milliseconds
/// * `attempts` - The number of attempts made so far
///
/// # Returns
/// The new timeout with exponential backoff applied: timeout * 2^(attempts-1)
pub fn get_resubmit_timeout_with_backoff(timeout: i64, attempts: usize) -> i64 {
    if attempts <= 1 {
        timeout
    } else {
        timeout * 2_i64.pow((attempts - 1) as u32)
    }
}

/// Gets the default gas limit for a given transaction
///
/// # Arguments
/// * `tx` - The transaction data
///
/// # Returns
/// The default gas limit for the transaction
pub fn get_evm_default_gas_limit_for_tx(tx: &EvmTransactionData) -> u64 {
    if tx.data.is_none() {
        DEFAULT_GAS_LIMIT
    } else if tx.data.as_ref().unwrap().starts_with("0xa9059cbb") {
        ERC20_TRANSFER_GAS_LIMIT
    } else if tx.data.as_ref().unwrap().starts_with("0x23b872dd") {
        ERC721_TRANSFER_GAS_LIMIT
    } else {
        COMPLEX_GAS_LIMIT
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::evm::Speed;
    use crate::models::EvmTransactionData;

    #[test]
    fn test_get_resubmit_timeout_for_speed() {
        // Test with existing speeds
        assert_eq!(
            get_resubmit_timeout_for_speed(&Some(Speed::SafeLow)),
            minutes_ms(10)
        );
        assert_eq!(
            get_resubmit_timeout_for_speed(&Some(Speed::Average)),
            minutes_ms(5)
        );
        assert_eq!(
            get_resubmit_timeout_for_speed(&Some(Speed::Fast)),
            minutes_ms(3)
        );
        assert_eq!(
            get_resubmit_timeout_for_speed(&Some(Speed::Fastest)),
            minutes_ms(2)
        );

        // Test with None speed (should return default)
        assert_eq!(
            get_resubmit_timeout_for_speed(&None),
            minutes_ms(3) // DEFAULT_TRANSACTION_SPEED is Speed::Fast
        );
    }

    #[test]
    fn test_get_resubmit_timeout_with_backoff() {
        let base_timeout = 300000; // 5 minutes in ms

        // First attempt - no backoff
        assert_eq!(get_resubmit_timeout_with_backoff(base_timeout, 1), 300000);

        // Second attempt - 2x backoff
        assert_eq!(get_resubmit_timeout_with_backoff(base_timeout, 2), 600000);

        // Third attempt - 4x backoff
        assert_eq!(get_resubmit_timeout_with_backoff(base_timeout, 3), 1200000);

        // Fourth attempt - 8x backoff
        assert_eq!(get_resubmit_timeout_with_backoff(base_timeout, 4), 2400000);

        // Edge case - attempt 0 should be treated as attempt 1
        assert_eq!(get_resubmit_timeout_with_backoff(base_timeout, 0), 300000);
    }

    #[test]
    fn test_get_evm_default_gas_limit_for_tx_no_data() {
        let tx = EvmTransactionData {
            from: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            to: Some("0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed".to_string()),
            value: crate::models::U256::from(1000000000000000000u128),
            data: None,
            gas_limit: None,
            gas_price: Some(20_000_000_000),
            nonce: Some(1),
            chain_id: 1,
            hash: None,
            signature: None,
            speed: Some(Speed::Average),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            raw: None,
        };

        assert_eq!(get_evm_default_gas_limit_for_tx(&tx), DEFAULT_GAS_LIMIT);
    }

    #[test]
    fn test_get_evm_default_gas_limit_for_tx_erc20_transfer() {
        let tx = EvmTransactionData {
            from: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            to: Some("0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed".to_string()),
            value: crate::models::U256::from(0u128),
            data: Some("0xa9059cbb000000000000000000000000742d35cc6634c0532925a3b844bc454e4438f44e0000000000000000000000000000000000000000000000000de0b6b3a7640000".to_string()),
            gas_limit: None,
            gas_price: Some(20_000_000_000),
            nonce: Some(1),
            chain_id: 1,
            hash: None,
            signature: None,
            speed: Some(Speed::Average),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            raw: None,
        };

        assert_eq!(
            get_evm_default_gas_limit_for_tx(&tx),
            ERC20_TRANSFER_GAS_LIMIT
        );
    }

    #[test]
    fn test_get_evm_default_gas_limit_for_tx_transfer_from() {
        let tx = EvmTransactionData {
            from: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            to: Some("0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed".to_string()),
            value: crate::models::U256::from(0u128),
            data: Some("0x23b872dd000000000000000000000000742d35cc6634c0532925a3b844bc454e4438f44e0000000000000000000000005aaeb6053f3e94c9b9a09f33669435e7ef1beaed0000000000000000000000000000000000000000000000000de0b6b3a7640000".to_string()),
            gas_limit: None,
            gas_price: Some(20_000_000_000),
            nonce: Some(1),
            chain_id: 1,
            hash: None,
            signature: None,
            speed: Some(Speed::Average),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            raw: None,
        };

        assert_eq!(
            get_evm_default_gas_limit_for_tx(&tx),
            ERC721_TRANSFER_GAS_LIMIT
        );
    }

    #[test]
    fn test_get_evm_default_gas_limit_for_tx_complex_transaction() {
        let tx = EvmTransactionData {
            from: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            to: Some("0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed".to_string()),
            value: crate::models::U256::from(0u128),
            data: Some("0x095ea7b3000000000000000000000000742d35cc6634c0532925a3b844bc454e4438f44e0000000000000000000000000000000000000000000000000de0b6b3a7640000".to_string()),
            gas_limit: None,
            gas_price: Some(20_000_000_000),
            nonce: Some(1),
            chain_id: 1,
            hash: None,
            signature: None,
            speed: Some(Speed::Average),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            raw: None,
        };

        assert_eq!(get_evm_default_gas_limit_for_tx(&tx), COMPLEX_GAS_LIMIT);
    }

    #[test]
    fn test_get_evm_default_gas_limit_for_tx_empty_data() {
        let tx = EvmTransactionData {
            from: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            to: Some("0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed".to_string()),
            value: crate::models::U256::from(1000000000000000000u128),
            data: Some("0x".to_string()),
            gas_limit: None,
            gas_price: Some(20_000_000_000),
            nonce: Some(1),
            chain_id: 1,
            hash: None,
            signature: None,
            speed: Some(Speed::Average),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            raw: None,
        };

        assert_eq!(get_evm_default_gas_limit_for_tx(&tx), COMPLEX_GAS_LIMIT);
    }

    #[test]
    fn test_get_evm_default_gas_limit_for_tx_malformed_data() {
        let tx = EvmTransactionData {
            from: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            to: Some("0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed".to_string()),
            value: crate::models::U256::from(0u128),
            data: Some("0xa9059c".to_string()), // Short data that starts with ERC20 transfer but is incomplete
            gas_limit: None,
            gas_price: Some(20_000_000_000),
            nonce: Some(1),
            chain_id: 1,
            hash: None,
            signature: None,
            speed: Some(Speed::Average),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            raw: None,
        };

        assert_eq!(get_evm_default_gas_limit_for_tx(&tx), COMPLEX_GAS_LIMIT);
    }

    #[test]
    fn test_get_evm_default_gas_limit_for_tx_partial_signature_match() {
        // Test with data that starts with ERC20 transfer signature but has additional data
        let tx = EvmTransactionData {
            from: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            to: Some("0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed".to_string()),
            value: crate::models::U256::from(0u128),
            data: Some("0xa9059cbb000000000000000000000000742d35cc6634c0532925a3b844bc454e4438f44e0000000000000000000000000000000000000000000000000de0b6b3a764000000000000000000000000000000000000000000000000000000000000000000001".to_string()),
            gas_limit: None,
            gas_price: Some(20_000_000_000),
            nonce: Some(1),
            chain_id: 1,
            hash: None,
            signature: None,
            speed: Some(Speed::Average),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            raw: None,
        };

        // Should still match ERC20 transfer since it starts with the signature
        assert_eq!(
            get_evm_default_gas_limit_for_tx(&tx),
            ERC20_TRANSFER_GAS_LIMIT
        );
    }

    #[test]
    fn test_get_evm_default_gas_limit_for_tx_case_sensitivity() {
        // Test with uppercase hex data
        let tx = EvmTransactionData {
            from: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            to: Some("0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed".to_string()),
            value: crate::models::U256::from(0u128),
            data: Some("0xA9059CBB000000000000000000000000742D35CC6634C0532925A3B844BC454E4438F44E0000000000000000000000000000000000000000000000000DE0B6B3A7640000".to_string()),
            gas_limit: None,
            gas_price: Some(20_000_000_000),
            nonce: Some(1),
            chain_id: 1,
            hash: None,
            signature: None,
            speed: Some(Speed::Average),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            raw: None,
        };

        // Should not match since the function signature is case-sensitive
        assert_eq!(get_evm_default_gas_limit_for_tx(&tx), COMPLEX_GAS_LIMIT);
    }
}
