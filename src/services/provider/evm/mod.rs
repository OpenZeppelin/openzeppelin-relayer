//! EVM Provider implementation for interacting with EVM-compatible blockchain networks.
//!
//! This module provides functionality to interact with EVM-based blockchains through RPC calls.
//! It implements common operations like getting balances, sending transactions, and querying
//! blockchain state.

use alloy::{
    primitives::{Bytes, TxKind, Uint},
    providers::{Provider, ProviderBuilder, RootProvider},
    rpc::types::{
        Block as BlockResponse, BlockNumberOrTag, BlockTransactionsKind, FeeHistory,
        TransactionInput, TransactionReceipt, TransactionRequest,
    },
    transports::http::{Client, Http},
};
use async_trait::async_trait;
use eyre::{eyre, Result};

use crate::models::{EvmTransactionData, TransactionError, U256};

#[cfg(test)]
use mockall::automock;

/// Provider implementation for EVM-compatible blockchain networks.
///
/// Wraps an HTTP RPC provider to interact with EVM chains like Ethereum, Polygon, etc.
#[derive(Clone)]
pub struct EvmProvider {
    provider: RootProvider<Http<Client>>,
}

/// Trait defining the interface for EVM blockchain interactions.
///
/// This trait provides methods for common blockchain operations like querying balances,
/// sending transactions, and getting network state.
#[async_trait]
#[cfg_attr(test, automock)]
#[allow(dead_code)]
pub trait EvmProviderTrait: Send + Sync {
    /// Gets the balance of an address in the native currency.
    ///
    /// # Arguments
    /// * `address` - The address to query the balance for
    async fn get_balance(&self, address: &str) -> Result<U256>;

    /// Gets the current block number of the chain.
    async fn get_block_number(&self) -> Result<u64>;

    /// Estimates the gas required for a transaction.
    ///
    /// # Arguments
    /// * `tx` - The transaction data to estimate gas for
    async fn estimate_gas(&self, tx: &EvmTransactionData) -> Result<u64>;

    /// Gets the current gas price from the network.
    async fn get_gas_price(&self) -> Result<u128>;

    /// Sends a transaction to the network.
    ///
    /// # Arguments
    /// * `tx` - The transaction request to send
    async fn send_transaction(&self, tx: TransactionRequest) -> Result<String>;

    /// Sends a raw signed transaction to the network.
    ///
    /// # Arguments
    /// * `tx` - The raw transaction bytes to send
    async fn send_raw_transaction(&self, tx: &[u8]) -> Result<String>;

    /// Performs a health check by attempting to get the latest block number.
    async fn health_check(&self) -> Result<bool>;

    /// Gets the transaction count (nonce) for an address.
    ///
    /// # Arguments
    /// * `address` - The address to query the transaction count for
    async fn get_transaction_count(&self, address: &str) -> Result<u64>;

    /// Gets the fee history for a range of blocks.
    ///
    /// # Arguments
    /// * `block_count` - Number of blocks to get fee history for
    /// * `newest_block` - The newest block to start from
    /// * `reward_percentiles` - Percentiles to sample reward data from
    async fn get_fee_history(
        &self,
        block_count: u64,
        newest_block: BlockNumberOrTag,
        reward_percentiles: Vec<f64>,
    ) -> Result<FeeHistory>;

    /// Gets the latest block from the network.
    async fn get_block_by_number(&self) -> Result<BlockResponse>;

    /// Gets a transaction receipt by its hash.
    ///
    /// # Arguments
    /// * `tx_hash` - The transaction hash to query
    async fn get_transaction_receipt(&self, tx_hash: &str) -> Result<Option<TransactionReceipt>>;

    /// Calls a contract function.
    ///
    /// # Arguments
    /// * `tx` - The transaction request to call the contract function
    async fn call_contract(&self, tx: &TransactionRequest) -> Result<Bytes>;
}

impl EvmProvider {
    /// Creates a new EVM provider instance.
    ///
    /// # Arguments
    /// * `url` - The RPC endpoint URL to connect to
    ///
    /// # Returns
    /// * `Result<Self>` - A new provider instance or an error
    pub fn new(url: &str) -> Result<Self> {
        let rpc_url = url.parse()?;
        let provider = ProviderBuilder::new().on_http(rpc_url);
        Ok(Self { provider })
    }
}

impl AsRef<EvmProvider> for EvmProvider {
    fn as_ref(&self) -> &EvmProvider {
        self
    }
}

#[async_trait]
impl EvmProviderTrait for EvmProvider {
    async fn get_balance(&self, address: &str) -> Result<U256> {
        let address = address.parse()?;
        self.provider
            .get_balance(address)
            .await
            .map_err(|e| eyre!("Failed to get balance: {}", e))
    }

