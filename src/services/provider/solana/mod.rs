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
use async_trait::async_trait;
use eyre::Result;
#[cfg(test)]
use mockall::automock;
use mpl_token_metadata::accounts::Metadata;
use reqwest::Url;
use serde::Serialize;
use solana_client::{
    client_error::{ClientError, ClientErrorKind},
    nonblocking::rpc_client::RpcClient,
    rpc_request::RpcRequest,
    rpc_response::{RpcPrioritizationFee, RpcSimulateTransactionResult},
};
use solana_commitment_config::CommitmentConfig;
use solana_sdk::{
    account::Account,
    hash::Hash,
    message::Message,
    program_pack::Pack,
    pubkey::Pubkey,
    signature::Signature,
    transaction::{Transaction, VersionedTransaction},
};
use spl_token_interface::state::Mint;
use std::{str::FromStr, sync::Arc, time::Duration};
use thiserror::Error;

use crate::{
    models::{RpcConfig, SolanaTransactionStatus},
    services::provider::{retry_rpc_call, should_mark_provider_failed_by_status_code},
};

use super::ProviderError;
use super::{
    rpc_selector::{RpcSelector, RpcSelectorError},
    RetryConfig,
};

/// Utility function to match error patterns by normalizing both strings.
/// Removes spaces and converts to lowercase for flexible matching.
///
/// This allows matching patterns like "invalid instruction data" against errors
/// containing "invalidinstructiondata", "invalid instruction data", etc.
fn matches_error_pattern(error_msg: &str, pattern: &str) -> bool {
    let normalized_msg = error_msg.to_lowercase().replace(' ', "");
    let normalized_pattern = pattern.to_lowercase().replace(' ', "");
    normalized_msg.contains(&normalized_pattern)
}

/// Errors that can occur when interacting with the Solana provider.
///
/// Use `is_transient()` to determine if an error should be retried.
#[derive(Error, Debug, Serialize)]
pub enum SolanaProviderError {
    /// Network/IO error (transient - connection issues, timeouts)
    #[error("Network error: {0}")]
    NetworkError(String),

    /// RPC protocol error (transient - RPC-level issues like node lag, sync pending)
    #[error("RPC error: {0}")]
    RpcError(String),

    /// HTTP request error with status code (transient/permanent based on status code)
    #[error("Request error (HTTP {status_code}): {error}")]
    RequestError { error: String, status_code: u16 },

    /// Invalid address format (permanent)
    #[error("Invalid address: {0}")]
    InvalidAddress(String),

    /// RPC selector error (transient - can retry with different node)
    #[error("RPC selector error: {0}")]
    SelectorError(RpcSelectorError),

    /// Network configuration error (permanent - missing data, unsupported operations)
    #[error("Network configuration error: {0}")]
    NetworkConfiguration(String),

    /// Insufficient funds for transaction (permanent)
    #[error("Insufficient funds for transaction: {0}")]
    InsufficientFunds(String),

    /// Blockhash not found or expired (transient - can rebuild with fresh blockhash)
    #[error("Blockhash not found or expired: {0}")]
    BlockhashNotFound(String),

    /// Invalid transaction structure or execution (permanent)
    #[error("Invalid transaction: {0}")]
    InvalidTransaction(String),

    /// Transaction already processed (permanent - duplicate)
    #[error("Transaction already processed: {0}")]
    AlreadyProcessed(String),
}

