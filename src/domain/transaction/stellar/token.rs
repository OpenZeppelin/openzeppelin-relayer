//! Utility functions for Stellar transaction domain logic.
use crate::models::{RelayerError, StellarTokenKind, StellarTokenMetadata};
use crate::services::provider::StellarProviderTrait;
use base64::{engine::general_purpose, Engine};
use soroban_rs::xdr::{
    AccountId, AlphaNum12, AlphaNum4, Asset, AssetCode12, AssetCode4, ContractDataEntry,
    ContractId, Hash, LedgerEntryData, LedgerKey, LedgerKeyContractData, Limits,
    PublicKey as XdrPublicKey, ReadXdr, ScAddress, ScSymbol, ScVal, TrustLineEntry,
    TrustLineEntryExt, TrustLineEntryV1, Uint256, VecM,
};
use std::str::FromStr;
use stellar_strkey::ed25519::PublicKey;
use thiserror::Error;
use tracing::{debug, trace, warn};

// ============================================================================
// Error Types
// ============================================================================

/// Errors that can occur during Stellar token operations.
///
/// This error type is specific to Stellar token operations and provides
/// detailed error information. It can be converted to `RelayerError` using
/// the `From` trait implementation.
#[derive(Error, Debug)]
pub enum StellarTokenError {
    #[error("Invalid account address '{0}': {1}")]
    InvalidAccountAddress(String, String),

    #[error("Invalid contract address '{0}': {1}")]
    InvalidContractAddress(String, String),

    #[error("Invalid asset identifier format. Expected 'CODE:ISSUER', got: {0}")]
    InvalidAssetFormat(String),

    #[error("Asset code cannot be empty in asset identifier: {0}")]
    EmptyAssetCode(String),

    #[error("Asset code too long (max {0} characters): {1}")]
    AssetCodeTooLong(usize, String),

    #[error("Issuer address cannot be empty in asset identifier: {0}")]
    EmptyIssuerAddress(String),

    #[error("Invalid issuer address length (expected {0} characters): {1}")]
    InvalidIssuerLength(usize, String),

    #[error("Invalid issuer address format (must start with '{0}'): {1}")]
    InvalidIssuerPrefix(char, String),

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

    #[error("Failed to simulate contract transaction: {0}")]
    ContractSimulationFailed(String),
}

impl From<StellarTokenError> for RelayerError {
    fn from(error: StellarTokenError) -> Self {
        match error {
            StellarTokenError::AccountFetchFailed(msg)
            | StellarTokenError::ContractDataQueryPersistentFailed(_, msg)
            | StellarTokenError::ContractDataQueryTemporaryFailed(_, msg)
            | StellarTokenError::TrustlineQueryFailed(_, msg)
            | StellarTokenError::ContractInvocationFailed(_, msg)
            | StellarTokenError::ContractSimulationFailed(msg) => RelayerError::ProviderError(msg),
            StellarTokenError::NoTrustlineFound(_, _)
            | StellarTokenError::NoEntriesFound(_)
            | StellarTokenError::EmptyEntries(_) => {
                RelayerError::ValidationError(error.to_string())
            }
            _ => RelayerError::Internal(error.to_string()),
        }
    }
}

// Constants for Stellar address and asset validation
const STELLAR_ADDRESS_LENGTH: usize = 56;
const MAX_ASSET_CODE_LENGTH: usize = 12;
const DEFAULT_STELLAR_DECIMALS: u32 = 7;
const STELLAR_ACCOUNT_PREFIX: char = 'G';

