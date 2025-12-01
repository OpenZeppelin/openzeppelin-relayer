use crate::{
    domain::evm::PriceParams,
    models::{EvmTransactionData, TransactionError, U256},
    services::provider::{evm::EvmProviderTrait, ProviderError},
    utils::{EthereumJsonRpcError, StandardJsonRpcError},
};
use serde_json;

/// Builds zkEVM RPC transaction parameters from EvmTransactionData.
///
/// This helper function converts transaction data into the JSON format expected
/// by zkEVM RPC methods like `zkevm_estimateFee`.
///
/// # Arguments
/// * `tx` - The transaction data to convert
///
/// # Returns
/// A JSON object with hex-encoded transaction parameters
fn build_zkevm_transaction_params(tx: &EvmTransactionData) -> serde_json::Value {
    serde_json::json!({
        "from": tx.from,
        "to": tx.to.clone(),
        "value": format!("0x{:x}", tx.value),
        "data": tx.data.as_ref().map(|d| {
            if d.starts_with("0x") { d.clone() } else { format!("0x{d}") }
        }).unwrap_or("0x".to_string()),
        "gas": tx.gas_limit.map(|g| format!("0x{g:x}")),
        "gasPrice": tx.gas_price.map(|gp| format!("0x{gp:x}")),
        "maxFeePerGas": tx.max_fee_per_gas.map(|mfpg| format!("0x{mfpg:x}")),
        "maxPriorityFeePerGas": tx.max_priority_fee_per_gas.map(|mpfpg| format!("0x{mpfpg:x}")),
    })
}

/// Price parameter handler for Polygon zkEVM networks
///
/// This implementation uses the custom zkEVM endpoints introduced by Polygon to solve
/// the gas estimation accuracy problem. As documented in Polygon's blog post
/// (https://polygon.technology/blog/new-custom-endpoint-for-dapps-on-polygon-zkevm),
/// these endpoints provide up to 20% more accurate fee estimation compared to standard methods.
#[derive(Debug, Clone)]
pub struct PolygonZKEvmPriceHandler<P> {
    provider: P,
}

impl<P: EvmProviderTrait> PolygonZKEvmPriceHandler<P> {
    pub fn new(provider: P) -> Self {
        Self { provider }
    }

    /// zkEVM-specific method to estimate fee for a transaction using the native zkEVM endpoint.
    ///
    /// This method calls `zkevm_estimateFee` which provides more accurate
    /// fee estimation that includes L1 data availability costs for Polygon zkEVM networks.
    ///
    /// # Arguments
    /// * `tx` - The transaction request to estimate fee for
    async fn zkevm_estimate_fee(&self, tx: &EvmTransactionData) -> Result<U256, ProviderError> {
        let tx_params = build_zkevm_transaction_params(tx);

        let result = self
            .provider
            .raw_request_dyn("zkevm_estimateFee", serde_json::json!([tx_params]))
            .await?;

        let fee_hex = result
            .as_str()
            .ok_or_else(|| ProviderError::Other("Invalid fee response".to_string()))?;

        let fee = U256::from_str_radix(fee_hex.trim_start_matches("0x"), 16)
            .map_err(|e| ProviderError::Other(format!("Failed to parse fee: {e}")))?;

        Ok(fee)
    }

