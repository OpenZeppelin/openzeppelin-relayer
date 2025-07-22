use thiserror::Error;

use crate::{
    constants::DEFAULT_EVM_MIN_BALANCE,
    models::{types::U256, RelayerEvmPolicy},
    services::EvmProviderTrait,
};

#[derive(Debug, Error)]
pub enum EvmTransactionValidationError {
    #[error("Provider error: {0}")]
    ProviderError(String),
    #[error("Validation error: {0}")]
    ValidationError(String),
    #[error("Insufficient balance: {0}")]
    InsufficientBalance(String),
}

pub struct EvmTransactionValidator {}

impl EvmTransactionValidator {
    pub async fn init_balance_validation(
        relayer_address: &str,
        policy: &RelayerEvmPolicy,
        provider: &impl EvmProviderTrait,
    ) -> Result<(), EvmTransactionValidationError> {
        let balance = provider
            .get_balance(relayer_address)
            .await
            .map_err(|e| EvmTransactionValidationError::ProviderError(e.to_string()))?;

        let min_balance = U256::from(policy.min_balance.unwrap_or(DEFAULT_EVM_MIN_BALANCE));

        if balance < min_balance {
            return Err(EvmTransactionValidationError::InsufficientBalance(format!(
                "Relayer balance ({}) is below minimum required balance ({})",
                balance, min_balance
            )));
        }

        Ok(())
    }

