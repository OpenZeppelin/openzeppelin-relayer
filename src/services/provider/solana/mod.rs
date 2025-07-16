//! Solana Provider Module
//!
//! This module provides an abstraction layer over the Solana RPC client,
//! offering common operations such as retrieving account balance, fetching
//! the latest blockhash, sending transactions, confirming transactions, and
//! querying the minimum balance for rent exemption.
//!
//! The provider uses the non-blocking `RpcClient` for asynchronous operations
//! and integrates detailed error handling through the `ProviderError` type.
//!
//! TODO: add support for using multiple RPCs and retries
use async_trait::async_trait;
use eyre::Result;
#[cfg(test)]
use mockall::automock;
use mpl_token_metadata::accounts::Metadata;
use reqwest::Url;
use serde::Serialize;
use solana_client::{
    nonblocking::rpc_client::RpcClient,
    rpc_response::{RpcPrioritizationFee, RpcSimulateTransactionResult},
};
use solana_sdk::{
    account::Account,
    commitment_config::CommitmentConfig,
    hash::Hash,
    message::Message,
    program_pack::Pack,
    pubkey::Pubkey,
    signature::Signature,
    transaction::{Transaction, VersionedTransaction},
};
use spl_token::state::Mint;
use std::{str::FromStr, sync::Arc, time::Duration};
use thiserror::Error;

use crate::{models::RpcConfig, services::retry_rpc_call};

use super::ProviderError;
use super::{
    rpc_selector::{RpcSelector, RpcSelectorError},
    RetryConfig,
};

#[derive(Error, Debug, Serialize)]
pub enum SolanaProviderError {
    #[error("RPC client error: {0}")]
    RpcError(String),
    #[error("Invalid address: {0}")]
    InvalidAddress(String),
    #[error("RPC selector error: {0}")]
    SelectorError(RpcSelectorError),
    #[error("Network configuration error: {0}")]
    NetworkConfiguration(String),
}

/// A trait that abstracts common Solana provider operations.
#[async_trait]
#[cfg_attr(test, automock)]
#[allow(dead_code)]
pub trait SolanaProviderTrait: Send + Sync {
    /// Retrieves the balance (in lamports) for the given address.
    async fn get_balance(&self, address: &str) -> Result<u64, SolanaProviderError>;

    /// Retrieves the latest blockhash as a 32-byte array.
    async fn get_latest_blockhash(&self) -> Result<Hash, SolanaProviderError>;

    // Retrieves the latest blockhash with the specified commitment.
    async fn get_latest_blockhash_with_commitment(
        &self,
        commitment: CommitmentConfig,
    ) -> Result<(Hash, u64), SolanaProviderError>;

    /// Sends a transaction to the Solana network.
    async fn send_transaction(
        &self,
        transaction: &Transaction,
    ) -> Result<Signature, SolanaProviderError>;

    /// Sends a transaction to the Solana network.
    async fn send_versioned_transaction(
        &self,
        transaction: &VersionedTransaction,
    ) -> Result<Signature, SolanaProviderError>;

    /// Confirms a transaction given its signature.
    async fn confirm_transaction(&self, signature: &Signature)
        -> Result<bool, SolanaProviderError>;

    /// Retrieves the minimum balance required for rent exemption for the specified data size.
    async fn get_minimum_balance_for_rent_exemption(
        &self,
        data_size: usize,
    ) -> Result<u64, SolanaProviderError>;

    /// Simulates a transaction and returns the simulation result.
    async fn simulate_transaction(
        &self,
        transaction: &Transaction,
    ) -> Result<RpcSimulateTransactionResult, SolanaProviderError>;

    /// Retrieve an account given its string representation.
    async fn get_account_from_str(&self, account: &str) -> Result<Account, SolanaProviderError>;

    /// Retrieve an account given its Pubkey.
    async fn get_account_from_pubkey(
        &self,
        pubkey: &Pubkey,
    ) -> Result<Account, SolanaProviderError>;

    /// Retrieve token metadata from the provided pubkey.
    async fn get_token_metadata_from_pubkey(
        &self,
        pubkey: &str,
    ) -> Result<TokenMetadata, SolanaProviderError>;

