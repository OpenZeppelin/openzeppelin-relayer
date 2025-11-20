//! Utility functions for Stellar transaction domain logic.
use crate::constants::{
    DEFAULT_CONVERSION_SLIPPAGE_PERCENTAGE, STELLAR_DEFAULT_TRANSACTION_FEE, STELLAR_MAX_OPERATIONS,
};
use crate::domain::relayer::xdr_utils::{extract_operations, xdr_needs_simulation};
use crate::models::{AssetSpec, OperationSpec, RelayerError, RelayerStellarPolicy};
use crate::services::provider::StellarProviderTrait;
use crate::services::stellar_dex::StellarDexServiceTrait;
use base64::{engine::general_purpose, Engine};
use chrono::{DateTime, Utc};
use serde::Serialize;
use soroban_rs::xdr::{
    AccountId, ContractDataEntry, ContractId, Hash, LedgerEntryData, LedgerKey,
    LedgerKeyContractData, Limits, Operation, Preconditions, PublicKey as XdrPublicKey, ReadXdr,
    ScAddress, ScSymbol, ScVal, TimeBounds, TimePoint, TransactionEnvelope, Uint256, VecM,
};
use std::str::FromStr;
use stellar_strkey::ed25519::PublicKey;
use thiserror::Error;
use tracing::{debug, warn};

// ============================================================================
// Error Types
// ============================================================================

/// Errors that can occur during Stellar transaction utility operations.
///
/// This error type is specific to Stellar transaction utilities and provides
/// detailed error information. It can be converted to `RelayerError` using
/// the `From` trait implementation.
#[derive(Error, Debug, Serialize)]
pub enum StellarTransactionUtilsError {
    #[error("Sequence overflow: {0}")]
    SequenceOverflow(String),

    #[error("Failed to parse XDR: {0}")]
    XdrParseFailed(String),

    #[error("Failed to extract operations: {0}")]
    OperationExtractionFailed(String),

    #[error("Failed to check if simulation is needed: {0}")]
    SimulationCheckFailed(String),

    #[error("Failed to simulate transaction: {0}")]
    SimulationFailed(String),

    #[error("Transaction simulation returned no results")]
    SimulationNoResults,

    #[error("Failed to get DEX quote: {0}")]
    DexQuoteFailed(String),

    #[error("Invalid asset identifier format: {0}")]
    InvalidAssetFormat(String),

    #[error("Asset code too long (max {0} characters): {1}")]
    AssetCodeTooLong(usize, String),

    #[error("Too many operations (max {0})")]
    TooManyOperations(usize),

    #[error("Cannot add operations to fee-bump transactions")]
    CannotModifyFeeBump,

    #[error("Cannot set time bounds on fee-bump transactions")]
    CannotSetTimeBoundsOnFeeBump,

    #[error("Invalid transaction format: {0}")]
    InvalidTransactionFormat(String),

    #[error("Invalid account address '{0}': {1}")]
    InvalidAccountAddress(String, String),

    #[error("Invalid contract address '{0}': {1}")]
    InvalidContractAddress(String, String),

    #[error("Failed to create {0} symbol: {1:?}")]
    SymbolCreationFailed(String, String),

    #[error("Failed to create {0} key vector: {1:?}")]
    KeyVectorCreationFailed(String, String),

    #[error("Failed to query contract data (Persistent) for {0}: {1}")]
    ContractDataQueryPersistentFailed(String, String),

    #[error("Failed to query contract data (Temporary) for {0}: {1}")]
    ContractDataQueryTemporaryFailed(String, String),

    #[error("Failed to parse ledger entry XDR for {0}: {1}")]
    LedgerEntryParseFailed(String, String),

    #[error("No entries found for {0}")]
    NoEntriesFound(String),

    #[error("Empty entries for {0}")]
    EmptyEntries(String),

    #[error("Unexpected ledger entry type for {0} (expected ContractData)")]
    UnexpectedLedgerEntryType(String),

    // Token-specific errors
    #[error("Asset code cannot be empty in asset identifier: {0}")]
    EmptyAssetCode(String),

    #[error("Issuer address cannot be empty in asset identifier: {0}")]
    EmptyIssuerAddress(String),

    #[error("Invalid issuer address length (expected {0} characters): {1}")]
    InvalidIssuerLength(usize, String),

    #[error("Invalid issuer address format (must start with '{0}'): {1}")]
    InvalidIssuerPrefix(char, String),

    #[error("Failed to fetch account for balance: {0}")]
    AccountFetchFailed(String),

    #[error("Failed to query trustline for asset {0}: {1}")]
    TrustlineQueryFailed(String, String),

    #[error("No trustline found for asset {0} on account {1}")]
    NoTrustlineFound(String, String),

    #[error("Unsupported trustline entry version")]
    UnsupportedTrustlineVersion,

    #[error("Unexpected ledger entry type for trustline query")]
    UnexpectedTrustlineEntryType,

    #[error("Balance too large (i128 hi={0}, lo={1}) to fit in u64")]
    BalanceTooLarge(i64, u64),

    #[error("Negative balance not allowed: i128 lo={0}")]
    NegativeBalanceI128(u64),

    #[error("Negative balance not allowed: i64={0}")]
    NegativeBalanceI64(i64),

    #[error("Unexpected balance value type in contract data: {0:?}. Expected I128, U64, or I64")]
    UnexpectedBalanceType(String),

    #[error("Unexpected ledger entry type for contract data query")]
    UnexpectedContractDataEntryType,

    #[error("Native asset should be handled before trustline query")]
    NativeAssetInTrustlineQuery,

    #[error("Failed to invoke contract function '{0}': {1}")]
    ContractInvocationFailed(String, String),
}