impl SolanaProviderError {
    /// Determines if this error is transient (can retry) or permanent (should fail).
    ///
    /// With comprehensive error code classification in `from_rpc_response_error()`,
    /// errors are properly categorized at the source, so we can simply match on variants.
    ///
    /// **Transient (can retry):**
    /// - `NetworkError`: IO/connection errors, timeouts, network unavailable
    /// - `RpcError`: RPC protocol issues, node lag, sync pending (-32004, -32005, -32014, -32016)
    /// - `BlockhashNotFound`: Can rebuild transaction with fresh blockhash (-32008)
    /// - `SelectorError`: Can retry with different RPC node
    /// - `RequestError`: HTTP errors with retriable status codes (5xx, 408, 425, 429)
    ///
    /// **Permanent (fail immediately):**
    /// - `InsufficientFunds`: Not enough balance for transaction
    /// - `InvalidTransaction`: Malformed transaction, invalid signatures, version mismatch (-32002, -32003, -32013, -32015, -32602)
    /// - `AlreadyProcessed`: Duplicate transaction already on-chain (-32009)
    /// - `InvalidAddress`: Invalid public key format
    /// - `NetworkConfiguration`: Missing data, unsupported operations (-32007, -32010)
    /// - `RequestError`: HTTP errors with non-retriable status codes (4xx except 408, 425, 429)
    pub fn is_transient(&self) -> bool {
        match self {
            // Transient errors - safe to retry
            SolanaProviderError::NetworkError(_) => true,
            SolanaProviderError::RpcError(_) => true,
            SolanaProviderError::BlockhashNotFound(_) => true,
            SolanaProviderError::SelectorError(_) => true,

            // RequestError - check status code to determine if retriable
            SolanaProviderError::RequestError { status_code, .. } => match *status_code {
                // Non-retriable 5xx: persistent server-side issues
                501 | 505 => false, // Not Implemented, HTTP Version Not Supported

                // Retriable 5xx: temporary server-side issues
                500 | 502..=504 | 506..=599 => true,

                // Retriable 4xx: timeout or rate-limit related
                408 | 425 | 429 => true,

                // Non-retriable 4xx: client errors
                400..=499 => false,

                // Other status codes: not retriable
                _ => false,
            },

            // Permanent errors - fail immediately
            SolanaProviderError::InsufficientFunds(_) => false,
            SolanaProviderError::InvalidTransaction(_) => false,
            SolanaProviderError::AlreadyProcessed(_) => false,
            SolanaProviderError::InvalidAddress(_) => false,
            SolanaProviderError::NetworkConfiguration(_) => false,
        }
    }

    /// Classifies a Solana RPC client error into the appropriate error variant.
    ///
    /// Uses structured error types from the Solana SDK for precise classification,
    /// including JSON-RPC error codes for enhanced accuracy.
    pub fn from_rpc_error(error: ClientError) -> Self {
        match error.kind() {
            // Network/IO errors - connection issues, timeouts (transient)
            ClientErrorKind::Io(_) => SolanaProviderError::NetworkError(error.to_string()),

            // Reqwest errors - extract status code if available
            ClientErrorKind::Reqwest(reqwest_err) => {
                if let Some(status) = reqwest_err.status() {
                    SolanaProviderError::RequestError {
                        error: error.to_string(),
                        status_code: status.as_u16(),
                    }
                } else {
                    // No status code available (e.g., connection error, timeout)
                    SolanaProviderError::NetworkError(error.to_string())
                }
            }

            // RPC errors - classify based on error code and message
            ClientErrorKind::RpcError(rpc_err) => {
                let rpc_err_str = format!("{rpc_err}");
                Self::from_rpc_response_error(&rpc_err_str, &error)
            }

            // Transaction errors - classify based on specific error type
            ClientErrorKind::TransactionError(tx_error) => {
                Self::from_transaction_error(tx_error, &error)
            }

            // Custom errors from Solana client - reuse pattern matching logic
            ClientErrorKind::Custom(msg) => {
                // Delegate to from_rpc_response_error for consistent classification
                Self::from_rpc_response_error(msg, &error)
            }

            // All other error types
            _ => SolanaProviderError::RpcError(error.to_string()),
        }
    }