    pub async fn validate_sufficient_relayer_balance(
        balance_to_use: U256,
        relayer_address: &str,
        policy: &RelayerEvmPolicy,
        provider: &impl EvmProviderTrait,
    ) -> Result<(), EvmTransactionValidationError> {
        let balance = provider
            .get_balance(relayer_address)
            .await
            .map_err(|e| EvmTransactionValidationError::ProviderError(e.to_string()))?;

        let min_balance = U256::from(policy.min_balance.unwrap_or(DEFAULT_EVM_MIN_BALANCE));

        let remaining_balance = balance.saturating_sub(balance_to_use);

        // Check if balance is insufficient to cover transaction cost
        if balance < balance_to_use {
            return Err(EvmTransactionValidationError::InsufficientBalance(format!(
                "Relayer balance {balance} is insufficient to cover {balance_to_use}"
            )));
        }

        // Check if remaining balance would fall below minimum requirement
        if !min_balance.is_zero() && remaining_balance < min_balance {
            return Err(EvmTransactionValidationError::InsufficientBalance(
                format!("Relayer balance {balance} is insufficient to cover {balance_to_use}, with an enforced minimum balance of {}", policy.min_balance.unwrap_or(DEFAULT_EVM_MIN_BALANCE))
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::future::ready;

    use super::*;
    use crate::services::provider::evm::MockEvmProviderTrait;
    use crate::services::ProviderError;
    use mockall::predicate::*;

    fn create_test_policy(min_balance: u128) -> RelayerEvmPolicy {
        RelayerEvmPolicy {
            min_balance: Some(min_balance),
            gas_limit_estimation: Some(true),
            gas_price_cap: None,
            whitelist_receivers: None,
            eip1559_pricing: None,
            private_transactions: Some(false),
        }
    }

    #[tokio::test]
    async fn test_validate_sufficient_balance_routine_check_success() {
        let mut mock_provider = MockEvmProviderTrait::new();
        mock_provider
            .expect_get_balance()
            .with(eq("0xSender"))
            .returning(|_| Box::pin(ready(Ok(U256::from(200000000000000000u64))))); // 0.2 ETH

        let result = EvmTransactionValidator::validate_sufficient_relayer_balance(
            U256::ZERO,
            "0xSender",
            &create_test_policy(100000000000000000u128), // 0.1 ETH min balance
            &mock_provider,
        )
        .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validate_sufficient_balance_routine_check_failure() {
        let mut mock_provider = MockEvmProviderTrait::new();
        mock_provider
            .expect_get_balance()
            .with(eq("0xSender"))
            .returning(|_| Box::pin(ready(Ok(U256::from(50000000000000000u64))))); // 0.05 ETH

        let result = EvmTransactionValidator::validate_sufficient_relayer_balance(
            U256::ZERO,
            "0xSender",
            &create_test_policy(100000000000000000u128), // 0.1 ETH min balance
            &mock_provider,
        )
        .await;

        assert!(matches!(
            result,
            Err(EvmTransactionValidationError::InsufficientBalance(_))
        ));
    }

    #[tokio::test]
    async fn test_validate_sufficient_balance_with_transaction_success() {
        let mut mock_provider = MockEvmProviderTrait::new();
        mock_provider
            .expect_get_balance()
            .with(eq("0xSender"))
            .returning(|_| Box::pin(ready(Ok(U256::from(300000000000000000u64))))); // 0.3 ETH

        let result = EvmTransactionValidator::validate_sufficient_relayer_balance(
            U256::from(100000000000000000u64), // 0.1 ETH to use
            "0xSender",
            &create_test_policy(100000000000000000u128), // 0.1 ETH min balance
            &mock_provider,
        )
        .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validate_sufficient_balance_with_transaction_failure() {
        let mut mock_provider = MockEvmProviderTrait::new();
        mock_provider
            .expect_get_balance()
            .with(eq("0xSender"))
            .returning(|_| Box::pin(ready(Ok(U256::from(150000000000000000u64))))); // 0.15 ETH

        let result = EvmTransactionValidator::validate_sufficient_relayer_balance(
            U256::from(100000000000000000u64), // 0.1 ETH to use
            "0xSender",
            &create_test_policy(100000000000000000u128), // 0.1 ETH min balance
            &mock_provider,
        )
        .await;

        assert!(matches!(
            result,
            Err(EvmTransactionValidationError::InsufficientBalance(_))
        ));
    }

    #[tokio::test]
    async fn test_validate_provider_error() {
        let mut mock_provider = MockEvmProviderTrait::new();
        mock_provider
            .expect_get_balance()
            .with(eq("0xSender"))
            .returning(|_| {
                Box::pin(ready(Err(ProviderError::Other(
                    "Provider error".to_string(),
                ))))
            });

        let result = EvmTransactionValidator::validate_sufficient_relayer_balance(
            U256::ZERO,
            "0xSender",
            &create_test_policy(100000000000000000u128),
            &mock_provider,
        )
        .await;

        assert!(matches!(
            result,
            Err(EvmTransactionValidationError::ProviderError(_))
        ));
    }

    #[tokio::test]
    async fn test_validate_no_min_balance_success() {
        let mut mock_provider = MockEvmProviderTrait::new();
        mock_provider
            .expect_get_balance()
            .with(eq("0xSender"))
            .returning(|_| Box::pin(ready(Ok(U256::from(100000000000000000u64))))); // 0.1 ETH

        let result = EvmTransactionValidator::validate_sufficient_relayer_balance(
            U256::from(50000000000000000u64), // 0.05 ETH to use
            "0xSender",
            &create_test_policy(0), // No min balance
            &mock_provider,
        )
        .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validate_no_min_balance_failure() {
        let mut mock_provider = MockEvmProviderTrait::new();
        mock_provider
            .expect_get_balance()
            .with(eq("0xSender"))
            .returning(|_| Box::pin(ready(Ok(U256::from(100000000000000000u64))))); // 0.1 ETH

        let result = EvmTransactionValidator::validate_sufficient_relayer_balance(
            U256::from(150000000000000000u64), // 0.15 ETH to use
            "0xSender",
            &create_test_policy(0), // No min balance
            &mock_provider,
        )
        .await;

        assert!(matches!(
            result,
            Err(EvmTransactionValidationError::InsufficientBalance(_))
        ));
    }

    #[tokio::test]
    async fn test_init_balance_validation_success() {
        let mut mock_provider = MockEvmProviderTrait::new();
        mock_provider
            .expect_get_balance()
            .with(eq("0xSender"))
            .returning(|_| Box::pin(ready(Ok(U256::from(200000000000000000u64)))));

        let result = EvmTransactionValidator::init_balance_validation(
            "0xSender",
            &create_test_policy(100000000000000000u128),
            &mock_provider,
        )
        .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_init_balance_validation_failure() {
        let mut mock_provider = MockEvmProviderTrait::new();
        mock_provider
            .expect_get_balance()
            .with(eq("0xSender"))
            .returning(|_| Box::pin(ready(Ok(U256::from(50000000000000000u64)))));

        let result = EvmTransactionValidator::init_balance_validation(
            "0xSender",
            &create_test_policy(100000000000000000u128),
            &mock_provider,
        )
        .await;

        assert!(matches!(
            result,
            Err(EvmTransactionValidationError::InsufficientBalance(_))
        ));
    }

    #[tokio::test]
    async fn test_init_balance_validation_provider_error() {
        let mut mock_provider = MockEvmProviderTrait::new();
        mock_provider
            .expect_get_balance()
            .with(eq("0xSender"))
            .returning(|_| {
                Box::pin(ready(Err(ProviderError::Other(
                    "Provider error".to_string(),
                ))))
            });

        let result = EvmTransactionValidator::init_balance_validation(
            "0xSender",
            &create_test_policy(100000000000000000u128),
            &mock_provider,
        )
        .await;

        assert!(matches!(
            result,
            Err(EvmTransactionValidationError::ProviderError(_))
        ));
    }

    #[tokio::test]
    async fn test_init_balance_validation_zero_min_balance() {
        let mut mock_provider = MockEvmProviderTrait::new();
        mock_provider
            .expect_get_balance()
            .with(eq("0xSender"))
            .returning(|_| Box::pin(ready(Ok(U256::from(0u64)))));

        let result = EvmTransactionValidator::init_balance_validation(
            "0xSender",
            &create_test_policy(0), // No min balance
            &mock_provider,
        )
        .await;

        assert!(result.is_ok());
    }
}