impl From<StellarTransactionUtilsError> for RelayerError {
    fn from(error: StellarTransactionUtilsError) -> Self {
        match &error {
            StellarTransactionUtilsError::SequenceOverflow(msg)
            | StellarTransactionUtilsError::SimulationCheckFailed(msg)
            | StellarTransactionUtilsError::SimulationFailed(msg)
            | StellarTransactionUtilsError::XdrParseFailed(msg)
            | StellarTransactionUtilsError::OperationExtractionFailed(msg)
            | StellarTransactionUtilsError::DexQuoteFailed(msg) => {
                RelayerError::Internal(msg.clone())
            }
            StellarTransactionUtilsError::SimulationNoResults => RelayerError::Internal(
                "Transaction simulation failed: no results returned".to_string(),
            ),
            StellarTransactionUtilsError::InvalidAssetFormat(msg)
            | StellarTransactionUtilsError::InvalidTransactionFormat(msg) => {
                RelayerError::ValidationError(msg.clone())
            }
            StellarTransactionUtilsError::AssetCodeTooLong(max_len, code) => {
                RelayerError::ValidationError(format!(
                    "Asset code too long (max {} characters): {}",
                    max_len, code
                ))
            }
            StellarTransactionUtilsError::TooManyOperations(max) => {
                RelayerError::ValidationError(format!("Too many operations (max {})", max))
            }
            StellarTransactionUtilsError::CannotModifyFeeBump => RelayerError::ValidationError(
                "Cannot add operations to fee-bump transactions".to_string(),
            ),
            StellarTransactionUtilsError::CannotSetTimeBoundsOnFeeBump => {
                RelayerError::ValidationError(
                    "Cannot set time bounds on fee-bump transactions".to_string(),
                )
            }
            StellarTransactionUtilsError::InvalidAccountAddress(_, msg)
            | StellarTransactionUtilsError::InvalidContractAddress(_, msg)
            | StellarTransactionUtilsError::SymbolCreationFailed(_, msg)
            | StellarTransactionUtilsError::KeyVectorCreationFailed(_, msg)
            | StellarTransactionUtilsError::ContractDataQueryPersistentFailed(_, msg)
            | StellarTransactionUtilsError::ContractDataQueryTemporaryFailed(_, msg)
            | StellarTransactionUtilsError::LedgerEntryParseFailed(_, msg) => {
                RelayerError::Internal(msg.clone())
            }
            StellarTransactionUtilsError::NoEntriesFound(_)
            | StellarTransactionUtilsError::EmptyEntries(_)
            | StellarTransactionUtilsError::UnexpectedLedgerEntryType(_)
            | StellarTransactionUtilsError::EmptyAssetCode(_)
            | StellarTransactionUtilsError::EmptyIssuerAddress(_)
            | StellarTransactionUtilsError::NoTrustlineFound(_, _)
            | StellarTransactionUtilsError::UnsupportedTrustlineVersion
            | StellarTransactionUtilsError::UnexpectedTrustlineEntryType
            | StellarTransactionUtilsError::BalanceTooLarge(_, _)
            | StellarTransactionUtilsError::NegativeBalanceI128(_)
            | StellarTransactionUtilsError::NegativeBalanceI64(_)
            | StellarTransactionUtilsError::UnexpectedBalanceType(_)
            | StellarTransactionUtilsError::UnexpectedContractDataEntryType
            | StellarTransactionUtilsError::NativeAssetInTrustlineQuery => {
                RelayerError::ValidationError(error.to_string())
            }
            StellarTransactionUtilsError::InvalidIssuerLength(expected, actual) => {
                RelayerError::ValidationError(format!(
                    "Invalid issuer address length (expected {} characters): {}",
                    expected, actual
                ))
            }
            StellarTransactionUtilsError::InvalidIssuerPrefix(prefix, addr) => {
                RelayerError::ValidationError(format!(
                    "Invalid issuer address format (must start with '{}'): {}",
                    prefix, addr
                ))
            }
            StellarTransactionUtilsError::AccountFetchFailed(msg)
            | StellarTransactionUtilsError::TrustlineQueryFailed(_, msg)
            | StellarTransactionUtilsError::ContractInvocationFailed(_, msg) => {
                RelayerError::ProviderError(msg.clone())
            }
        }
    }
}

/// Returns true if any operation needs simulation (contract invocation, creation, or wasm upload).
pub fn needs_simulation(operations: &[OperationSpec]) -> bool {
    operations.iter().any(|op| {
        matches!(
            op,
            OperationSpec::InvokeContract { .. }
                | OperationSpec::CreateContract { .. }
                | OperationSpec::UploadWasm { .. }
        )
    })
}

pub fn next_sequence_u64(seq_num: i64) -> Result<u64, RelayerError> {
    let next_i64 = seq_num
        .checked_add(1)
        .ok_or_else(|| RelayerError::ProviderError("sequence overflow".into()))?;
    u64::try_from(next_i64)
        .map_err(|_| RelayerError::ProviderError("sequence overflows u64".into()))
}

pub fn i64_from_u64(value: u64) -> Result<i64, RelayerError> {
    i64::try_from(value).map_err(|_| RelayerError::ProviderError("u64→i64 overflow".into()))
}

/// Detects if an error is due to a bad sequence number.
/// Returns true if the error message contains indicators of sequence number mismatch.
pub fn is_bad_sequence_error(error_msg: &str) -> bool {
    let error_lower = error_msg.to_lowercase();
    error_lower.contains("txbadseq")
}

/// Fetches the current sequence number from the blockchain and calculates the next usable sequence.
/// This is a shared helper that can be used by both stellar_relayer and stellar_transaction.
///
/// # Returns
/// The next usable sequence number (on-chain sequence + 1)
pub async fn fetch_next_sequence_from_chain<P>(
    provider: &P,
    relayer_address: &str,
) -> Result<u64, String>
where
    P: StellarProviderTrait,
{
    debug!(
        "Fetching sequence from chain for address: {}",
        relayer_address
    );

    // Fetch account info from chain
    let account = provider
        .get_account(relayer_address)
        .await
        .map_err(|e| format!("Failed to fetch account from chain: {e}"))?;

    let on_chain_seq = account.seq_num.0; // Extract the i64 value
    let next_usable = next_sequence_u64(on_chain_seq)
        .map_err(|e| format!("Failed to calculate next sequence: {e}"))?;

    debug!(
        "Fetched sequence from chain: on-chain={}, next usable={}",
        on_chain_seq, next_usable
    );
    Ok(next_usable)
}

/// Convert a V0 transaction to V1 format for signing.
/// This is needed because the signature payload for V0 transactions uses V1 format internally.
pub fn convert_v0_to_v1_transaction(
    v0_tx: &soroban_rs::xdr::TransactionV0,
) -> soroban_rs::xdr::Transaction {
    soroban_rs::xdr::Transaction {
        source_account: soroban_rs::xdr::MuxedAccount::Ed25519(
            v0_tx.source_account_ed25519.clone(),
        ),
        fee: v0_tx.fee,
        seq_num: v0_tx.seq_num.clone(),
        cond: match v0_tx.time_bounds.clone() {
            Some(tb) => soroban_rs::xdr::Preconditions::Time(tb),
            None => soroban_rs::xdr::Preconditions::None,
        },
        memo: v0_tx.memo.clone(),
        operations: v0_tx.operations.clone(),
        ext: soroban_rs::xdr::TransactionExt::V0,
    }
}