    /// Classifies RPC response errors using error codes and messages.
    ///
    /// Solana JSON-RPC 2.0 error codes (see https://www.quicknode.com/docs/solana/error-references):
    ///
    /// **Transient errors (can retry):**
    /// - `-32004`: Block not available for slot - temporary, retry recommended
    /// - `-32005`: Node is unhealthy/behind - temporary node lag
    /// - `-32008`: Blockhash not found - can rebuild transaction with fresh blockhash
    /// - `-32014`: Block status not yet available - pending sync, retry later
    /// - `-32016`: Minimum context slot not reached - future slot, retry later
    ///
    /// **Permanent errors (fail immediately):**
    /// - `-32002`: Transaction simulation failed - check message for specific cause
    /// - `-32003`: Signature verification failure - invalid signatures
    /// - `-32007`: Slot skipped/missing (snapshot jump) - data unavailable
    /// - `-32009`: Already processed - duplicate transaction
    /// - `-32010`: Key excluded from secondary indexes - RPC method unavailable
    /// - `-32013`: Transaction signature length mismatch - malformed transaction
    /// - `-32015`: Transaction version not supported - client version mismatch
    /// - `-32602`: Invalid params - malformed request parameters
    fn from_rpc_response_error(rpc_err: &str, full_error: &ClientError) -> Self {
        let error_str = rpc_err;

        // Check for specific error codes in the error string
        if error_str.contains("-32002") {
            // Transaction simulation failed - check message for specific issues
            if matches_error_pattern(error_str, "blockhash not found") {
                SolanaProviderError::BlockhashNotFound(full_error.to_string())
            } else if matches_error_pattern(error_str, "insufficient funds") {
                SolanaProviderError::InsufficientFunds(full_error.to_string())
            } else {
                // Most simulation failures are permanent (invalid instruction data, etc.)
                SolanaProviderError::InvalidTransaction(full_error.to_string())
            }
        } else if error_str.contains("-32003") {
            // Signature verification failure - permanent
            SolanaProviderError::InvalidTransaction(full_error.to_string())
        } else if error_str.contains("-32004") {
            // Block not available - transient, retry recommended
            SolanaProviderError::RpcError(full_error.to_string())
        } else if error_str.contains("-32005") {
            // Node is behind - transient
            SolanaProviderError::RpcError(full_error.to_string())
        } else if error_str.contains("-32007") {
            // Slot skipped/missing due to snapshot jump - permanent
            SolanaProviderError::NetworkConfiguration(full_error.to_string())
        } else if error_str.contains("-32008") {
            // Blockhash not found - transient (can rebuild transaction)
            SolanaProviderError::BlockhashNotFound(full_error.to_string())
        } else if error_str.contains("-32009") {
            // Already processed - permanent
            SolanaProviderError::AlreadyProcessed(full_error.to_string())
        } else if error_str.contains("-32010") {
            // Key excluded from secondary indexes - permanent
            SolanaProviderError::NetworkConfiguration(full_error.to_string())
        } else if error_str.contains("-32013") {
            // Transaction signature length mismatch - permanent
            SolanaProviderError::InvalidTransaction(full_error.to_string())
        } else if error_str.contains("-32014") {
            // Block status not yet available - transient, retry later
            SolanaProviderError::RpcError(full_error.to_string())
        } else if error_str.contains("-32015") {
            // Transaction version not supported - permanent
            SolanaProviderError::InvalidTransaction(full_error.to_string())
        } else if error_str.contains("-32016") {
            // Minimum context slot not reached - transient, retry later
            SolanaProviderError::RpcError(full_error.to_string())
        } else if error_str.contains("-32602") {
            // Invalid params - permanent
            SolanaProviderError::InvalidTransaction(full_error.to_string())
        } else {
            // For other codes, fall back to string matching
            if matches_error_pattern(error_str, "insufficient funds") {
                SolanaProviderError::InsufficientFunds(full_error.to_string())
            } else if matches_error_pattern(error_str, "blockhash not found") {
                SolanaProviderError::BlockhashNotFound(full_error.to_string())
            } else if matches_error_pattern(error_str, "already processed") {
                SolanaProviderError::AlreadyProcessed(full_error.to_string())
            } else {
                // Default to transient RPC error for unknown codes
                SolanaProviderError::RpcError(full_error.to_string())
            }
        }
    }