    /// Check if a blockhash is valid.
    async fn is_blockhash_valid(
        &self,
        hash: &Hash,
        commitment: CommitmentConfig,
    ) -> Result<bool, SolanaProviderError>;

    /// get fee for message
    async fn get_fee_for_message(&self, message: &Message) -> Result<u64, SolanaProviderError>;

    /// get recent prioritization fees
    async fn get_recent_prioritization_fees(
        &self,
        addresses: &[Pubkey],
    ) -> Result<Vec<RpcPrioritizationFee>, SolanaProviderError>;

    /// calculate total fee
    async fn calculate_total_fee(&self, message: &Message) -> Result<u64, SolanaProviderError>;
}

#[derive(Debug)]
pub struct SolanaProvider {
    // RPC selector for handling multiple client connections
    selector: RpcSelector,
    // Default timeout in seconds
    timeout_seconds: Duration,
    // Default commitment level
    commitment: CommitmentConfig,
    // Retry configuration for network requests
    retry_config: RetryConfig,
}

impl From<String> for SolanaProviderError {
    fn from(s: String) -> Self {
        SolanaProviderError::RpcError(s)
    }
}

const RETRIABLE_ERROR_SUBSTRINGS: &[&str] = &[
    "timeout",
    "connection",
    "reset",
    "temporarily unavailable",
    "rate limit",
    "too many requests",
    "503",
    "502",
    "504",
    "blockhash not found",
    "node is behind",
    "unhealthy",
];

fn is_retriable_error(msg: &str) -> bool {
    RETRIABLE_ERROR_SUBSTRINGS
        .iter()
        .any(|substr| msg.contains(substr))
}

#[derive(Error, Debug, PartialEq)]
pub struct TokenMetadata {
    pub decimals: u8,
    pub symbol: String,
    pub mint: String,
}

impl std::fmt::Display for TokenMetadata {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "TokenMetadata {{ decimals: {}, symbol: {}, mint: {} }}",
            self.decimals, self.symbol, self.mint
        )
    }
}

#[allow(dead_code)]
impl SolanaProvider {
    pub fn new(configs: Vec<RpcConfig>, timeout_seconds: u64) -> Result<Self, ProviderError> {
        Self::new_with_commitment(configs, timeout_seconds, CommitmentConfig::confirmed())
    }

    /// Creates a new SolanaProvider with RPC configurations and optional settings.
    ///
    /// # Arguments
    ///
    /// * `configs` - A vector of RPC configurations
    /// * `timeout` - Optional custom timeout
    /// * `commitment` - Optional custom commitment level
    ///
    /// # Returns
    ///
    /// A Result containing the provider or an error
    pub fn new_with_commitment(
        configs: Vec<RpcConfig>,
        timeout_seconds: u64,
        commitment: CommitmentConfig,
    ) -> Result<Self, ProviderError> {
        if configs.is_empty() {
            return Err(ProviderError::NetworkConfiguration(
                "At least one RPC configuration must be provided".to_string(),
            ));
        }

        RpcConfig::validate_list(&configs)
            .map_err(|e| ProviderError::NetworkConfiguration(format!("Invalid URL: {}", e)))?;

        // Now create the selector with validated configs
        let selector = RpcSelector::new(configs).map_err(|e| {
            ProviderError::NetworkConfiguration(format!("Failed to create RPC selector: {}", e))
        })?;

        let retry_config = RetryConfig::from_env();

        Ok(Self {
            selector,
            timeout_seconds: Duration::from_secs(timeout_seconds),
            commitment,
            retry_config,
        })
    }

    /// Retrieves an RPC client instance using the configured selector.
    ///
    /// # Returns
    ///
    /// A Result containing either:
    /// - A configured RPC client connected to a selected endpoint
    /// - A SolanaProviderError describing what went wrong
    ///
    fn get_client(&self) -> Result<RpcClient, SolanaProviderError> {
        self.selector
            .get_client(|url| {
                Ok(RpcClient::new_with_timeout_and_commitment(
                    url.to_string(),
                    self.timeout_seconds,
                    self.commitment,
                ))
            })
            .map_err(SolanaProviderError::SelectorError)
    }