/// Create a signature payload for the given envelope type
pub fn create_signature_payload(
    envelope: &soroban_rs::xdr::TransactionEnvelope,
    network_id: &soroban_rs::xdr::Hash,
) -> Result<soroban_rs::xdr::TransactionSignaturePayload, RelayerError> {
    let tagged_transaction = match envelope {
        soroban_rs::xdr::TransactionEnvelope::TxV0(e) => {
            // For V0, convert to V1 transaction format for signing
            let v1_tx = convert_v0_to_v1_transaction(&e.tx);
            soroban_rs::xdr::TransactionSignaturePayloadTaggedTransaction::Tx(v1_tx)
        }
        soroban_rs::xdr::TransactionEnvelope::Tx(e) => {
            soroban_rs::xdr::TransactionSignaturePayloadTaggedTransaction::Tx(e.tx.clone())
        }
        soroban_rs::xdr::TransactionEnvelope::TxFeeBump(e) => {
            soroban_rs::xdr::TransactionSignaturePayloadTaggedTransaction::TxFeeBump(e.tx.clone())
        }
    };

    Ok(soroban_rs::xdr::TransactionSignaturePayload {
        network_id: network_id.clone(),
        tagged_transaction,
    })
}

/// Create signature payload for a transaction directly (for operations-based signing)
pub fn create_transaction_signature_payload(
    transaction: &soroban_rs::xdr::Transaction,
    network_id: &soroban_rs::xdr::Hash,
) -> soroban_rs::xdr::TransactionSignaturePayload {
    soroban_rs::xdr::TransactionSignaturePayload {
        network_id: network_id.clone(),
        tagged_transaction: soroban_rs::xdr::TransactionSignaturePayloadTaggedTransaction::Tx(
            transaction.clone(),
        ),
    }
}

// ============================================================================
// Account and Contract Address Utilities
// ============================================================================

/// Parse a Stellar account address string into an AccountId XDR type.
///
/// # Arguments
///
/// * `account_id` - Stellar account address (must be valid PublicKey)
///
/// # Returns
///
/// AccountId XDR type or error if address is invalid
pub fn parse_account_id(account_id: &str) -> Result<AccountId, StellarTransactionUtilsError> {
    let account_pk = PublicKey::from_str(account_id).map_err(|e| {
        StellarTransactionUtilsError::InvalidAccountAddress(account_id.to_string(), e.to_string())
    })?;
    let account_uint256 = Uint256(account_pk.0);
    let account_xdr_pk = XdrPublicKey::PublicKeyTypeEd25519(account_uint256);
    Ok(AccountId(account_xdr_pk))
}

/// Parse a contract address string into a ContractId and extract the hash.
///
/// # Arguments
///
/// * `contract_address` - Contract address in StrKey format
///
/// # Returns
///
/// Contract hash (Hash) or error if address is invalid
pub fn parse_contract_address(
    contract_address: &str,
) -> Result<Hash, StellarTransactionUtilsError> {
    let contract_id = ContractId::from_str(contract_address).map_err(|e| {
        StellarTransactionUtilsError::InvalidContractAddress(
            contract_address.to_string(),
            e.to_string(),
        )
    })?;
    Ok(contract_id.0)
}

// ============================================================================
// Contract Data Utilities
// ============================================================================

/// Create an ScVal key for contract data queries.
///
/// Creates a ScVal::Vec containing a symbol and optional address.
/// Used for SEP-41 token interface keys like "Balance" and "Decimals".
///
/// # Arguments
///
/// * `symbol` - Symbol name (e.g., "Balance", "Decimals")
/// * `address` - Optional ScAddress to include in the key
///
/// # Returns
///
/// ScVal::Vec key or error if creation fails
pub fn create_contract_data_key(
    symbol: &str,
    address: Option<ScAddress>,
) -> Result<ScVal, StellarTransactionUtilsError> {
    if address.is_none() {
        let sym = ScSymbol::try_from(symbol).map_err(|e| {
            StellarTransactionUtilsError::SymbolCreationFailed(
                symbol.to_string(),
                format!("{:?}", e),
            )
        })?;
        return Ok(ScVal::Symbol(sym));
    }

    let mut key_items: Vec<ScVal> =
        vec![ScVal::Symbol(ScSymbol::try_from(symbol).map_err(|e| {
            StellarTransactionUtilsError::SymbolCreationFailed(
                symbol.to_string(),
                format!("{:?}", e),
            )
        })?)];

    if let Some(addr) = address {
        key_items.push(ScVal::Address(addr));
    }

    let key_vec: VecM<ScVal, { u32::MAX }> = VecM::try_from(key_items).map_err(|e| {
        StellarTransactionUtilsError::KeyVectorCreationFailed(
            symbol.to_string(),
            format!("{:?}", e),
        )
    })?;

    Ok(ScVal::Vec(Some(soroban_rs::xdr::ScVec(key_vec))))
}

/// Query contract data with Persistent/Temporary durability fallback.
///
/// Queries contract data storage, trying Persistent durability first,
/// then falling back to Temporary if not found. This handles both
/// production tokens (Persistent) and test tokens (Temporary).
///
/// # Arguments
///
/// * `provider` - Stellar provider for querying ledger entries
/// * `contract_hash` - Contract hash (Hash)
/// * `key` - ScVal key to query
/// * `error_context` - Context string for error messages
///
/// # Returns
///
/// GetLedgerEntriesResponse or error if query fails
pub async fn query_contract_data_with_fallback<P>(
    provider: &P,
    contract_hash: Hash,
    key: ScVal,
    error_context: &str,
) -> Result<soroban_rs::stellar_rpc_client::GetLedgerEntriesResponse, StellarTransactionUtilsError>
where
    P: StellarProviderTrait + Send + Sync,
{
    let contract_address_sc =
        soroban_rs::xdr::ScAddress::Contract(soroban_rs::xdr::ContractId(contract_hash));

    let mut ledger_key = LedgerKey::ContractData(LedgerKeyContractData {
        contract: contract_address_sc.clone(),
        key: key.clone(),
        durability: soroban_rs::xdr::ContractDataDurability::Persistent,
    });

    // Query ledger entry with Persistent durability
    let mut ledger_entries = provider
        .get_ledger_entries(&[ledger_key.clone()])
        .await
        .map_err(|e| {
            StellarTransactionUtilsError::ContractDataQueryPersistentFailed(
                error_context.to_string(),
                e.to_string(),
            )
        })?;

    // If not found, try Temporary durability
    if ledger_entries
        .entries
        .as_ref()
        .map(|e| e.is_empty())
        .unwrap_or(true)
    {
        ledger_key = LedgerKey::ContractData(LedgerKeyContractData {
            contract: contract_address_sc,
            key,
            durability: soroban_rs::xdr::ContractDataDurability::Temporary,
        });
        ledger_entries = provider
            .get_ledger_entries(&[ledger_key])
            .await
            .map_err(|e| {
                StellarTransactionUtilsError::ContractDataQueryTemporaryFailed(
                    error_context.to_string(),
                    e.to_string(),
                )
            })?;
    }

    Ok(ledger_entries)
}

