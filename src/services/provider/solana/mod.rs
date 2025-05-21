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
use std::{str::FromStr, time::Duration};
use thiserror::Error;

use crate::models::RpcConfig;

use super::rpc_selector::{RpcSelector, RpcSelectorError};
use super::ProviderError;

#[derive(Error, Debug, Serialize)]
pub enum SolanaProviderError {
    #[error("RPC client error: {0}")]
    RpcError(String),
    #[error("Invalid address: {0}")]
    InvalidAddress(String),
    #[error("RPC selector error: {0}")]
    SelectorError(RpcSelectorError),
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

        Ok(Self {
            selector,
            timeout_seconds: Duration::from_secs(timeout_seconds),
            commitment,
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

        self.get_client()?
            .get_balance(&pubkey)
            .await
            .map_err(|e| SolanaProviderError::RpcError(e.to_string()))
    }

    /// Check if a blockhash is valid
    async fn is_blockhash_valid(
        &self,
        hash: &Hash,
        commitment: CommitmentConfig,
    ) -> Result<bool, SolanaProviderError> {
        self.get_client()?
            .is_blockhash_valid(hash, commitment)
            .await
            .map_err(|e| SolanaProviderError::RpcError(e.to_string()))
    }

    /// Gets the latest blockhash.
    async fn get_latest_blockhash(&self) -> Result<Hash, SolanaProviderError> {
        self.get_client()?
            .get_latest_blockhash()
            .await
            .map_err(|e| SolanaProviderError::RpcError(e.to_string()))
    }

    async fn get_latest_blockhash_with_commitment(
        &self,
        commitment: CommitmentConfig,
    ) -> Result<(Hash, u64), SolanaProviderError> {
        self.get_client()?
            .get_latest_blockhash_with_commitment(commitment)
            .await
            .map_err(|e| SolanaProviderError::RpcError(e.to_string()))
    }

    /// Sends a transaction to the network.
    async fn send_transaction(
        &self,
        transaction: &Transaction,
    ) -> Result<Signature, SolanaProviderError> {
        self.get_client()?
            .send_transaction(transaction)
            .await
            .map_err(|e| SolanaProviderError::RpcError(e.to_string()))
    }

    /// Sends a transaction to the network.
    async fn send_versioned_transaction(
        &self,
        transaction: &VersionedTransaction,
    ) -> Result<Signature, SolanaProviderError> {
        self.get_client()?
            .send_transaction(transaction)
            .await
            .map_err(|e| SolanaProviderError::RpcError(e.to_string()))
    }

    /// Confirms the given transaction signature.
    async fn confirm_transaction(
        &self,
        signature: &Signature,
    ) -> Result<bool, SolanaProviderError> {
        self.get_client()?
            .confirm_transaction(signature)
            .await
            .map_err(|e| SolanaProviderError::RpcError(e.to_string()))
    }

    /// Retrieves the minimum balance for rent exemption for the given data size.
    async fn get_minimum_balance_for_rent_exemption(
        &self,
        data_size: usize,
    ) -> Result<u64, SolanaProviderError> {
        self.get_client()?
            .get_minimum_balance_for_rent_exemption(data_size)
            .await
            .map_err(|e| SolanaProviderError::RpcError(e.to_string()))
    }

    /// Simulate transaction.
    async fn simulate_transaction(
        &self,
        transaction: &Transaction,
    ) -> Result<RpcSimulateTransactionResult, SolanaProviderError> {
        self.get_client()?
            .simulate_transaction(transaction)
            .await
            .map_err(|e| SolanaProviderError::RpcError(e.to_string()))
            .map(|response| response.value)
    }

    /// Retrieves account data for the given account string.
    async fn get_account_from_str(&self, account: &str) -> Result<Account, SolanaProviderError> {
        let address = Pubkey::from_str(account).map_err(|e| {
            SolanaProviderError::InvalidAddress(format!("Invalid pubkey {}: {}", account, e))
        })?;
        self.get_client()?
            .get_account(&address)
            .await
            .map_err(|e| SolanaProviderError::RpcError(e.to_string()))
    }

    /// Retrieves account data for the given pubkey.
    async fn get_account_from_pubkey(
        &self,
        pubkey: &Pubkey,
    ) -> Result<Account, SolanaProviderError> {
        self.get_client()?
            .get_account(pubkey)
            .await
            .map_err(|e| SolanaProviderError::RpcError(e.to_string()))
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
        self.get_client()?
            .get_fee_for_message(message)
            .await
            .map_err(|e| SolanaProviderError::RpcError(e.to_string()))
    }

    async fn get_recent_prioritization_fees(
        &self,
        addresses: &[Pubkey],
    ) -> Result<Vec<RpcPrioritizationFee>, SolanaProviderError> {
        self.get_client()?
            .get_recent_prioritization_fees(addresses)
            .await
            .map_err(|e| SolanaProviderError::RpcError(e.to_string()))
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
    use solana_sdk::{
        hash::Hash,
        message::Message,
        signer::{keypair::Keypair, Signer},
        transaction::Transaction,
    };

    fn get_funded_keypair() -> Keypair {
        // address HCKHoE2jyk1qfAwpHQghvYH3cEfT8euCygBzF9AV6bhY
        Keypair::from_bytes(&[
            120, 248, 160, 20, 225, 60, 226, 195, 68, 137, 176, 87, 21, 129, 0, 76, 144, 129, 122,
            250, 80, 4, 247, 50, 248, 82, 146, 77, 139, 156, 40, 41, 240, 161, 15, 81, 198, 198,
            86, 167, 90, 148, 131, 13, 184, 222, 251, 71, 229, 212, 169, 2, 72, 202, 150, 184, 176,
            148, 75, 160, 255, 233, 73, 31,
        ])
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
        let configs = vec![create_test_rpc_config()];
        let timeout = 30;
        let provider = SolanaProvider::new(configs, timeout);
        assert!(provider.is_ok());
    }

    #[tokio::test]
    async fn test_get_balance() {
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
        let configs = vec![create_test_rpc_config()];
        let timeout = 30;
        let provider = SolanaProvider::new(configs, timeout).unwrap();
        let blockhash = provider.get_latest_blockhash().await;
        assert!(blockhash.is_ok());
    }

    #[tokio::test]
    async fn test_simulate_transaction() {
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
}