    /// Initialize a provider for a given URL
    fn initialize_provider(&self, url: &str) -> Result<Arc<RpcClient>, SolanaProviderError> {
        let rpc_url: Url = url.parse().map_err(|e| {
            SolanaProviderError::NetworkConfiguration(format!("Invalid URL format: {}", e))
        })?;

        let client = RpcClient::new_with_timeout_and_commitment(
            rpc_url.to_string(),
            self.timeout_seconds,
            self.commitment,
        );

        Ok(Arc::new(client))
    }

    /// Retry helper for Solana RPC calls
    async fn retry_rpc_call<T, F, Fut>(
        &self,
        operation_name: &str,
        operation: F,
    ) -> Result<T, SolanaProviderError>
    where
        F: Fn(Arc<RpcClient>) -> Fut,
        Fut: std::future::Future<Output = Result<T, SolanaProviderError>>,
    {
        let is_retriable = |e: &SolanaProviderError| match e {
            SolanaProviderError::RpcError(msg) => is_retriable_error(msg),
            _ => false,
        };

        log::debug!(
            "Starting RPC operation '{}' with timeout: {}s",
            operation_name,
            self.timeout_seconds.as_secs()
        );

        retry_rpc_call(
            &self.selector,
            operation_name,
            is_retriable,
            |_| false, // TODO: implement fn to mark provider failed based on error
            |url| match self.initialize_provider(url) {
                Ok(provider) => Ok(provider),
                Err(e) => Err(e),
            },
            operation,
            Some(self.retry_config.clone()),
        )
        .await
    }
}

#[async_trait]
#[allow(dead_code)]
impl SolanaProviderTrait for SolanaProvider {
    /// Retrieves the balance (in lamports) for the given address.
    /// # Errors
    ///
    /// Returns `ProviderError::InvalidAddress` if address parsing fails,
    /// and `ProviderError::RpcError` if the RPC call fails.
    async fn get_balance(&self, address: &str) -> Result<u64, SolanaProviderError> {
        let pubkey = Pubkey::from_str(address)
            .map_err(|e| SolanaProviderError::InvalidAddress(e.to_string()))?;

        self.retry_rpc_call("get_balance", |client| async move {
            client
                .get_balance(&pubkey)
                .await
                .map_err(|e| SolanaProviderError::RpcError(e.to_string()))
        })
        .await
    }

    /// Check if a blockhash is valid
    async fn is_blockhash_valid(
        &self,
        hash: &Hash,
        commitment: CommitmentConfig,
    ) -> Result<bool, SolanaProviderError> {
        self.retry_rpc_call("is_blockhash_valid", |client| async move {
            client
                .is_blockhash_valid(hash, commitment)
                .await
                .map_err(|e| SolanaProviderError::RpcError(e.to_string()))
        })
        .await
    }

    /// Gets the latest blockhash.
    async fn get_latest_blockhash(&self) -> Result<Hash, SolanaProviderError> {
        self.retry_rpc_call("get_latest_blockhash", |client| async move {
            client
                .get_latest_blockhash()
                .await
                .map_err(|e| SolanaProviderError::RpcError(e.to_string()))
        })
        .await
    }

    async fn get_latest_blockhash_with_commitment(
        &self,
        commitment: CommitmentConfig,
    ) -> Result<(Hash, u64), SolanaProviderError> {
        self.retry_rpc_call(
            "get_latest_blockhash_with_commitment",
            |client| async move {
                client
                    .get_latest_blockhash_with_commitment(commitment)
                    .await
                    .map_err(|e| SolanaProviderError::RpcError(e.to_string()))
            },
        )
        .await
    }

    /// Sends a transaction to the network.
    async fn send_transaction(
        &self,
        transaction: &Transaction,
    ) -> Result<Signature, SolanaProviderError> {
        self.retry_rpc_call("send_transaction", |client| async move {
            client
                .send_transaction(transaction)
                .await
                .map_err(|e| SolanaProviderError::RpcError(e.to_string()))
        })
        .await
    }