    pub async fn handle_price_params(
        &self,
        tx: &EvmTransactionData,
        mut original_params: PriceParams,
    ) -> Result<PriceParams, TransactionError> {
        // Use zkEVM-specific endpoints for accurate pricing (recommended by Polygon)
        let zkevm_fee_estimate = self.zkevm_estimate_fee(tx).await;

        // Handle case where zkEVM methods are not available on this rpc or network
        let zkevm_fee_estimate = match zkevm_fee_estimate {
            Err(ProviderError::RpcErrorCode { code, .. })
                if code == StandardJsonRpcError::MethodNotFound.code()
                    || code == EthereumJsonRpcError::MethodNotSupported.code() =>
            {
                // zkEVM methods not supported on this rpc or network, return original params
                return Ok(original_params);
            }
            Ok(fee_estimate) => fee_estimate,
            Err(e) => {
                return Err(TransactionError::UnexpectedError(format!(
                    "Failed to estimate zkEVM fee: {e}"
                )))
            }
        };

        // The zkEVM fee estimate provides a more accurate total cost calculation
        // that includes both L2 execution costs and L1 data availability costs
        original_params.total_cost = zkevm_fee_estimate;

        Ok(original_params)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{models::U256, services::provider::evm::MockEvmProviderTrait};
    use mockall::predicate::*;

    #[tokio::test]
    async fn test_polygon_zkevm_price_handler_legacy() {
        // Create mock provider
        let mut mock_provider = MockEvmProviderTrait::new();

        // Mock zkevm_estimateFee to return 0.0005 ETH fee
        mock_provider
            .expect_raw_request_dyn()
            .with(eq("zkevm_estimateFee"), always())
            .returning(|_, _| {
                Box::pin(async move {
                    Ok(serde_json::json!("0x1c6bf52634000")) // 500_000_000_000_000 in hex
                })
            });

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

        // Verify that total cost was set from zkEVM fee estimate
        assert_eq!(
            handled_params.total_cost,
            U256::from(500_000_000_000_000u128)
        );
    }

    #[tokio::test]
    async fn test_polygon_zkevm_price_handler_eip1559() {
        // Create mock provider
        let mut mock_provider = MockEvmProviderTrait::new();

        // Mock zkevm_estimateFee to return 0.00075 ETH fee
        mock_provider
            .expect_raw_request_dyn()
            .with(eq("zkevm_estimateFee"), always())
            .returning(|_, _| {
                Box::pin(async move {
                    Ok(serde_json::json!("0x2aa1efb94e000")) // 750_000_000_000_000 in hex
                })
            });

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

        // Verify that total cost was set from zkEVM fee estimate
        assert_eq!(
            handled_params.total_cost,
            U256::from(750_000_000_000_000u128)
        );
    }

    #[tokio::test]
    async fn test_polygon_zkevm_fee_estimation_integration() {
        // Test with empty data - create mock provider for no data scenario
        let mut mock_provider_no_data = MockEvmProviderTrait::new();
        mock_provider_no_data
            .expect_raw_request_dyn()
            .with(eq("zkevm_estimateFee"), always())
            .returning(|_, _| {
                Box::pin(async move {
                    Ok(serde_json::json!("0xbefe6f672000")) // 210_000_000_000_000 in hex
                })
            });

        let handler_no_data = PolygonZKEvmPriceHandler::new(mock_provider_no_data);

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

        let result = handler_no_data
            .handle_price_params(&empty_tx, original_params)
            .await;
        assert!(result.is_ok());
        let handled_params = result.unwrap();

        // Gas price should remain unchanged (already set)
        assert_eq!(handled_params.gas_price.unwrap(), 15_000_000_000);
        assert_eq!(
            handled_params.total_cost,
            U256::from(210_000_000_000_000u128)
        );

        // Test with data - create mock provider for data scenario
        let mut mock_provider_with_data = MockEvmProviderTrait::new();
        mock_provider_with_data
            .expect_raw_request_dyn()
            .with(eq("zkevm_estimateFee"), always())
            .returning(|_, _| {
                Box::pin(async move {
                    Ok(serde_json::json!("0x16bcc41e90000")) // 400_000_000_000_000 in hex (correct)
                })
            });

        let handler_with_data = PolygonZKEvmPriceHandler::new(mock_provider_with_data);

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

        let result_with_data = handler_with_data
            .handle_price_params(&data_tx, original_params_with_data)
            .await;
        assert!(result_with_data.is_ok());
        let handled_params_with_data = result_with_data.unwrap();

        // Should have higher total cost due to data
        assert!(handled_params_with_data.total_cost > handled_params.total_cost);
        assert_eq!(
            handled_params_with_data.total_cost,
            U256::from(400_000_000_000_000u128)
        );
    }

    #[tokio::test]
    async fn test_polygon_zkevm_uses_gas_price_when_not_set() {
        // Create mock provider
        let mut mock_provider = MockEvmProviderTrait::new();
        mock_provider
            .expect_raw_request_dyn()
            .with(eq("zkevm_estimateFee"), always())
            .returning(|_, _| {
                Box::pin(async move {
                    Ok(serde_json::json!("0x221b262dd8000")) // 600_000_000_000_000 in hex
                })
            });

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

        // Gas price should remain None since handler no longer sets it
        assert!(handled_params.gas_price.is_none());
        assert_eq!(
            handled_params.total_cost,
            U256::from(600_000_000_000_000u128)
        );
    }

    #[tokio::test]
    async fn test_polygon_zkevm_method_not_available() {
        // Create mock provider that returns MethodNotFound error
        let mut mock_provider = MockEvmProviderTrait::new();
        mock_provider
            .expect_raw_request_dyn()
            .with(eq("zkevm_estimateFee"), always())
            .returning(|_, _| {
                Box::pin(async move {
                    Err(ProviderError::RpcErrorCode {
                        code: StandardJsonRpcError::MethodNotFound.code(),
                        message: "Method not found".to_string(),
                    })
                })
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
        assert_eq!(handled_params.total_cost, original_params.total_cost);
    }

    #[tokio::test]
    async fn test_polygon_zkevm_partial_method_not_available() {
        // Create mock provider that returns MethodNotSupported error
        let mut mock_provider = MockEvmProviderTrait::new();
        mock_provider
            .expect_raw_request_dyn()
            .with(eq("zkevm_estimateFee"), always())
            .returning(|_, _| {
                Box::pin(async move {
                    Err(ProviderError::RpcErrorCode {
                        code: EthereumJsonRpcError::MethodNotSupported.code(),
                        message: "Method not supported".to_string(),
                    })
                })
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
        assert_eq!(handled_params.total_cost, original_params.total_cost);
    }

    #[test]
    fn test_build_zkevm_transaction_params() {
        // Test with complete transaction data
        let tx = EvmTransactionData {
            from: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            to: Some("0x742d35Cc6634C0532925a3b844Bc454e4438f44f".to_string()),
            value: U256::from(1000000000000000000u64), // 1 ETH
            data: Some("0x1234567890abcdef".to_string()),
            gas_limit: Some(21000),
            gas_price: Some(20000000000),               // 20 Gwei
            max_fee_per_gas: Some(30000000000),         // 30 Gwei
            max_priority_fee_per_gas: Some(2000000000), // 2 Gwei
            speed: None,
            nonce: Some(42),
            chain_id: 1101,
            hash: None,
            signature: None,
            raw: None,
        };

        let params = build_zkevm_transaction_params(&tx);

        // Verify the structure and values
        assert_eq!(params["from"], "0x742d35Cc6634C0532925a3b844Bc454e4438f44e");
        assert_eq!(params["to"], "0x742d35Cc6634C0532925a3b844Bc454e4438f44f");
        assert_eq!(params["value"], "0xde0b6b3a7640000"); // 1 ETH in hex
        assert_eq!(params["data"], "0x1234567890abcdef");
        assert_eq!(params["gas"], "0x5208"); // 21000 in hex
        assert_eq!(params["gasPrice"], "0x4a817c800"); // 20 Gwei in hex
        assert_eq!(params["maxFeePerGas"], "0x6fc23ac00"); // 30 Gwei in hex
        assert_eq!(params["maxPriorityFeePerGas"], "0x77359400"); // 2 Gwei in hex

        // Test with minimal transaction data
        let minimal_tx = EvmTransactionData {
            from: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            to: None,
            value: U256::ZERO,
            data: None,
            gas_limit: None,
            gas_price: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            speed: None,
            nonce: None,
            chain_id: 1101,
            hash: None,
            signature: None,
            raw: None,
        };

        let minimal_params = build_zkevm_transaction_params(&minimal_tx);

        assert_eq!(
            minimal_params["from"],
            "0x742d35Cc6634C0532925a3b844Bc454e4438f44e"
        );
        assert_eq!(minimal_params["to"], serde_json::Value::Null); // None becomes JSON null
        assert_eq!(minimal_params["value"], "0x0");
        assert_eq!(minimal_params["data"], "0x");
        assert_eq!(minimal_params["gas"], serde_json::Value::Null);
        assert_eq!(minimal_params["gasPrice"], serde_json::Value::Null);
        assert_eq!(minimal_params["maxFeePerGas"], serde_json::Value::Null);
        assert_eq!(
            minimal_params["maxPriorityFeePerGas"],
            serde_json::Value::Null
        );

        // Test data field normalization (without 0x prefix)
        let tx_without_prefix = EvmTransactionData {
            from: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            to: Some("0x742d35Cc6634C0532925a3b844Bc454e4438f44f".to_string()),
            value: U256::ZERO,
            data: Some("abcdef1234".to_string()), // No 0x prefix
            gas_limit: None,
            gas_price: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            speed: None,
            nonce: None,
            chain_id: 1101,
            hash: None,
            signature: None,
            raw: None,
        };

        let params_no_prefix = build_zkevm_transaction_params(&tx_without_prefix);
        assert_eq!(params_no_prefix["data"], "0xabcdef1234"); // Should add 0x prefix
    }
}