    async fn get_block_number(&self) -> Result<u64> {
        self.provider
            .get_block_number()
            .await
            .map_err(|e| eyre!("Failed to get block number: {}", e))
    }

    async fn estimate_gas(&self, tx: &EvmTransactionData) -> Result<u64> {
        // transform the tx to a transaction request
        let transaction_request = TransactionRequest::try_from(tx)?;
        self.provider
            .estimate_gas(&transaction_request)
            .await
            .map_err(|e| eyre!("Failed to estimate gas: {}", e))
    }

    async fn get_gas_price(&self) -> Result<u128> {
        self.provider
            .get_gas_price()
            .await
            .map_err(|e| eyre!("Failed to get gas price: {}", e))
    }

    async fn send_transaction(&self, tx: TransactionRequest) -> Result<String> {
        let pending_tx = self
            .provider
            .send_transaction(tx)
            .await
            .map_err(|e| eyre!("Failed to send transaction: {}", e))?;

        let tx_hash = pending_tx.tx_hash().to_string();
        Ok(tx_hash)
    }

    async fn send_raw_transaction(&self, tx: &[u8]) -> Result<String> {
        let pending_tx = self
            .provider
            .send_raw_transaction(tx)
            .await
            .map_err(|e| eyre!("Failed to send raw transaction: {}", e))?;

        let tx_hash = pending_tx.tx_hash().to_string();
        Ok(tx_hash)
    }

    async fn health_check(&self) -> Result<bool> {
        self.get_block_number()
            .await
            .map(|_| true)
            .map_err(|e| eyre!("Health check failed: {}", e))
    }

    async fn get_transaction_count(&self, address: &str) -> Result<u64> {
        let address = address.parse()?;
        let result = self
            .provider
            .get_transaction_count(address)
            .await
            .map_err(|e| eyre!("Health check failed: {}", e))?;

        Ok(result)
    }

    async fn get_fee_history(
        &self,
        block_count: u64,
        newest_block: BlockNumberOrTag,
        reward_percentiles: Vec<f64>,
    ) -> Result<FeeHistory> {
        let fee_history = self
            .provider
            .get_fee_history(block_count, newest_block, &reward_percentiles)
            .await
            .map_err(|e| eyre!("Failed to get fee history: {}", e))?;
        Ok(fee_history)
    }

    async fn get_block_by_number(&self) -> Result<BlockResponse> {
        self.provider
            .get_block_by_number(BlockNumberOrTag::Latest, BlockTransactionsKind::Hashes)
            .await
            .map_err(|e| eyre!("Failed to get block by number: {}", e))?
            .ok_or_else(|| eyre!("Block not found"))
    }

    async fn get_transaction_receipt(&self, tx_hash: &str) -> Result<Option<TransactionReceipt>> {
        let tx_hash = tx_hash.parse()?;
        self.provider
            .get_transaction_receipt(tx_hash)
            .await
            .map_err(|e| eyre!("Failed to get transaction receipt: {}", e))
    }

    async fn call_contract(&self, tx: &TransactionRequest) -> Result<Bytes> {
        self.provider
            .call(tx)
            .await
            .map_err(|e| eyre!("Failed to call contract: {}", e))
    }
}