/// Parse a ledger entry from base64 XDR string.
///
/// Handles both LedgerEntry and LedgerEntryChange formats. If the XDR is a
/// LedgerEntryChange, extracts the LedgerEntry from it.
///
/// # Arguments
///
/// * `xdr_string` - Base64-encoded XDR string
/// * `context` - Context string for error messages
///
/// # Returns
///
/// Parsed LedgerEntry or error if parsing fails
pub fn parse_ledger_entry_from_xdr(
    xdr_string: &str,
    context: &str,
) -> Result<LedgerEntryData, StellarTransactionUtilsError> {
    let trimmed_xdr = xdr_string.trim();

    // Ensure valid base64
    if general_purpose::STANDARD.decode(trimmed_xdr).is_err() {
        return Err(StellarTransactionUtilsError::LedgerEntryParseFailed(
            context.to_string(),
            "Invalid base64".to_string(),
        ));
    }

    // Parse as LedgerEntryData (what Soroban RPC actually returns)
    match LedgerEntryData::from_xdr_base64(trimmed_xdr, Limits::none()) {
        Ok(data) => Ok(data),
        Err(e) => Err(StellarTransactionUtilsError::LedgerEntryParseFailed(
            context.to_string(),
            format!("Failed to parse LedgerEntryData: {}", e),
        )),
    }
}

/// Extract ScVal from contract data entry.
///
/// Parses the first entry from GetLedgerEntriesResponse and extracts
/// the ScVal from ContractDataEntry.
///
/// # Arguments
///
/// * `ledger_entries` - Response from get_ledger_entries
/// * `context` - Context string for error messages and logging
///
/// # Returns
///
/// ScVal from contract data or error if extraction fails
pub fn extract_scval_from_contract_data(
    ledger_entries: &soroban_rs::stellar_rpc_client::GetLedgerEntriesResponse,
    context: &str,
) -> Result<ScVal, StellarTransactionUtilsError> {
    let entries = ledger_entries
        .entries
        .as_ref()
        .ok_or_else(|| StellarTransactionUtilsError::NoEntriesFound(context.into()))?;

    if entries.is_empty() {
        return Err(StellarTransactionUtilsError::EmptyEntries(context.into()));
    }

    let entry_xdr = &entries[0].xdr;
    let entry = parse_ledger_entry_from_xdr(entry_xdr, context)?;

    match entry {
        LedgerEntryData::ContractData(ContractDataEntry { val, .. }) => Ok(val.clone()),

        _ => Err(StellarTransactionUtilsError::UnexpectedLedgerEntryType(
            context.into(),
        )),
    }
}

/// Extract a u32 value from an ScVal.
///
/// Handles multiple ScVal types that can represent numeric values.
///
/// # Arguments
///
/// * `val` - ScVal to extract from
/// * `context` - Context string (for logging)
///
/// # Returns
///
/// Some(u32) if extraction succeeds, None otherwise
pub fn extract_u32_from_scval(val: &ScVal, context: &str) -> Option<u32> {
    let result = match val {
        ScVal::U32(n) => Ok(*n),
        ScVal::I32(n) => (*n).try_into().map_err(|_| "Negative I32"),
        ScVal::U64(n) => (*n).try_into().map_err(|_| "U64 overflow"),
        ScVal::I64(n) => (*n).try_into().map_err(|_| "I64 overflow/negative"),
        ScVal::U128(n) => {
            if n.hi == 0 {
                n.lo.try_into().map_err(|_| "U128 lo overflow")
            } else {
                Err("U128 hi set")
            }
        }
        ScVal::I128(n) => {
            if n.hi == 0 {
                n.lo.try_into().map_err(|_| "I128 lo overflow")
            } else {
                Err("I128 hi set/negative")
            }
        }
        _ => Err("Unsupported ScVal type"),
    };

    match result {
        Ok(v) => Some(v),
        Err(msg) => {
            warn!(context = %context, val = ?val, "Failed to extract u32: {}", msg);
            None
        }
    }
}

// ============================================================================
// Gas Abstraction Utility Functions
// ============================================================================

/// Convert raw token amount to UI amount based on decimals
///
/// Uses pure integer arithmetic to avoid floating-point precision errors.
/// This is safer for financial calculations where precision is critical.
pub fn amount_to_ui_amount(amount: u64, decimals: u8) -> String {
    if decimals == 0 {
        return amount.to_string();
    }

    let amount_str = amount.to_string();
    let len = amount_str.len();
    let decimals_usize = decimals as usize;

    let combined = if len > decimals_usize {
        let split_idx = len - decimals_usize;
        let whole = &amount_str[..split_idx];
        let frac = &amount_str[split_idx..];
        format!("{}.{}", whole, frac)
    } else {
        // Need to pad with leading zeros
        let zeros = "0".repeat(decimals_usize - len);
        format!("0.{}{}", zeros, amount_str)
    };

    // Trim trailing zeros
    let mut trimmed = combined.trim_end_matches('0').to_string();
    if trimmed.ends_with('.') {
        trimmed.pop();
    }

    // If we stripped everything (e.g. amount 0), return "0"
    if trimmed.is_empty() {
        "0".to_string()
    } else {
        trimmed
    }
}

/// Count operations in a transaction envelope from XDR base64 string
///
/// Parses the XDR string, extracts operations, and returns the count.
pub fn count_operations_from_xdr(xdr: &str) -> Result<usize, StellarTransactionUtilsError> {
    let envelope = TransactionEnvelope::from_xdr_base64(xdr, Limits::none()).map_err(|e| {
        StellarTransactionUtilsError::XdrParseFailed(format!("Failed to parse XDR: {}", e))
    })?;

    let operations = extract_operations(&envelope).map_err(|e| {
        StellarTransactionUtilsError::OperationExtractionFailed(format!(
            "Failed to extract operations: {}",
            e
        ))
    })?;

    Ok(operations.len())
}

/// Parse transaction and count operations
///
/// Supports both XDR (base64 string) and operations array formats
pub fn parse_transaction_and_count_operations(
    transaction_json: &serde_json::Value,
) -> Result<usize, StellarTransactionUtilsError> {
    // Try to parse as XDR string first
    if let Some(xdr_str) = transaction_json.as_str() {
        let envelope =
            TransactionEnvelope::from_xdr_base64(xdr_str, Limits::none()).map_err(|e| {
                StellarTransactionUtilsError::XdrParseFailed(format!("Failed to parse XDR: {}", e))
            })?;

        let operations = extract_operations(&envelope).map_err(|e| {
            StellarTransactionUtilsError::OperationExtractionFailed(format!(
                "Failed to extract operations: {}",
                e
            ))
        })?;

        return Ok(operations.len());
    }

    // Try to parse as operations array
    if let Some(ops_array) = transaction_json.as_array() {
        return Ok(ops_array.len());
    }

    // Try to parse as object with operations field
    if let Some(obj) = transaction_json.as_object() {
        if let Some(ops) = obj.get("operations") {
            if let Some(ops_array) = ops.as_array() {
                return Ok(ops_array.len());
            }
        }
        if let Some(xdr_str) = obj.get("transaction_xdr").and_then(|v| v.as_str()) {
            let envelope =
                TransactionEnvelope::from_xdr_base64(xdr_str, Limits::none()).map_err(|e| {
                    StellarTransactionUtilsError::XdrParseFailed(format!(
                        "Failed to parse XDR: {}",
                        e
                    ))
                })?;

            let operations = extract_operations(&envelope).map_err(|e| {
                StellarTransactionUtilsError::OperationExtractionFailed(format!(
                    "Failed to extract operations: {}",
                    e
                ))
            })?;

            return Ok(operations.len());
        }
    }

    Err(StellarTransactionUtilsError::InvalidTransactionFormat(
        "Transaction must be either XDR string or operations array".to_string(),
    ))
}

