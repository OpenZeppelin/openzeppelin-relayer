//! Utility functions for Stellar transaction domain logic.
use crate::domain::transaction::stellar::utils::{
    create_contract_data_key, extract_scval_from_contract_data, extract_u32_from_scval,
    parse_account_id, parse_contract_address, parse_ledger_entry_from_xdr,
    query_contract_data_with_fallback, StellarTransactionUtilsError,
};
use crate::models::{StellarTokenKind, StellarTokenMetadata};
use crate::services::provider::StellarProviderTrait;
use soroban_rs::xdr::{
    AccountId, AlphaNum12, AlphaNum4, Asset, AssetCode12, AssetCode4, ContractDataEntry,
    ContractId, Hash, LedgerEntryData, LedgerKey, ScAddress, ScSymbol, ScVal, TrustLineEntry,
    TrustLineEntryExt, TrustLineEntryV1,
};
use std::str::FromStr;
use tracing::{debug, trace, warn};

// Constants for Stellar address and asset validation
const STELLAR_ADDRESS_LENGTH: usize = 56;
const MAX_ASSET_CODE_LENGTH: usize = 12;
const DEFAULT_STELLAR_DECIMALS: u32 = 7;
const STELLAR_ACCOUNT_PREFIX: char = 'G';

// ============================================================================
// Helper Functions for Common Operations
// ============================================================================