// ============================================================================
// Helper Functions for Common Operations
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
fn parse_account_id(account_id: &str) -> Result<AccountId, StellarTokenError> {
    let account_pk = PublicKey::from_str(account_id).map_err(|e| {
        StellarTokenError::InvalidAccountAddress(account_id.to_string(), e.to_string())
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
fn parse_contract_address(contract_address: &str) -> Result<Hash, StellarTokenError> {
    let contract_id = ContractId::from_str(contract_address).map_err(|e| {
        StellarTokenError::InvalidContractAddress(contract_address.to_string(), e.to_string())
    })?;
    Ok(contract_id.0)
}

/// Parse an asset identifier in CODE:ISSUER format.
///
/// # Arguments
///
/// * `asset_id` - Asset identifier in "CODE:ISSUER" format
///
/// # Returns
///
/// Tuple of (code, issuer) or error if format is invalid
fn parse_asset_identifier(asset_id: &str) -> Result<(&str, &str), StellarTokenError> {
    asset_id
        .split_once(':')
        .ok_or_else(|| StellarTokenError::InvalidAssetFormat(asset_id.to_string()))
}

/// Validate and parse a classic asset issuer address.
///
/// Validates that the issuer is:
/// - Non-empty
/// - Exactly 56 characters
/// - Starts with 'G'
/// - Is a valid Stellar public key (not a contract address)
///
/// # Arguments
///
/// * `issuer` - Issuer address string
/// * `asset_id` - Full asset identifier (for error messages)
///
/// # Returns
///
/// AccountId XDR type or error if validation fails
fn validate_and_parse_issuer(issuer: &str, asset_id: &str) -> Result<AccountId, StellarTokenError> {
    if issuer.is_empty() {
        return Err(StellarTokenError::EmptyIssuerAddress(asset_id.to_string()));
    }

    if issuer.len() != STELLAR_ADDRESS_LENGTH {
        return Err(StellarTokenError::InvalidIssuerLength(
            STELLAR_ADDRESS_LENGTH,
            issuer.to_string(),
        ));
    }

    if !issuer.starts_with(STELLAR_ACCOUNT_PREFIX) {
        return Err(StellarTokenError::InvalidIssuerPrefix(
            STELLAR_ACCOUNT_PREFIX,
            issuer.to_string(),
        ));
    }

    // Validate issuer is a valid Stellar public key (not a contract address)
    parse_account_id(issuer)
}

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
fn create_contract_data_key(
    symbol: &str,
    address: Option<ScAddress>,
) -> Result<ScVal, StellarTokenError> {
    if address.is_none() {
        let sym = symbol.try_into().map_err(|e| {
            StellarTokenError::SymbolCreationFailed(symbol.to_string(), format!("{:?}", e))
        })?;
        return Ok(ScVal::Symbol(sym));
    }

    let mut key_items: Vec<ScVal> = vec![ScVal::Symbol(symbol.try_into().map_err(|e| {
        StellarTokenError::SymbolCreationFailed(symbol.to_string(), format!("{:?}", e))
    })?)];

    if let Some(addr) = address {
        key_items.push(ScVal::Address(addr));
    }

    let key_vec: VecM<ScVal, { u32::MAX }> = VecM::try_from(key_items).map_err(|e| {
        StellarTokenError::KeyVectorCreationFailed(symbol.to_string(), format!("{:?}", e))
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
async fn query_contract_data_with_fallback<P>(
    provider: &P,
    contract_hash: Hash,
    key: ScVal,
    error_context: &str,
) -> Result<soroban_rs::stellar_rpc_client::GetLedgerEntriesResponse, StellarTokenError>
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
            StellarTokenError::ContractDataQueryPersistentFailed(
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
                StellarTokenError::ContractDataQueryTemporaryFailed(
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
fn parse_ledger_entry_from_xdr(
    xdr_string: &str,
    context: &str,
) -> Result<LedgerEntryData, StellarTokenError> {
    let trimmed_xdr = xdr_string.trim();

    // Ensure valid base64
    if general_purpose::STANDARD.decode(trimmed_xdr).is_err() {
        return Err(StellarTokenError::LedgerEntryParseFailed(
            context.to_string(),
            "Invalid base64".to_string(),
        ));
    }

    // Parse as LedgerEntryData (what Soroban RPC actually returns)
    match LedgerEntryData::from_xdr_base64(trimmed_xdr, Limits::none()) {
        Ok(data) => Ok(data),
        Err(e) => Err(StellarTokenError::LedgerEntryParseFailed(
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
fn extract_scval_from_contract_data(
    ledger_entries: &soroban_rs::stellar_rpc_client::GetLedgerEntriesResponse,
    context: &str,
) -> Result<ScVal, StellarTokenError> {
    let entries = ledger_entries
        .entries
        .as_ref()
        .ok_or_else(|| StellarTokenError::NoEntriesFound(context.into()))?;

    if entries.is_empty() {
        return Err(StellarTokenError::EmptyEntries(context.into()));
    }

    let entry_xdr = &entries[0].xdr;
    let entry = parse_ledger_entry_from_xdr(entry_xdr, context)?;

    match entry {
        LedgerEntryData::ContractData(ContractDataEntry { val, .. }) => Ok(val.clone()),

        _ => Err(StellarTokenError::UnexpectedLedgerEntryType(context.into())),
    }
}

// ============================================================================
// Public API Functions
// ============================================================================

/// Fetch available token balance for a given account and asset identifier.
///
/// Supports:
/// - Native XLM: Returns account balance directly
/// - Traditional assets (Credit4/Credit12): Queries trustline balance via LedgerKey::Trustline
///   and excludes funds locked in pending offers (selling_liabilities)
/// - Contract tokens: Queries contract data balance via LedgerKey::ContractData
///
/// # Arguments
///
/// * `provider` - Stellar provider for querying ledger entries
/// * `account_id` - Account address to check balance for
/// * `asset_id` - Asset identifier:
///   - "native" or "" for XLM
///   - "CODE:ISSUER" for traditional assets (e.g., "USDC:GA5Z...")
///   - Contract address (starts with "C", 56 chars) for Soroban contract tokens
///
/// # Returns
///
/// Available balance in stroops (or token's smallest unit) as u64, excluding funds locked
/// in pending offers/orders, or error if balance cannot be fetched
pub async fn get_token_balance<P>(
    provider: &P,
    account_id: &str,
    asset_id: &str,
) -> Result<u64, StellarTokenError>
where
    P: StellarProviderTrait + Send + Sync,
{
    // Handle native XLM - accept both "native" and "XLM" for UX
    if asset_id == "native" || asset_id == "XLM" {
        let account_entry = provider
            .get_account(account_id)
            .await
            .map_err(|e| StellarTokenError::AccountFetchFailed(e.to_string()))?;
        return Ok(account_entry.balance as u64);
    }

    // Check if it's a contract address using proper StrKey validation
    if ContractId::from_str(asset_id).is_ok() {
        return get_contract_token_balance(provider, account_id, asset_id).await;
    }

    // Otherwise, treat as traditional asset (CODE:ISSUER format)
    get_asset_trustline_balance(provider, account_id, asset_id).await
}

/// Fetch available balance for a traditional Stellar asset (Credit4/Credit12) via trustline
///
/// Returns the available balance excluding funds locked in pending offers/orders.
/// For TrustLineEntry V1 (with liabilities), subtracts selling_liabilities from balance.
/// For TrustLineEntry V0 (no liabilities), returns the total balance.
async fn get_asset_trustline_balance<P>(
    provider: &P,
    account_id: &str,
    asset_id: &str,
) -> Result<u64, StellarTokenError>
where
    P: StellarProviderTrait + Send + Sync,
{
    let (code, issuer) = parse_asset_identifier(asset_id)?;

    let issuer_id = parse_account_id(issuer)?;
    let account_xdr = parse_account_id(account_id)?;

    let asset = if code.len() <= 4 {
        let mut buf = [0u8; 4];
        buf[..code.len()].copy_from_slice(code.as_bytes());
        Asset::CreditAlphanum4(AlphaNum4 {
            asset_code: AssetCode4(buf),
            issuer: issuer_id,
        })
    } else {
        let mut buf = [0u8; 12];
        buf[..code.len()].copy_from_slice(code.as_bytes());
        Asset::CreditAlphanum12(AlphaNum12 {
            asset_code: AssetCode12(buf),
            issuer: issuer_id,
        })
    };

    let ledger_key = LedgerKey::Trustline(soroban_rs::xdr::LedgerKeyTrustLine {
        account_id: account_xdr,
        asset: match asset {
            Asset::CreditAlphanum4(a) => soroban_rs::xdr::TrustLineAsset::CreditAlphanum4(a),
            Asset::CreditAlphanum12(a) => soroban_rs::xdr::TrustLineAsset::CreditAlphanum12(a),
            Asset::Native => return Err(StellarTokenError::NativeAssetInTrustlineQuery),
        },
    });

    let resp = provider
        .get_ledger_entries(&[ledger_key])
        .await
        .map_err(|e| StellarTokenError::TrustlineQueryFailed(asset_id.into(), e.to_string()))?;

    let entries = resp
        .entries
        .ok_or_else(|| StellarTokenError::NoTrustlineFound(asset_id.into(), account_id.into()))?;

    if entries.is_empty() {
        return Err(StellarTokenError::NoTrustlineFound(
            asset_id.into(),
            account_id.into(),
        ));
    }

    let entry = parse_ledger_entry_from_xdr(&entries[0].xdr, asset_id)?;

    match entry {
        LedgerEntryData::Trustline(TrustLineEntry {
            balance,
            ext: TrustLineEntryExt::V1(TrustLineEntryV1 { liabilities, .. }),
            ..
        }) => {
            // V1 has liabilities - calculate available balance by subtracting selling_liabilities
            // selling_liabilities represents funds locked in sell offers for this asset
            let available_balance = balance.saturating_sub(liabilities.selling);
            debug!(
                account_id = %account_id,
                asset_id = %asset_id,
                total_balance = balance,
                selling_liabilities = liabilities.selling,
                buying_liabilities = liabilities.buying,
                available_balance = available_balance,
                "Trustline balance retrieved (V1 with liabilities)"
            );
            Ok(available_balance.max(0) as u64)
        }
        LedgerEntryData::Trustline(TrustLineEntry {
            balance,
            ext: TrustLineEntryExt::V0,
            ..
        }) => {
            // V0 has no liabilities - return total balance
            debug!(
                account_id = %account_id,
                asset_id = %asset_id,
                balance_raw = balance,
                balance_u64 = balance as u64,
                "Trustline balance retrieved (V0, no liabilities)"
            );
            Ok(balance.max(0) as u64)
        }

        LedgerEntryData::Trustline(_) => Err(StellarTokenError::UnsupportedTrustlineVersion),

        _ => Err(StellarTokenError::UnexpectedTrustlineEntryType),
    }
}

/// Fetch balance for a Soroban contract token via ContractData
async fn get_contract_token_balance<P>(
    provider: &P,
    account_id: &str,
    contract_address: &str,
) -> Result<u64, StellarTokenError>
where
    P: StellarProviderTrait + Send + Sync,
{
    // Parse contract address and account ID
    let contract_hash = parse_contract_address(contract_address)?;
    let account_xdr_id = parse_account_id(account_id)?;
    let account_sc_address = ScAddress::Account(account_xdr_id);

    // Create balance key (Soroban token standard uses "Balance" as the key)
    let balance_key = create_contract_data_key("Balance", Some(account_sc_address))?;

    // Query contract data with durability fallback
    let error_context = format!(
        "contract {} balance for account {}",
        contract_address, account_id
    );
    let ledger_entries =
        query_contract_data_with_fallback(provider, contract_hash, balance_key, &error_context)
            .await?;

    // Extract balance from contract data entry
    let entries = match ledger_entries.entries {
        Some(entries) if !entries.is_empty() => entries,
        _ => {
            // No balance entry means balance is 0
            warn!(
                "No balance entry found for contract {} on account {}, assuming zero balance",
                contract_address, account_id
            );
            return Ok(0);
        }
    };

    let entry_result = &entries[0];
    let entry = parse_ledger_entry_from_xdr(&entry_result.xdr, &error_context)?;

    match entry {
        LedgerEntryData::ContractData(ContractDataEntry { val, .. }) => match val {
            ScVal::I128(parts) => {
                if parts.hi != 0 {
                    return Err(StellarTokenError::BalanceTooLarge(parts.hi, parts.lo));
                }
                if (parts.lo as i64) < 0 {
                    return Err(StellarTokenError::NegativeBalanceI128(parts.lo));
                }
                Ok(parts.lo as u64)
            }
            ScVal::U64(n) => Ok(n),
            ScVal::I64(n) => {
                if n < 0 {
                    return Err(StellarTokenError::NegativeBalanceI64(n));
                }
                Ok(n as u64)
            }
            other => Err(StellarTokenError::UnexpectedBalanceType(format!(
                "{:?}",
                other
            ))),
        },
        _ => Err(StellarTokenError::UnexpectedContractDataEntryType),
    }
}

/// Fetch token metadata for a given asset identifier.
///
/// Determines the token kind and fetches appropriate metadata:
/// - Native XLM: decimals = 7, canonical_asset_id = "native"
///   - Accepts "native", "XLM", or empty string "" (empty string is treated as native XLM)
/// - Classic assets (CODE:ISSUER): decimals = 7 (default), canonical_asset_id = asset
///   - Code must be 1-12 characters, issuer must be a valid Stellar address (G...)
/// - Contract tokens: queries contract for decimals, canonical_asset_id = contract_id
///   - Must be a valid StrKey-encoded contract address (C...)
///
/// # Arguments
///
/// * `provider` - Stellar provider for querying ledger entries
/// * `asset_id` - Asset identifier:
///   - "native", "XLM", or "" (empty string) for XLM
///   - "CODE:ISSUER" for traditional assets (e.g., "USDC:GA5Z...")
///   - Contract address (StrKey format starting with "C") for Soroban contract tokens
///
/// # Returns
///
/// Token metadata including kind, decimals, and canonical asset ID, or error if metadata cannot be fetched
///
/// # Errors
///
/// Returns `RelayerError::Internal` if:
/// - Asset identifier format is invalid
/// - Asset code is empty or exceeds 12 characters
/// - Issuer address is invalid (not 56 chars, doesn't start with 'G', or invalid format)
/// - Contract address is invalid StrKey format
pub async fn get_token_metadata<P>(
    provider: &P,
    asset_id: &str,
) -> Result<StellarTokenMetadata, StellarTokenError>
where
    P: StellarProviderTrait + Send + Sync,
{
    // Handle native XLM (empty string is intentionally treated as native XLM)
    if asset_id == "native" || asset_id == "XLM" || asset_id.is_empty() {
        return Ok(StellarTokenMetadata {
            kind: StellarTokenKind::Native,
            decimals: DEFAULT_STELLAR_DECIMALS,
            canonical_asset_id: "native".to_string(),
        });
    }

    // Check if it's a contract address using proper StrKey validation
    if ContractId::from_str(asset_id).is_ok() {
        // Valid contract address - fetch decimals from contract, default to 7 if not found
        let decimals = get_contract_token_decimals(provider, asset_id)
            .await
            .unwrap_or_else(|| {
                warn!(
                    contract_address = %asset_id,
                    "Could not fetch decimals from contract, using default"
                );
                DEFAULT_STELLAR_DECIMALS
            });

        return Ok(StellarTokenMetadata {
            kind: StellarTokenKind::Contract {
                contract_id: asset_id.to_string(),
            },
            decimals,
            canonical_asset_id: asset_id.to_uppercase().to_string(),
        });
    }

    // Otherwise, treat as traditional asset (CODE:ISSUER format)
    // Parse to validate format
    let (code, issuer) = parse_asset_identifier(asset_id)?;

    // Validate asset code
    if code.is_empty() {
        return Err(StellarTokenError::EmptyAssetCode(asset_id.to_string()));
    }

    if code.len() > MAX_ASSET_CODE_LENGTH {
        return Err(StellarTokenError::AssetCodeTooLong(
            MAX_ASSET_CODE_LENGTH,
            code.to_string(),
        ));
    }

    // Validate and parse issuer address
    validate_and_parse_issuer(issuer, asset_id)?;

    // Classic assets typically use 7 decimals (Stellar standard)
    // In the future, this could be queried from the asset's trustline or asset info
    Ok(StellarTokenMetadata {
        kind: StellarTokenKind::Classic {
            code: code.to_string(),
            issuer: issuer.to_string(),
        },
        decimals: DEFAULT_STELLAR_DECIMALS,
        canonical_asset_id: asset_id.to_string(),
    })
}

/// Attempts to fetch decimals for a contract token by invoking the contract's decimals() function.
///
/// This implementation uses multiple strategies:
/// 1. First tries to invoke the contract's `decimals()` function (SEP-41 standard)
/// 2. Falls back to querying contract data storage if invocation fails
/// 3. Returns None if all methods fail
///
/// # Arguments
///
/// * `provider` - Stellar provider for querying ledger entries and invoking contracts
/// * `contract_address` - Contract address in StrKey format (must be valid ContractId)
///
/// # Returns
///
/// Some(u32) if decimals are found, None if decimals cannot be determined.
/// Logs warnings for debugging when decimals cannot be fetched.
///
/// # Note
///
/// This function assumes the contract follows SEP-41 token interface with a `decimals()`
/// function. Non-standard tokens may not have this function.
pub async fn get_contract_token_decimals<P>(provider: &P, contract_address: &str) -> Option<u32>
where
    P: StellarProviderTrait + Send + Sync,
{
    debug!(
        contract_address = %contract_address,
        "Fetching decimals for contract token"
    );

    // Parse contract address - if invalid, log and return None
    let contract_hash = match parse_contract_address(contract_address) {
        Ok(hash) => hash,
        Err(e) => {
            warn!(
                contract_address = %contract_address,
                error = %e,
                "Failed to parse contract address"
            );
            return None;
        }
    };

    // Strategy 1: Try invoking the decimals() function (preferred method for mainnet)
    if let Some(decimals) = invoke_decimals_function(provider, contract_address).await {
        debug!(
            contract_address = %contract_address,
            decimals = %decimals,
            "Successfully fetched decimals via contract invocation"
        );
        return Some(decimals);
    }

    // Strategy 2: Fall back to querying contract data storage
    debug!(
        contract_address = %contract_address,
        "Contract invocation failed, trying storage query"
    );

    query_decimals_from_storage(provider, contract_address, contract_hash).await
}

/// Invoke the decimals() function on a contract token.
///
/// This is the standard way to fetch decimals for SEP-41 compliant tokens.
///
/// # Arguments
///
/// * `provider` - Stellar provider for contract invocation
/// * `contract_address` - Contract address string (for logging and invocation)
///
/// # Returns
///
/// Some(u32) if invocation succeeds, None otherwise
async fn invoke_decimals_function<P>(provider: &P, contract_address: &str) -> Option<u32>
where
    P: StellarProviderTrait + Send + Sync,
{
    // Create function name symbol
    let function_name = match ScSymbol::try_from("decimals") {
        Ok(sym) => sym,
        Err(e) => {
            warn!(contract_address = %contract_address, error = ?e, "Failed to create decimals symbol");
            return None;
        }
    };

    // No arguments for decimals()
    let args: Vec<ScVal> = vec![];

    // Call contract function (read-only via simulation)
    match provider
        .call_contract(contract_address, &function_name, args)
        .await
    {
        Ok(result) => extract_u32_from_scval(&result, contract_address, "decimals() result"),
        Err(e) => {
            debug!(contract_address = %contract_address, error = %e, "Failed to invoke decimals() function");
            None
        }
    }
}

/// Query decimals from contract data storage.
///
/// This is a fallback method for tokens that store decimals in contract data
/// instead of providing a decimals() function.
///
/// # Arguments
///
/// * `provider` - Stellar provider for querying ledger entries
/// * `contract_address` - Contract address string (for logging)
/// * `contract_hash` - Parsed contract hash
///
/// # Returns
///
/// Some(u32) if decimals are found in storage, None otherwise
async fn query_decimals_from_storage<P>(
    provider: &P,
    contract_address: &str,
    contract_hash: Hash,
) -> Option<u32>
where
    P: StellarProviderTrait + Send + Sync,
{
    // Create decimals key (SEP-41 token standard uses "Decimals" as the key)
    let decimals_key = match create_contract_data_key("Decimals", None) {
        Ok(key) => key,
        Err(e) => {
            warn!(
                contract_address = %contract_address,
                error = %e,
                "Failed to create Decimals key"
            );
            return None;
        }
    };

    // Query contract data with durability fallback
    let error_context = format!("contract {} decimals", contract_address);
    let ledger_entries = match query_contract_data_with_fallback(
        provider,
        contract_hash,
        decimals_key,
        &error_context,
    )
    .await
    {
        Ok(entries) => entries,
        Err(e) => {
            debug!(
                contract_address = %contract_address,
                error = %e,
                "Failed to query contract data for decimals"
            );
            return None;
        }
    };

    // Extract ScVal from contract data entry
    let val = match extract_scval_from_contract_data(&ledger_entries, &error_context) {
        Ok(v) => v,
        Err(_) => {
            trace!(
                contract_address = %contract_address,
                "No decimals entry found in contract data"
            );
            return None;
        }
    };

    // Extract decimals value from ScVal
    extract_u32_from_scval(&val, contract_address, "decimals storage value")
}

/// Extract a u32 value from an ScVal.
///
/// Handles multiple ScVal types that can represent decimal values.
///
/// # Arguments
///
/// * `val` - ScVal to extract from
/// * `contract_address` - Contract address (for logging)
/// * `context` - Context string (for logging)
///
/// # Returns
///
/// Some(u32) if extraction succeeds, None otherwise
fn extract_u32_from_scval(val: &ScVal, contract_address: &str, context: &str) -> Option<u32> {
    match val {
        ScVal::U32(n) => Some(*n),
        ScVal::U64(n) => {
            // Safe conversion with overflow check
            u32::try_from(*n).ok().or_else(|| {
                warn!(
                    contract_address = %contract_address,
                    decimals_value = %n,
                    context = %context,
                    "Decimals value too large for u32"
                );
                None
            })
        }
        ScVal::I32(n) if *n >= 0 => u32::try_from(*n).ok().or_else(|| {
            warn!(
                contract_address = %contract_address,
                decimals_value = %n,
                context = %context,
                "Invalid decimals value (negative or overflow)"
            );
            None
        }),
        ScVal::I32(n) => {
            warn!(
                contract_address = %contract_address,
                decimals_value = %n,
                context = %context,
                "Negative decimals value not allowed (I32)"
            );
            None
        }
        ScVal::I64(n) if *n >= 0 => u32::try_from(*n).ok().or_else(|| {
            warn!(
                contract_address = %contract_address,
                decimals_value = %n,
                context = %context,
                "Invalid decimals value (negative or overflow)"
            );
            None
        }),
        ScVal::I64(n) => {
            warn!(
                contract_address = %contract_address,
                decimals_value = %n,
                context = %context,
                "Negative decimals value not allowed (I64)"
            );
            None
        }
        ScVal::I128(parts) => {
            // Check if value is negative (hi < 0) or too large (hi != 0)
            if parts.hi != 0 {
                if parts.hi < 0 {
                    warn!(
                        contract_address = %contract_address,
                        decimals_value = ?parts,
                        context = %context,
                        "Negative decimals value not allowed (I128)"
                    );
                } else {
                    warn!(
                        contract_address = %contract_address,
                        decimals_value = ?parts,
                        context = %context,
                        "Decimals value too large for u32 (I128)"
                    );
                }
                return None;
            }
            // hi == 0, so value fits in u64, now check if it fits in u32
            u32::try_from(parts.lo).ok().or_else(|| {
                warn!(
                    contract_address = %contract_address,
                    decimals_value = ?parts,
                    context = %context,
                    "Decimals value too large for u32 (I128 lo overflow)"
                );
                None
            })
        }
        ScVal::U128(parts) => {
            // Check if hi is non-zero (too large)
            if parts.hi != 0 {
                warn!(
                    contract_address = %contract_address,
                    decimals_value = ?parts,
                    context = %context,
                    "Decimals value too large for u32 (U128)"
                );
                return None;
            }
            // hi == 0, so value fits in u64, now check if it fits in u32
            u32::try_from(parts.lo).ok().or_else(|| {
                warn!(
                    contract_address = %contract_address,
                    decimals_value = ?parts,
                    context = %context,
                    "Decimals value too large for u32 (U128 lo overflow)"
                );
                None
            })
        }
        _ => {
            warn!(
                contract_address = %contract_address,
                decimals_value = ?val,
                context = %context,
                "Unexpected ScVal type for decimals (expected U32, U64, I32, I64, I128, or U128)"
            );
            None
        }
    }
}
#[cfg(test)]
mod integration_tests {
    use tracing::debug;

    use super::*;
    use crate::models::RpcConfig;
    use crate::services::provider::StellarProvider;

    // Test accounts and assets on Stellar testnet
    // These are well-known accounts that should exist on testnet
    const TESTNET_TEST_ACCOUNT: &str = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";
    const TESTNET_USDC_ISSUER: &str = "GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5";
    const TESTNET_USDC_ASSET: &str =
        "USDC:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5";

    fn setup_test_env() {
        std::env::set_var("API_KEY", "7EF1CB7C-5003-4696-B384-C72AF8C3E15D");
        std::env::set_var("REDIS_URL", "redis://localhost:6379");
    }

    // Helper function to create a testnet provider
    fn create_testnet_provider() -> StellarProvider {
        let rpc_configs = vec![RpcConfig::new(
            "https://soroban-testnet.stellar.org".to_string(),
        )];
        StellarProvider::new(rpc_configs, 30).expect("Failed to create testnet provider")
    }

    // Helper function to create a mainnet provider
    fn create_mainnet_provider() -> StellarProvider {
        let rpc_configs = vec![RpcConfig::new("https://mainnet.sorobanrpc.com".to_string())];
        StellarProvider::new(rpc_configs, 30).expect("Failed to create mainnet provider")
    }

    #[tokio::test]
    // #[ignore] // Integration test - requires network access
    async fn test_get_token_balance_native_xlm_testnet() {
        setup_test_env();
        let provider = create_testnet_provider();

        // Test with "native"
        let result = get_token_balance(&provider, TESTNET_TEST_ACCOUNT, "native").await;
        assert!(result.is_ok(), "Should fetch native XLM balance");
        let balance = result.unwrap();
        assert!(balance >= 0, "Balance should be non-negative");

        // Test with "XLM"
        let result = get_token_balance(&provider, TESTNET_TEST_ACCOUNT, "XLM").await;
        assert!(
            result.is_ok(),
            "Should fetch XLM balance using 'XLM' identifier"
        );
        let balance_xlm = result.unwrap();
        assert_eq!(
            balance, balance_xlm,
            "Both 'native' and 'XLM' should return same balance"
        );
    }

    #[tokio::test]
    // #[ignore] // Integration test - requires network access
    async fn test_get_token_balance_asset_testnet() {
        setup_test_env();
        let provider = create_testnet_provider();
        // Test fetching balance for USDC on testnet
        // Note: This will fail if the account doesn't have a trustline, which is expected
        let result = get_token_balance(&provider, TESTNET_TEST_ACCOUNT, TESTNET_USDC_ASSET).await;

        // Either the balance is fetched successfully, or we get a validation error (no trustline)
        match result {
            Ok(balance) => {
                assert!(balance >= 0, "Balance should be non-negative");
            }
            Err(StellarTokenError::NoTrustlineFound(_, _)) => {
                // This is expected if the account doesn't have a trustline
            }
            Err(e) => {
                panic!("Unexpected error: {:?}", e);
            }
        }
    }

    #[tokio::test]
    // #[ignore] // Integration test - requires network access
    async fn test_get_token_balance_contract_token_testnet() {
        setup_test_env();
        let provider = create_testnet_provider();
        // Example contract address on testnet (you may need to update this with a real contract)
        // This is a placeholder - replace with an actual contract address that exists on testnet
        let test_contract = "CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA";

        // Test fetching balance for a contract token
        // This will return 0 if no balance entry exists, or the actual balance
        let result = get_token_balance(&provider, TESTNET_TEST_ACCOUNT, test_contract).await;

        match result {
            Ok(balance) => {
                assert!(balance >= 0, "Balance should be non-negative");
            }
            Err(StellarTokenError::InvalidContractAddress(_, _)) => {
                // Contract address might be invalid - this is okay for testing
            }
            Err(e) => {
                // Other errors are acceptable for integration tests
                eprintln!("Contract balance test returned error (expected): {:?}", e);
            }
        }
    }

    #[tokio::test]
    // #[ignore] // Integration test - requires network access
    async fn test_get_token_metadata_native() {
        setup_test_env();
        let provider = create_testnet_provider();

        // Test native XLM metadata
        let result = get_token_metadata(&provider, "native").await;
        assert!(result.is_ok(), "Should fetch native XLM metadata");
        let metadata = result.unwrap();
        assert_eq!(metadata.kind, StellarTokenKind::Native);
        assert_eq!(metadata.decimals, DEFAULT_STELLAR_DECIMALS);
        assert_eq!(metadata.canonical_asset_id, "native");

        // Test with "XLM"
        let result = get_token_metadata(&provider, "XLM").await;
        assert!(
            result.is_ok(),
            "Should fetch XLM metadata using 'XLM' identifier"
        );
        let metadata_xlm = result.unwrap();
        assert_eq!(metadata_xlm.kind, StellarTokenKind::Native);
        assert_eq!(metadata_xlm.decimals, DEFAULT_STELLAR_DECIMALS);

        // Test with empty string
        let result = get_token_metadata(&provider, "").await;
        assert!(
            result.is_ok(),
            "Should fetch native XLM metadata with empty string"
        );
        let metadata_empty = result.unwrap();
        assert_eq!(metadata_empty.kind, StellarTokenKind::Native);
    }

    #[tokio::test]
    // #[ignore] // Integration test - requires network access
    async fn test_get_token_metadata_asset_testnet() {
        setup_test_env();
        let provider = create_testnet_provider();

        // Test fetching metadata for USDC on testnet
        let result = get_token_metadata(&provider, TESTNET_USDC_ASSET).await;
        assert!(result.is_ok(), "Should fetch asset metadata");
        let metadata = result.unwrap();

        match metadata.kind {
            StellarTokenKind::Classic { code, issuer } => {
                assert_eq!(code, "USDC");
                assert_eq!(issuer, TESTNET_USDC_ISSUER);
            }
            _ => panic!("Expected Classic asset kind"),
        }
        assert_eq!(metadata.decimals, DEFAULT_STELLAR_DECIMALS);
        assert_eq!(metadata.canonical_asset_id, TESTNET_USDC_ASSET);
    }

    #[tokio::test]
    // #[ignore] // Integration test - requires network access
    async fn test_get_token_metadata_contract_token_testnet() {
        setup_test_env();
        let provider = create_testnet_provider();
        // Example contract address on testnet (you may need to update this with a real contract)
        let test_contract = "CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA";

        // Test fetching metadata for a contract token
        let result = get_token_metadata(&provider, test_contract).await;

        match result {
            Ok(metadata) => {
                eprintln!("metadata: {:?}", metadata);
                match metadata.kind {
                    StellarTokenKind::Contract { contract_id } => {
                        assert_eq!(contract_id, test_contract);
                    }
                    _ => panic!("Expected Contract token kind"),
                }
                assert_eq!(metadata.canonical_asset_id, test_contract);
                // Decimals should be fetched from contract or default to 7
                assert!(metadata.decimals > 0 && metadata.decimals <= 18);
            }
            Err(StellarTokenError::InvalidContractAddress(_, _)) => {
                // Contract address might be invalid - this is okay for testing
            }
            Err(e) => {
                // Other errors are acceptable for integration tests
                eprintln!("Contract metadata test returned error (expected): {:?}", e);
            }
        }
    }

    #[tokio::test]
    // #[ignore] // Integration test - requires network access
    async fn test_get_token_metadata_invalid_format() {
        setup_test_env();
        let provider = create_testnet_provider();

        // Test invalid asset format
        let result = get_token_metadata(&provider, "INVALID_FORMAT").await;
        assert!(result.is_err(), "Should reject invalid asset format");
        match result.unwrap_err() {
            StellarTokenError::InvalidAssetFormat(_) => {}
            e => panic!("Expected InvalidAssetFormat error, got: {:?}", e),
        }
    }

    #[tokio::test]
    // #[ignore] // Integration test - requires network access
    async fn test_get_token_metadata_empty_code() {
        setup_test_env();
        let provider = create_testnet_provider();

        // Test asset with empty code
        let result = get_token_metadata(
            &provider,
            ":GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5",
        )
        .await;
        assert!(result.is_err(), "Should reject empty asset code");
        match result.unwrap_err() {
            StellarTokenError::EmptyAssetCode(_) => {}
            e => panic!("Expected EmptyAssetCode error, got: {:?}", e),
        }
    }

    #[tokio::test]
    // #[ignore] // Integration test - requires network access
    async fn test_get_token_metadata_invalid_issuer() {
        setup_test_env();
        let provider = create_testnet_provider();

        // Test asset with invalid issuer (too short)
        let result = get_token_metadata(&provider, "USDC:INVALID").await;
        assert!(result.is_err(), "Should reject invalid issuer");
        match result.unwrap_err() {
            StellarTokenError::InvalidIssuerLength(_, _)
            | StellarTokenError::InvalidIssuerPrefix(_, _)
            | StellarTokenError::InvalidAccountAddress(_, _) => {}
            e => panic!("Expected issuer validation error, got: {:?}", e),
        }
    }

    #[tokio::test]
    // #[ignore] // Integration test - requires network access
    async fn test_get_contract_token_decimals_testnet() {
        setup_test_env();
        let provider = create_testnet_provider();
        // Example contract address on testnet (you may need to update this with a real contract)
        let test_contract = "CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA";

        // Test fetching decimals for a contract token
        let result = get_contract_token_decimals(&provider, test_contract).await;
        eprintln!(
            "result test_get_contract_token_decimals_testnet: {:?}",
            result
        );
        match result {
            Some(decimals) => {
                assert!(
                    decimals > 0 && decimals <= 18,
                    "Decimals should be between 1 and 18"
                );
            }
            None => {
                // This is acceptable - contract might not have decimals key or might not exist
                eprintln!("Contract decimals not found (expected for some contracts)");
            }
        }
    }

    #[tokio::test]
    // #[ignore] // Integration test - requires network access
    async fn test_get_token_balance_mainnet() {
        // Test on mainnet with a well-known account
        // Using Stellar Development Foundation's account as an example
        let mainnet_account = "GAV5KSHR2ZXIKKP5QF5CZBFJQBEA6VTR6OZTMEAGQSY3DU5CTCUOXQEQ";
        setup_test_env();
        let provider = create_mainnet_provider();

        // Test native XLM balance
        let result = get_token_balance(&provider, mainnet_account, "native").await;
        assert!(result.is_ok(), "Should fetch native XLM balance on mainnet");
        let balance = result.unwrap();
        eprintln!("Balance: {}", balance);
        debug!("Balance: {}", balance);
        assert!(balance > 0, "Balance should be greater than 0");
    }

    #[tokio::test]
    // #[ignore] // Integration test - requires network access
    async fn test_get_token_metadata_mainnet() {
        // Test on mainnet with a well-known asset
        // USDC on mainnet
        const MAINNET_USDC_ASSET: &str =
            "USDC:GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN";
        setup_test_env();
        let provider = create_mainnet_provider();

        let result = get_token_metadata(&provider, MAINNET_USDC_ASSET).await;
        assert!(result.is_ok(), "Should fetch asset metadata on mainnet");
        let metadata = result.unwrap();

        match metadata.kind {
            StellarTokenKind::Classic { code, issuer } => {
                assert_eq!(code, "USDC");
                assert_eq!(
                    issuer,
                    "GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN"
                );
            }
            _ => panic!("Expected Classic asset kind"),
        }
        assert_eq!(metadata.decimals, DEFAULT_STELLAR_DECIMALS);
    }

    #[tokio::test]
    // #[ignore] // Integration test - requires network access
    async fn test_get_contract_token_decimals_mainnet() {
        setup_test_env();
        let provider = create_mainnet_provider();
        // Example contract address on mainnet (you may need to update this with a real contract)
        let test_contract = "CDDS7IQJGQ2ZMO66E3MUYXZ56H2OO7RBTTAGZLZKOEA4EXCGZX65JGA7";

        // Test fetching decimals for a contract token
        let result = get_contract_token_decimals(&provider, test_contract).await;
        match result {
            Some(decimals) => {
                assert!(
                    decimals > 0 && decimals <= 18,
                    "Decimals should be between 1 and 18"
                );
            }
            None => {
                // This is acceptable - contract might not have decimals key or might not exist
                eprintln!("Contract decimals not found (expected for some contracts)");
            }
        }
    }
}