    /// Classifies a Solana TransactionError into the appropriate error variant.
    fn from_transaction_error(
        tx_error: &solana_sdk::transaction::TransactionError,
        full_error: &ClientError,
    ) -> Self {
        use solana_sdk::transaction::TransactionError as TxErr;

        match tx_error {
            // Insufficient funds - permanent
            TxErr::InsufficientFundsForFee | TxErr::InsufficientFundsForRent { .. } => {
                SolanaProviderError::InsufficientFunds(full_error.to_string())
            }

            // Blockhash not found - transient (can rebuild transaction with fresh blockhash)
            TxErr::BlockhashNotFound => {
                SolanaProviderError::BlockhashNotFound(full_error.to_string())
            }

            // Already processed - permanent
            TxErr::AlreadyProcessed => {
                SolanaProviderError::AlreadyProcessed(full_error.to_string())
            }

            // Invalid transaction structure/signatures - permanent
            TxErr::SignatureFailure
            | TxErr::MissingSignatureForFee
            | TxErr::InvalidAccountForFee
            | TxErr::AccountNotFound
            | TxErr::InvalidAccountIndex
            | TxErr::InvalidProgramForExecution
            | TxErr::ProgramAccountNotFound
            | TxErr::InstructionError(_, _)
            | TxErr::CallChainTooDeep
            | TxErr::InvalidWritableAccount
            | TxErr::InvalidRentPayingAccount
            | TxErr::WouldExceedMaxBlockCostLimit
            | TxErr::WouldExceedMaxAccountCostLimit
            | TxErr::WouldExceedMaxVoteCostLimit
            | TxErr::WouldExceedAccountDataBlockLimit
            | TxErr::TooManyAccountLocks
            | TxErr::AddressLookupTableNotFound
            | TxErr::InvalidAddressLookupTableOwner
            | TxErr::InvalidAddressLookupTableData
            | TxErr::InvalidAddressLookupTableIndex
            | TxErr::MaxLoadedAccountsDataSizeExceeded
            | TxErr::InvalidLoadedAccountsDataSizeLimit
            | TxErr::ResanitizationNeeded
            | TxErr::ProgramExecutionTemporarilyRestricted { .. }
            | TxErr::AccountBorrowOutstanding => {
                SolanaProviderError::InvalidTransaction(full_error.to_string())
            }

            // Transient errors that might succeed on retry
            TxErr::AccountInUse | TxErr::AccountLoadedTwice | TxErr::ClusterMaintenance => {
                SolanaProviderError::RpcError(full_error.to_string())
            }

            // Treat unknown errors as generic RPC errors (transient by default)
            _ => SolanaProviderError::RpcError(full_error.to_string()),
        }
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

    /// get transaction status
    async fn get_transaction_status(
        &self,
        signature: &Signature,
    ) -> Result<SolanaTransactionStatus, SolanaProviderError>;

    /// Send a raw JSON-RPC request to the Solana node
    async fn raw_request_dyn(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, SolanaProviderError>;
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

/// Determines if a Solana provider error should mark the provider as failed.
///
/// This function identifies errors that indicate the RPC provider itself is having issues
/// and should be marked as failed to trigger failover to another provider.
///
/// Uses the shared `should_mark_provider_failed_by_status_code` function for HTTP status code logic.
fn should_mark_solana_provider_failed(error: &SolanaProviderError) -> bool {
    match error {
        SolanaProviderError::RequestError { status_code, .. } => {
            should_mark_provider_failed_by_status_code(*status_code)
        }
        _ => false,
    }
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
            .map_err(|e| ProviderError::NetworkConfiguration(format!("Invalid URL: {e}")))?;

        // Now create the selector with validated configs
        let selector = RpcSelector::new(configs).map_err(|e| {
            ProviderError::NetworkConfiguration(format!("Failed to create RPC selector: {e}"))
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
            SolanaProviderError::NetworkConfiguration(format!("Invalid URL format: {e}"))
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
        let is_retriable = |e: &SolanaProviderError| e.is_transient();

        tracing::debug!(
            "Starting RPC operation '{}' with timeout: {}s",
            operation_name,
            self.timeout_seconds.as_secs()
        );

        retry_rpc_call(
            &self.selector,
            operation_name,
            is_retriable,
            should_mark_solana_provider_failed,
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
                .map_err(SolanaProviderError::from_rpc_error)
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
                .map_err(SolanaProviderError::from_rpc_error)
        })
        .await
    }

    /// Gets the latest blockhash.
    async fn get_latest_blockhash(&self) -> Result<Hash, SolanaProviderError> {
        self.retry_rpc_call("get_latest_blockhash", |client| async move {
            client
                .get_latest_blockhash()
                .await
                .map_err(SolanaProviderError::from_rpc_error)
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
                    .map_err(SolanaProviderError::from_rpc_error)
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
                .map_err(SolanaProviderError::from_rpc_error)
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
                .map_err(SolanaProviderError::from_rpc_error)
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
                .map_err(SolanaProviderError::from_rpc_error)
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
                    .map_err(SolanaProviderError::from_rpc_error)
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
                .map_err(SolanaProviderError::from_rpc_error)
                .map(|response| response.value)
        })
        .await
    }

    /// Retrieves account data for the given account string.
    async fn get_account_from_str(&self, account: &str) -> Result<Account, SolanaProviderError> {
        let address = Pubkey::from_str(account).map_err(|e| {
            SolanaProviderError::InvalidAddress(format!("Invalid pubkey {account}: {e}"))
        })?;
        self.retry_rpc_call("get_account", |client| async move {
            client
                .get_account(&address)
                .await
                .map_err(SolanaProviderError::from_rpc_error)
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
                .map_err(SolanaProviderError::from_rpc_error)
        })
        .await
    }

    /// Retrieves token metadata from a provided mint address.
    async fn get_token_metadata_from_pubkey(
        &self,
        pubkey: &str,
    ) -> Result<TokenMetadata, SolanaProviderError> {
        // Parse and validate pubkey once
        let mint_pubkey = Pubkey::from_str(pubkey).map_err(|e| {
            SolanaProviderError::InvalidAddress(format!("Invalid pubkey {pubkey}: {e}"))
        })?;

        // Retrieve account using already-parsed pubkey (avoids re-parsing)
        let account = self.get_account_from_pubkey(&mint_pubkey).await?;

        // Unpack the mint info from the account's data
        let decimals = Mint::unpack(&account.data)
            .map_err(|e| {
                SolanaProviderError::InvalidTransaction(format!(
                    "Failed to unpack mint info for {pubkey}: {e}"
                ))
            })?
            .decimals;

        // Derive the PDA for the token metadata
        // Convert bytes directly between Pubkey types (no string conversion needed)
        let mint_pubkey_program =
            solana_program::pubkey::Pubkey::new_from_array(mint_pubkey.to_bytes());
        let metadata_pda_program = Metadata::find_pda(&mint_pubkey_program).0;

        // Convert bytes directly (no string conversion)
        let metadata_pda = Pubkey::new_from_array(metadata_pda_program.to_bytes());

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
                .map_err(SolanaProviderError::from_rpc_error)
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
                .map_err(SolanaProviderError::from_rpc_error)
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

    async fn get_transaction_status(
        &self,
        signature: &Signature,
    ) -> Result<SolanaTransactionStatus, SolanaProviderError> {
        let result = self
            .retry_rpc_call("get_transaction_status", |client| async move {
                client
                    .get_signature_statuses_with_history(&[*signature])
                    .await
                    .map_err(SolanaProviderError::from_rpc_error)
            })
            .await?;

        let status = result.value.first();

        match status {
            Some(Some(v)) => {
                if v.err.is_some() {
                    Ok(SolanaTransactionStatus::Failed)
                } else if v.satisfies_commitment(CommitmentConfig::finalized()) {
                    Ok(SolanaTransactionStatus::Finalized)
                } else if v.satisfies_commitment(CommitmentConfig::confirmed()) {
                    Ok(SolanaTransactionStatus::Confirmed)
                } else {
                    Ok(SolanaTransactionStatus::Processed)
                }
            }
            Some(None) => Err(SolanaProviderError::RpcError(
                "Transaction confirmation status not available".to_string(),
            )),
            None => Err(SolanaProviderError::RpcError(
                "Transaction confirmation status not available".to_string(),
            )),
        }
    }

    /// Send a raw JSON-RPC request to the Solana node
    async fn raw_request_dyn(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, SolanaProviderError> {
        let params_owned = params.clone();
        let method_static: &'static str = Box::leak(method.to_string().into_boxed_str());
        self.retry_rpc_call("raw_request_dyn", move |client| {
            let params_for_call = params_owned.clone();
            async move {
                client
                    .send(
                        RpcRequest::Custom {
                            method: method_static,
                        },
                        params_for_call,
                    )
                    .await
                    .map_err(|e| SolanaProviderError::RpcError(e.to_string()))
            }
        })
        .await
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
    fn test_matches_error_pattern() {
        // Test exact matches
        assert!(matches_error_pattern(
            "blockhash not found",
            "blockhash not found"
        ));
        assert!(matches_error_pattern(
            "insufficient funds",
            "insufficient funds"
        ));

        // Test case insensitive matching
        assert!(matches_error_pattern(
            "BLOCKHASH NOT FOUND",
            "blockhash not found"
        ));
        assert!(matches_error_pattern(
            "blockhash not found",
            "BLOCKHASH NOT FOUND"
        ));
        assert!(matches_error_pattern(
            "BlockHash Not Found",
            "blockhash not found"
        ));

        // Test space insensitive matching
        assert!(matches_error_pattern(
            "blockhashnotfound",
            "blockhash not found"
        ));
        assert!(matches_error_pattern(
            "blockhash not found",
            "blockhashnotfound"
        ));
        assert!(matches_error_pattern(
            "insufficientfunds",
            "insufficient funds"
        ));

        // Test mixed case and space insensitive
        assert!(matches_error_pattern(
            "BLOCKHASHNOTFOUND",
            "blockhash not found"
        ));
        assert!(matches_error_pattern(
            "blockhash not found",
            "BLOCKHASHNOTFOUND"
        ));
        assert!(matches_error_pattern(
            "BlockHashNotFound",
            "blockhash not found"
        ));
        assert!(matches_error_pattern(
            "INSUFFICIENTFUNDS",
            "insufficient funds"
        ));

        // Test partial matches within longer strings
        assert!(matches_error_pattern(
            "transaction failed: blockhash not found",
            "blockhash not found"
        ));
        assert!(matches_error_pattern(
            "error: insufficient funds for transaction",
            "insufficient funds"
        ));
        assert!(matches_error_pattern(
            "BLOCKHASHNOTFOUND in simulation",
            "blockhash not found"
        ));

        // Test multiple spaces handling
        assert!(matches_error_pattern(
            "blockhash  not   found",
            "blockhash not found"
        ));
        assert!(matches_error_pattern(
            "insufficient   funds",
            "insufficient funds"
        ));

        // Test no matches
        assert!(!matches_error_pattern(
            "account not found",
            "blockhash not found"
        ));
        assert!(!matches_error_pattern(
            "invalid signature",
            "insufficient funds"
        ));
        assert!(!matches_error_pattern(
            "timeout error",
            "blockhash not found"
        ));

        // Test empty strings
        assert!(matches_error_pattern("", ""));
        assert!(matches_error_pattern("blockhash not found", "")); // Empty pattern matches everything
        assert!(!matches_error_pattern("", "blockhash not found"));

        // Test special characters and numbers
        assert!(matches_error_pattern(
            "error code -32008: blockhash not found",
            "-32008"
        ));
        assert!(matches_error_pattern("slot 123456 skipped", "slot"));
        assert!(matches_error_pattern("RPC_ERROR_503", "rpc_error_503"));
    }

    #[test]
    fn test_solana_provider_error_is_transient() {
        // Test transient errors (should return true)
        assert!(SolanaProviderError::NetworkError("connection timeout".to_string()).is_transient());
        assert!(SolanaProviderError::RpcError("node is behind".to_string()).is_transient());
        assert!(
            SolanaProviderError::BlockhashNotFound("blockhash expired".to_string()).is_transient()
        );
        assert!(
            SolanaProviderError::SelectorError(RpcSelectorError::AllProvidersFailed).is_transient()
        );

        // Test permanent errors (should return false)
        assert!(
            !SolanaProviderError::InsufficientFunds("not enough balance".to_string())
                .is_transient()
        );
        assert!(
            !SolanaProviderError::InvalidTransaction("invalid signature".to_string())
                .is_transient()
        );
        assert!(
            !SolanaProviderError::AlreadyProcessed("duplicate transaction".to_string())
                .is_transient()
        );
        assert!(
            !SolanaProviderError::InvalidAddress("invalid pubkey format".to_string())
                .is_transient()
        );
        assert!(
            !SolanaProviderError::NetworkConfiguration("unsupported operation".to_string())
                .is_transient()
        );
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

    #[test]
    fn test_from_rpc_response_error_transaction_simulation_failed() {
        // Create a simple mock ClientError for testing
        let mock_error = create_mock_client_error();

        // -32002 with "blockhash not found" should be BlockhashNotFound
        let error_str =
            r#"{"code": -32002, "message": "Transaction simulation failed: Blockhash not found"}"#;
        let result = SolanaProviderError::from_rpc_response_error(error_str, &mock_error);
        assert!(matches!(result, SolanaProviderError::BlockhashNotFound(_)));

        // -32002 with "insufficient funds" should be InsufficientFunds
        let error_str =
            r#"{"code": -32002, "message": "Transaction simulation failed: Insufficient funds"}"#;
        let result = SolanaProviderError::from_rpc_response_error(error_str, &mock_error);
        assert!(matches!(result, SolanaProviderError::InsufficientFunds(_)));

        // -32002 with other message should be InvalidTransaction
        let error_str = r#"{"code": -32002, "message": "Transaction simulation failed: Invalid instruction data"}"#;
        let result = SolanaProviderError::from_rpc_response_error(error_str, &mock_error);
        assert!(matches!(result, SolanaProviderError::InvalidTransaction(_)));
    }

    #[test]
    fn test_from_rpc_response_error_signature_verification() {
        let mock_error = create_mock_client_error();

        // -32003 should be InvalidTransaction
        let error_str = r#"{"code": -32003, "message": "Signature verification failure"}"#;
        let result = SolanaProviderError::from_rpc_response_error(error_str, &mock_error);
        assert!(matches!(result, SolanaProviderError::InvalidTransaction(_)));
    }

    #[test]
    fn test_from_rpc_response_error_transient_errors() {
        let mock_error = create_mock_client_error();

        // -32004: Block not available - should be RpcError (transient)
        let error_str = r#"{"code": -32004, "message": "Block not available for slot"}"#;
        let result = SolanaProviderError::from_rpc_response_error(error_str, &mock_error);
        assert!(matches!(result, SolanaProviderError::RpcError(_)));

        // -32005: Node is behind - should be RpcError (transient)
        let error_str = r#"{"code": -32005, "message": "Node is behind"}"#;
        let result = SolanaProviderError::from_rpc_response_error(error_str, &mock_error);
        assert!(matches!(result, SolanaProviderError::RpcError(_)));

        // -32008: Blockhash not found - should be BlockhashNotFound (transient)
        let error_str = r#"{"code": -32008, "message": "Blockhash not found"}"#;
        let result = SolanaProviderError::from_rpc_response_error(error_str, &mock_error);
        assert!(matches!(result, SolanaProviderError::BlockhashNotFound(_)));

        // -32014: Block status not available - should be RpcError (transient)
        let error_str = r#"{"code": -32014, "message": "Block status not yet available"}"#;
        let result = SolanaProviderError::from_rpc_response_error(error_str, &mock_error);
        assert!(matches!(result, SolanaProviderError::RpcError(_)));

        // -32016: Minimum context slot not reached - should be RpcError (transient)
        let error_str = r#"{"code": -32016, "message": "Minimum context slot not reached"}"#;
        let result = SolanaProviderError::from_rpc_response_error(error_str, &mock_error);
        assert!(matches!(result, SolanaProviderError::RpcError(_)));
    }

    #[test]
    fn test_from_rpc_response_error_permanent_errors() {
        let mock_error = create_mock_client_error();

        // -32007: Slot skipped - should be NetworkConfiguration (permanent)
        let error_str = r#"{"code": -32007, "message": "Slot skipped"}"#;
        let result = SolanaProviderError::from_rpc_response_error(error_str, &mock_error);
        assert!(matches!(
            result,
            SolanaProviderError::NetworkConfiguration(_)
        ));

        // -32009: Already processed - should be AlreadyProcessed (permanent)
        let error_str = r#"{"code": -32009, "message": "Already processed"}"#;
        let result = SolanaProviderError::from_rpc_response_error(error_str, &mock_error);
        assert!(matches!(result, SolanaProviderError::AlreadyProcessed(_)));

        // -32010: Key excluded from secondary indexes - should be NetworkConfiguration (permanent)
        let error_str = r#"{"code": -32010, "message": "Key excluded from secondary indexes"}"#;
        let result = SolanaProviderError::from_rpc_response_error(error_str, &mock_error);
        assert!(matches!(
            result,
            SolanaProviderError::NetworkConfiguration(_)
        ));

        // -32013: Transaction signature length mismatch - should be InvalidTransaction (permanent)
        let error_str = r#"{"code": -32013, "message": "Transaction signature length mismatch"}"#;
        let result = SolanaProviderError::from_rpc_response_error(error_str, &mock_error);
        assert!(matches!(result, SolanaProviderError::InvalidTransaction(_)));

        // -32015: Transaction version not supported - should be InvalidTransaction (permanent)
        let error_str = r#"{"code": -32015, "message": "Transaction version not supported"}"#;
        let result = SolanaProviderError::from_rpc_response_error(error_str, &mock_error);
        assert!(matches!(result, SolanaProviderError::InvalidTransaction(_)));

        // -32602: Invalid params - should be InvalidTransaction (permanent)
        let error_str = r#"{"code": -32602, "message": "Invalid params"}"#;
        let result = SolanaProviderError::from_rpc_response_error(error_str, &mock_error);
        assert!(matches!(result, SolanaProviderError::InvalidTransaction(_)));
    }

    #[test]
    fn test_from_rpc_response_error_string_pattern_matching() {
        let mock_error = create_mock_client_error();

        // Test case-insensitive and space-insensitive pattern matching
        let error_str = r#"{"code": -32000, "message": "INSUFFICIENTFUNDS for transaction"}"#;
        let result = SolanaProviderError::from_rpc_response_error(error_str, &mock_error);
        assert!(matches!(result, SolanaProviderError::InsufficientFunds(_)));

        let error_str = r#"{"code": -32000, "message": "BlockhashNotFound"}"#;
        let result = SolanaProviderError::from_rpc_response_error(error_str, &mock_error);
        assert!(matches!(result, SolanaProviderError::BlockhashNotFound(_)));

        let error_str = r#"{"code": -32000, "message": "AlreadyProcessed"}"#;
        let result = SolanaProviderError::from_rpc_response_error(error_str, &mock_error);
        assert!(matches!(result, SolanaProviderError::AlreadyProcessed(_)));
    }

    #[test]
    fn test_from_rpc_response_error_unknown_code() {
        let mock_error = create_mock_client_error();

        // Unknown error code should default to RpcError (transient)
        let error_str = r#"{"code": -99999, "message": "Unknown error"}"#;
        let result = SolanaProviderError::from_rpc_response_error(error_str, &mock_error);
        assert!(matches!(result, SolanaProviderError::RpcError(_)));
    }

    // Helper function to create a mock ClientError for testing
    fn create_mock_client_error() -> ClientError {
        use solana_client::rpc_request::RpcRequest;
        // Create a simple ClientError using available constructors
        ClientError::new_with_request(
            ClientErrorKind::RpcError(solana_client::rpc_request::RpcError::RpcRequestError(
                "test".to_string(),
            )),
            RpcRequest::GetHealth,
        )
    }

    #[test]
    fn test_from_rpc_error_integration() {
        // Test that a typical RPC error string gets classified correctly
        let mock_error = create_mock_client_error();

        // Test the fallback string matching for "insufficient funds"
        let error_str = r#"{"code": -32000, "message": "Account has insufficient funds"}"#;
        let result = SolanaProviderError::from_rpc_response_error(error_str, &mock_error);
        assert!(matches!(result, SolanaProviderError::InsufficientFunds(_)));

        // Test the fallback string matching for "blockhash not found"
        let error_str = r#"{"code": -32000, "message": "Blockhash not found"}"#;
        let result = SolanaProviderError::from_rpc_response_error(error_str, &mock_error);
        assert!(matches!(result, SolanaProviderError::BlockhashNotFound(_)));

        // Test the fallback string matching for "already processed"
        let error_str = r#"{"code": -32000, "message": "Transaction was already processed"}"#;
        let result = SolanaProviderError::from_rpc_response_error(error_str, &mock_error);
        assert!(matches!(result, SolanaProviderError::AlreadyProcessed(_)));
    }

    #[test]
    fn test_request_error_is_transient() {
        // Test retriable 5xx errors
        let error = SolanaProviderError::RequestError {
            error: "Server error".to_string(),
            status_code: 500,
        };
        assert!(error.is_transient());

        let error = SolanaProviderError::RequestError {
            error: "Bad gateway".to_string(),
            status_code: 502,
        };
        assert!(error.is_transient());

        let error = SolanaProviderError::RequestError {
            error: "Service unavailable".to_string(),
            status_code: 503,
        };
        assert!(error.is_transient());

        let error = SolanaProviderError::RequestError {
            error: "Gateway timeout".to_string(),
            status_code: 504,
        };
        assert!(error.is_transient());

        // Test retriable 4xx errors
        let error = SolanaProviderError::RequestError {
            error: "Request timeout".to_string(),
            status_code: 408,
        };
        assert!(error.is_transient());

        let error = SolanaProviderError::RequestError {
            error: "Too early".to_string(),
            status_code: 425,
        };
        assert!(error.is_transient());

        let error = SolanaProviderError::RequestError {
            error: "Too many requests".to_string(),
            status_code: 429,
        };
        assert!(error.is_transient());

        // Test non-retriable 5xx errors
        let error = SolanaProviderError::RequestError {
            error: "Not implemented".to_string(),
            status_code: 501,
        };
        assert!(!error.is_transient());

        let error = SolanaProviderError::RequestError {
            error: "HTTP version not supported".to_string(),
            status_code: 505,
        };
        assert!(!error.is_transient());

        // Test non-retriable 4xx errors
        let error = SolanaProviderError::RequestError {
            error: "Bad request".to_string(),
            status_code: 400,
        };
        assert!(!error.is_transient());

        let error = SolanaProviderError::RequestError {
            error: "Unauthorized".to_string(),
            status_code: 401,
        };
        assert!(!error.is_transient());

        let error = SolanaProviderError::RequestError {
            error: "Forbidden".to_string(),
            status_code: 403,
        };
        assert!(!error.is_transient());

        let error = SolanaProviderError::RequestError {
            error: "Not found".to_string(),
            status_code: 404,
        };
        assert!(!error.is_transient());
    }

    #[test]
    fn test_request_error_display() {
        let error = SolanaProviderError::RequestError {
            error: "Server error".to_string(),
            status_code: 500,
        };
        let error_str = format!("{}", error);
        assert!(error_str.contains("HTTP 500"));
        assert!(error_str.contains("Server error"));
    }
}