/// Parse an asset identifier in CODE:ISSUER format.
///
/// # Arguments
///
/// * `asset_id` - Asset identifier in "CODE:ISSUER" format
///
/// # Returns
///
/// Tuple of (code, issuer) or error if format is invalid
fn parse_asset_identifier(asset_id: &str) -> Result<(&str, &str), StellarTransactionUtilsError> {
    asset_id
        .split_once(':')
        .ok_or_else(|| StellarTransactionUtilsError::InvalidAssetFormat(asset_id.to_string()))
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
fn validate_and_parse_issuer(
    issuer: &str,
    asset_id: &str,
) -> Result<AccountId, StellarTransactionUtilsError> {
    if issuer.is_empty() {
        return Err(StellarTransactionUtilsError::EmptyIssuerAddress(
            asset_id.to_string(),
        ));
    }

    if issuer.len() != STELLAR_ADDRESS_LENGTH {
        return Err(StellarTransactionUtilsError::InvalidIssuerLength(
            STELLAR_ADDRESS_LENGTH,
            issuer.to_string(),
        ));
    }

    if !issuer.starts_with(STELLAR_ACCOUNT_PREFIX) {
        return Err(StellarTransactionUtilsError::InvalidIssuerPrefix(
            STELLAR_ACCOUNT_PREFIX,
            issuer.to_string(),
        ));
    }

    // Validate issuer is a valid Stellar public key (not a contract address)
    parse_account_id(issuer)
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
) -> Result<u64, StellarTransactionUtilsError>
where
    P: StellarProviderTrait + Send + Sync,
{
    // Handle native XLM - accept both "native" and "XLM" for UX
    if asset_id == "native" || asset_id == "XLM" {
        let account_entry = provider
            .get_account(account_id)
            .await
            .map_err(|e| StellarTransactionUtilsError::AccountFetchFailed(e.to_string()))?;
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
) -> Result<u64, StellarTransactionUtilsError>
where
    P: StellarProviderTrait + Send + Sync,
{
    let (code, issuer) = parse_asset_identifier(asset_id)?;

    // Validate asset code length before constructing buffer
    // Stellar asset codes must be between 1 and 12 characters (inclusive)
    if code.is_empty() || code.len() > MAX_ASSET_CODE_LENGTH {
        return Err(StellarTransactionUtilsError::AssetCodeTooLong(
            MAX_ASSET_CODE_LENGTH,
            code.to_string(),
        ));
    }

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
            Asset::Native => return Err(StellarTransactionUtilsError::NativeAssetInTrustlineQuery),
        },
    });

    let resp = provider
        .get_ledger_entries(&[ledger_key])
        .await
        .map_err(|e| {
            StellarTransactionUtilsError::TrustlineQueryFailed(asset_id.into(), e.to_string())
        })?;

    let entries = resp.entries.ok_or_else(|| {
        StellarTransactionUtilsError::NoTrustlineFound(asset_id.into(), account_id.into())
    })?;

    if entries.is_empty() {
        return Err(StellarTransactionUtilsError::NoTrustlineFound(
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

        _ => Err(StellarTransactionUtilsError::UnexpectedTrustlineEntryType),
    }
}

/// Fetch balance for a Soroban contract token via ContractData
async fn get_contract_token_balance<P>(
    provider: &P,
    account_id: &str,
    contract_address: &str,
) -> Result<u64, StellarTransactionUtilsError>
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
    let error_context = format!("contract {contract_address} balance for account {account_id}");
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
                    return Err(StellarTransactionUtilsError::BalanceTooLarge(
                        parts.hi, parts.lo,
                    ));
                }
                // Check if parts.lo represents a negative value when interpreted as i64
                // Similar to the I64 branch, we check for negative before casting to u64
                let lo_as_i64 = parts.lo as i64;
                if lo_as_i64 < 0 {
                    return Err(StellarTransactionUtilsError::NegativeBalanceI128(parts.lo));
                }
                Ok(lo_as_i64 as u64)
            }
            ScVal::U64(n) => Ok(n),
            ScVal::I64(n) => {
                if n < 0 {
                    return Err(StellarTransactionUtilsError::NegativeBalanceI64(n));
                }
                Ok(n as u64)
            }
            other => Err(StellarTransactionUtilsError::UnexpectedBalanceType(
                format!("{other:?}"),
            )),
        },
        _ => Err(StellarTransactionUtilsError::UnexpectedContractDataEntryType),
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
) -> Result<StellarTokenMetadata, StellarTransactionUtilsError>
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
        return Err(StellarTransactionUtilsError::EmptyAssetCode(
            asset_id.to_string(),
        ));
    }

    if code.len() > MAX_ASSET_CODE_LENGTH {
        return Err(StellarTransactionUtilsError::AssetCodeTooLong(
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
        Ok(result) => extract_u32_from_scval(&result, "decimals() result"),
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
    let error_context = format!("contract {contract_address} decimals");
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
    extract_u32_from_scval(&val, "decimals storage value")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::transaction::stellar::test_helpers::{create_account_id, TEST_PK};
    use crate::services::provider::MockStellarProviderTrait;
    use futures::future::ready;
    use mockall::predicate::*;
    use soroban_rs::xdr::{AccountEntry, AccountEntryExt, SequenceNumber, Thresholds};
    use std::str::FromStr;

    // Helper function to create a test provider
    fn create_mock_provider() -> MockStellarProviderTrait {
        MockStellarProviderTrait::new()
    }

    // Helper function to create a mock AccountEntry
    fn create_mock_account_entry(balance: i64) -> AccountEntry {
        AccountEntry {
            account_id: create_account_id(TEST_PK),
            balance,
            seq_num: SequenceNumber(1),
            num_sub_entries: 0,
            inflation_dest: None,
            flags: 0,
            home_domain: Default::default(),
            thresholds: Thresholds([1, 0, 0, 0]),
            signers: Default::default(),
            ext: AccountEntryExt::V0,
        }
    }

    #[test]
    fn test_parse_asset_identifier_valid() {
        let result =
            parse_asset_identifier("USDC:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5");
        assert!(result.is_ok());
        let (code, issuer) = result.unwrap();
        assert_eq!(code, "USDC");
        assert_eq!(
            issuer,
            "GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5"
        );
    }

    #[test]
    fn test_parse_asset_identifier_invalid() {
        // Missing colon
        let result =
            parse_asset_identifier("USDCGBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5");
        assert!(result.is_err());
        match result.unwrap_err() {
            StellarTransactionUtilsError::InvalidAssetFormat(_) => {}
            e => panic!("Expected InvalidAssetFormat, got: {:?}", e),
        }

        // Empty string
        let result = parse_asset_identifier("");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_asset_identifier_multiple_colons() {
        // Multiple colons - only first is used
        let result = parse_asset_identifier(
            "USD:C:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5",
        );
        assert!(result.is_ok());
        let (code, issuer) = result.unwrap();
        assert_eq!(code, "USD");
        assert_eq!(
            issuer,
            "C:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5"
        );
    }

    #[test]
    fn test_validate_and_parse_issuer_valid() {
        let issuer = "GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5";
        let result = validate_and_parse_issuer(issuer, "USDC:GBBD47...");
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_and_parse_issuer_empty() {
        let result = validate_and_parse_issuer("", "USDC:");
        assert!(result.is_err());
        match result.unwrap_err() {
            StellarTransactionUtilsError::EmptyIssuerAddress(_) => {}
            e => panic!("Expected EmptyIssuerAddress, got: {:?}", e),
        }
    }

    #[test]
    fn test_validate_and_parse_issuer_wrong_length() {
        let result = validate_and_parse_issuer("SHORTADDR", "USDC:SHORTADDR");
        assert!(result.is_err());
        match result.unwrap_err() {
            StellarTransactionUtilsError::InvalidIssuerLength(expected, _) => {
                assert_eq!(expected, STELLAR_ADDRESS_LENGTH);
            }
            e => panic!("Expected InvalidIssuerLength, got: {:?}", e),
        }
    }

    #[test]
    fn test_validate_and_parse_issuer_wrong_prefix() {
        // Contract address (starts with 'C') is not valid as issuer
        let contract_addr = "CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA";
        let result = validate_and_parse_issuer(contract_addr, "USDC:C...");
        assert!(result.is_err());
        match result.unwrap_err() {
            StellarTransactionUtilsError::InvalidIssuerPrefix(expected, _) => {
                assert_eq!(expected, STELLAR_ACCOUNT_PREFIX);
            }
            e => panic!("Expected InvalidIssuerPrefix, got: {:?}", e),
        }
    }

    #[test]
    fn test_validate_and_parse_issuer_invalid_checksum() {
        // Valid length and prefix but invalid checksum
        let bad_issuer = "GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA6"; // Changed last char
        let result = validate_and_parse_issuer(bad_issuer, "USDC:G...");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_get_token_balance_native_xlm() {
        let mut provider = create_mock_provider();

        let test_balance = 100_0000000i64; // 100 XLM
        let account_entry = create_mock_account_entry(test_balance);

        provider
            .expect_get_account()
            .with(eq(TEST_PK))
            .times(1)
            .returning(move |_| Box::pin(ready(Ok(account_entry.clone()))));

        let result = get_token_balance(&provider, TEST_PK, "native").await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), test_balance as u64);
    }

    #[tokio::test]
    async fn test_get_token_balance_xlm_identifier() {
        let mut provider = create_mock_provider();

        let test_balance = 50_0000000i64; // 50 XLM
        let account_entry = create_mock_account_entry(test_balance);

        provider
            .expect_get_account()
            .with(eq(TEST_PK))
            .times(1)
            .returning(move |_| Box::pin(ready(Ok(account_entry.clone()))));

        // Test with "XLM" identifier
        let result = get_token_balance(&provider, TEST_PK, "XLM").await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), test_balance as u64);
    }

    #[test]
    fn test_asset_code_length_validation() {
        // Valid codes (1-12 characters)
        assert!(parse_asset_identifier(
            "A:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5"
        )
        .is_ok());
        assert!(parse_asset_identifier(
            "USDC:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5"
        )
        .is_ok());
        assert!(parse_asset_identifier(
            "MAXLENCODE12:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5"
        )
        .is_ok());

        // Empty code - parsed successfully, but should fail in validation
        let (code, _) =
            parse_asset_identifier(":GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5")
                .unwrap();
        assert_eq!(code, "");
    }

    #[tokio::test]
    async fn test_get_token_metadata_native() {
        let provider = create_mock_provider();

        let result = get_token_metadata(&provider, "native").await;
        assert!(result.is_ok());
        let metadata = result.unwrap();

        assert_eq!(metadata.kind, StellarTokenKind::Native);
        assert_eq!(metadata.decimals, DEFAULT_STELLAR_DECIMALS);
        assert_eq!(metadata.canonical_asset_id, "native");
    }

    #[tokio::test]
    async fn test_get_token_metadata_xlm_identifier() {
        let provider = create_mock_provider();

        let result = get_token_metadata(&provider, "XLM").await;
        assert!(result.is_ok());
        let metadata = result.unwrap();

        assert_eq!(metadata.kind, StellarTokenKind::Native);
        assert_eq!(metadata.decimals, DEFAULT_STELLAR_DECIMALS);
        assert_eq!(metadata.canonical_asset_id, "native");
    }

    #[tokio::test]
    async fn test_get_token_metadata_empty_string() {
        let provider = create_mock_provider();

        // Empty string should be treated as native XLM
        let result = get_token_metadata(&provider, "").await;
        assert!(result.is_ok());
        let metadata = result.unwrap();

        assert_eq!(metadata.kind, StellarTokenKind::Native);
        assert_eq!(metadata.decimals, DEFAULT_STELLAR_DECIMALS);
        assert_eq!(metadata.canonical_asset_id, "native");
    }

    #[tokio::test]
    async fn test_get_token_metadata_classic_asset() {
        let provider = create_mock_provider();

        let asset_id = "USDC:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5";
        let result = get_token_metadata(&provider, asset_id).await;
        assert!(result.is_ok());
        let metadata = result.unwrap();

        match metadata.kind {
            StellarTokenKind::Classic { code, issuer } => {
                assert_eq!(code, "USDC");
                assert_eq!(
                    issuer,
                    "GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5"
                );
            }
            _ => panic!("Expected Classic token kind"),
        }
        assert_eq!(metadata.decimals, DEFAULT_STELLAR_DECIMALS);
        assert_eq!(metadata.canonical_asset_id, asset_id);
    }

    #[tokio::test]
    async fn test_get_token_metadata_classic_asset_credit12() {
        let provider = create_mock_provider();

        let asset_id = "LONGASSETCD:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5";
        let result = get_token_metadata(&provider, asset_id).await;
        assert!(result.is_ok());
        let metadata = result.unwrap();

        match metadata.kind {
            StellarTokenKind::Classic { code, issuer } => {
                assert_eq!(code, "LONGASSETCD");
                assert_eq!(
                    issuer,
                    "GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5"
                );
            }
            _ => panic!("Expected Classic token kind"),
        }
        assert_eq!(metadata.decimals, DEFAULT_STELLAR_DECIMALS);
    }

    #[tokio::test]
    async fn test_get_token_metadata_invalid_format() {
        let provider = create_mock_provider();

        let result = get_token_metadata(&provider, "INVALID_NO_COLON").await;
        assert!(result.is_err());
        match result.unwrap_err() {
            StellarTransactionUtilsError::InvalidAssetFormat(_) => {}
            e => panic!("Expected InvalidAssetFormat, got: {:?}", e),
        }
    }

    #[tokio::test]
    async fn test_get_token_metadata_empty_code() {
        let provider = create_mock_provider();

        let result = get_token_metadata(
            &provider,
            ":GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5",
        )
        .await;
        assert!(result.is_err());
        match result.unwrap_err() {
            StellarTransactionUtilsError::EmptyAssetCode(_) => {}
            e => panic!("Expected EmptyAssetCode, got: {:?}", e),
        }
    }

    #[tokio::test]
    async fn test_get_token_metadata_code_too_long() {
        let provider = create_mock_provider();

        let result = get_token_metadata(
            &provider,
            "VERYLONGASSETCODE:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5",
        )
        .await;
        assert!(result.is_err());
        match result.unwrap_err() {
            StellarTransactionUtilsError::AssetCodeTooLong(max, code) => {
                assert_eq!(max, MAX_ASSET_CODE_LENGTH);
                assert_eq!(code, "VERYLONGASSETCODE");
            }
            e => panic!("Expected AssetCodeTooLong, got: {:?}", e),
        }
    }

    #[tokio::test]
    async fn test_get_token_metadata_empty_issuer() {
        let provider = create_mock_provider();

        let result = get_token_metadata(&provider, "USDC:").await;
        assert!(result.is_err());
        match result.unwrap_err() {
            StellarTransactionUtilsError::EmptyIssuerAddress(_) => {}
            e => panic!("Expected EmptyIssuerAddress, got: {:?}", e),
        }
    }

    #[tokio::test]
    async fn test_get_token_metadata_invalid_issuer_length() {
        let provider = create_mock_provider();

        let result = get_token_metadata(&provider, "USDC:INVALID").await;
        assert!(result.is_err());
        match result.unwrap_err() {
            StellarTransactionUtilsError::InvalidIssuerLength(expected, _) => {
                assert_eq!(expected, STELLAR_ADDRESS_LENGTH);
            }
            e => panic!("Expected InvalidIssuerLength, got: {:?}", e),
        }
    }

    #[tokio::test]
    async fn test_get_token_metadata_invalid_issuer_prefix() {
        let provider = create_mock_provider();

        // Using contract address as issuer (starts with C, not G)
        let result = get_token_metadata(
            &provider,
            "USDC:CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA",
        )
        .await;
        assert!(result.is_err());
        match result.unwrap_err() {
            StellarTransactionUtilsError::InvalidIssuerPrefix(expected, _) => {
                assert_eq!(expected, STELLAR_ACCOUNT_PREFIX);
            }
            e => panic!("Expected InvalidIssuerPrefix, got: {:?}", e),
        }
    }

    #[tokio::test]
    async fn test_get_token_metadata_contract_valid() {
        let mut provider = create_mock_provider();

        // Mock contract decimals query to return None (uses default)
        provider.expect_call_contract().returning(|_, _, _| {
            Box::pin(ready(Err(crate::services::provider::ProviderError::Other(
                "Contract call failed".to_string(),
            ))))
        });

        provider.expect_get_ledger_entries().returning(|_| {
            Box::pin(ready(Ok(
                soroban_rs::stellar_rpc_client::GetLedgerEntriesResponse {
                    entries: None,
                    latest_ledger: 0,
                },
            )))
        });

        let contract_addr = "CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA";
        let result = get_token_metadata(&provider, contract_addr).await;
        assert!(result.is_ok());
        let metadata = result.unwrap();

        match metadata.kind {
            StellarTokenKind::Contract { contract_id } => {
                assert_eq!(contract_id, contract_addr);
            }
            _ => panic!("Expected Contract token kind"),
        }
        assert_eq!(metadata.decimals, DEFAULT_STELLAR_DECIMALS);
        assert_eq!(metadata.canonical_asset_id, contract_addr.to_uppercase());
    }

    #[tokio::test]
    async fn test_get_token_balance_trustline_v0_success() {
        use soroban_rs::stellar_rpc_client::{GetLedgerEntriesResponse, LedgerEntryResult};
        use soroban_rs::xdr::{
            LedgerEntry, LedgerEntryData, LedgerEntryExt, TrustLineAsset, WriteXdr,
        };

        let mut provider = create_mock_provider();

        // Mock trustline response with V0 extension (no liabilities)
        provider.expect_get_ledger_entries().returning(|_| {
            let trustline_entry = TrustLineEntry {
                account_id: create_account_id(TEST_PK),
                asset: TrustLineAsset::CreditAlphanum4(AlphaNum4 {
                    asset_code: AssetCode4(*b"USDC"),
                    issuer: create_account_id(
                        "GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5",
                    ),
                }),
                balance: 10_0000000, // 10 USDC
                limit: 1000_0000000,
                flags: 1,
                ext: TrustLineEntryExt::V0,
            };

            let ledger_entry = LedgerEntry {
                last_modified_ledger_seq: 0,
                data: LedgerEntryData::Trustline(trustline_entry),
                ext: LedgerEntryExt::V0,
            };

            let xdr_base64 = ledger_entry
                .data
                .to_xdr_base64(soroban_rs::xdr::Limits::none())
                .unwrap();

            Box::pin(ready(Ok(GetLedgerEntriesResponse {
                entries: Some(vec![LedgerEntryResult {
                    key: String::new(),
                    xdr: xdr_base64,
                    last_modified_ledger: 0,
                    live_until_ledger_seq_ledger_seq: None,
                }]),
                latest_ledger: 0,
            })))
        });

        let account = TEST_PK;
        let asset = "USDC:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5";

        let result = get_token_balance(&provider, account, asset).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 10_0000000);
    }

    #[tokio::test]
    async fn test_get_token_balance_trustline_v1_with_liabilities() {
        use soroban_rs::stellar_rpc_client::{GetLedgerEntriesResponse, LedgerEntryResult};
        use soroban_rs::xdr::{
            LedgerEntry, LedgerEntryData, LedgerEntryExt, Liabilities, TrustLineAsset,
            TrustLineEntryV1, TrustLineEntryV1Ext, WriteXdr,
        };

        let mut provider = create_mock_provider();

        // Mock trustline response with V1 extension (with liabilities)
        provider.expect_get_ledger_entries().returning(|_| {
            let trustline_entry = TrustLineEntry {
                account_id: create_account_id(TEST_PK),
                asset: TrustLineAsset::CreditAlphanum4(AlphaNum4 {
                    asset_code: AssetCode4(*b"USDC"),
                    issuer: create_account_id(
                        "GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5",
                    ),
                }),
                balance: 10_0000000, // 10 USDC
                limit: 1000_0000000,
                flags: 1,
                ext: TrustLineEntryExt::V1(TrustLineEntryV1 {
                    liabilities: Liabilities {
                        buying: 1_0000000,  // 1 USDC buying liability
                        selling: 2_0000000, // 2 USDC selling liability
                    },
                    ext: TrustLineEntryV1Ext::V0,
                }),
            };

            let ledger_entry = LedgerEntry {
                last_modified_ledger_seq: 0,
                data: LedgerEntryData::Trustline(trustline_entry),
                ext: LedgerEntryExt::V0,
            };

            let xdr_base64 = ledger_entry
                .data
                .to_xdr_base64(soroban_rs::xdr::Limits::none())
                .unwrap();

            Box::pin(ready(Ok(GetLedgerEntriesResponse {
                entries: Some(vec![LedgerEntryResult {
                    key: String::new(),
                    xdr: xdr_base64,
                    last_modified_ledger: 0,
                    live_until_ledger_seq_ledger_seq: None,
                }]),
                latest_ledger: 0,
            })))
        });

        let account = TEST_PK;
        let asset = "USDC:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5";

        let result = get_token_balance(&provider, account, asset).await;
        assert!(result.is_ok());
        // Available balance = 10 - 2 (selling liabilities) = 8 USDC
        assert_eq!(result.unwrap(), 8_0000000);
    }

    #[tokio::test]
    async fn test_get_token_balance_trustline_v1_selling_exceeds_balance() {
        use soroban_rs::stellar_rpc_client::{GetLedgerEntriesResponse, LedgerEntryResult};
        use soroban_rs::xdr::{
            LedgerEntry, LedgerEntryData, LedgerEntryExt, Liabilities, TrustLineAsset,
            TrustLineEntryV1, TrustLineEntryV1Ext, WriteXdr,
        };

        let mut provider = create_mock_provider();

        // Mock trustline where selling liabilities exceed balance (edge case)
        provider.expect_get_ledger_entries().returning(|_| {
            let trustline_entry = TrustLineEntry {
                account_id: create_account_id(TEST_PK),
                asset: TrustLineAsset::CreditAlphanum4(AlphaNum4 {
                    asset_code: AssetCode4(*b"USDC"),
                    issuer: create_account_id(
                        "GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5",
                    ),
                }),
                balance: 5_0000000, // 5 USDC
                limit: 1000_0000000,
                flags: 1,
                ext: TrustLineEntryExt::V1(TrustLineEntryV1 {
                    liabilities: Liabilities {
                        buying: 0,
                        selling: 10_0000000, // 10 USDC selling (more than balance)
                    },
                    ext: TrustLineEntryV1Ext::V0,
                }),
            };

            let ledger_entry = LedgerEntry {
                last_modified_ledger_seq: 0,
                data: LedgerEntryData::Trustline(trustline_entry),
                ext: LedgerEntryExt::V0,
            };

            let xdr_base64 = ledger_entry
                .data
                .to_xdr_base64(soroban_rs::xdr::Limits::none())
                .unwrap();

            Box::pin(ready(Ok(GetLedgerEntriesResponse {
                entries: Some(vec![LedgerEntryResult {
                    key: String::new(),
                    xdr: xdr_base64,
                    last_modified_ledger: 0,
                    live_until_ledger_seq_ledger_seq: None,
                }]),
                latest_ledger: 0,
            })))
        });

        let account = TEST_PK;
        let asset = "USDC:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5";

        let result = get_token_balance(&provider, account, asset).await;
        assert!(result.is_ok());
        // saturating_sub should return 0 when selling exceeds balance
        assert_eq!(result.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_get_token_balance_trustline_not_found() {
        use soroban_rs::stellar_rpc_client::GetLedgerEntriesResponse;

        let mut provider = create_mock_provider();

        // Mock empty response (no trustline)
        provider.expect_get_ledger_entries().returning(|_| {
            Box::pin(ready(Ok(GetLedgerEntriesResponse {
                entries: None,
                latest_ledger: 0,
            })))
        });

        let account = TEST_PK;
        let asset = "USDC:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5";

        let result = get_token_balance(&provider, account, asset).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            StellarTransactionUtilsError::NoTrustlineFound(asset_id, account_id) => {
                assert_eq!(asset_id, asset);
                assert_eq!(account_id, account);
            }
            e => panic!("Expected NoTrustlineFound, got: {:?}", e),
        }
    }

    #[tokio::test]
    async fn test_get_token_balance_trustline_empty_entries() {
        use soroban_rs::stellar_rpc_client::GetLedgerEntriesResponse;

        let mut provider = create_mock_provider();

        // Mock response with empty entries vec
        provider.expect_get_ledger_entries().returning(|_| {
            Box::pin(ready(Ok(GetLedgerEntriesResponse {
                entries: Some(vec![]),
                latest_ledger: 0,
            })))
        });

        let account = TEST_PK;
        let asset = "USDC:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5";

        let result = get_token_balance(&provider, account, asset).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            StellarTransactionUtilsError::NoTrustlineFound(asset_id, account_id) => {
                assert_eq!(asset_id, asset);
                assert_eq!(account_id, account);
            }
            e => panic!("Expected NoTrustlineFound, got: {:?}", e),
        }
    }

    #[tokio::test]
    async fn test_get_token_balance_trustline_credit12() {
        use soroban_rs::stellar_rpc_client::{GetLedgerEntriesResponse, LedgerEntryResult};
        use soroban_rs::xdr::{
            LedgerEntry, LedgerEntryData, LedgerEntryExt, TrustLineAsset, WriteXdr,
        };

        let mut provider = create_mock_provider();

        // Mock trustline for Credit12 asset
        provider.expect_get_ledger_entries().returning(|_| {
            let trustline_entry = TrustLineEntry {
                account_id: create_account_id(TEST_PK),
                asset: TrustLineAsset::CreditAlphanum12(AlphaNum12 {
                    asset_code: AssetCode12(*b"LONGASSETCD\0"),
                    issuer: create_account_id(
                        "GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5",
                    ),
                }),
                balance: 25_0000000, // 25 units
                limit: 1000_0000000,
                flags: 1,
                ext: TrustLineEntryExt::V0,
            };

            let ledger_entry = LedgerEntry {
                last_modified_ledger_seq: 0,
                data: LedgerEntryData::Trustline(trustline_entry),
                ext: LedgerEntryExt::V0,
            };

            let xdr_base64 = ledger_entry
                .data
                .to_xdr_base64(soroban_rs::xdr::Limits::none())
                .unwrap();

            Box::pin(ready(Ok(GetLedgerEntriesResponse {
                entries: Some(vec![LedgerEntryResult {
                    key: String::new(),
                    xdr: xdr_base64,
                    last_modified_ledger: 0,
                    live_until_ledger_seq_ledger_seq: None,
                }]),
                latest_ledger: 0,
            })))
        });

        let account = TEST_PK;
        let asset = "LONGASSETCD:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5";

        let result = get_token_balance(&provider, account, asset).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 25_0000000);
    }

    #[tokio::test]
    async fn test_get_token_balance_trustline_invalid_asset_code_too_long() {
        let provider = create_mock_provider();

        let account = TEST_PK;
        let asset = "VERYLONGASSETCODE:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5";

        let result = get_token_balance(&provider, account, asset).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            StellarTransactionUtilsError::AssetCodeTooLong(max, code) => {
                assert_eq!(max, MAX_ASSET_CODE_LENGTH);
                assert_eq!(code, "VERYLONGASSETCODE");
            }
            e => panic!("Expected AssetCodeTooLong, got: {:?}", e),
        }
    }

    #[test]
    fn test_constants() {
        // Verify constants are set correctly
        assert_eq!(STELLAR_ADDRESS_LENGTH, 56);
        assert_eq!(MAX_ASSET_CODE_LENGTH, 12);
        assert_eq!(DEFAULT_STELLAR_DECIMALS, 7);
        assert_eq!(STELLAR_ACCOUNT_PREFIX, 'G');
    }

    #[test]
    fn test_contract_id_validation() {
        // Valid contract address
        let valid_contract = "CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA";
        assert!(ContractId::from_str(valid_contract).is_ok());

        // Invalid contract address (not a contract, it's an account)
        let account = "GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5";
        assert!(ContractId::from_str(account).is_err());

        // Invalid format
        assert!(ContractId::from_str("INVALID").is_err());
        assert!(ContractId::from_str("").is_err());
    }

    #[test]
    fn test_asset_identifier_edge_cases() {
        // Whitespace handling
        let result =
            parse_asset_identifier("USD :GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5");
        assert!(result.is_ok());
        let (code, _) = result.unwrap();
        assert_eq!(code, "USD "); // Whitespace preserved

        // Unicode characters (if supported)
        let result =
            parse_asset_identifier("U$D:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5");
        assert!(result.is_ok());
        let (code, _) = result.unwrap();
        assert_eq!(code, "U$D");
    }

    #[tokio::test]
    async fn test_get_token_balance_contract_token_no_balance_entry() {
        let mut provider = create_mock_provider();

        // Mock empty response (no balance entry)
        provider.expect_get_ledger_entries().returning(|_| {
            Box::pin(ready(Ok(
                soroban_rs::stellar_rpc_client::GetLedgerEntriesResponse {
                    entries: None,
                    latest_ledger: 0,
                },
            )))
        });

        let contract_addr = "CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA";
        let account = TEST_PK;

        let result = get_token_balance(&provider, account, contract_addr).await;

        // Should return 0 for non-existent balance
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_get_token_balance_contract_token_i128_balance() {
        use soroban_rs::stellar_rpc_client::{GetLedgerEntriesResponse, LedgerEntryResult};
        use soroban_rs::xdr::{
            ContractDataDurability, ContractDataEntry, ExtensionPoint, Int128Parts, LedgerEntry,
            LedgerEntryData, LedgerEntryExt, ScVal, WriteXdr,
        };

        let mut provider = create_mock_provider();

        // Mock response with I128 balance
        provider.expect_get_ledger_entries().returning(|_| {
            let balance_val = ScVal::I128(Int128Parts { hi: 0, lo: 1000000 });

            let contract_data = ContractDataEntry {
                ext: ExtensionPoint::V0,
                contract: ScAddress::Contract(ContractId(Hash([0u8; 32]))),
                key: ScVal::Vec(None),
                durability: ContractDataDurability::Persistent,
                val: balance_val,
            };

            let ledger_entry = LedgerEntry {
                last_modified_ledger_seq: 0,
                data: LedgerEntryData::ContractData(contract_data),
                ext: LedgerEntryExt::V0,
            };

            let xdr_base64 = ledger_entry
                .data
                .to_xdr_base64(soroban_rs::xdr::Limits::none())
                .unwrap();

            Box::pin(ready(Ok(GetLedgerEntriesResponse {
                entries: Some(vec![LedgerEntryResult {
                    key: String::new(),
                    xdr: xdr_base64,
                    last_modified_ledger: 0,
                    live_until_ledger_seq_ledger_seq: None,
                }]),
                latest_ledger: 0,
            })))
        });

        let contract_addr = "CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA";
        let account = TEST_PK;

        let result = get_token_balance(&provider, account, contract_addr).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 1000000);
    }

    #[tokio::test]
    async fn test_get_token_balance_contract_token_i128_balance_too_large() {
        use soroban_rs::stellar_rpc_client::{GetLedgerEntriesResponse, LedgerEntryResult};
        use soroban_rs::xdr::{
            ContractDataDurability, ContractDataEntry, ExtensionPoint, Int128Parts, LedgerEntry,
            LedgerEntryData, LedgerEntryExt, ScVal, WriteXdr,
        };

        let mut provider = create_mock_provider();

        // Mock response with I128 balance where hi != 0
        provider.expect_get_ledger_entries().returning(|_| {
            let balance_val = ScVal::I128(Int128Parts {
                hi: 1, // Non-zero hi means balance is too large
                lo: 1000000,
            });

            let contract_data = ContractDataEntry {
                ext: ExtensionPoint::V0,
                contract: ScAddress::Contract(ContractId(Hash([0u8; 32]))),
                key: ScVal::Vec(None),
                durability: ContractDataDurability::Persistent,
                val: balance_val,
            };

            let ledger_entry = LedgerEntry {
                last_modified_ledger_seq: 0,
                data: LedgerEntryData::ContractData(contract_data),
                ext: LedgerEntryExt::V0,
            };

            let xdr_base64 = ledger_entry
                .data
                .to_xdr_base64(soroban_rs::xdr::Limits::none())
                .unwrap();

            Box::pin(ready(Ok(GetLedgerEntriesResponse {
                entries: Some(vec![LedgerEntryResult {
                    key: String::new(),
                    xdr: xdr_base64,
                    last_modified_ledger: 0,
                    live_until_ledger_seq_ledger_seq: None,
                }]),
                latest_ledger: 0,
            })))
        });

        let contract_addr = "CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA";
        let account = TEST_PK;

        let result = get_token_balance(&provider, account, contract_addr).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            StellarTransactionUtilsError::BalanceTooLarge(hi, lo) => {
                assert_eq!(hi, 1);
                assert_eq!(lo, 1000000);
            }
            e => panic!("Expected BalanceTooLarge, got: {:?}", e),
        }
    }

    #[tokio::test]
    async fn test_get_token_balance_contract_token_i128_negative() {
        use soroban_rs::stellar_rpc_client::{GetLedgerEntriesResponse, LedgerEntryResult};
        use soroban_rs::xdr::{
            ContractDataDurability, ContractDataEntry, ExtensionPoint, Int128Parts, LedgerEntry,
            LedgerEntryData, LedgerEntryExt, ScVal, WriteXdr,
        };

        let mut provider = create_mock_provider();

        // Mock response with negative I128 balance
        provider.expect_get_ledger_entries().returning(|_| {
            let balance_val = ScVal::I128(Int128Parts {
                hi: 0,
                lo: u64::MAX, // When cast to i64, this is negative
            });

            let contract_data = ContractDataEntry {
                ext: ExtensionPoint::V0,
                contract: ScAddress::Contract(ContractId(Hash([0u8; 32]))),
                key: ScVal::Vec(None),
                durability: ContractDataDurability::Persistent,
                val: balance_val,
            };

            let ledger_entry = LedgerEntry {
                last_modified_ledger_seq: 0,
                data: LedgerEntryData::ContractData(contract_data),
                ext: LedgerEntryExt::V0,
            };

            let xdr_base64 = ledger_entry
                .data
                .to_xdr_base64(soroban_rs::xdr::Limits::none())
                .unwrap();

            Box::pin(ready(Ok(GetLedgerEntriesResponse {
                entries: Some(vec![LedgerEntryResult {
                    key: String::new(),
                    xdr: xdr_base64,
                    last_modified_ledger: 0,
                    live_until_ledger_seq_ledger_seq: None,
                }]),
                latest_ledger: 0,
            })))
        });

        let contract_addr = "CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA";
        let account = TEST_PK;

        let result = get_token_balance(&provider, account, contract_addr).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            StellarTransactionUtilsError::NegativeBalanceI128(_) => {}
            e => panic!("Expected NegativeBalanceI128, got: {:?}", e),
        }
    }

    #[tokio::test]
    async fn test_get_token_balance_contract_token_u64_balance() {
        use soroban_rs::stellar_rpc_client::{GetLedgerEntriesResponse, LedgerEntryResult};
        use soroban_rs::xdr::{
            ContractDataDurability, ContractDataEntry, ExtensionPoint, LedgerEntry,
            LedgerEntryData, LedgerEntryExt, ScVal, WriteXdr,
        };

        let mut provider = create_mock_provider();

        // Mock response with U64 balance
        provider.expect_get_ledger_entries().returning(|_| {
            let balance_val = ScVal::U64(5000000);

            let contract_data = ContractDataEntry {
                ext: ExtensionPoint::V0,
                contract: ScAddress::Contract(ContractId(Hash([0u8; 32]))),
                key: ScVal::Vec(None),
                durability: ContractDataDurability::Persistent,
                val: balance_val,
            };

            let ledger_entry = LedgerEntry {
                last_modified_ledger_seq: 0,
                data: LedgerEntryData::ContractData(contract_data),
                ext: LedgerEntryExt::V0,
            };

            let xdr_base64 = ledger_entry
                .data
                .to_xdr_base64(soroban_rs::xdr::Limits::none())
                .unwrap();

            Box::pin(ready(Ok(GetLedgerEntriesResponse {
                entries: Some(vec![LedgerEntryResult {
                    key: String::new(),
                    xdr: xdr_base64,
                    last_modified_ledger: 0,
                    live_until_ledger_seq_ledger_seq: None,
                }]),
                latest_ledger: 0,
            })))
        });

        let contract_addr = "CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA";
        let account = TEST_PK;

        let result = get_token_balance(&provider, account, contract_addr).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 5000000);
    }

    #[tokio::test]
    async fn test_get_token_balance_contract_token_i64_positive() {
        use soroban_rs::stellar_rpc_client::{GetLedgerEntriesResponse, LedgerEntryResult};
        use soroban_rs::xdr::{
            ContractDataDurability, ContractDataEntry, ExtensionPoint, LedgerEntry,
            LedgerEntryData, LedgerEntryExt, ScVal, WriteXdr,
        };

        let mut provider = create_mock_provider();

        // Mock response with positive I64 balance
        provider.expect_get_ledger_entries().returning(|_| {
            let balance_val = ScVal::I64(3000000);

            let contract_data = ContractDataEntry {
                ext: ExtensionPoint::V0,
                contract: ScAddress::Contract(ContractId(Hash([0u8; 32]))),
                key: ScVal::Vec(None),
                durability: ContractDataDurability::Persistent,
                val: balance_val,
            };

            let ledger_entry = LedgerEntry {
                last_modified_ledger_seq: 0,
                data: LedgerEntryData::ContractData(contract_data),
                ext: LedgerEntryExt::V0,
            };

            let xdr_base64 = ledger_entry
                .data
                .to_xdr_base64(soroban_rs::xdr::Limits::none())
                .unwrap();

            Box::pin(ready(Ok(GetLedgerEntriesResponse {
                entries: Some(vec![LedgerEntryResult {
                    key: String::new(),
                    xdr: xdr_base64,
                    last_modified_ledger: 0,
                    live_until_ledger_seq_ledger_seq: None,
                }]),
                latest_ledger: 0,
            })))
        });

        let contract_addr = "CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA";
        let account = TEST_PK;

        let result = get_token_balance(&provider, account, contract_addr).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 3000000);
    }

    #[tokio::test]
    async fn test_get_token_balance_contract_token_i64_negative() {
        use soroban_rs::stellar_rpc_client::{GetLedgerEntriesResponse, LedgerEntryResult};
        use soroban_rs::xdr::{
            ContractDataDurability, ContractDataEntry, ExtensionPoint, LedgerEntry,
            LedgerEntryData, LedgerEntryExt, ScVal, WriteXdr,
        };

        let mut provider = create_mock_provider();

        // Mock response with negative I64 balance
        provider.expect_get_ledger_entries().returning(|_| {
            let balance_val = ScVal::I64(-1000);

            let contract_data = ContractDataEntry {
                ext: ExtensionPoint::V0,
                contract: ScAddress::Contract(ContractId(Hash([0u8; 32]))),
                key: ScVal::Vec(None),
                durability: ContractDataDurability::Persistent,
                val: balance_val,
            };

            let ledger_entry = LedgerEntry {
                last_modified_ledger_seq: 0,
                data: LedgerEntryData::ContractData(contract_data),
                ext: LedgerEntryExt::V0,
            };

            let xdr_base64 = ledger_entry
                .data
                .to_xdr_base64(soroban_rs::xdr::Limits::none())
                .unwrap();

            Box::pin(ready(Ok(GetLedgerEntriesResponse {
                entries: Some(vec![LedgerEntryResult {
                    key: String::new(),
                    xdr: xdr_base64,
                    last_modified_ledger: 0,
                    live_until_ledger_seq_ledger_seq: None,
                }]),
                latest_ledger: 0,
            })))
        });

        let contract_addr = "CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA";
        let account = TEST_PK;

        let result = get_token_balance(&provider, account, contract_addr).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            StellarTransactionUtilsError::NegativeBalanceI64(n) => {
                assert_eq!(n, -1000);
            }
            e => panic!("Expected NegativeBalanceI64, got: {:?}", e),
        }
    }

    #[tokio::test]
    async fn test_get_token_balance_contract_token_unexpected_balance_type() {
        use soroban_rs::stellar_rpc_client::{GetLedgerEntriesResponse, LedgerEntryResult};
        use soroban_rs::xdr::{
            ContractDataDurability, ContractDataEntry, ExtensionPoint, LedgerEntry,
            LedgerEntryData, LedgerEntryExt, ScVal, WriteXdr,
        };

        let mut provider = create_mock_provider();

        // Mock response with unexpected balance type (Bool)
        provider.expect_get_ledger_entries().returning(|_| {
            let balance_val = ScVal::Bool(true);

            let contract_data = ContractDataEntry {
                ext: ExtensionPoint::V0,
                contract: ScAddress::Contract(ContractId(Hash([0u8; 32]))),
                key: ScVal::Vec(None),
                durability: ContractDataDurability::Persistent,
                val: balance_val,
            };

            let ledger_entry = LedgerEntry {
                last_modified_ledger_seq: 0,
                data: LedgerEntryData::ContractData(contract_data),
                ext: LedgerEntryExt::V0,
            };

            let xdr_base64 = ledger_entry
                .data
                .to_xdr_base64(soroban_rs::xdr::Limits::none())
                .unwrap();

            Box::pin(ready(Ok(GetLedgerEntriesResponse {
                entries: Some(vec![LedgerEntryResult {
                    key: String::new(),
                    xdr: xdr_base64,
                    last_modified_ledger: 0,
                    live_until_ledger_seq_ledger_seq: None,
                }]),
                latest_ledger: 0,
            })))
        });

        let contract_addr = "CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA";
        let account = TEST_PK;

        let result = get_token_balance(&provider, account, contract_addr).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            StellarTransactionUtilsError::UnexpectedBalanceType(_) => {}
            e => panic!("Expected UnexpectedBalanceType, got: {:?}", e),
        }
    }

    #[test]
    fn test_asset_code_boundary_cases() {
        // Exactly 4 characters (Credit4)
        let (code, _) =
            parse_asset_identifier("ABCD:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5")
                .unwrap();
        assert_eq!(code, "ABCD");
        assert!(code.len() <= 4);

        // Exactly 5 characters (Credit12)
        let (code, _) = parse_asset_identifier(
            "ABCDE:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5",
        )
        .unwrap();
        assert_eq!(code, "ABCDE");
        assert!(code.len() > 4 && code.len() <= 12);

        // Exactly 12 characters (Credit12 max)
        let (code, _) = parse_asset_identifier(
            "ABCDEFGHIJKL:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5",
        )
        .unwrap();
        assert_eq!(code, "ABCDEFGHIJKL");
        assert_eq!(code.len(), 12);

        // 13 characters (too long) - parsing succeeds, but validation should fail
        let (code, _) = parse_asset_identifier(
            "ABCDEFGHIJKLM:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5",
        )
        .unwrap();
        assert_eq!(code, "ABCDEFGHIJKLM");
        assert!(code.len() > MAX_ASSET_CODE_LENGTH);
    }

    #[tokio::test]
    async fn test_get_token_metadata_contract_with_decimals() {
        let mut provider = create_mock_provider();

        // Mock successful decimals query
        let decimals_value = ScVal::U32(6);
        provider
            .expect_call_contract()
            .returning(move |_, _, _| Box::pin(ready(Ok(decimals_value.clone()))));

        let contract_addr = "CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA";
        let result = get_token_metadata(&provider, contract_addr).await;
        assert!(result.is_ok());
        let metadata = result.unwrap();

        assert_eq!(metadata.decimals, 6);
        match metadata.kind {
            StellarTokenKind::Contract { contract_id } => {
                assert_eq!(contract_id, contract_addr);
            }
            _ => panic!("Expected Contract token kind"),
        }
    }

    #[tokio::test]
    async fn test_get_contract_token_decimals_from_storage_fallback() {
        use soroban_rs::stellar_rpc_client::{GetLedgerEntriesResponse, LedgerEntryResult};
        use soroban_rs::xdr::{
            ContractDataDurability, ContractDataEntry, ExtensionPoint, LedgerEntry,
            LedgerEntryData, LedgerEntryExt, ScVal, WriteXdr,
        };

        let mut provider = create_mock_provider();

        // Mock failed contract invocation
        provider.expect_call_contract().returning(|_, _, _| {
            Box::pin(ready(Err(crate::services::provider::ProviderError::Other(
                "Contract call failed".to_string(),
            ))))
        });

        // Mock successful storage query with decimals = 8
        provider.expect_get_ledger_entries().returning(|_| {
            let decimals_val = ScVal::U32(8);

            let contract_data = ContractDataEntry {
                ext: ExtensionPoint::V0,
                contract: ScAddress::Contract(ContractId(Hash([0u8; 32]))),
                key: ScVal::Vec(None),
                durability: ContractDataDurability::Persistent,
                val: decimals_val,
            };

            let ledger_entry = LedgerEntry {
                last_modified_ledger_seq: 0,
                data: LedgerEntryData::ContractData(contract_data),
                ext: LedgerEntryExt::V0,
            };

            let xdr_base64 = ledger_entry
                .data
                .to_xdr_base64(soroban_rs::xdr::Limits::none())
                .unwrap();

            Box::pin(ready(Ok(GetLedgerEntriesResponse {
                entries: Some(vec![LedgerEntryResult {
                    key: String::new(),
                    xdr: xdr_base64,
                    last_modified_ledger: 0,
                    live_until_ledger_seq_ledger_seq: None,
                }]),
                latest_ledger: 0,
            })))
        });

        let contract_addr = "CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA";
        let result = get_contract_token_decimals(&provider, contract_addr).await;

        assert!(result.is_some());
        assert_eq!(result.unwrap(), 8);
    }

    #[tokio::test]
    async fn test_get_contract_token_decimals_both_methods_fail() {
        use soroban_rs::stellar_rpc_client::GetLedgerEntriesResponse;

        let mut provider = create_mock_provider();

        // Mock failed contract invocation
        provider.expect_call_contract().returning(|_, _, _| {
            Box::pin(ready(Err(crate::services::provider::ProviderError::Other(
                "Contract call failed".to_string(),
            ))))
        });

        // Mock failed storage query (no entries)
        provider.expect_get_ledger_entries().returning(|_| {
            Box::pin(ready(Ok(GetLedgerEntriesResponse {
                entries: None,
                latest_ledger: 0,
            })))
        });

        let contract_addr = "CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA";
        let result = get_contract_token_decimals(&provider, contract_addr).await;

        // Should return None when both methods fail
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_get_contract_token_decimals_invalid_contract_address() {
        let provider = create_mock_provider();

        let invalid_addr = "INVALID_CONTRACT_ADDRESS";
        let result = get_contract_token_decimals(&provider, invalid_addr).await;

        // Should return None for invalid contract address
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_invoke_decimals_function_success() {
        let mut provider = create_mock_provider();

        let decimals_value = ScVal::U32(9);
        provider
            .expect_call_contract()
            .returning(move |_, _, _| Box::pin(ready(Ok(decimals_value.clone()))));

        let contract_addr = "CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA";
        let result = invoke_decimals_function(&provider, contract_addr).await;

        assert!(result.is_some());
        assert_eq!(result.unwrap(), 9);
    }

    #[tokio::test]
    async fn test_invoke_decimals_function_failure() {
        let mut provider = create_mock_provider();

        provider.expect_call_contract().returning(|_, _, _| {
            Box::pin(ready(Err(crate::services::provider::ProviderError::Other(
                "Contract not found".to_string(),
            ))))
        });

        let contract_addr = "CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA";
        let result = invoke_decimals_function(&provider, contract_addr).await;

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_query_decimals_from_storage_success() {
        use soroban_rs::stellar_rpc_client::{GetLedgerEntriesResponse, LedgerEntryResult};
        use soroban_rs::xdr::{
            ContractDataDurability, ContractDataEntry, ExtensionPoint, LedgerEntry,
            LedgerEntryData, LedgerEntryExt, ScVal, WriteXdr,
        };

        let mut provider = create_mock_provider();

        provider.expect_get_ledger_entries().returning(|_| {
            let decimals_val = ScVal::U32(18);

            let contract_data = ContractDataEntry {
                ext: ExtensionPoint::V0,
                contract: ScAddress::Contract(ContractId(Hash([0u8; 32]))),
                key: ScVal::Vec(None),
                durability: ContractDataDurability::Persistent,
                val: decimals_val,
            };

            let ledger_entry = LedgerEntry {
                last_modified_ledger_seq: 0,
                data: LedgerEntryData::ContractData(contract_data),
                ext: LedgerEntryExt::V0,
            };

            let xdr_base64 = ledger_entry
                .data
                .to_xdr_base64(soroban_rs::xdr::Limits::none())
                .unwrap();

            Box::pin(ready(Ok(GetLedgerEntriesResponse {
                entries: Some(vec![LedgerEntryResult {
                    key: String::new(),
                    xdr: xdr_base64,
                    last_modified_ledger: 0,
                    live_until_ledger_seq_ledger_seq: None,
                }]),
                latest_ledger: 0,
            })))
        });

        let contract_addr = "CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA";
        let contract_hash = parse_contract_address(contract_addr).unwrap();
        let result = query_decimals_from_storage(&provider, contract_addr, contract_hash).await;

        assert!(result.is_some());
        assert_eq!(result.unwrap(), 18);
    }

    #[tokio::test]
    async fn test_query_decimals_from_storage_no_entry() {
        use soroban_rs::stellar_rpc_client::GetLedgerEntriesResponse;

        let mut provider = create_mock_provider();

        provider.expect_get_ledger_entries().returning(|_| {
            Box::pin(ready(Ok(GetLedgerEntriesResponse {
                entries: None,
                latest_ledger: 0,
            })))
        });

        let contract_addr = "CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA";
        let contract_hash = parse_contract_address(contract_addr).unwrap();
        let result = query_decimals_from_storage(&provider, contract_addr, contract_hash).await;

        assert!(result.is_none());
    }

    #[test]
    fn test_stellar_address_validation() {
        // Valid Stellar account address (starts with G)
        let valid_account = "GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5";
        assert_eq!(valid_account.len(), STELLAR_ADDRESS_LENGTH);
        assert!(valid_account.starts_with(STELLAR_ACCOUNT_PREFIX));

        // Valid contract address (starts with C)
        let valid_contract = "CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA";
        assert_eq!(valid_contract.len(), STELLAR_ADDRESS_LENGTH);
        assert!(!valid_contract.starts_with(STELLAR_ACCOUNT_PREFIX));

        // Invalid length
        let short = "GBBD47IF6LWK7P7";
        assert!(short.len() != STELLAR_ADDRESS_LENGTH);

        // Invalid prefix (M is for muxed accounts)
        let muxed = "MAAAAAAAAAAABBBBBBBBBBBBCCCCCCCCCCCCDDDDDDDDDDDDEEEEEEEE";
        assert!(!muxed.starts_with(STELLAR_ACCOUNT_PREFIX));
    }

    #[tokio::test]
    async fn test_get_token_balance_different_identifiers() {
        let mut provider = create_mock_provider();

        let test_balance = 75_0000000i64; // 75 XLM
        let account_entry = create_mock_account_entry(test_balance);

        // Mock get_account to be called twice
        provider
            .expect_get_account()
            .times(2)
            .returning(move |_| Box::pin(ready(Ok(account_entry.clone()))));

        let account = TEST_PK;

        // Test with "native"
        let result1 = get_token_balance(&provider, account, "native").await;
        assert!(result1.is_ok());
        assert_eq!(result1.unwrap(), test_balance as u64);

        // Test with "XLM"
        let result2 = get_token_balance(&provider, account, "XLM").await;
        assert!(result2.is_ok());
        assert_eq!(result2.unwrap(), test_balance as u64);
    }

    #[test]
    fn test_parse_asset_identifier_colon_in_issuer() {
        // Edge case: what if issuer somehow contains a colon?
        // split_once only splits on first colon
        let result = parse_asset_identifier(
            "USDC:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5:EXTRA",
        );
        assert!(result.is_ok());
        let (code, issuer) = result.unwrap();
        assert_eq!(code, "USDC");
        assert_eq!(
            issuer,
            "GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5:EXTRA"
        );
    }

    #[tokio::test]
    async fn test_get_token_metadata_case_sensitivity() {
        let provider = create_mock_provider();

        // Asset codes are case-sensitive in Stellar
        let asset_id = "usdc:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5";
        let result = get_token_metadata(&provider, asset_id).await;
        assert!(result.is_ok());
        let metadata = result.unwrap();

        match metadata.kind {
            StellarTokenKind::Classic { code, .. } => {
                assert_eq!(code, "usdc"); // Lowercase preserved
            }
            _ => panic!("Expected Classic token kind"),
        }
    }

    #[test]
    fn test_max_asset_code_length() {
        // Test that MAX_ASSET_CODE_LENGTH is correctly set
        assert_eq!(MAX_ASSET_CODE_LENGTH, 12);

        // Asset codes up to 4 chars should be Credit4
        for len in 1..=4 {
            let code = "A".repeat(len);
            assert!(code.len() <= 4);
        }

        // Asset codes 5-12 chars should be Credit12
        for len in 5..=12 {
            let code = "A".repeat(len);
            assert!(code.len() > 4 && code.len() <= MAX_ASSET_CODE_LENGTH);
        }

        // Asset codes > 12 should be invalid
        let too_long = "A".repeat(13);
        assert!(too_long.len() > MAX_ASSET_CODE_LENGTH);
    }
}
