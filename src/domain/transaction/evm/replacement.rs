//! This module contains the replacement and resubmission functionality for EVM transactions.
//! It includes methods for determining replacement pricing, validating price bumps,
//! and handling transaction compatibility checks.

use crate::{
    constants::{DEFAULT_EVM_GAS_PRICE_CAP, DEFAULT_GAS_LIMIT},
    domain::transaction::evm::price_calculator::{calculate_min_bump, PriceCalculatorTrait},
    models::{
        EvmTransactionData, EvmTransactionDataTrait, RelayerRepoModel, TransactionError, U256,
    },
};

use super::PriceParams;

/// Checks if an EVM transaction data has explicit prices.
///
/// # Arguments
///
/// * `evm_data` - The EVM transaction data to check
///
/// # Returns
///
/// A `bool` indicating whether the transaction data has explicit prices.
pub fn has_explicit_prices(evm_data: &EvmTransactionData) -> bool {
    evm_data.gas_price.is_some()
        || evm_data.max_fee_per_gas.is_some()
        || evm_data.max_priority_fee_per_gas.is_some()
}

/// Checks if an old transaction and new transaction request are compatible for replacement.
///
/// # Arguments
///
/// * `old_evm_data` - The EVM transaction data from the old transaction
/// * `new_evm_data` - The EVM transaction data for the new transaction
///
/// # Returns
///
/// A `Result` indicating compatibility or a `TransactionError` if incompatible.
pub fn check_transaction_compatibility(
    old_evm_data: &EvmTransactionData,
    new_evm_data: &EvmTransactionData,
) -> Result<(), TransactionError> {
    let old_is_legacy = old_evm_data.is_legacy();
    let new_is_legacy = new_evm_data.is_legacy();
    let new_is_eip1559 = new_evm_data.is_eip1559();

    // Allow replacement if new transaction has no explicit prices (will use market prices)
    if !has_explicit_prices(new_evm_data) {
        return Ok(());
    }

    // Check incompatible combinations when explicit prices are provided
    if old_is_legacy && new_is_eip1559 {
        return Err(TransactionError::ValidationError(
            "Cannot replace legacy transaction with EIP1559 transaction".to_string(),
        ));
    }

    if !old_is_legacy && new_is_legacy {
        return Err(TransactionError::ValidationError(
            "Cannot replace EIP1559 transaction with legacy transaction".to_string(),
        ));
    }

    Ok(())
}

/// Determines the pricing strategy for a replacement transaction.
///
/// # Arguments
///
/// * `old_evm_data` - The EVM transaction data from the old transaction
/// * `new_evm_data` - The EVM transaction data for the new transaction
/// * `relayer` - The relayer model for policy validation
/// * `price_calculator` - The price calculator instance
/// * `network_lacks_mempool` - Whether the network lacks mempool (skips bump validation)
///
/// # Returns
///
/// A `Result` containing the price parameters or a `TransactionError`.
pub async fn determine_replacement_pricing<PC: PriceCalculatorTrait>(
    old_evm_data: &EvmTransactionData,
    new_evm_data: &EvmTransactionData,
    relayer: &RelayerRepoModel,
    price_calculator: &PC,
    network_lacks_mempool: bool,
) -> Result<PriceParams, TransactionError> {
    // Check transaction compatibility first for both paths
    check_transaction_compatibility(old_evm_data, new_evm_data)?;

    if has_explicit_prices(new_evm_data) {
        // User provided explicit gas prices - validate they meet bump requirements
        // Skip validation if network lacks mempool
        validate_explicit_price_bump(old_evm_data, new_evm_data, relayer, network_lacks_mempool)
    } else {
        calculate_replacement_price(
            old_evm_data,
            new_evm_data,
            relayer,
            price_calculator,
            network_lacks_mempool,
        )
        .await
    }
}