/// Fee quote structure containing fee estimates in both tokens and stroops
#[derive(Debug)]
pub struct FeeQuote {
    pub fee_in_token: u64,
    pub fee_in_token_ui: String,
    pub fee_in_stroops: u64,
    pub conversion_rate: f64,
}

/// Estimate the base transaction fee in XLM (stroops)
///
/// For Stellar, the base fee is typically 100 stroops per operation.
pub fn estimate_base_fee(num_operations: usize) -> u64 {
    (num_operations.max(1) as u64) * STELLAR_DEFAULT_TRANSACTION_FEE as u64
}

/// Estimate transaction fee in XLM (stroops) based on envelope content
///
/// This function intelligently estimates fees by:
/// 1. Checking if the transaction needs simulation (contains Soroban operations)
/// 2. If simulation is needed, performs simulation and uses `min_resource_fee` from the response
/// 3. If simulation is not needed, counts operations and uses `estimate_base_fee`
///
/// # Arguments
/// * `envelope` - The transaction envelope to estimate fees for
/// * `provider` - Stellar provider for simulation (required if simulation is needed)
/// * `operations_override` - Optional override for operations count (useful when operations will be added, e.g., +1 for fee payment)
///
/// # Returns
/// Estimated fee in stroops (XLM)
pub async fn estimate_fee<P>(
    envelope: &TransactionEnvelope,
    provider: &P,
    operations_override: Option<usize>,
) -> Result<u64, StellarTransactionUtilsError>
where
    P: StellarProviderTrait + Send + Sync,
{
    // Check if simulation is needed
    let needs_sim = xdr_needs_simulation(envelope).map_err(|e| {
        StellarTransactionUtilsError::SimulationCheckFailed(format!(
            "Failed to check if simulation is needed: {}",
            e
        ))
    })?;

    if needs_sim {
        debug!("Transaction contains Soroban operations, simulating to get accurate fee");

        // For simulation, we simulate the envelope as-is
        let simulation_result = provider
            .simulate_transaction_envelope(envelope)
            .await
            .map_err(|e| {
                StellarTransactionUtilsError::SimulationFailed(format!(
                    "Failed to simulate transaction: {}",
                    e
                ))
            })?;

        // Check simulation success
        if simulation_result.results.is_empty() {
            return Err(StellarTransactionUtilsError::SimulationNoResults);
        }

        // Use min_resource_fee from simulation (this includes all fees for Soroban operations)
        // If operations_override is provided, we add the base fee for additional operations
        let resource_fee = simulation_result.min_resource_fee as u64;
        let inclusion_fee = STELLAR_DEFAULT_TRANSACTION_FEE as u64;
        let required_fee = inclusion_fee + resource_fee;

        debug!("Simulation returned fee: {} stroops", required_fee);
        Ok(required_fee)
    } else {
        // No simulation needed, count operations and estimate base fee
        let num_operations = if let Some(override_count) = operations_override {
            override_count
        } else {
            let operations = extract_operations(envelope).map_err(|e| {
                StellarTransactionUtilsError::OperationExtractionFailed(format!(
                    "Failed to extract operations: {}",
                    e
                ))
            })?;
            operations.len()
        };

        let fee = estimate_base_fee(num_operations);
        debug!(
            "No simulation needed, estimated fee from {} operations: {} stroops",
            num_operations, fee
        );
        Ok(fee)
    }
}

/// Convert XLM fee to token amount using DEX service
///
/// This function converts an XLM fee (in stroops) to the equivalent amount in the requested token
/// using the DEX service. For native XLM, no conversion is needed.
/// Optionally applies a fee margin percentage to the XLM fee before conversion.
///
/// # Arguments
/// * `dex_service` - DEX service for token conversion quotes
/// * `policy` - Stellar relayer policy for slippage and token decimals
/// * `xlm_fee` - Fee amount in XLM stroops (already estimated)
/// * `fee_token` - Token identifier (e.g., "native" or "USDC:GA5Z...")
/// * `fee_margin_percentage` - Optional fee margin percentage to add as buffer
///
/// # Returns
/// A tuple containing:
/// * `FeeQuote` - Fee quote with amounts in both token and XLM
/// * `u64` - Buffered XLM fee (with margin applied if specified)
pub async fn convert_xlm_fee_to_token<D>(
    dex_service: &D,
    policy: &RelayerStellarPolicy,
    xlm_fee: u64,
    fee_token: &str,
    fee_margin_percentage: Option<f32>,
) -> Result<FeeQuote, StellarTransactionUtilsError>
where
    D: StellarDexServiceTrait + Send + Sync,
{
    // Handle native XLM - no conversion needed
    if fee_token == "native" || fee_token.is_empty() {
        debug!("Converting XLM fee to native XLM: {}", xlm_fee);
        let buffered_fee = if let Some(margin) = fee_margin_percentage {
            (xlm_fee as f64 * (1.0 + margin as f64 / 100.0)) as u64
        } else {
            xlm_fee
        };

        return Ok(FeeQuote {
            fee_in_token: buffered_fee,
            fee_in_token_ui: amount_to_ui_amount(buffered_fee, 7),
            fee_in_stroops: buffered_fee,
            conversion_rate: 1.0,
        });
    }

    debug!("Converting XLM fee to token: {}", fee_token);

    // Apply fee margin if specified
    let buffered_xlm_fee = if let Some(margin) = fee_margin_percentage {
        (xlm_fee as f64 * (1.0 + margin as f64 / 100.0)) as u64
    } else {
        xlm_fee
    };

    // Get slippage from policy or use default
    let slippage = policy
        .get_allowed_token_entry(fee_token)
        .and_then(|token| {
            token
                .swap_config
                .as_ref()
                .and_then(|config| config.slippage_percentage)
        })
        .or(policy.slippage_percentage)
        .unwrap_or(DEFAULT_CONVERSION_SLIPPAGE_PERCENTAGE);

    // Get quote from DEX service
    // Get token decimals from policy or default to 7
    let token_decimals = policy.get_allowed_token_decimals(fee_token);
    let quote = dex_service
        .get_xlm_to_token_quote(fee_token, buffered_xlm_fee, slippage, token_decimals)
        .await
        .map_err(|e| {
            StellarTransactionUtilsError::DexQuoteFailed(format!("Failed to get quote: {}", e))
        })?;

    debug!(
        "Quote from DEX: input={} stroops XLM, output={} stroops token, input_asset={}, output_asset={}",
        quote.in_amount, quote.out_amount, quote.input_asset, quote.output_asset
    );

    // Calculate conversion rate
    let conversion_rate = if buffered_xlm_fee > 0 {
        quote.out_amount as f64 / buffered_xlm_fee as f64
    } else {
        0.0
    };

    let fee_quote = FeeQuote {
        fee_in_token: quote.out_amount,
        fee_in_token_ui: amount_to_ui_amount(quote.out_amount, token_decimals.unwrap_or(7)),
        fee_in_stroops: buffered_xlm_fee,
        conversion_rate,
    };

    debug!(
        "Final fee quote: fee_in_token={} stroops ({} {}), fee_in_stroops={} stroops XLM, conversion_rate={}",
        fee_quote.fee_in_token, fee_quote.fee_in_token_ui, fee_token, fee_quote.fee_in_stroops, fee_quote.conversion_rate
    );

    Ok(fee_quote)
}

