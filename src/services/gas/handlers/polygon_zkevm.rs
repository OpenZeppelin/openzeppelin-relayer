use crate::{
    domain::evm::PriceParams,
    models::{EvmTransactionData, TransactionError},
    services::provider::{evm::EvmProviderTrait, ProviderError},
};

/// Price parameter handler for Polygon zkEVM networks
///
/// This implementation uses the custom zkEVM endpoints introduced by Polygon to solve
/// the gas estimation accuracy problem. As documented in Polygon's blog post
/// (https://polygon.technology/blog/new-custom-endpoint-for-dapps-on-polygon-zkevm),
/// these endpoints provide up to 20% more accurate fee estimation compared to standard methods.
///
/// Key features:
/// - Uses `zkevm_estimateGasPrice` for precise gas price estimation based on transaction type and calldata
/// - Uses `zkevm_estimateFee` for comprehensive fee estimation including L1 data availability costs
/// - Accounts for both L2 execution costs and L1 data availability costs specific to zkEVM rollups
/// - Directly leverages the accurate pricing data from the gas price service (no fallbacks needed)
#[derive(Debug, Clone)]
pub struct PolygonZKEvmPriceHandler<P> {
    provider: P,
}

impl<P: EvmProviderTrait> PolygonZKEvmPriceHandler<P> {
    pub fn new(provider: P) -> Self {
        Self { provider }
    }