/// Validates explicit gas prices from a replacement request against bump requirements.
///
/// # Arguments
///
/// * `old_evm_data` - The original transaction data
/// * `new_evm_data` - The new transaction data with explicit prices
/// * `relayer` - The relayer model for policy validation
/// * `network_lacks_mempool` - Whether the network lacks mempool (skips bump validation)
///
/// # Returns
///
/// A `Result` containing validated price parameters or a `TransactionError`.
pub fn validate_explicit_price_bump(
    old_evm_data: &EvmTransactionData,
    new_evm_data: &EvmTransactionData,
    relayer: &RelayerRepoModel,
    network_lacks_mempool: bool,
) -> Result<PriceParams, TransactionError> {
    // Create price params from the explicit values in the request
    let mut price_params = PriceParams {
        gas_price: new_evm_data.gas_price,
        max_fee_per_gas: new_evm_data.max_fee_per_gas,
        max_priority_fee_per_gas: new_evm_data.max_priority_fee_per_gas,
        is_min_bumped: None,
        extra_fee: None,
        total_cost: U256::ZERO,
    };

    // First check gas price cap before bump validation
    let gas_price_cap = relayer
        .policies
        .get_evm_policy()
        .gas_price_cap
        .unwrap_or(DEFAULT_EVM_GAS_PRICE_CAP);

    // Check if gas prices exceed gas price cap
    if let Some(gas_price) = new_evm_data.gas_price {
        if gas_price > gas_price_cap {
            return Err(TransactionError::ValidationError(format!(
                "Gas price {} exceeds gas price cap {}",
                gas_price, gas_price_cap
            )));
        }
    }

    if let Some(max_fee) = new_evm_data.max_fee_per_gas {
        if max_fee > gas_price_cap {
            return Err(TransactionError::ValidationError(format!(
                "Max fee per gas {} exceeds gas price cap {}",
                max_fee, gas_price_cap
            )));
        }
    }

    // both max_fee_per_gas and max_priority_fee_per_gas must be provided together
    if price_params.max_fee_per_gas.is_some() != price_params.max_priority_fee_per_gas.is_some() {
        return Err(TransactionError::ValidationError(
            "Partial EIP1559 transaction: both max_fee_per_gas and max_priority_fee_per_gas must be provided together".to_string(),
        ));
    }

    // Skip bump validation if network lacks mempool
    if !network_lacks_mempool {
        validate_price_bump_requirements(old_evm_data, new_evm_data)?;
    }

    // Ensure max priority fee doesn't exceed max fee per gas for EIP1559 transactions
    if let (Some(max_fee), Some(max_priority)) = (
        price_params.max_fee_per_gas,
        price_params.max_priority_fee_per_gas,
    ) {
        if max_priority > max_fee {
            return Err(TransactionError::ValidationError(
                "Max priority fee cannot exceed max fee per gas".to_string(),
            ));
        }
    }

    // Calculate total cost
    let gas_limit = old_evm_data.gas_limit;
    let value = new_evm_data.value;
    let is_eip1559 = price_params.max_fee_per_gas.is_some();

    price_params.total_cost = price_params.calculate_total_cost(
        is_eip1559,
        gas_limit.unwrap_or(DEFAULT_GAS_LIMIT),
        value,
    );
    price_params.is_min_bumped = Some(true);

    Ok(price_params)
}

/// Validates that explicit prices meet bump requirements
fn validate_price_bump_requirements(
    old_evm_data: &EvmTransactionData,
    new_evm_data: &EvmTransactionData,
) -> Result<(), TransactionError> {
    let old_has_legacy_pricing = old_evm_data.gas_price.is_some();
    let old_has_eip1559_pricing =
        old_evm_data.max_fee_per_gas.is_some() && old_evm_data.max_priority_fee_per_gas.is_some();
    let new_has_legacy_pricing = new_evm_data.gas_price.is_some();
    let new_has_eip1559_pricing =
        new_evm_data.max_fee_per_gas.is_some() && new_evm_data.max_priority_fee_per_gas.is_some();

    // New transaction must always have pricing data
    if !new_has_legacy_pricing && !new_has_eip1559_pricing {
        return Err(TransactionError::ValidationError(
            "New transaction must have pricing data".to_string(),
        ));
    }

    // Validate EIP1559 consistency in new transaction
    if !new_evm_data.is_legacy()
        && new_evm_data.max_fee_per_gas.is_some() != new_evm_data.max_priority_fee_per_gas.is_some()
    {
        return Err(TransactionError::ValidationError(
            "Partial EIP1559 transaction: both max_fee_per_gas and max_priority_fee_per_gas must be provided together".to_string(),
        ));
    }

    // If old transaction has no pricing data, accept any new pricing that has data
    if !old_has_legacy_pricing && !old_has_eip1559_pricing {
        return Ok(());
    }

    let is_sufficient_bump = if let (Some(old_gas_price), Some(new_gas_price)) =
        (old_evm_data.gas_price, new_evm_data.gas_price)
    {
        // Legacy transaction comparison
        let min_required = calculate_min_bump(old_gas_price);
        new_gas_price >= min_required
    } else if let (Some(old_max_fee), Some(new_max_fee)) =
        (old_evm_data.max_fee_per_gas, new_evm_data.max_fee_per_gas)
    {
        // EIP1559 transaction comparison - max_fee_per_gas must meet bump requirements
        let min_required_max_fee = calculate_min_bump(old_max_fee);
        let max_fee_sufficient = new_max_fee >= min_required_max_fee;

        // Check max_priority_fee_per_gas if both transactions have it
        let priority_fee_sufficient = match (
            old_evm_data.max_priority_fee_per_gas,
            new_evm_data.max_priority_fee_per_gas,
        ) {
            (Some(old_priority), Some(new_priority)) => {
                let min_required_priority = calculate_min_bump(old_priority);
                new_priority >= min_required_priority
            }
            _ => {
                return Err(TransactionError::ValidationError(
                    "Partial EIP1559 transaction: both max_fee_per_gas and max_priority_fee_per_gas must be provided together".to_string(),
                ));
            }
        };

        max_fee_sufficient && priority_fee_sufficient
    } else {
        // Handle missing data - return early with error
        return Err(TransactionError::ValidationError(
            "Partial EIP1559 transaction: both max_fee_per_gas and max_priority_fee_per_gas must be provided together".to_string(),
        ));
    };

    if !is_sufficient_bump {
        return Err(TransactionError::ValidationError(
            "Gas price increase does not meet minimum bump requirement".to_string(),
        ));
    }

    Ok(())
}

