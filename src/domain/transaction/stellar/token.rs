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
                    return Err(StellarTransactionUtilsError::BalanceTooLarge(
                        parts.hi, parts.lo,
                    ));
                }
                if (parts.lo as i64) < 0 {
                    return Err(StellarTransactionUtilsError::NegativeBalanceI128(parts.lo));
                }
                Ok(parts.lo)
            }
            ScVal::U64(n) => Ok(n),
            ScVal::I64(n) => {
                if n < 0 {
                    return Err(StellarTransactionUtilsError::NegativeBalanceI64(n));
                }
                Ok(n as u64)
            }
            other => Err(StellarTransactionUtilsError::UnexpectedBalanceType(
                format!("{:?}", other),
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
    extract_u32_from_scval(&val, "decimals storage value")
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
        let _balance = result.unwrap();

        // Test with "XLM"
        let result = get_token_balance(&provider, TESTNET_TEST_ACCOUNT, "XLM").await;
        assert!(
            result.is_ok(),
            "Should fetch XLM balance using 'XLM' identifier"
        );
        let balance_xlm = result.unwrap();
        assert_eq!(
            _balance, balance_xlm,
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
            Ok(_balance) => {
                // Balance is u64, so it's always non-negative by type
            }
            Err(StellarTransactionUtilsError::NoTrustlineFound(_, _)) => {
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
            Ok(_balance) => {
                // Balance is u64, so it's always non-negative by type
            }
            Err(StellarTransactionUtilsError::InvalidContractAddress(_, _)) => {
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
            Err(StellarTransactionUtilsError::InvalidContractAddress(_, _)) => {
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
            StellarTransactionUtilsError::InvalidAssetFormat(_) => {}
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
            StellarTransactionUtilsError::EmptyAssetCode(_) => {}
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
            StellarTransactionUtilsError::InvalidIssuerLength(_, _)
            | StellarTransactionUtilsError::InvalidIssuerPrefix(_, _)
            | StellarTransactionUtilsError::InvalidAccountAddress(_, _) => {}
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