impl TryFrom<&EvmTransactionData> for TransactionRequest {
    type Error = TransactionError;
    fn try_from(tx: &EvmTransactionData) -> Result<Self, Self::Error> {
        Ok(TransactionRequest {
            from: Some(tx.from.clone().parse().map_err(|_| {
                TransactionError::InvalidType("Invalid address format".to_string())
            })?),
            to: Some(TxKind::Call(
                tx.to
                    .clone()
                    .unwrap_or("".to_string())
                    .parse()
                    .map_err(|_| {
                        TransactionError::InvalidType("Invalid address format".to_string())
                    })?,
            )),
            gas_price: Some(
                Uint::<256, 4>::from(tx.gas_price.unwrap_or(0))
                    .try_into()
                    .map_err(|_| TransactionError::InvalidType("Invalid gas price".to_string()))?,
            ),
            // we should not set gas here
            // gas: Some(
            //     Uint::<256, 4>::from(tx.gas_limit)
            //         .try_into()
            //         .map_err(|_| TransactionError::InvalidType("Invalid gas
            // limit".to_string()))?, ),
            value: Some(Uint::<256, 4>::from(tx.value)),
            input: TransactionInput::from(tx.data.clone().unwrap_or("".to_string()).into_bytes()),
            nonce: Some(
                Uint::<256, 4>::from(tx.nonce.ok_or_else(|| {
                    TransactionError::InvalidType("Nonce must be defined".to_string())
                })?)
                .try_into()
                .map_err(|_| TransactionError::InvalidType("Invalid nonce".to_string()))?,
            ),
            chain_id: Some(tx.chain_id),
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::Address;
    use futures::FutureExt;
    use std::str::FromStr;

    #[test]
    fn test_new_provider() {
        let provider = EvmProvider::new("http://localhost:8545");
        assert!(provider.is_ok());

        let provider = EvmProvider::new("invalid-url");
        assert!(provider.is_err());
    }

    #[test]
    fn test_transaction_request_conversion() {
        let tx_data = EvmTransactionData {
            from: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            to: Some("0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string()),
            gas_price: Some(1000000000),
            value: Uint::<256, 4>::from(1000000000),
            data: Some("0x".to_string()),
            nonce: Some(1),
            chain_id: 1,
            gas_limit: 21000,
            hash: None,
            signature: None,
            speed: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            raw: None,
        };

        let result = TransactionRequest::try_from(&tx_data);
        assert!(result.is_ok());

        let tx_request = result.unwrap();
        assert_eq!(
            tx_request.from,
            Some(Address::from_str("0x742d35Cc6634C0532925a3b844Bc454e4438f44e").unwrap())
        );
        assert_eq!(tx_request.chain_id, Some(1));
    }

    #[tokio::test]
    async fn test_mock_provider_methods() {
        let mut mock = MockEvmProviderTrait::new();

        mock.expect_get_balance()
            .with(mockall::predicate::eq(
                "0x742d35Cc6634C0532925a3b844Bc454e4438f44e",
            ))
            .times(1)
            .returning(|_| async { Ok(U256::from(100)) }.boxed());

        mock.expect_get_block_number()
            .times(1)
            .returning(|| async { Ok(12345) }.boxed());

        mock.expect_get_gas_price()
            .times(1)
            .returning(|| async { Ok(20000000000) }.boxed());

        mock.expect_health_check()
            .times(1)
            .returning(|| async { Ok(true) }.boxed());

        mock.expect_get_transaction_count()
            .with(mockall::predicate::eq(
                "0x742d35Cc6634C0532925a3b844Bc454e4438f44e",
            ))
            .times(1)
            .returning(|_| async { Ok(42) }.boxed());

        mock.expect_get_fee_history()
            .with(
                mockall::predicate::eq(10u64),
                mockall::predicate::eq(BlockNumberOrTag::Latest),
                mockall::predicate::eq(vec![25.0, 50.0, 75.0]),
            )
            .times(1)
            .returning(|_, _, _| {
                async {
                    Ok(FeeHistory {
                        oldest_block: 100,
                        base_fee_per_gas: vec![1000],
                        gas_used_ratio: vec![0.5],
                        reward: Some(vec![vec![500]]),
                        base_fee_per_blob_gas: vec![1000],
                        blob_gas_used_ratio: vec![0.5],
                    })
                }
                .boxed()
            });

        // Test all methods
        let balance = mock
            .get_balance("0x742d35Cc6634C0532925a3b844Bc454e4438f44e")
            .await;
        assert!(balance.is_ok());
        assert_eq!(balance.unwrap(), U256::from(100));

        let block_number = mock.get_block_number().await;
        assert!(block_number.is_ok());
        assert_eq!(block_number.unwrap(), 12345);

        let gas_price = mock.get_gas_price().await;
        assert!(gas_price.is_ok());
        assert_eq!(gas_price.unwrap(), 20000000000);

        let health = mock.health_check().await;
        assert!(health.is_ok());
        assert!(health.unwrap());

        let count = mock
            .get_transaction_count("0x742d35Cc6634C0532925a3b844Bc454e4438f44e")
            .await;
        assert!(count.is_ok());
        assert_eq!(count.unwrap(), 42);

        let fee_history = mock
            .get_fee_history(10, BlockNumberOrTag::Latest, vec![25.0, 50.0, 75.0])
            .await;
        assert!(fee_history.is_ok());
        let fee_history = fee_history.unwrap();
        assert_eq!(fee_history.oldest_block, 100);
        assert_eq!(fee_history.gas_used_ratio, vec![0.5]);
    }

    #[tokio::test]
    async fn test_mock_transaction_operations() {
        let mut mock = MockEvmProviderTrait::new();

        // Setup mock for estimate_gas
        let tx_data = EvmTransactionData {
            from: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            to: Some("0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string()),
            gas_price: Some(1000000000),
            value: Uint::<256, 4>::from(1000000000),
            data: Some("0x".to_string()),
            nonce: Some(1),
            chain_id: 1,
            gas_limit: 21000,
            hash: None,
            signature: None,
            speed: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            raw: None,
        };

        mock.expect_estimate_gas()
            .with(mockall::predicate::always())
            .times(1)
            .returning(|_| async { Ok(21000) }.boxed());

        // Setup mock for send_raw_transaction
        mock.expect_send_raw_transaction()
            .with(mockall::predicate::always())
            .times(1)
            .returning(|_| async { Ok("0x123456789abcdef".to_string()) }.boxed());

        // Test the mocked methods
        let gas_estimate = mock.estimate_gas(&tx_data).await;
        assert!(gas_estimate.is_ok());
        assert_eq!(gas_estimate.unwrap(), 21000);

        let tx_hash = mock.send_raw_transaction(&[0u8; 32]).await;
        assert!(tx_hash.is_ok());
        assert_eq!(tx_hash.unwrap(), "0x123456789abcdef");
    }

    #[test]
    fn test_invalid_transaction_request_conversion() {
        let tx_data = EvmTransactionData {
            from: "invalid-address".to_string(),
            to: Some("0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string()),
            gas_price: Some(1000000000),
            value: Uint::<256, 4>::from(1000000000),
            data: Some("0x".to_string()),
            nonce: Some(1),
            chain_id: 1,
            gas_limit: 21000,
            hash: None,
            signature: None,
            speed: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            raw: None,
        };

        let result = TransactionRequest::try_from(&tx_data);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_mock_additional_methods() {
        let mut mock = MockEvmProviderTrait::new();

        // Setup mock for health_check
        mock.expect_health_check()
            .times(1)
            .returning(|| async { Ok(true) }.boxed());

        // Setup mock for get_transaction_count
        mock.expect_get_transaction_count()
            .with(mockall::predicate::eq(
                "0x742d35Cc6634C0532925a3b844Bc454e4438f44e",
            ))
            .times(1)
            .returning(|_| async { Ok(42) }.boxed());

        // Setup mock for get_fee_history
        mock.expect_get_fee_history()
            .with(
                mockall::predicate::eq(10u64),
                mockall::predicate::eq(BlockNumberOrTag::Latest),
                mockall::predicate::eq(vec![25.0, 50.0, 75.0]),
            )
            .times(1)
            .returning(|_, _, _| {
                async {
                    Ok(FeeHistory {
                        oldest_block: 100,
                        base_fee_per_gas: vec![1000],
                        gas_used_ratio: vec![0.5],
                        reward: Some(vec![vec![500]]),
                        base_fee_per_blob_gas: vec![1000],
                        blob_gas_used_ratio: vec![0.5],
                    })
                }
                .boxed()
            });

        // Test health check
        let health = mock.health_check().await;
        assert!(health.is_ok());
        assert!(health.unwrap());

        // Test get_transaction_count
        let count = mock
            .get_transaction_count("0x742d35Cc6634C0532925a3b844Bc454e4438f44e")
            .await;
        assert!(count.is_ok());
        assert_eq!(count.unwrap(), 42);

        // Test get_fee_history
        let fee_history = mock
            .get_fee_history(10, BlockNumberOrTag::Latest, vec![25.0, 50.0, 75.0])
            .await;
        assert!(fee_history.is_ok());
        let fee_history = fee_history.unwrap();
        assert_eq!(fee_history.oldest_block, 100);
        assert_eq!(fee_history.gas_used_ratio, vec![0.5]);
    }

    #[tokio::test]
    async fn test_call_contract() {
        let mut mock = MockEvmProviderTrait::new();

        let tx = TransactionRequest {
            from: Some(Address::from_str("0x742d35Cc6634C0532925a3b844Bc454e4438f44e").unwrap()),
            to: Some(TxKind::Call(
                Address::from_str("0xCcCCccccCCCCcCCCCCCcCcCccCcCCCcCcccccccC").unwrap(),
            )),
            input: TransactionInput::from(
                hex::decode("a9059cbb000000000000000000000000742d35cc6634c0532925a3b844bc454e4438f44e0000000000000000000000000000000000000000000000000de0b6b3a7640000").unwrap()
            ),
            ..Default::default()
        };

        // Setup mock for call_contract
        mock.expect_call_contract()
            .with(mockall::predicate::always())
            .times(1)
            .returning(|_| {
                async {
                    Ok(Bytes::from(
                        hex::decode(
                            "0000000000000000000000000000000000000000000000000000000000000001",
                        )
                        .unwrap(),
                    ))
                }
                .boxed()
            });

        let result = mock.call_contract(&tx).await;
        assert!(result.is_ok());

        let data = result.unwrap();
        assert_eq!(
            hex::encode(data),
            "0000000000000000000000000000000000000000000000000000000000000001"
        );
    }
}