    pub async fn handle_price_params(
        &self,
        tx: &EvmTransactionData,
        mut original_params: PriceParams,
    ) -> Result<PriceParams, TransactionError> {
        // Use zkEVM-specific endpoints for accurate pricing (recommended by Polygon)
        let zkevm_gas_price = self.provider.zkevm_estimate_gas_price().await;
        let zkevm_fee_estimate = self.provider.zkevm_estimate_fee(tx).await;

        // Handle case where zkEVM methods are not available on this rpc or network
        // If either method returns MethodNotAvailable, return original params unchanged
        let (zkevm_gas_price, zkevm_fee_estimate) = match (zkevm_gas_price, zkevm_fee_estimate) {
            (Err(ProviderError::MethodNotAvailable(_)), _)
            | (_, Err(ProviderError::MethodNotAvailable(_))) => {
                // zkEVM methods not supported on this rpc or network, return original params
                return Ok(original_params);
            }
            (Ok(gas_price), Ok(fee_estimate)) => (gas_price, fee_estimate),
            (Err(e), _) => {
                return Err(TransactionError::UnexpectedError(format!(
                    "Failed to get zkEVM gas price: {}",
                    e
                )))
            }
            (_, Err(e)) => {
                return Err(TransactionError::UnexpectedError(format!(
                    "Failed to estimate zkEVM fee: {}",
                    e
                )))
            }
        };

        // Only set gas price parameters if they weren't already provided
        let is_eip1559 = original_params.max_fee_per_gas.is_some()
            || original_params.max_priority_fee_per_gas.is_some();

        if is_eip1559 {
            // For EIP1559 transactions, use zkEVM gas price only if not already set
            if original_params.max_fee_per_gas.is_none() {
                original_params.max_fee_per_gas = Some(zkevm_gas_price);
            }
        } else {
            // For legacy transactions, use zkEVM gas price only if not already set
            if original_params.gas_price.is_none() {
                original_params.gas_price = Some(zkevm_gas_price);
            }
        }

        // The zkEVM fee estimate includes L1 data availability costs
        // Set this as the extra fee to account for the total zkEVM-specific costs
        original_params.extra_fee = Some(zkevm_fee_estimate);

        Ok(original_params)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{models::U256, services::provider::evm::MockEvmProviderTrait};
    use futures::FutureExt;

    #[tokio::test]
    async fn test_polygon_zkevm_price_handler_legacy() {
        let mut mock_provider = MockEvmProviderTrait::new();

        // Mock the zkEVM-specific methods
        mock_provider
            .expect_zkevm_estimate_gas_price()
            .returning(|| async { Ok(25_000_000_000u128) }.boxed()); // 25 Gwei

        mock_provider
            .expect_zkevm_estimate_fee()
            .returning(|_| async { Ok(U256::from(500_000_000_000_000u128)) }.boxed()); // 0.0005 ETH

        // Create the price handler
        let handler = PolygonZKEvmPriceHandler::new(mock_provider);

        // Create test transaction with data
        let tx = EvmTransactionData {
            from: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            to: Some("0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string()),
            value: U256::from(1_000_000_000_000_000_000u128), // 1 ETH
            data: Some("0x1234567890abcdef".to_string()),     // 8 bytes of data
            gas_limit: Some(21000),
            gas_price: Some(20_000_000_000), // 20 Gwei
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            speed: None,
            nonce: None,
            chain_id: 1101,
            hash: None,
            signature: None,
            raw: None,
        };

        // Create original price params (legacy)
        let original_params = PriceParams {
            gas_price: Some(20_000_000_000), // 20 Gwei
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            is_min_bumped: None,
            extra_fee: None,
            total_cost: U256::ZERO,
        };

        // Handle the price params
        let result = handler.handle_price_params(&tx, original_params).await;

        assert!(result.is_ok());
        let handled_params = result.unwrap();

        // Verify that the original gas price remains unchanged
        assert_eq!(handled_params.gas_price.unwrap(), 20_000_000_000); // Should remain original

        // Verify that extra fee was added from zkEVM fee estimate
        assert!(handled_params.extra_fee.is_some());
        assert_eq!(
            handled_params.extra_fee.unwrap(),
            U256::from(500_000_000_000_000u128)
        );
    }

    #[tokio::test]
    async fn test_polygon_zkevm_price_handler_eip1559() {
        let mut mock_provider = MockEvmProviderTrait::new();

        // Mock the zkEVM-specific methods
        mock_provider
            .expect_zkevm_estimate_gas_price()
            .returning(|| async { Ok(35_000_000_000u128) }.boxed()); // 35 Gwei

        mock_provider
            .expect_zkevm_estimate_fee()
            .returning(|_| async { Ok(U256::from(750_000_000_000_000u128)) }.boxed()); // 0.00075 ETH

        // Create the price handler
        let handler = PolygonZKEvmPriceHandler::new(mock_provider);

        // Create test transaction with data
        let tx = EvmTransactionData {
            from: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            to: Some("0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string()),
            value: U256::from(1_000_000_000_000_000_000u128), // 1 ETH
            data: Some("0x1234567890abcdef".to_string()),     // 8 bytes of data
            gas_limit: Some(21000),
            gas_price: None,
            max_fee_per_gas: Some(30_000_000_000), // 30 Gwei
            max_priority_fee_per_gas: Some(2_000_000_000), // 2 Gwei
            speed: None,
            nonce: None,
            chain_id: 1101,
            hash: None,
            signature: None,
            raw: None,
        };

        // Create original price params (EIP1559)
        let original_params = PriceParams {
            gas_price: None,
            max_fee_per_gas: Some(30_000_000_000), // 30 Gwei
            max_priority_fee_per_gas: Some(2_000_000_000), // 2 Gwei
            is_min_bumped: None,
            extra_fee: None,
            total_cost: U256::ZERO,
        };

        // Handle the price params
        let result = handler.handle_price_params(&tx, original_params).await;

        assert!(result.is_ok());
        let handled_params = result.unwrap();

        // Verify that the original EIP1559 fees remain unchanged
        assert_eq!(handled_params.max_fee_per_gas.unwrap(), 30_000_000_000); // Should remain original
        assert_eq!(
            handled_params.max_priority_fee_per_gas.unwrap(),
            2_000_000_000
        ); // Should remain original

        // Verify that extra fee was added from zkEVM fee estimate
        assert!(handled_params.extra_fee.is_some());
        assert_eq!(
            handled_params.extra_fee.unwrap(),
            U256::from(750_000_000_000_000u128)
        );
    }

    #[tokio::test]
    async fn test_polygon_zkevm_fee_estimation_integration() {
        let mut mock_provider = MockEvmProviderTrait::new();

        // Mock the zkEVM-specific methods with different values for different transactions
        mock_provider
            .expect_zkevm_estimate_gas_price()
            .times(2)
            .returning(|| async { Ok(20_000_000_000u128) }.boxed()); // 20 Gwei

        mock_provider
            .expect_zkevm_estimate_fee()
            .times(2)
            .returning(|tx| {
                // Return different fees based on transaction data
                let has_data = tx
                    .data
                    .as_ref()
                    .map_or(false, |d| !d.is_empty() && d != "0x");
                if has_data {
                    async { Ok(U256::from(400_000_000_000_000u128)) }.boxed() // 0.0004 ETH with data
                } else {
                    async { Ok(U256::from(210_000_000_000_000u128)) }.boxed() // 0.00021 ETH without data
                }
            });

        let handler = PolygonZKEvmPriceHandler::new(mock_provider);

        // Test with empty data
        let empty_tx = EvmTransactionData {
            from: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            to: Some("0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string()),
            value: U256::from(1_000_000_000_000_000_000u128),
            data: None,
            gas_limit: Some(21000),
            gas_price: Some(15_000_000_000), // Lower than zkEVM estimate
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            speed: None,
            nonce: None,
            chain_id: 1101,
            hash: None,
            signature: None,
            raw: None,
        };

        let original_params = PriceParams {
            gas_price: Some(15_000_000_000),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            is_min_bumped: None,
            extra_fee: None,
            total_cost: U256::ZERO,
        };

        let result = handler
            .handle_price_params(&empty_tx, original_params)
            .await;
        assert!(result.is_ok());
        let handled_params = result.unwrap();

        // Gas price should remain unchanged (already set)
        assert_eq!(handled_params.gas_price.unwrap(), 15_000_000_000);
        assert_eq!(
            handled_params.extra_fee.unwrap(),
            U256::from(210_000_000_000_000u128)
        );

        // Test with data
        let data_tx = EvmTransactionData {
            data: Some("0x1234567890abcdef".to_string()), // 8 bytes
            ..empty_tx
        };

        let original_params_with_data = PriceParams {
            gas_price: Some(15_000_000_000),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            is_min_bumped: None,
            extra_fee: None,
            total_cost: U256::ZERO,
        };

        let result_with_data = handler
            .handle_price_params(&data_tx, original_params_with_data)
            .await;
        assert!(result_with_data.is_ok());
        let handled_params_with_data = result_with_data.unwrap();

        // Should have higher fee due to data
        assert!(handled_params_with_data.extra_fee.unwrap() > handled_params.extra_fee.unwrap());
        assert_eq!(
            handled_params_with_data.extra_fee.unwrap(),
            U256::from(400_000_000_000_000u128)
        );
    }

    #[tokio::test]
    async fn test_polygon_zkevm_uses_gas_price_when_not_set() {
        let mut mock_provider = MockEvmProviderTrait::new();

        // Mock the zkEVM-specific methods
        mock_provider
            .expect_zkevm_estimate_gas_price()
            .returning(|| async { Ok(30_000_000_000u128) }.boxed()); // 30 Gwei

        mock_provider
            .expect_zkevm_estimate_fee()
            .returning(|_| async { Ok(U256::from(600_000_000_000_000u128)) }.boxed()); // 0.0006 ETH

        let handler = PolygonZKEvmPriceHandler::new(mock_provider);

        // Test with no gas price set initially
        let tx = EvmTransactionData {
            from: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            to: Some("0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string()),
            value: U256::from(1_000_000_000_000_000_000u128),
            data: Some("0x1234".to_string()),
            gas_limit: Some(21000),
            gas_price: None, // No gas price set
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            speed: None,
            nonce: None,
            chain_id: 1101,
            hash: None,
            signature: None,
            raw: None,
        };

        let original_params = PriceParams {
            gas_price: None, // No gas price set
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            is_min_bumped: None,
            extra_fee: None,
            total_cost: U256::ZERO,
        };

        let result = handler.handle_price_params(&tx, original_params).await;
        assert!(result.is_ok());
        let handled_params = result.unwrap();

        // Should use zkEVM gas price since none was provided
        assert_eq!(handled_params.gas_price.unwrap(), 30_000_000_000);
        assert_eq!(
            handled_params.extra_fee.unwrap(),
            U256::from(600_000_000_000_000u128)
        );
    }

    #[tokio::test]
    async fn test_polygon_zkevm_method_not_available() {
        let mut mock_provider = MockEvmProviderTrait::new();

        // Mock the zkEVM methods to return MethodNotAvailable error
        mock_provider
            .expect_zkevm_estimate_gas_price()
            .returning(|| {
                async {
                    Err(ProviderError::MethodNotAvailable(
                        "zkevm_estimateGasPrice method not available".to_string(),
                    ))
                }
                .boxed()
            });

        mock_provider.expect_zkevm_estimate_fee().returning(|_| {
            async {
                Err(ProviderError::MethodNotAvailable(
                    "zkevm_estimateFee method not available".to_string(),
                ))
            }
            .boxed()
        });

        let handler = PolygonZKEvmPriceHandler::new(mock_provider);

        let tx = EvmTransactionData {
            from: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            to: Some("0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string()),
            value: U256::from(1_000_000_000_000_000_000u128),
            data: Some("0x1234".to_string()),
            gas_limit: Some(21000),
            gas_price: Some(15_000_000_000), // 15 Gwei
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            speed: None,
            nonce: None,
            chain_id: 1101,
            hash: None,
            signature: None,
            raw: None,
        };

        let original_params = PriceParams {
            gas_price: Some(15_000_000_000),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            is_min_bumped: None,
            extra_fee: None,
            total_cost: U256::from(100_000),
        };

        let result = handler
            .handle_price_params(&tx, original_params.clone())
            .await;

        assert!(result.is_ok());
        let handled_params = result.unwrap();

        // Should return original params unchanged when zkEVM methods are not available
        assert_eq!(handled_params.gas_price, original_params.gas_price);
        assert_eq!(
            handled_params.max_fee_per_gas,
            original_params.max_fee_per_gas
        );
        assert_eq!(
            handled_params.max_priority_fee_per_gas,
            original_params.max_priority_fee_per_gas
        );
        assert_eq!(handled_params.extra_fee, original_params.extra_fee);
        assert_eq!(handled_params.total_cost, original_params.total_cost);
    }

    #[tokio::test]
    async fn test_polygon_zkevm_partial_method_not_available() {
        let mut mock_provider = MockEvmProviderTrait::new();

        // Mock one method to succeed and one to return MethodNotAvailable
        mock_provider
            .expect_zkevm_estimate_gas_price()
            .returning(|| async { Ok(25_000_000_000u128) }.boxed());

        mock_provider.expect_zkevm_estimate_fee().returning(|_| {
            async {
                Err(ProviderError::MethodNotAvailable(
                    "zkevm_estimateFee method not available".to_string(),
                ))
            }
            .boxed()
        });

        let handler = PolygonZKEvmPriceHandler::new(mock_provider);

        let tx = EvmTransactionData {
            from: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            to: Some("0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string()),
            value: U256::from(1_000_000_000_000_000_000u128),
            data: Some("0x1234".to_string()),
            gas_limit: Some(21000),
            gas_price: Some(15_000_000_000),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            speed: None,
            nonce: None,
            chain_id: 1101,
            hash: None,
            signature: None,
            raw: None,
        };

        let original_params = PriceParams {
            gas_price: Some(15_000_000_000),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            is_min_bumped: None,
            extra_fee: None,
            total_cost: U256::from(100_000),
        };

        let result = handler
            .handle_price_params(&tx, original_params.clone())
            .await;

        assert!(result.is_ok());
        let handled_params = result.unwrap();

        // Should return original params unchanged when any zkEVM method is not available
        assert_eq!(handled_params.gas_price, original_params.gas_price);
        assert_eq!(
            handled_params.max_fee_per_gas,
            original_params.max_fee_per_gas
        );
        assert_eq!(
            handled_params.max_priority_fee_per_gas,
            original_params.max_priority_fee_per_gas
        );
        assert_eq!(handled_params.extra_fee, original_params.extra_fee);
        assert_eq!(handled_params.total_cost, original_params.total_cost);
    }
}