/// Parse transaction envelope from JSON value
pub fn parse_transaction_envelope(
    transaction_json: &serde_json::Value,
) -> Result<TransactionEnvelope, StellarTransactionUtilsError> {
    // Try to parse as XDR string first
    if let Some(xdr_str) = transaction_json.as_str() {
        return TransactionEnvelope::from_xdr_base64(xdr_str, Limits::none()).map_err(|e| {
            StellarTransactionUtilsError::XdrParseFailed(format!("Failed to parse XDR: {}", e))
        });
    }

    // Try to parse as object with transaction_xdr field
    if let Some(obj) = transaction_json.as_object() {
        if let Some(xdr_str) = obj.get("transaction_xdr").and_then(|v| v.as_str()) {
            return TransactionEnvelope::from_xdr_base64(xdr_str, Limits::none()).map_err(|e| {
                StellarTransactionUtilsError::XdrParseFailed(format!("Failed to parse XDR: {}", e))
            });
        }
    }

    Err(StellarTransactionUtilsError::InvalidTransactionFormat(
        "Transaction must be XDR string or object with transaction_xdr field".to_string(),
    ))
}

/// Create fee payment operation
pub fn create_fee_payment_operation(
    destination: &str,
    asset_id: &str,
    amount: i64,
) -> Result<OperationSpec, StellarTransactionUtilsError> {
    // Parse asset identifier
    let asset = if asset_id == "native" || asset_id.is_empty() {
        AssetSpec::Native
    } else {
        // Parse "CODE:ISSUER" format
        if let Some(colon_pos) = asset_id.find(':') {
            let code = asset_id[..colon_pos].to_string();
            let issuer = asset_id[colon_pos + 1..].to_string();

            // Determine if it's Credit4 or Credit12 based on code length
            if code.len() <= 4 {
                AssetSpec::Credit4 { code, issuer }
            } else if code.len() <= 12 {
                AssetSpec::Credit12 { code, issuer }
            } else {
                return Err(StellarTransactionUtilsError::AssetCodeTooLong(
                    12, // Stellar max asset code length
                    code,
                ));
            }
        } else {
            return Err(StellarTransactionUtilsError::InvalidAssetFormat(format!(
                "Invalid asset identifier format. Expected 'native' or 'CODE:ISSUER', got: {}",
                asset_id
            )));
        }
    };

    Ok(OperationSpec::Payment {
        destination: destination.to_string(),
        amount,
        asset,
    })
}

/// Add operation to transaction envelope
pub fn add_operation_to_envelope(
    envelope: &mut TransactionEnvelope,
    operation: Operation,
) -> Result<(), StellarTransactionUtilsError> {
    match envelope {
        TransactionEnvelope::TxV0(ref mut e) => {
            // Extract existing operations
            let mut ops: Vec<Operation> = e.tx.operations.iter().cloned().collect();
            ops.push(operation);

            // Convert back to VecM
            let operations: VecM<Operation, 100> = ops.try_into().map_err(|_| {
                StellarTransactionUtilsError::TooManyOperations(STELLAR_MAX_OPERATIONS)
            })?;

            e.tx.operations = operations;

            // Update fee to account for new operation
            e.tx.fee = (e.tx.operations.len() as u32) * 100; // 100 stroops per operation
        }
        TransactionEnvelope::Tx(ref mut e) => {
            // Extract existing operations
            let mut ops: Vec<Operation> = e.tx.operations.iter().cloned().collect();
            ops.push(operation);

            // Convert back to VecM
            let operations: VecM<Operation, 100> = ops.try_into().map_err(|_| {
                StellarTransactionUtilsError::TooManyOperations(STELLAR_MAX_OPERATIONS)
            })?;

            e.tx.operations = operations;

            // Update fee to account for new operation
            e.tx.fee = (e.tx.operations.len() as u32) * 100; // 100 stroops per operation
        }
        TransactionEnvelope::TxFeeBump(_) => {
            return Err(StellarTransactionUtilsError::CannotModifyFeeBump);
        }
    }
    Ok(())
}

/// Extract time bounds from a transaction envelope
///
/// Handles both regular transactions (TxV0, Tx) and fee-bump transactions
/// (extracts from inner transaction).
///
/// # Arguments
/// * `envelope` - The transaction envelope to extract time bounds from
///
/// # Returns
/// Some(TimeBounds) if present, None otherwise
pub fn extract_time_bounds(envelope: &TransactionEnvelope) -> Option<&TimeBounds> {
    match envelope {
        TransactionEnvelope::TxV0(e) => e.tx.time_bounds.as_ref(),
        TransactionEnvelope::Tx(e) => match &e.tx.cond {
            Preconditions::Time(tb) => Some(tb),
            _ => None,
        },
        TransactionEnvelope::TxFeeBump(fb) => {
            // Extract from inner transaction
            match &fb.tx.inner_tx {
                soroban_rs::xdr::FeeBumpTransactionInnerTx::Tx(inner_tx) => {
                    match &inner_tx.tx.cond {
                        Preconditions::Time(tb) => Some(tb),
                        _ => None,
                    }
                }
            }
        }
    }
}

