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
use serde::ser::{Serialize, SerializeStruct, Serializer};
use solana_client::{
    nonblocking::rpc_client::RpcClient, rpc_response::RpcSimulateTransactionResult,
};
use solana_sdk::{
    commitment_config::CommitmentConfig, pubkey::Pubkey, signature::Signature,
    transaction::Transaction,
};
use std::{str::FromStr, time::Duration};
use thiserror::Error;

use super::ProviderError;

#[derive(Error, Debug)]
pub enum SolanaProviderError {
    #[error("RPC client error: {0}")]
    RpcError(#[from] solana_client::client_error::ClientError),
    #[error("Invalid address: {0}")]
    InvalidAddress(String),
}

// Implement the Serialize trait for SolanaProviderError to allow serialization
// due to missing Serialize implementation in the solana_client::client_error::ClientError.
impl Serialize for SolanaProviderError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("ProviderError", 2)?;
        match self {
            SolanaProviderError::RpcError(err) => {
                state.serialize_field("type", "RpcError")?;
                state.serialize_field("message", &format!("{}", err))?;
            }
            SolanaProviderError::InvalidAddress(address) => {
                state.serialize_field("type", "InvalidAddress")?;
                state.serialize_field("message", address)?;
            }
        }
        state.end()
    }
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
}

pub struct SolanaProvider {
    client: RpcClient,
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
        let commitment = commitment.unwrap_or_else(|| CommitmentConfig::processed());
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
            .map_err(SolanaProviderError::RpcError)
    }

    /// Gets the latest blockhash.
    async fn get_latest_blockhash(&self) -> Result<[u8; 32], SolanaProviderError> {
        self.client
            .get_latest_blockhash()
            .await
            .map(|blockhash| blockhash.to_bytes())
            .map_err(SolanaProviderError::RpcError)
    }

    /// Sends a transaction to the network.
    async fn send_transaction(
        &self,
        transaction: &Transaction,
    ) -> Result<Signature, SolanaProviderError> {
        self.client
            .send_transaction(transaction)
            .await
            .map_err(SolanaProviderError::RpcError)
    }

    /// Confirms the given transaction signature.
    async fn confirm_transaction(
        &self,
        signature: &Signature,
    ) -> Result<bool, SolanaProviderError> {
        self.client
            .confirm_transaction(signature)
            .await
            .map_err(SolanaProviderError::RpcError)
            .and_then(|confirmed| if confirmed { Ok(true) } else { Ok(false) })
    }

    /// Retrieves the minimum balance for rent exemption for the given data size.
    async fn get_minimum_balance_for_rent_exemption(
        &self,
        data_size: usize,
    ) -> Result<u64, SolanaProviderError> {
        self.client
            .get_minimum_balance_for_rent_exemption(data_size)
            .await
            .map_err(SolanaProviderError::RpcError)
    }

    /// Simulate transaction.
    async fn simulate_transaction(
        &self,
        transaction: &Transaction,
    ) -> Result<RpcSimulateTransactionResult, SolanaProviderError> {
        self.client
            .simulate_transaction(transaction)
            .await
            .map_err(SolanaProviderError::RpcError)
            .map(|response| response.value)
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
        let funded_keypair = Keypair::from_bytes(&[
            120, 248, 160, 20, 225, 60, 226, 195, 68, 137, 176, 87, 21, 129, 0, 76, 144, 129, 122,
            250, 80, 4, 247, 50, 248, 82, 146, 77, 139, 156, 40, 41, 240, 161, 15, 81, 198, 198,
            86, 167, 90, 148, 131, 13, 184, 222, 251, 71, 229, 212, 169, 2, 72, 202, 150, 184, 176,
            148, 75, 160, 255, 233, 73, 31,
        ])
        .unwrap();

        funded_keypair
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
}
