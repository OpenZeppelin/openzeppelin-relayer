//! Solana Provider Module
//!
//! This module provides an abstraction layer over the Solana RPC client,
//! offering common operations such as retrieving account balance, fetching
//! the latest blockhash, sending transactions, confirming transactions, and
//! querying the minimum balance for rent exemption.
//!
//! The provider uses the nonblocking `RpcClient` for asynchronous operations
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
    account::Account, commitment_config::CommitmentConfig, hash::Hash, message::Message,
    program_pack::Pack, pubkey::Pubkey, signature::Signature, transaction::Transaction,
};
use spl_token::state::Mint;
use std::{str::FromStr, time::Duration};
use thiserror::Error;

use super::ProviderError;

#[derive(Error, Debug, Serialize)]
pub enum SolanaProviderError {
    #[error("RPC client error: {0}")]
    RpcError(String),
    #[error("Invalid address: {0}")]
    InvalidAddress(String),
}

/// A trait that abstracts common Solana provider operations.
#[async_trait]
#[cfg_attr(test, automock)]
#[allow(dead_code)]
pub trait SolanaProviderTrait: Send + Sync {
    /// Retrieves the balance (in lamports) for the given address.
    async fn get_balance(&self, address: &str) -> Result<u64, SolanaProviderError>;

    /// Retrieves the latest blockhash as a 32-byte array.
    async fn get_latest_blockhash(&self) -> Result<[u8; 32], SolanaProviderError>;

    /// Sends a transaction to the Solana network.
    async fn send_transaction(
        &self,
        transaction: &Transaction,
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

pub struct SolanaProvider {
    client: RpcClient,
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
    pub fn new(url: &str) -> Result<Self, ProviderError> {
        let client = RpcClient::new_with_timeout_and_commitment(
            url.to_string(),
            Duration::from_secs(30),
            CommitmentConfig::processed(),
        );
        Ok(Self { client })
    }

    pub fn new_with_timeout_and_commitment(
        url: &str,
        timeout: Option<Duration>,
        commitment: Option<CommitmentConfig>,
    ) -> Result<Self, ProviderError> {
        let timeout = timeout.unwrap_or_else(|| Duration::from_secs(30));
        let commitment = commitment.unwrap_or_else(CommitmentConfig::processed);
        let client =
            RpcClient::new_with_timeout_and_commitment(url.to_string(), timeout, commitment);
        Ok(Self { client })
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

        self.client
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
        self.client
            .is_blockhash_valid(hash, commitment)
            .await
            .map_err(|e| SolanaProviderError::RpcError(e.to_string()))
    }

    /// Gets the latest blockhash.
    async fn get_latest_blockhash(&self) -> Result<[u8; 32], SolanaProviderError> {
        self.client
            .get_latest_blockhash()
            .await
            .map(|blockhash| blockhash.to_bytes())
            .map_err(|e| SolanaProviderError::RpcError(e.to_string()))
    }

    /// Sends a transaction to the network.
    async fn send_transaction(
        &self,
        transaction: &Transaction,
    ) -> Result<Signature, SolanaProviderError> {
        self.client
            .send_transaction(transaction)
            .await
            .map_err(|e| SolanaProviderError::RpcError(e.to_string()))
    }

    /// Confirms the given transaction signature.
    async fn confirm_transaction(
        &self,
        signature: &Signature,
    ) -> Result<bool, SolanaProviderError> {
        self.client
            .confirm_transaction(signature)
            .await
            .map_err(|e| SolanaProviderError::RpcError(e.to_string()))
    }

    /// Retrieves the minimum balance for rent exemption for the given data size.
    async fn get_minimum_balance_for_rent_exemption(
        &self,
        data_size: usize,
    ) -> Result<u64, SolanaProviderError> {
        self.client
            .get_minimum_balance_for_rent_exemption(data_size)
            .await
            .map_err(|e| SolanaProviderError::RpcError(e.to_string()))
    }

    /// Simulate transaction.
    async fn simulate_transaction(
        &self,
        transaction: &Transaction,
    ) -> Result<RpcSimulateTransactionResult, SolanaProviderError> {
        self.client
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
        self.client
            .get_account(&address)
            .await
            .map_err(|e| SolanaProviderError::RpcError(e.to_string()))
    }

    /// Retrieves account data for the given pubkey.
    async fn get_account_from_pubkey(
        &self,
        pubkey: &Pubkey,
    ) -> Result<Account, SolanaProviderError> {
        self.client
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

        // Get the metadata account data from the provider
        let metadata_account = self
            .get_account_from_pubkey(&metadata_pda)
            .await
            .map_err(|e| {
                SolanaProviderError::RpcError(format!(
                    "Failed to fetch metadata account {}: {}",
                    metadata_pda, e
                ))
            })?;

        // Deserialize the metadata from the account data
        let metadata = Metadata::from_bytes(&metadata_account.data).map_err(|e| {
            SolanaProviderError::RpcError(format!("Failed to deserialize metadata: {}", e))
        })?;

        // Remove trailing null bytes (padding) from the symbol
        let normalized_symbol = metadata.symbol.trim_end_matches('\u{0}').to_string();

        Ok(TokenMetadata {
            decimals,
            symbol: normalized_symbol,
            mint: pubkey.to_string(),
        })
    }

    /// Get the fee for a message
    async fn get_fee_for_message(&self, message: &Message) -> Result<u64, SolanaProviderError> {
        self.client
            .get_fee_for_message(message)
            .await
            .map_err(|e| SolanaProviderError::RpcError(e.to_string()))
    }

    async fn get_recent_prioritization_fees(
        &self,
        addresses: &[Pubkey],
    ) -> Result<Vec<RpcPrioritizationFee>, SolanaProviderError> {
        self.client
            .get_recent_prioritization_fees(&addresses)
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
        let blockhash_bytes = provider
            .get_latest_blockhash()
            .await
            .expect("Failed to get blockhash");
        Hash::new_from_array(blockhash_bytes)
    }

    #[tokio::test]
    async fn test_provider_creation() {
        let provider = SolanaProvider::new("https://api.devnet.solana.com");
        assert!(provider.is_ok());
    }

    #[tokio::test]
    async fn test_get_balance() {
        let provider = SolanaProvider::new("https://api.devnet.solana.com").unwrap();
        let keypair = Keypair::new();
        let balance = provider.get_balance(&keypair.pubkey().to_string()).await;
        assert!(balance.is_ok());
        assert_eq!(balance.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_get_balance_funded_account() {
        let provider = SolanaProvider::new("https://api.devnet.solana.com").unwrap();
        let keypair = get_funded_keypair();
        let balance = provider.get_balance(&keypair.pubkey().to_string()).await;
        assert!(balance.is_ok());
        assert_eq!(balance.unwrap(), 1000000000);
    }

    #[tokio::test]
    async fn test_get_latest_blockhash() {
        let provider = SolanaProvider::new("https://api.devnet.solana.com").unwrap();
        let blockhash = provider.get_latest_blockhash().await;
        assert!(blockhash.is_ok());
    }

    #[tokio::test]
    async fn test_simulate_transaction() {
        let provider = SolanaProvider::new("https://api.devnet.solana.com")
            .expect("Failed to create provider");

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
        let provider = SolanaProvider::new("https://api.mainnet-beta.solana.com").unwrap();
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
}