    /// Sends a transaction to the network.
    async fn send_versioned_transaction(
        &self,
        transaction: &VersionedTransaction,
    ) -> Result<Signature, SolanaProviderError> {
        self.retry_rpc_call("send_transaction", |client| async move {
            client
                .send_transaction(transaction)
                .await
                .map_err(|e| SolanaProviderError::RpcError(e.to_string()))
        })
        .await
    }

    /// Confirms the given transaction signature.
    async fn confirm_transaction(
        &self,
        signature: &Signature,
    ) -> Result<bool, SolanaProviderError> {
        self.retry_rpc_call("confirm_transaction", |client| async move {
            client
                .confirm_transaction(signature)
                .await
                .map_err(|e| SolanaProviderError::RpcError(e.to_string()))
        })
        .await
    }

    /// Retrieves the minimum balance for rent exemption for the given data size.
    async fn get_minimum_balance_for_rent_exemption(
        &self,
        data_size: usize,
    ) -> Result<u64, SolanaProviderError> {
        self.retry_rpc_call(
            "get_minimum_balance_for_rent_exemption",
            |client| async move {
                client
                    .get_minimum_balance_for_rent_exemption(data_size)
                    .await
                    .map_err(|e| SolanaProviderError::RpcError(e.to_string()))
            },
        )
        .await
    }

    /// Simulate transaction.
    async fn simulate_transaction(
        &self,
        transaction: &Transaction,
    ) -> Result<RpcSimulateTransactionResult, SolanaProviderError> {
        self.retry_rpc_call("simulate_transaction", |client| async move {
            client
                .simulate_transaction(transaction)
                .await
                .map_err(|e| SolanaProviderError::RpcError(e.to_string()))
                .map(|response| response.value)
        })
        .await
    }

    /// Retrieves account data for the given account string.
    async fn get_account_from_str(&self, account: &str) -> Result<Account, SolanaProviderError> {
        let address = Pubkey::from_str(account).map_err(|e| {
            SolanaProviderError::InvalidAddress(format!("Invalid pubkey {}: {}", account, e))
        })?;
        self.retry_rpc_call("get_account", |client| async move {
            client
                .get_account(&address)
                .await
                .map_err(|e| SolanaProviderError::RpcError(e.to_string()))
        })
        .await
    }

    /// Retrieves account data for the given pubkey.
    async fn get_account_from_pubkey(
        &self,
        pubkey: &Pubkey,
    ) -> Result<Account, SolanaProviderError> {
        self.retry_rpc_call("get_account_from_pubkey", |client| async move {
            client
                .get_account(pubkey)
                .await
                .map_err(|e| SolanaProviderError::RpcError(e.to_string()))
        })
        .await
    }

    /// Retrieves token metadata from a provided mint address.
    async fn get_token_metadata_from_pubkey(
        &self,
        pubkey: &str,
    ) -> Result<TokenMetadata, SolanaProviderError> {
        // Retrieve account associated with the given pubkey
        let account = self.get_account_from_str(pubkey).await.map_err(|e| {
            SolanaProviderError::RpcError(format!("Failed to fetch account for {}: {}", pubkey, e))
        })?;

        // Unpack the mint info from the account's data
        let mint_info = Mint::unpack(&account.data).map_err(|e| {
            SolanaProviderError::RpcError(format!("Failed to unpack mint info: {}", e))
        })?;
        let decimals = mint_info.decimals;

        // Convert provided string into a Pubkey
        let mint_pubkey = Pubkey::try_from(pubkey).map_err(|e| {
            SolanaProviderError::RpcError(format!("Invalid pubkey {}: {}", pubkey, e))
        })?;

        // Derive the PDA for the token metadata
        let metadata_pda = Metadata::find_pda(&mint_pubkey).0;

        let symbol = match self.get_account_from_pubkey(&metadata_pda).await {
            Ok(metadata_account) => match Metadata::from_bytes(&metadata_account.data) {
                Ok(metadata) => metadata.symbol.trim_end_matches('\u{0}').to_string(),
                Err(_) => String::new(),
            },
            Err(_) => String::new(), // Return empty symbol if metadata doesn't exist
        };

        Ok(TokenMetadata {
            decimals,
            symbol,
            mint: pubkey.to_string(),
        })
    }

