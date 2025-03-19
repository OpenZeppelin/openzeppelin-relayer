use crate::{
    models::{EvmNetwork, EvmTransactionData, TransactionError, TransactionStatus, U256},
    services::EvmProviderTrait,
};
use eyre::Result;

use super::PriceParams;

/// Creates a "noop" transaction (transaction to self with zero value and no data)
/// This is commonly used for cancellation and replacement transactions
pub async fn make_noop<P: EvmProviderTrait>(
    provider: &P,
    from: String,
    gas_params: PriceParams,
    network: EvmNetwork,
) -> Result<EvmTransactionData, TransactionError> {
    let gas_limit = if network.is_arbitrum() {
        // Arbitrum gas estimation can be flaky, use a safe default
        // 20,000,000 is Arbitrum's default gas limit when simulation is skipped
        20_000_000
    } else if network.is_optimism() {
        // For L2s like Optimism, we need to estimate gas as transfers cost more than 21k
        let tx = EvmTransactionData {
            gas_price: gas_params.gas_price,
            gas_limit: 21_000, // Initial estimate
            nonce: None,
            value: U256::from(0u64),
            data: Some("0x".to_string()),
            from: from.clone(),
            to: Some(from.clone()),
            chain_id: network.id(),
            hash: None,
            signature: None,
            speed: None,
            max_fee_per_gas: gas_params.max_fee_per_gas,
            max_priority_fee_per_gas: gas_params.max_priority_fee_per_gas,
            raw: None,
        };

        let estimate = provider.estimate_gas(&tx).await?;
        // Add a 10% buffer for safety
        (estimate as f64 * 1.1) as u64
    } else {
        // Standard ETH transfer gas limit
        21_000
    };

    Ok(EvmTransactionData {
        gas_price: gas_params.gas_price,
        gas_limit,
        nonce: None, // Will be set by the caller
        value: U256::from(0u64),
        data: Some("0x".to_string()),
        to: Some(from.clone()), // Send to self
        from,
        chain_id: network.id(),
        hash: None,
        signature: None,
        speed: None,
        max_fee_per_gas: gas_params.max_fee_per_gas,
        max_priority_fee_per_gas: gas_params.max_priority_fee_per_gas,
        raw: None,
    })
}

pub fn is_transaction_not_yet_mined(tx_status: TransactionStatus) -> bool {
    tx_status == TransactionStatus::Pending
        || tx_status == TransactionStatus::Sent
        || tx_status == TransactionStatus::Submitted
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::EvmNamedNetwork;
    use crate::services::MockEvmProviderTrait;
    use futures::FutureExt;

    fn create_mock_provider() -> MockEvmProviderTrait {
        let mut mock = MockEvmProviderTrait::new();
        mock.expect_estimate_gas()
            .returning(|_| async { Ok(21_000) }.boxed());
        mock
    }

    #[tokio::test]
    async fn test_make_noop_standard_network() {
        let network = EvmNetwork::from_named(EvmNamedNetwork::Mainnet);
        let from = "0x1234567890123456789012345678901234567890".to_string();
        let gas_params = PriceParams {
            gas_price: Some(20_000_000_000),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            is_min_bumped: None,
        };

        let provider = create_mock_provider();

        let result = make_noop(&provider, from.clone(), gas_params, network).await;
        assert!(result.is_ok());

        let tx = result.unwrap();
        assert_eq!(tx.gas_limit, 21_000); // Standard gas limit
        assert_eq!(tx.from, from);
        assert_eq!(tx.to.unwrap(), from);
        assert_eq!(tx.value, U256::from(0u64));
        assert_eq!(tx.data.unwrap(), "0x");
        assert_eq!(tx.gas_price, Some(20_000_000_000));
    }

    #[tokio::test]
    async fn test_make_noop_arbitrum() {
        let network = EvmNetwork::from_named(EvmNamedNetwork::Arbitrum);
        let from = "0x1234567890123456789012345678901234567890".to_string();
        let gas_params = PriceParams {
            gas_price: Some(20_000_000_000),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            is_min_bumped: None,
        };

        let provider = create_mock_provider();

        let result = make_noop(&provider, from.clone(), gas_params, network).await;
        assert!(result.is_ok());

        let tx = result.unwrap();
        assert_eq!(tx.gas_limit, 20_000_000); // Arbitrum's default gas limit
        assert_eq!(tx.from, from);
        assert_eq!(tx.to.unwrap(), from);
        assert_eq!(tx.value, U256::from(0u64));
        assert_eq!(tx.data.unwrap(), "0x");
        assert_eq!(tx.gas_price, Some(20_000_000_000));
    }

    #[tokio::test]
    async fn test_make_noop_optimism() {
        let network = EvmNetwork::from_named(EvmNamedNetwork::Optimism);
        let from = "0x1234567890123456789012345678901234567890".to_string();
        let gas_params = PriceParams {
            gas_price: Some(20_000_000_000),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            is_min_bumped: None,
        };

        let provider = create_mock_provider();

        let result = make_noop(&provider, from.clone(), gas_params, network).await;
        assert!(result.is_ok());

        let tx = result.unwrap();
        assert_eq!(tx.gas_limit, 23_100); // 21,000 * 1.1 (10% buffer)
        assert_eq!(tx.from, from);
        assert_eq!(tx.to.unwrap(), from);
        assert_eq!(tx.value, U256::from(0u64));
        assert_eq!(tx.data.unwrap(), "0x");
        assert_eq!(tx.gas_price, Some(20_000_000_000));
    }
}