/// Calculates replacement pricing with fresh market rates.
///
/// # Arguments
///
/// * `old_evm_data` - The original transaction data for bump validation
/// * `new_evm_data` - The new transaction data
/// * `relayer` - The relayer model for policy validation
/// * `price_calculator` - The price calculator instance
/// * `network_lacks_mempool` - Whether the network lacks mempool (skips bump validation)
///
/// # Returns
///
/// A `Result` containing calculated price parameters or a `TransactionError`.
pub async fn calculate_replacement_price<PC: PriceCalculatorTrait>(
    old_evm_data: &EvmTransactionData,
    new_evm_data: &EvmTransactionData,
    relayer: &RelayerRepoModel,
    price_calculator: &PC,
    network_lacks_mempool: bool,
) -> Result<PriceParams, TransactionError> {
    // Determine transaction type based on old transaction and network policy
    let use_legacy = old_evm_data.is_legacy()
        || relayer.policies.get_evm_policy().eip1559_pricing == Some(false);

    // Get fresh market price for the updated transaction data
    let mut price_params = price_calculator
        .get_transaction_price_params(new_evm_data, relayer)
        .await?;

    // Skip bump requirements if network lacks mempool
    if network_lacks_mempool {
        price_params.is_min_bumped = Some(true);
        return Ok(price_params);
    }

    // For replacement transactions, we need to ensure the new price meets bump requirements
    // compared to the old transaction
    let is_sufficient_bump = if use_legacy {
        if let (Some(old_gas_price), Some(new_gas_price)) =
            (old_evm_data.gas_price, price_params.gas_price)
        {
            let min_required = calculate_min_bump(old_gas_price);
            if new_gas_price < min_required {
                // Market price is too low, use minimum bump
                price_params.gas_price = Some(min_required);
            }
            price_params.is_min_bumped = Some(true);
            true
        } else {
            false
        }
    } else {
        // EIP1559 comparison
        if let (Some(old_max_fee), Some(new_max_fee), Some(old_priority), Some(new_priority)) = (
            old_evm_data.max_fee_per_gas,
            price_params.max_fee_per_gas,
            old_evm_data.max_priority_fee_per_gas,
            price_params.max_priority_fee_per_gas,
        ) {
            let min_required = calculate_min_bump(old_max_fee);
            let min_required_priority = calculate_min_bump(old_priority);
            if new_max_fee < min_required {
                price_params.max_fee_per_gas = Some(min_required);
            }

            if new_priority < min_required_priority {
                price_params.max_priority_fee_per_gas = Some(min_required_priority);
            }

            price_params.is_min_bumped = Some(true);
            true
        } else {
            false
        }
    };

    if !is_sufficient_bump {
        return Err(TransactionError::ValidationError(
            "Unable to calculate sufficient price bump for speed-based replacement".to_string(),
        ));
    }

    Ok(price_params)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domain::transaction::evm::price_calculator::PriceCalculatorTrait,
        models::{
            evm::Speed, EvmTransactionData, RelayerEvmPolicy, RelayerNetworkPolicy,
            RelayerRepoModel, TransactionError, U256,
        },
    };
    use async_trait::async_trait;

    // Mock price calculator for testing
    struct MockPriceCalculator {
        pub gas_price: Option<u128>,
        pub max_fee_per_gas: Option<u128>,
        pub max_priority_fee_per_gas: Option<u128>,
        pub should_error: bool,
    }

    #[async_trait]
    impl PriceCalculatorTrait for MockPriceCalculator {
        async fn get_transaction_price_params(
            &self,
            _evm_data: &EvmTransactionData,
            _relayer: &RelayerRepoModel,
        ) -> Result<PriceParams, TransactionError> {
            if self.should_error {
                return Err(TransactionError::ValidationError("Mock error".to_string()));
            }

            Ok(PriceParams {
                gas_price: self.gas_price,
                max_fee_per_gas: self.max_fee_per_gas,
                max_priority_fee_per_gas: self.max_priority_fee_per_gas,
                is_min_bumped: Some(false),
                extra_fee: None,
                total_cost: U256::ZERO,
            })
        }

        async fn calculate_bumped_gas_price(
            &self,
            _evm_data: &EvmTransactionData,
            _relayer: &RelayerRepoModel,
        ) -> Result<PriceParams, TransactionError> {
            if self.should_error {
                return Err(TransactionError::ValidationError("Mock error".to_string()));
            }

            Ok(PriceParams {
                gas_price: self.gas_price,
                max_fee_per_gas: self.max_fee_per_gas,
                max_priority_fee_per_gas: self.max_priority_fee_per_gas,
                is_min_bumped: Some(true),
                extra_fee: None,
                total_cost: U256::ZERO,
            })
        }
    }

    fn create_legacy_transaction_data() -> EvmTransactionData {
        EvmTransactionData {
            gas_price: Some(20_000_000_000), // 20 gwei
            gas_limit: Some(21000),
            nonce: Some(1),
            value: U256::from(1000000000000000000u128), // 1 ETH
            data: Some("0x".to_string()),
            from: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            to: Some("0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed".to_string()),
            chain_id: 1,
            hash: None,
            signature: None,
            speed: Some(Speed::Average),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            raw: None,
        }
    }

    fn create_eip1559_transaction_data() -> EvmTransactionData {
        EvmTransactionData {
            gas_price: None,
            gas_limit: Some(21000),
            nonce: Some(1),
            value: U256::from(1000000000000000000u128), // 1 ETH
            data: Some("0x".to_string()),
            from: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            to: Some("0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed".to_string()),
            chain_id: 1,
            hash: None,
            signature: None,
            speed: Some(Speed::Average),
            max_fee_per_gas: Some(30_000_000_000), // 30 gwei
            max_priority_fee_per_gas: Some(2_000_000_000), // 2 gwei
            raw: None,
        }
    }

    fn create_test_relayer() -> RelayerRepoModel {
        RelayerRepoModel {
            id: "test-relayer".to_string(),
            name: "Test Relayer".to_string(),
            network: "ethereum".to_string(),
            paused: false,
            network_type: crate::models::NetworkType::Evm,
            signer_id: "test-signer".to_string(),
            policies: RelayerNetworkPolicy::Evm(RelayerEvmPolicy {
                gas_price_cap: Some(100_000_000_000), // 100 gwei
                eip1559_pricing: Some(true),
                ..Default::default()
            }),
            address: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            notification_id: None,
            system_disabled: false,
            custom_rpc_urls: None,
        }
    }

    fn create_relayer_with_gas_cap(gas_cap: u128) -> RelayerRepoModel {
        let mut relayer = create_test_relayer();
        if let RelayerNetworkPolicy::Evm(ref mut policy) = relayer.policies {
            policy.gas_price_cap = Some(gas_cap);
        }
        relayer
    }

    #[test]
    fn test_has_explicit_prices() {
        let legacy_tx = create_legacy_transaction_data();
        assert!(has_explicit_prices(&legacy_tx));

        let eip1559_tx = create_eip1559_transaction_data();
        assert!(has_explicit_prices(&eip1559_tx));

        let mut no_prices_tx = create_legacy_transaction_data();
        no_prices_tx.gas_price = None;
        assert!(!has_explicit_prices(&no_prices_tx));

        // Test partial EIP1559 (only max_fee_per_gas)
        let mut partial_eip1559 = create_legacy_transaction_data();
        partial_eip1559.gas_price = None;
        partial_eip1559.max_fee_per_gas = Some(30_000_000_000);
        assert!(has_explicit_prices(&partial_eip1559));

        // Test partial EIP1559 (only max_priority_fee_per_gas)
        let mut partial_priority = create_legacy_transaction_data();
        partial_priority.gas_price = None;
        partial_priority.max_priority_fee_per_gas = Some(2_000_000_000);
        assert!(has_explicit_prices(&partial_priority));
    }

    #[test]
    fn test_check_transaction_compatibility_success() {
        // Legacy to legacy - should succeed
        let old_legacy = create_legacy_transaction_data();
        let new_legacy = create_legacy_transaction_data();
        assert!(check_transaction_compatibility(&old_legacy, &new_legacy).is_ok());

        // EIP1559 to EIP1559 - should succeed
        let old_eip1559 = create_eip1559_transaction_data();
        let new_eip1559 = create_eip1559_transaction_data();
        assert!(check_transaction_compatibility(&old_eip1559, &new_eip1559).is_ok());

        // No explicit prices - should succeed
        let mut no_prices = create_legacy_transaction_data();
        no_prices.gas_price = None;
        assert!(check_transaction_compatibility(&old_legacy, &no_prices).is_ok());
    }

    #[test]
    fn test_check_transaction_compatibility_failures() {
        let old_legacy = create_legacy_transaction_data();
        let old_eip1559 = create_eip1559_transaction_data();

        // Legacy to EIP1559 - should fail
        let result = check_transaction_compatibility(&old_legacy, &old_eip1559);
        assert!(result.is_err());

        // EIP1559 to Legacy - should fail
        let result = check_transaction_compatibility(&old_eip1559, &old_legacy);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_explicit_price_bump_gas_price_cap() {
        let old_tx = create_legacy_transaction_data();
        let relayer = create_relayer_with_gas_cap(25_000_000_000);

        let mut new_tx = create_legacy_transaction_data();
        new_tx.gas_price = Some(50_000_000_000);

        let result = validate_explicit_price_bump(&old_tx, &new_tx, &relayer, false);
        assert!(result.is_err());

        let mut new_eip1559 = create_eip1559_transaction_data();
        new_eip1559.max_fee_per_gas = Some(50_000_000_000);

        let old_eip1559 = create_eip1559_transaction_data();
        let result = validate_explicit_price_bump(&old_eip1559, &new_eip1559, &relayer, false);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_explicit_price_bump_insufficient_bump() {
        let relayer = create_test_relayer();

        let old_legacy = create_legacy_transaction_data();
        let mut new_legacy = create_legacy_transaction_data();
        new_legacy.gas_price = Some(21_000_000_000); // 21 gwei (insufficient because minimum bump const)

        let result = validate_explicit_price_bump(&old_legacy, &new_legacy, &relayer, false);
        assert!(result.is_err());

        let old_eip1559 = create_eip1559_transaction_data();
        let mut new_eip1559 = create_eip1559_transaction_data();
        new_eip1559.max_fee_per_gas = Some(32_000_000_000); // 32 gwei (insufficient because minimum bump const)

        let result = validate_explicit_price_bump(&old_eip1559, &new_eip1559, &relayer, false);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_explicit_price_bump_sufficient_bump() {
        let relayer = create_test_relayer();

        let old_legacy = create_legacy_transaction_data();
        let mut new_legacy = create_legacy_transaction_data();
        new_legacy.gas_price = Some(22_000_000_000);

        let result = validate_explicit_price_bump(&old_legacy, &new_legacy, &relayer, false);
        assert!(result.is_ok());

        let old_eip1559 = create_eip1559_transaction_data();
        let mut new_eip1559 = create_eip1559_transaction_data();
        new_eip1559.max_fee_per_gas = Some(33_000_000_000);
        new_eip1559.max_priority_fee_per_gas = Some(3_000_000_000);

        let result = validate_explicit_price_bump(&old_eip1559, &new_eip1559, &relayer, false);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_explicit_price_bump_network_lacks_mempool() {
        let relayer = create_test_relayer();
        let old_legacy = create_legacy_transaction_data();
        let mut new_legacy = create_legacy_transaction_data();
        new_legacy.gas_price = Some(15_000_000_000); // 15 gwei (would normally be insufficient)

        // Should succeed when network lacks mempool (bump validation skipped)
        let result = validate_explicit_price_bump(&old_legacy, &new_legacy, &relayer, true);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_explicit_price_bump_partial_eip1559_error() {
        let relayer = create_test_relayer();
        let old_eip1559 = create_eip1559_transaction_data();

        // Test only max_fee_per_gas provided
        let mut partial_max_fee = create_legacy_transaction_data();
        partial_max_fee.gas_price = None;
        partial_max_fee.max_fee_per_gas = Some(35_000_000_000);
        partial_max_fee.max_priority_fee_per_gas = None;

        let result = validate_explicit_price_bump(&old_eip1559, &partial_max_fee, &relayer, false);
        assert!(result.is_err());

        // Test only max_priority_fee_per_gas provided
        let mut partial_priority = create_legacy_transaction_data();
        partial_priority.gas_price = None;
        partial_priority.max_fee_per_gas = None;
        partial_priority.max_priority_fee_per_gas = Some(3_000_000_000);

        let result = validate_explicit_price_bump(&old_eip1559, &partial_priority, &relayer, false);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_explicit_price_bump_priority_fee_exceeds_max_fee() {
        let relayer = create_test_relayer();
        let old_eip1559 = create_eip1559_transaction_data();
        let mut new_eip1559 = create_eip1559_transaction_data();
        new_eip1559.max_fee_per_gas = Some(35_000_000_000);
        new_eip1559.max_priority_fee_per_gas = Some(40_000_000_000);

        let result = validate_explicit_price_bump(&old_eip1559, &new_eip1559, &relayer, false);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_explicit_price_bump_priority_fee_equals_max_fee() {
        let relayer = create_test_relayer();
        let old_eip1559 = create_eip1559_transaction_data();
        let mut new_eip1559 = create_eip1559_transaction_data();
        new_eip1559.max_fee_per_gas = Some(35_000_000_000);
        new_eip1559.max_priority_fee_per_gas = Some(35_000_000_000);

        let result = validate_explicit_price_bump(&old_eip1559, &new_eip1559, &relayer, false);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_calculate_replacement_price_legacy_sufficient_market_price() {
        let old_tx = create_legacy_transaction_data();
        let new_tx = create_legacy_transaction_data();
        let relayer = create_test_relayer();

        let price_calculator = MockPriceCalculator {
            gas_price: Some(25_000_000_000),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            should_error: false,
        };

        let result =
            calculate_replacement_price(&old_tx, &new_tx, &relayer, &price_calculator, false).await;
        assert!(result.is_ok());

        let price_params = result.unwrap();
        assert_eq!(price_params.gas_price, Some(25_000_000_000));
        assert_eq!(price_params.is_min_bumped, Some(true));
    }

    #[tokio::test]
    async fn test_calculate_replacement_price_legacy_insufficient_market_price() {
        let old_tx = create_legacy_transaction_data();
        let new_tx = create_legacy_transaction_data();
        let relayer = create_test_relayer();

        let price_calculator = MockPriceCalculator {
            gas_price: Some(18_000_000_000), // 18 gwei (insufficient, needs 22 gwei)
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            should_error: false,
        };

        let result =
            calculate_replacement_price(&old_tx, &new_tx, &relayer, &price_calculator, false).await;
        assert!(result.is_ok());

        let price_params = result.unwrap();
        assert_eq!(price_params.gas_price, Some(22_000_000_000)); // Should be bumped to minimum
        assert_eq!(price_params.is_min_bumped, Some(true));
    }

    #[tokio::test]
    async fn test_calculate_replacement_price_eip1559_sufficient() {
        let old_tx = create_eip1559_transaction_data();
        let new_tx = create_eip1559_transaction_data();
        let relayer = create_test_relayer();

        let price_calculator = MockPriceCalculator {
            gas_price: None,
            max_fee_per_gas: Some(40_000_000_000),
            max_priority_fee_per_gas: Some(3_000_000_000),
            should_error: false,
        };

        let result =
            calculate_replacement_price(&old_tx, &new_tx, &relayer, &price_calculator, false).await;
        assert!(result.is_ok());

        let price_params = result.unwrap();
        assert_eq!(price_params.max_fee_per_gas, Some(40_000_000_000));
        assert_eq!(price_params.is_min_bumped, Some(true));
    }

    #[tokio::test]
    async fn test_calculate_replacement_price_eip1559_insufficient_with_priority_fee_bump() {
        let mut old_tx = create_eip1559_transaction_data();
        old_tx.max_fee_per_gas = Some(30_000_000_000);
        old_tx.max_priority_fee_per_gas = Some(5_000_000_000);

        let new_tx = create_eip1559_transaction_data();
        let relayer = create_test_relayer();

        let price_calculator = MockPriceCalculator {
            gas_price: None,
            max_fee_per_gas: Some(25_000_000_000), // 25 gwei (insufficient, needs 33 gwei)
            max_priority_fee_per_gas: Some(4_000_000_000), // 4 gwei (insufficient, needs 5.5 gwei)
            should_error: false,
        };

        let result =
            calculate_replacement_price(&old_tx, &new_tx, &relayer, &price_calculator, false).await;
        assert!(result.is_ok());

        let price_params = result.unwrap();
        assert_eq!(price_params.max_fee_per_gas, Some(33_000_000_000));

        // Priority fee should also be bumped if old transaction had it
        let expected_priority_bump = calculate_min_bump(5_000_000_000); // 5.5 gwei
        let capped_priority = expected_priority_bump.min(33_000_000_000); // Capped at max_fee
        assert_eq!(price_params.max_priority_fee_per_gas, Some(capped_priority));
    }

    #[tokio::test]
    async fn test_calculate_replacement_price_network_lacks_mempool() {
        let old_tx = create_legacy_transaction_data();
        let new_tx = create_legacy_transaction_data();
        let relayer = create_test_relayer();

        let price_calculator = MockPriceCalculator {
            gas_price: Some(15_000_000_000), // 15 gwei (would be insufficient normally)
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            should_error: false,
        };

        let result =
            calculate_replacement_price(&old_tx, &new_tx, &relayer, &price_calculator, true).await;
        assert!(result.is_ok());

        let price_params = result.unwrap();
        assert_eq!(price_params.gas_price, Some(15_000_000_000)); // Uses market price as-is
        assert_eq!(price_params.is_min_bumped, Some(true));
    }

    #[tokio::test]
    async fn test_calculate_replacement_price_calculator_error() {
        let old_tx = create_legacy_transaction_data();
        let new_tx = create_legacy_transaction_data();
        let relayer = create_test_relayer();

        let price_calculator = MockPriceCalculator {
            gas_price: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            should_error: true,
        };

        let result =
            calculate_replacement_price(&old_tx, &new_tx, &relayer, &price_calculator, false).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_determine_replacement_pricing_explicit_prices() {
        let old_tx = create_legacy_transaction_data();
        let mut new_tx = create_legacy_transaction_data();
        new_tx.gas_price = Some(25_000_000_000);
        let relayer = create_test_relayer();

        let price_calculator = MockPriceCalculator {
            gas_price: Some(30_000_000_000),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            should_error: false,
        };

        let result =
            determine_replacement_pricing(&old_tx, &new_tx, &relayer, &price_calculator, false)
                .await;
        assert!(result.is_ok());

        let price_params = result.unwrap();
        assert_eq!(price_params.gas_price, Some(25_000_000_000));
    }

    #[tokio::test]
    async fn test_determine_replacement_pricing_market_prices() {
        let old_tx = create_legacy_transaction_data();
        let mut new_tx = create_legacy_transaction_data();
        new_tx.gas_price = None;
        let relayer = create_test_relayer();

        let price_calculator = MockPriceCalculator {
            gas_price: Some(30_000_000_000),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            should_error: false,
        };

        let result =
            determine_replacement_pricing(&old_tx, &new_tx, &relayer, &price_calculator, false)
                .await;
        assert!(result.is_ok());

        let price_params = result.unwrap();
        assert_eq!(price_params.gas_price, Some(30_000_000_000));
    }

    #[tokio::test]
    async fn test_determine_replacement_pricing_compatibility_error() {
        let old_legacy = create_legacy_transaction_data();
        let new_eip1559 = create_eip1559_transaction_data();
        let relayer = create_test_relayer();

        let price_calculator = MockPriceCalculator {
            gas_price: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            should_error: false,
        };

        let result = determine_replacement_pricing(
            &old_legacy,
            &new_eip1559,
            &relayer,
            &price_calculator,
            false,
        )
        .await;
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_price_bump_requirements_legacy() {
        let old_tx = create_legacy_transaction_data();

        let mut new_tx_sufficient = create_legacy_transaction_data();
        new_tx_sufficient.gas_price = Some(22_000_000_000);
        assert!(validate_price_bump_requirements(&old_tx, &new_tx_sufficient).is_ok());

        let mut new_tx_insufficient = create_legacy_transaction_data();
        new_tx_insufficient.gas_price = Some(21_000_000_000);
        assert!(validate_price_bump_requirements(&old_tx, &new_tx_insufficient).is_err());
    }

    #[test]
    fn test_validate_price_bump_requirements_eip1559() {
        let old_tx = create_eip1559_transaction_data();

        let mut new_tx_sufficient = create_eip1559_transaction_data();
        new_tx_sufficient.max_fee_per_gas = Some(33_000_000_000);
        new_tx_sufficient.max_priority_fee_per_gas = Some(3_000_000_000);
        assert!(validate_price_bump_requirements(&old_tx, &new_tx_sufficient).is_ok());

        let mut new_tx_insufficient_max = create_eip1559_transaction_data();
        new_tx_insufficient_max.max_fee_per_gas = Some(32_000_000_000);
        new_tx_insufficient_max.max_priority_fee_per_gas = Some(3_000_000_000);
        assert!(validate_price_bump_requirements(&old_tx, &new_tx_insufficient_max).is_err());

        let mut new_tx_insufficient_priority = create_eip1559_transaction_data();
        new_tx_insufficient_priority.max_fee_per_gas = Some(33_000_000_000);
        new_tx_insufficient_priority.max_priority_fee_per_gas = Some(2_100_000_000);
        assert!(validate_price_bump_requirements(&old_tx, &new_tx_insufficient_priority).is_err());
    }

    #[test]
    fn test_validate_price_bump_requirements_partial_eip1559() {
        let mut old_tx = create_eip1559_transaction_data();
        old_tx.max_fee_per_gas = Some(30_000_000_000);
        old_tx.max_priority_fee_per_gas = Some(5_000_000_000);

        let mut new_tx_only_priority = create_legacy_transaction_data();
        new_tx_only_priority.gas_price = None;
        new_tx_only_priority.max_fee_per_gas = None;
        new_tx_only_priority.max_priority_fee_per_gas = Some(6_000_000_000);
        let result = validate_price_bump_requirements(&old_tx, &new_tx_only_priority);
        assert!(result.is_err());

        let mut new_tx_only_max = create_legacy_transaction_data();
        new_tx_only_max.gas_price = None;
        new_tx_only_max.max_fee_per_gas = Some(33_000_000_000);
        new_tx_only_max.max_priority_fee_per_gas = None;
        let result = validate_price_bump_requirements(&old_tx, &new_tx_only_max);
        assert!(result.is_err());

        let new_legacy = create_legacy_transaction_data();
        let result = validate_price_bump_requirements(&old_tx, &new_legacy);
        assert!(result.is_err());

        let old_legacy = create_legacy_transaction_data();
        let result = validate_price_bump_requirements(&old_legacy, &new_tx_only_priority);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_price_bump_requirements_missing_pricing_data() {
        let mut old_tx_no_price = create_legacy_transaction_data();
        old_tx_no_price.gas_price = None;
        old_tx_no_price.max_fee_per_gas = None;
        old_tx_no_price.max_priority_fee_per_gas = None;

        let mut new_tx_no_price = create_legacy_transaction_data();
        new_tx_no_price.gas_price = None;
        new_tx_no_price.max_fee_per_gas = None;
        new_tx_no_price.max_priority_fee_per_gas = None;

        let result = validate_price_bump_requirements(&old_tx_no_price, &new_tx_no_price);
        assert!(result.is_err()); // Should fail because new transaction has no pricing

        // Test old transaction with no pricing, new with legacy pricing - should succeed
        let new_legacy = create_legacy_transaction_data();
        let result = validate_price_bump_requirements(&old_tx_no_price, &new_legacy);
        assert!(result.is_ok());

        // Test old transaction with no pricing, new with EIP1559 pricing - should succeed
        let new_eip1559 = create_eip1559_transaction_data();
        let result = validate_price_bump_requirements(&old_tx_no_price, &new_eip1559);
        assert!(result.is_ok());

        // Test old legacy, new with no pricing - should fail
        let old_legacy = create_legacy_transaction_data();
        let result = validate_price_bump_requirements(&old_legacy, &new_tx_no_price);
        assert!(result.is_err()); // Should fail because new transaction has no pricing
    }

    #[test]
    fn test_validate_explicit_price_bump_zero_gas_price_cap() {
        let old_tx = create_legacy_transaction_data();
        let relayer = create_relayer_with_gas_cap(0);
        let mut new_tx = create_legacy_transaction_data();
        new_tx.gas_price = Some(1);

        let result = validate_explicit_price_bump(&old_tx, &new_tx, &relayer, false);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_calculate_replacement_price_legacy_missing_old_gas_price() {
        let mut old_tx = create_legacy_transaction_data();
        old_tx.gas_price = None;
        let new_tx = create_legacy_transaction_data();
        let relayer = create_test_relayer();

        let price_calculator = MockPriceCalculator {
            gas_price: Some(25_000_000_000),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            should_error: false,
        };

        let result =
            calculate_replacement_price(&old_tx, &new_tx, &relayer, &price_calculator, false).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_calculate_replacement_price_eip1559_missing_old_fees() {
        let mut old_tx = create_eip1559_transaction_data();
        old_tx.max_fee_per_gas = None;
        old_tx.max_priority_fee_per_gas = None;
        let new_tx = create_eip1559_transaction_data();
        let relayer = create_test_relayer();

        let price_calculator = MockPriceCalculator {
            gas_price: None,
            max_fee_per_gas: Some(40_000_000_000),
            max_priority_fee_per_gas: Some(3_000_000_000),
            should_error: false,
        };

        let result =
            calculate_replacement_price(&old_tx, &new_tx, &relayer, &price_calculator, false).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_calculate_replacement_price_force_legacy_with_eip1559_policy_disabled() {
        let old_tx = create_eip1559_transaction_data();
        let new_tx = create_eip1559_transaction_data();
        let mut relayer = create_test_relayer();
        if let crate::models::RelayerNetworkPolicy::Evm(ref mut policy) = relayer.policies {
            policy.eip1559_pricing = Some(false);
        }

        let price_calculator = MockPriceCalculator {
            gas_price: Some(25_000_000_000),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            should_error: false,
        };

        let result =
            calculate_replacement_price(&old_tx, &new_tx, &relayer, &price_calculator, false).await;
        assert!(result.is_err());
    }
}