    /// Get the fee for a message
    async fn get_fee_for_message(&self, message: &Message) -> Result<u64, SolanaProviderError> {
        self.retry_rpc_call("get_fee_for_message", |client| async move {
            client
                .get_fee_for_message(message)
                .await
                .map_err(|e| SolanaProviderError::RpcError(e.to_string()))
        })
        .await
    }

    async fn get_recent_prioritization_fees(
        &self,
        addresses: &[Pubkey],
    ) -> Result<Vec<RpcPrioritizationFee>, SolanaProviderError> {
        self.retry_rpc_call("get_recent_prioritization_fees", |client| async move {
            client
                .get_recent_prioritization_fees(addresses)
                .await
                .map_err(|e| SolanaProviderError::RpcError(e.to_string()))
        })
        .await
    }

    async fn calculate_total_fee(&self, message: &Message) -> Result<u64, SolanaProviderError> {
        let base_fee = self.get_fee_for_message(message).await?;
        let priority_fees = self.get_recent_prioritization_fees(&[]).await?;

        let max_priority_fee = priority_fees
            .iter()
            .map(|fee| fee.prioritization_fee)
            .max()
            .unwrap_or(0);

        Ok(base_fee + max_priority_fee)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lazy_static::lazy_static;
    use solana_sdk::{
        hash::Hash,
        message::Message,
        signer::{keypair::Keypair, Signer},
        transaction::Transaction,
    };
    use std::sync::Mutex;

    lazy_static! {
        static ref EVM_TEST_ENV_MUTEX: Mutex<()> = Mutex::new(());
    }

    struct EvmTestEnvGuard {
        _mutex_guard: std::sync::MutexGuard<'static, ()>,
    }

    impl EvmTestEnvGuard {
        fn new(mutex_guard: std::sync::MutexGuard<'static, ()>) -> Self {
            std::env::set_var(
                "API_KEY",
                "test_api_key_for_evm_provider_new_this_is_long_enough_32_chars",
            );
            std::env::set_var("REDIS_URL", "redis://test-dummy-url-for-evm-provider");

            Self {
                _mutex_guard: mutex_guard,
            }
        }
    }

    impl Drop for EvmTestEnvGuard {
        fn drop(&mut self) {
            std::env::remove_var("API_KEY");
            std::env::remove_var("REDIS_URL");
        }
    }

    // Helper function to set up the test environment
    fn setup_test_env() -> EvmTestEnvGuard {
        let guard = EVM_TEST_ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        EvmTestEnvGuard::new(guard)
    }

    fn get_funded_keypair() -> Keypair {
        // address HCKHoE2jyk1qfAwpHQghvYH3cEfT8euCygBzF9AV6bhY
        Keypair::try_from(
            [
                120, 248, 160, 20, 225, 60, 226, 195, 68, 137, 176, 87, 21, 129, 0, 76, 144, 129,
                122, 250, 80, 4, 247, 50, 248, 82, 146, 77, 139, 156, 40, 41, 240, 161, 15, 81,
                198, 198, 86, 167, 90, 148, 131, 13, 184, 222, 251, 71, 229, 212, 169, 2, 72, 202,
                150, 184, 176, 148, 75, 160, 255, 233, 73, 31,
            ]
            .as_slice(),
        )
        .unwrap()
    }

    // Helper function to obtain a recent blockhash from the provider.
    async fn get_recent_blockhash(provider: &SolanaProvider) -> Hash {
        provider
            .get_latest_blockhash()
            .await
            .expect("Failed to get blockhash")
    }

    fn create_test_rpc_config() -> RpcConfig {
        RpcConfig {
            url: "https://api.devnet.solana.com".to_string(),
            weight: 1,
        }
    }

    #[tokio::test]
    async fn test_new_with_valid_config() {
        let _env_guard = setup_test_env();
        let configs = vec![create_test_rpc_config()];
        let timeout = 30;

        let result = SolanaProvider::new(configs, timeout);

        assert!(result.is_ok());
        let provider = result.unwrap();
        assert_eq!(provider.timeout_seconds, Duration::from_secs(timeout));
        assert_eq!(provider.commitment, CommitmentConfig::confirmed());
    }

    #[tokio::test]
    async fn test_new_with_commitment_valid_config() {
        let _env_guard = setup_test_env();

        let configs = vec![create_test_rpc_config()];
        let timeout = 30;
        let commitment = CommitmentConfig::finalized();

        let result = SolanaProvider::new_with_commitment(configs, timeout, commitment);

        assert!(result.is_ok());
        let provider = result.unwrap();
        assert_eq!(provider.timeout_seconds, Duration::from_secs(timeout));
        assert_eq!(provider.commitment, commitment);
    }

    #[tokio::test]
    async fn test_new_with_empty_configs() {
        let _env_guard = setup_test_env();
        let configs: Vec<RpcConfig> = vec![];
        let timeout = 30;

        let result = SolanaProvider::new(configs, timeout);

        assert!(result.is_err());
        assert!(matches!(
            result,
            Err(ProviderError::NetworkConfiguration(_))
        ));
    }

    #[tokio::test]
    async fn test_new_with_commitment_empty_configs() {
        let _env_guard = setup_test_env();
        let configs: Vec<RpcConfig> = vec![];
        let timeout = 30;
        let commitment = CommitmentConfig::finalized();

        let result = SolanaProvider::new_with_commitment(configs, timeout, commitment);

        assert!(result.is_err());
        assert!(matches!(
            result,
            Err(ProviderError::NetworkConfiguration(_))
        ));
    }

    #[tokio::test]
    async fn test_new_with_invalid_url() {
        let _env_guard = setup_test_env();
        let configs = vec![RpcConfig {
            url: "invalid-url".to_string(),
            weight: 1,
        }];
        let timeout = 30;

        let result = SolanaProvider::new(configs, timeout);

        assert!(result.is_err());
        assert!(matches!(
            result,
            Err(ProviderError::NetworkConfiguration(_))
        ));
    }

    #[tokio::test]
    async fn test_new_with_commitment_invalid_url() {
        let _env_guard = setup_test_env();
        let configs = vec![RpcConfig {
            url: "invalid-url".to_string(),
            weight: 1,
        }];
        let timeout = 30;
        let commitment = CommitmentConfig::finalized();

        let result = SolanaProvider::new_with_commitment(configs, timeout, commitment);

        assert!(result.is_err());
        assert!(matches!(
            result,
            Err(ProviderError::NetworkConfiguration(_))
        ));
    }

    #[tokio::test]
    async fn test_new_with_multiple_configs() {
        let _env_guard = setup_test_env();
        let configs = vec![
            create_test_rpc_config(),
            RpcConfig {
                url: "https://api.mainnet-beta.solana.com".to_string(),
                weight: 1,
            },
        ];
        let timeout = 30;

        let result = SolanaProvider::new(configs, timeout);

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_provider_creation() {
        let _env_guard = setup_test_env();
        let configs = vec![create_test_rpc_config()];
        let timeout = 30;
        let provider = SolanaProvider::new(configs, timeout);
        assert!(provider.is_ok());
    }

    #[tokio::test]
    async fn test_get_balance() {
        let _env_guard = setup_test_env();
        let configs = vec![create_test_rpc_config()];
        let timeout = 30;
        let provider = SolanaProvider::new(configs, timeout).unwrap();
        let keypair = Keypair::new();
        let balance = provider.get_balance(&keypair.pubkey().to_string()).await;
        assert!(balance.is_ok());
        assert_eq!(balance.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_get_balance_funded_account() {
        let _env_guard = setup_test_env();
        let configs = vec![create_test_rpc_config()];
        let timeout = 30;
        let provider = SolanaProvider::new(configs, timeout).unwrap();
        let keypair = get_funded_keypair();
        let balance = provider.get_balance(&keypair.pubkey().to_string()).await;
        assert!(balance.is_ok());
        assert_eq!(balance.unwrap(), 1000000000);
    }

    #[tokio::test]
    async fn test_get_latest_blockhash() {
        let _env_guard = setup_test_env();
        let configs = vec![create_test_rpc_config()];
        let timeout = 30;
        let provider = SolanaProvider::new(configs, timeout).unwrap();
        let blockhash = provider.get_latest_blockhash().await;
        assert!(blockhash.is_ok());
    }

    #[tokio::test]
    async fn test_simulate_transaction() {
        let _env_guard = setup_test_env();
        let configs = vec![create_test_rpc_config()];
        let timeout = 30;
        let provider = SolanaProvider::new(configs, timeout).expect("Failed to create provider");

        let fee_payer = get_funded_keypair();

        // Construct a message with no instructions (a no-op transaction).
        // Note: An empty instruction set is acceptable for simulation purposes.
        let message = Message::new(&[], Some(&fee_payer.pubkey()));

        let mut tx = Transaction::new_unsigned(message);

        let recent_blockhash = get_recent_blockhash(&provider).await;
        tx.try_sign(&[&fee_payer], recent_blockhash)
            .expect("Failed to sign transaction");

        let simulation_result = provider.simulate_transaction(&tx).await;

        assert!(
            simulation_result.is_ok(),
            "Simulation failed: {:?}",
            simulation_result
        );

        let result = simulation_result.unwrap();
        // The simulation result may contain logs or an error field.
        // For a no-op transaction, we expect no errors and possibly empty logs.
        assert!(
            result.err.is_none(),
            "Simulation encountered an error: {:?}",
            result.err
        );
    }

    #[tokio::test]
    async fn test_get_token_metadata_from_pubkey() {
        let _env_guard = setup_test_env();
        let configs = vec![RpcConfig {
            url: "https://api.mainnet-beta.solana.com".to_string(),
            weight: 1,
        }];
        let timeout = 30;
        let provider = SolanaProvider::new(configs, timeout).unwrap();
        let usdc_token_metadata = provider
            .get_token_metadata_from_pubkey("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v")
            .await
            .unwrap();

        assert_eq!(
            usdc_token_metadata,
            TokenMetadata {
                decimals: 6,
                symbol: "USDC".to_string(),
                mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
            }
        );

        let usdt_token_metadata = provider
            .get_token_metadata_from_pubkey("Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB")
            .await
            .unwrap();

        assert_eq!(
            usdt_token_metadata,
            TokenMetadata {
                decimals: 6,
                symbol: "USDT".to_string(),
                mint: "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn test_get_client_success() {
        let _env_guard = setup_test_env();
        let configs = vec![create_test_rpc_config()];
        let timeout = 30;
        let provider = SolanaProvider::new(configs, timeout).unwrap();

        let client = provider.get_client();
        assert!(client.is_ok());

        let client = client.unwrap();
        let health_result = client.get_health().await;
        assert!(health_result.is_ok());
    }

    #[tokio::test]
    async fn test_get_client_with_custom_commitment() {
        let _env_guard = setup_test_env();
        let configs = vec![create_test_rpc_config()];
        let timeout = 30;
        let commitment = CommitmentConfig::finalized();

        let provider = SolanaProvider::new_with_commitment(configs, timeout, commitment).unwrap();

        let client = provider.get_client();
        assert!(client.is_ok());

        let client = client.unwrap();
        let health_result = client.get_health().await;
        assert!(health_result.is_ok());
    }

    #[tokio::test]
    async fn test_get_client_with_multiple_rpcs() {
        let _env_guard = setup_test_env();
        let configs = vec![
            create_test_rpc_config(),
            RpcConfig {
                url: "https://api.mainnet-beta.solana.com".to_string(),
                weight: 2,
            },
        ];
        let timeout = 30;

        let provider = SolanaProvider::new(configs, timeout).unwrap();

        let client_result = provider.get_client();
        assert!(client_result.is_ok());

        // Call multiple times to exercise the selection logic
        for _ in 0..5 {
            let client = provider.get_client();
            assert!(client.is_ok());
        }
    }

    #[test]
    fn test_initialize_provider_valid_url() {
        let _env_guard = setup_test_env();

        let configs = vec![RpcConfig {
            url: "https://api.devnet.solana.com".to_string(),
            weight: 1,
        }];
        let provider = SolanaProvider::new(configs, 10).unwrap();
        let result = provider.initialize_provider("https://api.devnet.solana.com");
        assert!(result.is_ok());
        let arc_client = result.unwrap();
        // Arc pointer should not be null and should point to RpcClient
        let _client: &RpcClient = Arc::as_ref(&arc_client);
    }

    #[test]
    fn test_initialize_provider_invalid_url() {
        let _env_guard = setup_test_env();

        let configs = vec![RpcConfig {
            url: "https://api.devnet.solana.com".to_string(),
            weight: 1,
        }];
        let provider = SolanaProvider::new(configs, 10).unwrap();
        let result = provider.initialize_provider("not-a-valid-url");
        assert!(result.is_err());
        match result {
            Err(SolanaProviderError::NetworkConfiguration(msg)) => {
                assert!(msg.contains("Invalid URL format"))
            }
            _ => panic!("Expected NetworkConfiguration error"),
        }
    }

    #[test]
    fn test_from_string_for_solana_provider_error() {
        let msg = "some rpc error".to_string();
        let err: SolanaProviderError = msg.clone().into();
        match err {
            SolanaProviderError::RpcError(inner) => assert_eq!(inner, msg),
            _ => panic!("Expected RpcError variant"),
        }
    }

    #[test]
    fn test_is_retriable_error_true() {
        for msg in RETRIABLE_ERROR_SUBSTRINGS {
            assert!(is_retriable_error(msg), "Should be retriable: {}", msg);
        }
    }

    #[test]
    fn test_is_retriable_error_false() {
        let non_retriable_cases = [
            "account not found",
            "invalid signature",
            "insufficient funds",
            "unknown error",
        ];
        for msg in non_retriable_cases {
            assert!(!is_retriable_error(msg), "Should NOT be retriable: {}", msg);
        }
    }

    #[tokio::test]
    async fn test_get_minimum_balance_for_rent_exemption() {
        let _env_guard = super::tests::setup_test_env();
        let configs = vec![super::tests::create_test_rpc_config()];
        let timeout = 30;
        let provider = SolanaProvider::new(configs, timeout).unwrap();

        // 0 bytes is always valid, should return a value >= 0
        let result = provider.get_minimum_balance_for_rent_exemption(0).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_is_blockhash_valid_for_recent_blockhash() {
        let _env_guard = super::tests::setup_test_env();
        let configs = vec![super::tests::create_test_rpc_config()];
        let timeout = 30;
        let provider = SolanaProvider::new(configs, timeout).unwrap();

        // Get a recent blockhash (should be valid)
        let blockhash = provider.get_latest_blockhash().await.unwrap();
        let is_valid = provider
            .is_blockhash_valid(&blockhash, CommitmentConfig::confirmed())
            .await;
        assert!(is_valid.is_ok());
    }

    #[tokio::test]
    async fn test_is_blockhash_valid_for_invalid_blockhash() {
        let _env_guard = super::tests::setup_test_env();
        let configs = vec![super::tests::create_test_rpc_config()];
        let timeout = 30;
        let provider = SolanaProvider::new(configs, timeout).unwrap();

        let invalid_blockhash = solana_sdk::hash::Hash::new_from_array([0u8; 32]);
        let is_valid = provider
            .is_blockhash_valid(&invalid_blockhash, CommitmentConfig::confirmed())
            .await;
        assert!(is_valid.is_ok());
    }

    #[tokio::test]
    async fn test_get_latest_blockhash_with_commitment() {
        let _env_guard = super::tests::setup_test_env();
        let configs = vec![super::tests::create_test_rpc_config()];
        let timeout = 30;
        let provider = SolanaProvider::new(configs, timeout).unwrap();

        let commitment = CommitmentConfig::confirmed();
        let result = provider
            .get_latest_blockhash_with_commitment(commitment)
            .await;
        assert!(result.is_ok());
        let (blockhash, last_valid_block_height) = result.unwrap();
        // Blockhash should not be all zeros and block height should be > 0
        assert_ne!(blockhash, solana_sdk::hash::Hash::new_from_array([0u8; 32]));
        assert!(last_valid_block_height > 0);
    }
}