/// Set time bounds on transaction envelope
pub fn set_time_bounds(
    envelope: &mut TransactionEnvelope,
    valid_until: DateTime<Utc>,
) -> Result<(), StellarTransactionUtilsError> {
    let max_time = valid_until.timestamp() as u64;
    let time_bounds = TimeBounds {
        min_time: TimePoint(0),
        max_time: TimePoint(max_time),
    };

    match envelope {
        TransactionEnvelope::TxV0(ref mut e) => {
            e.tx.time_bounds = Some(time_bounds);
        }
        TransactionEnvelope::Tx(ref mut e) => {
            e.tx.cond = Preconditions::Time(time_bounds);
        }
        TransactionEnvelope::TxFeeBump(_) => {
            return Err(StellarTransactionUtilsError::CannotSetTimeBoundsOnFeeBump);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::AssetSpec;
    use crate::models::{AuthSpec, ContractSource, WasmSource};

    const TEST_PK: &str = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";

    fn payment_op(destination: &str) -> OperationSpec {
        OperationSpec::Payment {
            destination: destination.to_string(),
            amount: 100,
            asset: AssetSpec::Native,
        }
    }

    #[test]
    fn returns_false_for_only_payment_ops() {
        let ops = vec![payment_op(TEST_PK)];
        assert!(!needs_simulation(&ops));
    }

    #[test]
    fn returns_true_for_invoke_contract_ops() {
        let ops = vec![OperationSpec::InvokeContract {
            contract_address: "CA7QYNF7SOWQ3GLR2BGMZEHXAVIRZA4KVWLTJJFC7MGXUA74P7UJUWDA"
                .to_string(),
            function_name: "transfer".to_string(),
            args: vec![],
            auth: None,
        }];
        assert!(needs_simulation(&ops));
    }

    #[test]
    fn returns_true_for_upload_wasm_ops() {
        let ops = vec![OperationSpec::UploadWasm {
            wasm: WasmSource::Hex {
                hex: "deadbeef".to_string(),
            },
            auth: None,
        }];
        assert!(needs_simulation(&ops));
    }

    #[test]
    fn returns_true_for_create_contract_ops() {
        let ops = vec![OperationSpec::CreateContract {
            source: ContractSource::Address {
                address: TEST_PK.to_string(),
            },
            wasm_hash: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                .to_string(),
            salt: None,
            constructor_args: None,
            auth: None,
        }];
        assert!(needs_simulation(&ops));
    }

    #[test]
    fn returns_true_for_single_invoke_host_function() {
        let ops = vec![OperationSpec::InvokeContract {
            contract_address: "CA7QYNF7SOWQ3GLR2BGMZEHXAVIRZA4KVWLTJJFC7MGXUA74P7UJUWDA"
                .to_string(),
            function_name: "transfer".to_string(),
            args: vec![],
            auth: Some(AuthSpec::SourceAccount),
        }];
        assert!(needs_simulation(&ops));
    }

    #[test]
    fn returns_false_for_multiple_payment_ops() {
        let ops = vec![payment_op(TEST_PK), payment_op(TEST_PK)];
        assert!(!needs_simulation(&ops));
    }

    mod next_sequence_u64_tests {
        use super::*;

        #[test]
        fn test_increment() {
            assert_eq!(next_sequence_u64(0).unwrap(), 1);

            assert_eq!(next_sequence_u64(12345).unwrap(), 12346);
        }

        #[test]
        fn test_error_path_overflow_i64_max() {
            let result = next_sequence_u64(i64::MAX);
            assert!(result.is_err());
            match result.unwrap_err() {
                RelayerError::ProviderError(msg) => assert_eq!(msg, "sequence overflow"),
                _ => panic!("Unexpected error type"),
            }
        }
    }

    mod i64_from_u64_tests {
        use super::*;

        #[test]
        fn test_happy_path_conversion() {
            assert_eq!(i64_from_u64(0).unwrap(), 0);
            assert_eq!(i64_from_u64(12345).unwrap(), 12345);
            assert_eq!(i64_from_u64(i64::MAX as u64).unwrap(), i64::MAX);
        }

        #[test]
        fn test_error_path_overflow_u64_max() {
            let result = i64_from_u64(u64::MAX);
            assert!(result.is_err());
            match result.unwrap_err() {
                RelayerError::ProviderError(msg) => assert_eq!(msg, "u64→i64 overflow"),
                _ => panic!("Unexpected error type"),
            }
        }

        #[test]
        fn test_edge_case_just_above_i64_max() {
            // Smallest u64 value that will overflow i64
            let value = (i64::MAX as u64) + 1;
            let result = i64_from_u64(value);
            assert!(result.is_err());
            match result.unwrap_err() {
                RelayerError::ProviderError(msg) => assert_eq!(msg, "u64→i64 overflow"),
                _ => panic!("Unexpected error type"),
            }
        }
    }

    mod is_bad_sequence_error_tests {
        use super::*;

        #[test]
        fn test_detects_txbadseq() {
            assert!(is_bad_sequence_error(
                "Failed to send transaction: transaction submission failed: TxBadSeq"
            ));
            assert!(is_bad_sequence_error("Error: TxBadSeq"));
            assert!(is_bad_sequence_error("txbadseq"));
            assert!(is_bad_sequence_error("TXBADSEQ"));
        }

        #[test]
        fn test_returns_false_for_other_errors() {
            assert!(!is_bad_sequence_error("network timeout"));
            assert!(!is_bad_sequence_error("insufficient balance"));
            assert!(!is_bad_sequence_error("tx_insufficient_fee"));
            assert!(!is_bad_sequence_error("bad_auth"));
            assert!(!is_bad_sequence_error(""));
        }
    }

    mod status_check_utils_tests {
        use crate::models::{
            NetworkTransactionData, StellarTransactionData, TransactionError, TransactionInput,
            TransactionRepoModel,
        };
        use crate::utils::mocks::mockutils::create_mock_transaction;
        use chrono::{Duration, Utc};

        /// Helper to create a test transaction with a specific created_at timestamp
        fn create_test_tx_with_age(seconds_ago: i64) -> TransactionRepoModel {
            let created_at = (Utc::now() - Duration::seconds(seconds_ago)).to_rfc3339();
            let mut tx = create_mock_transaction();
            tx.id = format!("test-tx-{}", seconds_ago);
            tx.created_at = created_at;
            tx.network_data = NetworkTransactionData::Stellar(StellarTransactionData {
                source_account: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF"
                    .to_string(),
                fee: None,
                sequence_number: None,
                memo: None,
                valid_until: None,
                network_passphrase: "Test SDF Network ; September 2015".to_string(),
                signatures: vec![],
                hash: Some("test-hash-12345".to_string()),
                simulation_transaction_data: None,
                transaction_input: TransactionInput::Operations(vec![]),
                signed_envelope_xdr: None,
            });
            tx
        }

        mod get_age_since_created_tests {
            use crate::domain::transaction::util::get_age_since_created;

            use super::*;

            #[test]
            fn test_returns_correct_age_for_recent_transaction() {
                let tx = create_test_tx_with_age(30); // 30 seconds ago
                let age = get_age_since_created(&tx).unwrap();

                // Allow for small timing differences (within 1 second)
                assert!(age.num_seconds() >= 29 && age.num_seconds() <= 31);
            }

            #[test]
            fn test_returns_correct_age_for_old_transaction() {
                let tx = create_test_tx_with_age(3600); // 1 hour ago
                let age = get_age_since_created(&tx).unwrap();

                // Allow for small timing differences
                assert!(age.num_seconds() >= 3599 && age.num_seconds() <= 3601);
            }

            #[test]
            fn test_returns_zero_age_for_just_created_transaction() {
                let tx = create_test_tx_with_age(0); // Just now
                let age = get_age_since_created(&tx).unwrap();

                // Should be very close to 0
                assert!(age.num_seconds() >= 0 && age.num_seconds() <= 1);
            }

            #[test]
            fn test_handles_negative_age_gracefully() {
                // Create transaction with future timestamp (clock skew scenario)
                let created_at = (Utc::now() + Duration::seconds(10)).to_rfc3339();
                let mut tx = create_mock_transaction();
                tx.created_at = created_at;

                let age = get_age_since_created(&tx).unwrap();

                // Age should be negative
                assert!(age.num_seconds() < 0);
            }

            #[test]
            fn test_returns_error_for_invalid_created_at() {
                let mut tx = create_mock_transaction();
                tx.created_at = "invalid-timestamp".to_string();

                let result = get_age_since_created(&tx);
                assert!(result.is_err());

                match result.unwrap_err() {
                    TransactionError::UnexpectedError(msg) => {
                        assert!(msg.contains("Invalid created_at timestamp"));
                    }
                    _ => panic!("Expected UnexpectedError"),
                }
            }

            #[test]
            fn test_returns_error_for_empty_created_at() {
                let mut tx = create_mock_transaction();
                tx.created_at = "".to_string();

                let result = get_age_since_created(&tx);
                assert!(result.is_err());
            }

            #[test]
            fn test_handles_various_rfc3339_formats() {
                let mut tx = create_mock_transaction();

                // Test with UTC timezone
                tx.created_at = "2025-01-01T12:00:00Z".to_string();
                assert!(get_age_since_created(&tx).is_ok());

                // Test with offset timezone
                tx.created_at = "2025-01-01T12:00:00+00:00".to_string();
                assert!(get_age_since_created(&tx).is_ok());

                // Test with milliseconds
                tx.created_at = "2025-01-01T12:00:00.123Z".to_string();
                assert!(get_age_since_created(&tx).is_ok());
            }
        }
    }

    #[test]
    fn test_create_signature_payload_functions() {
        use soroban_rs::xdr::{
            Hash, SequenceNumber, TransactionEnvelope, TransactionV0, TransactionV0Envelope,
            Uint256,
        };

        // Test create_transaction_signature_payload
        let transaction = soroban_rs::xdr::Transaction {
            source_account: soroban_rs::xdr::MuxedAccount::Ed25519(Uint256([1u8; 32])),
            fee: 100,
            seq_num: SequenceNumber(123),
            cond: soroban_rs::xdr::Preconditions::None,
            memo: soroban_rs::xdr::Memo::None,
            operations: vec![].try_into().unwrap(),
            ext: soroban_rs::xdr::TransactionExt::V0,
        };
        let network_id = Hash([2u8; 32]);

        let payload = create_transaction_signature_payload(&transaction, &network_id);
        assert_eq!(payload.network_id, network_id);

        // Test create_signature_payload with V0 envelope
        let v0_tx = TransactionV0 {
            source_account_ed25519: Uint256([1u8; 32]),
            fee: 100,
            seq_num: SequenceNumber(123),
            time_bounds: None,
            memo: soroban_rs::xdr::Memo::None,
            operations: vec![].try_into().unwrap(),
            ext: soroban_rs::xdr::TransactionV0Ext::V0,
        };
        let v0_envelope = TransactionEnvelope::TxV0(TransactionV0Envelope {
            tx: v0_tx,
            signatures: vec![].try_into().unwrap(),
        });

        let v0_payload = create_signature_payload(&v0_envelope, &network_id).unwrap();
        assert_eq!(v0_payload.network_id, network_id);
    }

    mod convert_v0_to_v1_transaction_tests {
        use super::*;
        use soroban_rs::xdr::{SequenceNumber, TransactionV0, Uint256};

        #[test]
        fn test_convert_v0_to_v1_transaction() {
            // Create a simple V0 transaction
            let v0_tx = TransactionV0 {
                source_account_ed25519: Uint256([1u8; 32]),
                fee: 100,
                seq_num: SequenceNumber(123),
                time_bounds: None,
                memo: soroban_rs::xdr::Memo::None,
                operations: vec![].try_into().unwrap(),
                ext: soroban_rs::xdr::TransactionV0Ext::V0,
            };

            // Convert to V1
            let v1_tx = convert_v0_to_v1_transaction(&v0_tx);

            // Check that conversion worked correctly
            assert_eq!(v1_tx.fee, v0_tx.fee);
            assert_eq!(v1_tx.seq_num, v0_tx.seq_num);
            assert_eq!(v1_tx.memo, v0_tx.memo);
            assert_eq!(v1_tx.operations, v0_tx.operations);
            assert!(matches!(v1_tx.ext, soroban_rs::xdr::TransactionExt::V0));
            assert!(matches!(v1_tx.cond, soroban_rs::xdr::Preconditions::None));

            // Check source account conversion
            match v1_tx.source_account {
                soroban_rs::xdr::MuxedAccount::Ed25519(addr) => {
                    assert_eq!(addr, v0_tx.source_account_ed25519);
                }
                _ => panic!("Expected Ed25519 muxed account"),
            }
        }

        #[test]
        fn test_convert_v0_to_v1_transaction_with_time_bounds() {
            // Create a V0 transaction with time bounds
            let time_bounds = soroban_rs::xdr::TimeBounds {
                min_time: soroban_rs::xdr::TimePoint(100),
                max_time: soroban_rs::xdr::TimePoint(200),
            };

            let v0_tx = TransactionV0 {
                source_account_ed25519: Uint256([2u8; 32]),
                fee: 200,
                seq_num: SequenceNumber(456),
                time_bounds: Some(time_bounds.clone()),
                memo: soroban_rs::xdr::Memo::Text("test".try_into().unwrap()),
                operations: vec![].try_into().unwrap(),
                ext: soroban_rs::xdr::TransactionV0Ext::V0,
            };

            // Convert to V1
            let v1_tx = convert_v0_to_v1_transaction(&v0_tx);

            // Check that time bounds were correctly converted to preconditions
            match v1_tx.cond {
                soroban_rs::xdr::Preconditions::Time(tb) => {
                    assert_eq!(tb, time_bounds);
                }
                _ => panic!("Expected Time preconditions"),
            }
        }
    }
}
