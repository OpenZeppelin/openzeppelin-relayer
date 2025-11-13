//! Utility functions for Stellar transaction domain logic.
use crate::domain::relayer::xdr_utils::extract_operations;
use crate::models::{AssetSpec, OperationSpec, RelayerError};
use crate::services::provider::StellarProviderTrait;
use chrono::{DateTime, Utc};
use soroban_rs::xdr::{
    AccountId, AlphaNum12, AlphaNum4, Asset, AssetCode12, AssetCode4, ContractDataEntry,
    ContractId, LedgerEntry, LedgerEntryData, LedgerKey, LedgerKeyContractData, Limits, Operation,
    Preconditions, PublicKey as XdrPublicKey, ReadXdr, ScAddress, ScVal, TimeBounds, TimePoint,
    TransactionEnvelope, TrustLineEntry, TrustLineEntryExt, Uint256, VecM,
};
use std::str::FromStr;
use stellar_strkey::ed25519::PublicKey;
use tracing::{info, trace};

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
    info!(
        "Fetching sequence from chain for address: {}",
        relayer_address
    );

    // Fetch account info from chain
    let account = provider
        .get_account(relayer_address)
        .await
        .map_err(|e| format!("Failed to fetch account from chain: {}", e))?;

    let on_chain_seq = account.seq_num.0; // Extract the i64 value
    let next_usable = next_sequence_u64(on_chain_seq)
        .map_err(|e| format!("Failed to calculate next sequence: {}", e))?;

    info!(
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
// Gas Abstraction Utility Functions
// ============================================================================

/// Convert raw token amount to UI amount based on decimals
pub fn amount_to_ui_amount(amount: u64, decimals: u8) -> String {
    let divisor = 10_f64.powi(decimals as i32);
    let ui_amount = amount as f64 / divisor;

    // Format to avoid scientific notation and unnecessary decimals
    if ui_amount.fract() == 0.0 {
        format!("{:.0}", ui_amount)
    } else {
        // Find the number of significant decimal places
        let mut formatted = format!("{:.7}", ui_amount);
        // Trim trailing zeros
        while formatted.ends_with('0') {
            formatted.pop();
        }
        if formatted.ends_with('.') {
            formatted.pop();
        }
        formatted
    }
}

/// Parse transaction and count operations
///
/// Supports both XDR (base64 string) and operations array formats
pub fn parse_transaction_and_count_operations(
    transaction_json: &serde_json::Value,
) -> Result<usize, RelayerError> {
    // Try to parse as XDR string first
    if let Some(xdr_str) = transaction_json.as_str() {
        let envelope = TransactionEnvelope::from_xdr_base64(xdr_str, Limits::none())
            .map_err(|e| RelayerError::Internal(format!("Failed to parse XDR: {}", e)))?;

        let operations = extract_operations(&envelope)
            .map_err(|e| RelayerError::Internal(format!("Failed to extract operations: {}", e)))?;

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
            let envelope = TransactionEnvelope::from_xdr_base64(xdr_str, Limits::none())
                .map_err(|e| RelayerError::Internal(format!("Failed to parse XDR: {}", e)))?;

            let operations = extract_operations(&envelope).map_err(|e| {
                RelayerError::Internal(format!("Failed to extract operations: {}", e))
            })?;

            return Ok(operations.len());
        }
    }

    Err(RelayerError::Internal(
        "Transaction must be either XDR string or operations array".to_string(),
    ))
}

/// Estimate the base transaction fee in XLM (stroops)
///
/// For Stellar, the base fee is typically 100 stroops per operation.
pub fn estimate_base_fee(num_operations: usize) -> u64 {
    // Stellar base fee is 100 stroops per operation
    // Minimum transaction fee is 100 stroops
    const BASE_FEE_PER_OPERATION: u64 = 100;
    (num_operations.max(1) as u64) * BASE_FEE_PER_OPERATION
}

/// Parse transaction envelope from JSON value
pub fn parse_transaction_envelope(
    transaction_json: &serde_json::Value,
) -> Result<TransactionEnvelope, RelayerError> {
    // Try to parse as XDR string first
    if let Some(xdr_str) = transaction_json.as_str() {
        return TransactionEnvelope::from_xdr_base64(xdr_str, Limits::none())
            .map_err(|e| RelayerError::Internal(format!("Failed to parse XDR: {}", e)));
    }

    // Try to parse as object with transaction_xdr field
    if let Some(obj) = transaction_json.as_object() {
        if let Some(xdr_str) = obj.get("transaction_xdr").and_then(|v| v.as_str()) {
            return TransactionEnvelope::from_xdr_base64(xdr_str, Limits::none())
                .map_err(|e| RelayerError::Internal(format!("Failed to parse XDR: {}", e)));
        }
    }

    Err(RelayerError::Internal(
        "Transaction must be XDR string or object with transaction_xdr field".to_string(),
    ))
}

/// Create fee payment operation
pub fn create_fee_payment_operation(
    destination: &str,
    asset_id: &str,
    amount: i64,
) -> Result<OperationSpec, RelayerError> {
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
                return Err(RelayerError::Internal(format!(
                    "Asset code too long (max 12 characters): {}",
                    code
                )));
            }
        } else {
            return Err(RelayerError::Internal(format!(
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
) -> Result<(), RelayerError> {
    match envelope {
        TransactionEnvelope::TxV0(ref mut e) => {
            // Extract existing operations
            let mut ops: Vec<Operation> = e.tx.operations.iter().cloned().collect();
            ops.push(operation);

            // Convert back to VecM
            let operations: VecM<Operation, 100> = ops
                .try_into()
                .map_err(|_| RelayerError::Internal("Too many operations (max 100)".to_string()))?;

            e.tx.operations = operations;

            // Update fee to account for new operation
            e.tx.fee = (e.tx.operations.len() as u32) * 100; // 100 stroops per operation
        }
        TransactionEnvelope::Tx(ref mut e) => {
            // Extract existing operations
            let mut ops: Vec<Operation> = e.tx.operations.iter().cloned().collect();
            ops.push(operation);

            // Convert back to VecM
            let operations: VecM<Operation, 100> = ops
                .try_into()
                .map_err(|_| RelayerError::Internal("Too many operations (max 100)".to_string()))?;

            e.tx.operations = operations;

            // Update fee to account for new operation
            e.tx.fee = (e.tx.operations.len() as u32) * 100; // 100 stroops per operation
        }
        TransactionEnvelope::TxFeeBump(_) => {
            return Err(RelayerError::Internal(
                "Cannot add operations to fee-bump transactions".to_string(),
            ));
        }
    }
    Ok(())
}

/// Set time bounds on transaction envelope
pub fn set_time_bounds(
    envelope: &mut TransactionEnvelope,
    valid_until: DateTime<Utc>,
) -> Result<(), RelayerError> {
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
            return Err(RelayerError::Internal(
                "Cannot set time bounds on fee-bump transactions".to_string(),
            ));
        }
    }
    Ok(())
}

/// Fetch token balance for a given account and asset identifier.
///
/// Supports:
/// - Native XLM: Returns account balance directly
/// - Traditional assets (Credit4/Credit12): Queries trustline balance via LedgerKey::Trustline
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
/// Balance in stroops (or token's smallest unit) as u64, or error if balance cannot be fetched
pub async fn get_token_balance<P>(
    provider: &P,
    account_id: &str,
    asset_id: &str,
) -> Result<u64, RelayerError>
where
    P: StellarProviderTrait + Send + Sync,
{
    // Handle native XLM - accept both "native" and "XLM" for UX
    if asset_id == "native" || asset_id == "XLM" {
        let account_entry = provider.get_account(account_id).await.map_err(|e| {
            RelayerError::ProviderError(format!("Failed to fetch account for balance: {}", e))
        })?;
        return Ok(account_entry.balance as u64);
    }

    // Check if it's a contract address (starts with 'C' and is 56 characters)
    if asset_id.starts_with('C') && asset_id.len() == 56 {
        return get_contract_token_balance(provider, account_id, asset_id).await;
    }

    // Otherwise, treat as traditional asset (CODE:ISSUER format)
    get_asset_trustline_balance(provider, account_id, asset_id).await
}

/// Fetch balance for a traditional Stellar asset (Credit4/Credit12) via trustline
async fn get_asset_trustline_balance<P>(
    provider: &P,
    account_id: &str,
    asset_id: &str,
) -> Result<u64, RelayerError>
where
    P: StellarProviderTrait + Send + Sync,
{
    // Parse asset identifier (format: "CODE:ISSUER")
    let (code, issuer) = asset_id.split_once(':').ok_or_else(|| {
        RelayerError::Internal(format!(
            "Invalid asset identifier format. Expected 'CODE:ISSUER', got: {}",
            asset_id
        ))
    })?;

    // Parse issuer public key
    let issuer_pk = PublicKey::from_str(issuer).map_err(|e| {
        RelayerError::Internal(format!("Invalid issuer public key '{}': {}", issuer, e))
    })?;

    // Convert to XDR types
    let uint256 = Uint256(issuer_pk.0);
    let xdr_pk = XdrPublicKey::PublicKeyTypeEd25519(uint256);
    let issuer_account_id = AccountId(xdr_pk);

    // Parse account ID
    let account_pk = PublicKey::from_str(account_id).map_err(|e| {
        RelayerError::Internal(format!(
            "Invalid account public key '{}': {}",
            account_id, e
        ))
    })?;
    let account_uint256 = Uint256(account_pk.0);
    let account_xdr_pk = XdrPublicKey::PublicKeyTypeEd25519(account_uint256);
    let account_xdr_id = AccountId(account_xdr_pk);

    // Create Asset XDR based on code length
    let asset = if code.len() <= 4 {
        let mut buf = [0u8; 4];
        buf[..code.len()].copy_from_slice(code.as_bytes());
        Asset::CreditAlphanum4(AlphaNum4 {
            asset_code: AssetCode4(buf),
            issuer: issuer_account_id,
        })
    } else if code.len() <= 12 {
        let mut buf = [0u8; 12];
        buf[..code.len()].copy_from_slice(code.as_bytes());
        Asset::CreditAlphanum12(AlphaNum12 {
            asset_code: AssetCode12(buf),
            issuer: issuer_account_id,
        })
    } else {
        return Err(RelayerError::Internal(format!(
            "Asset code too long (max 12 characters): {}",
            code
        )));
    };

    // Construct LedgerKey::Trustline
    // Note: LedgerKeyTrustLine uses TrustLineAsset which wraps Asset
    // Native assets are already handled above, so this is unreachable for Native
    let ledger_key = LedgerKey::Trustline(soroban_rs::xdr::LedgerKeyTrustLine {
        account_id: account_xdr_id,
        asset: match asset {
            Asset::CreditAlphanum4(alpha4) => {
                soroban_rs::xdr::TrustLineAsset::CreditAlphanum4(alpha4)
            }
            Asset::CreditAlphanum12(alpha12) => {
                soroban_rs::xdr::TrustLineAsset::CreditAlphanum12(alpha12)
            }
            Asset::Native => {
                // This should never happen as native is handled earlier
                return Err(RelayerError::Internal(
                    "Native asset should be handled before trustline query".to_string(),
                ));
            }
        },
    });

    // Query ledger entry
    let ledger_entries = provider
        .get_ledger_entries(&[ledger_key])
        .await
        .map_err(|e| {
            RelayerError::ProviderError(format!(
                "Failed to query trustline for asset {}: {}",
                asset_id, e
            ))
        })?;

    // Extract balance from trustline entry
    let entries = ledger_entries.entries.ok_or_else(|| {
        RelayerError::ValidationError(format!(
            "No trustline found for asset {} on account {}",
            asset_id, account_id
        ))
    })?;

    if entries.is_empty() {
        return Err(RelayerError::ValidationError(format!(
            "No trustline found for asset {} on account {}",
            asset_id, account_id
        )));
    }

    let entry_result = &entries[0];
    // Parse XDR string to LedgerEntry
    let entry = LedgerEntry::from_xdr_base64(&entry_result.xdr, Limits::none()).map_err(|e| {
        RelayerError::Internal(format!("Failed to parse trustline ledger entry XDR: {}", e))
    })?;

    match &entry.data {
        LedgerEntryData::Trustline(TrustLineEntry {
            balance,
            ext: TrustLineEntryExt::V0,
            ..
        }) => Ok(*balance as u64),
        LedgerEntryData::Trustline(_) => Err(RelayerError::Internal(
            "Unsupported trustline entry version".to_string(),
        )),
        _ => Err(RelayerError::Internal(
            "Unexpected ledger entry type for trustline query".to_string(),
        )),
    }
}

/// Fetch balance for a Soroban contract token via ContractData
async fn get_contract_token_balance<P>(
    provider: &P,
    account_id: &str,
    contract_address: &str,
) -> Result<u64, RelayerError>
where
    P: StellarProviderTrait + Send + Sync,
{
    // Parse contract address using ContractId::from_str to ensure valid Strkey encoding
    let contract_id = ContractId::from_str(contract_address).map_err(|e| {
        RelayerError::Internal(format!(
            "Invalid contract address '{}': {}",
            contract_address, e
        ))
    })?;
    let contract_hash = contract_id.0;

    // Parse account ID
    let account_pk = PublicKey::from_str(account_id).map_err(|e| {
        RelayerError::Internal(format!(
            "Invalid account public key '{}': {}",
            account_id, e
        ))
    })?;
    let account_uint256 = Uint256(account_pk.0);
    let account_xdr_pk = XdrPublicKey::PublicKeyTypeEd25519(account_uint256);
    let account_xdr_id = AccountId(account_xdr_pk);

    // Create ScAddress for the account
    let account_sc_address = ScAddress::Account(account_xdr_id);

    // Create balance key (Soroban token standard uses "Balance" as the key)
    // The key format is: ScVal::Vec with [ScVal::Symbol("Balance"), ScVal::Address(account)]
    // ScVec requires VecM with u32::MAX capacity (though actual XDR limit is 9223)
    let balance_vec_items: Vec<ScVal> = vec![
        ScVal::Symbol("Balance".try_into().map_err(|e| {
            RelayerError::Internal(format!("Failed to create balance symbol: {:?}", e))
        })?),
        ScVal::Address(account_sc_address.clone()),
    ];
    // Convert to VecM - ScVec type requires u32::MAX, but we only have 2 items (safe)
    let balance_vec: VecM<ScVal, { u32::MAX }> =
        VecM::try_from(balance_vec_items).map_err(|e| {
            RelayerError::Internal(format!("Failed to create balance key vector: {:?}", e))
        })?;
    let balance_key = ScVal::Vec(Some(soroban_rs::xdr::ScVec(balance_vec)));

    // Construct LedgerKey::ContractData
    // Try Persistent first (most common), fallback to Temporary if not found
    let contract_address_sc =
        soroban_rs::xdr::ScAddress::Contract(soroban_rs::xdr::ContractId(contract_hash));

    let mut ledger_key = LedgerKey::ContractData(LedgerKeyContractData {
        contract: contract_address_sc.clone(),
        key: balance_key.clone(),
        durability: soroban_rs::xdr::ContractDataDurability::Persistent,
    });

    // Query ledger entry with Persistent durability
    let mut ledger_entries = provider
        .get_ledger_entries(&[ledger_key.clone()])
        .await
        .map_err(|e| {
            RelayerError::ProviderError(format!(
                "Failed to query contract balance for contract {}: {}",
                contract_address, e
            ))
        })?;

    // If not found, try Temporary durability (some test tokens use temporary storage)
    if ledger_entries
        .entries
        .as_ref()
        .map(|e| e.is_empty())
        .unwrap_or(true)
    {
        ledger_key = LedgerKey::ContractData(LedgerKeyContractData {
            contract: contract_address_sc,
            key: balance_key,
            durability: soroban_rs::xdr::ContractDataDurability::Temporary,
        });
        ledger_entries = provider
            .get_ledger_entries(&[ledger_key])
            .await
            .map_err(|e| {
                RelayerError::ProviderError(format!(
                    "Failed to query contract balance (temporary) for contract {}: {}",
                    contract_address, e
                ))
            })?;
    }

    // Extract balance from contract data entry
    let entries = match ledger_entries.entries {
        Some(entries) if !entries.is_empty() => entries,
        _ => {
            // No balance entry means balance is 0
            trace!(
                "No balance entry found for contract {} on account {}, assuming zero balance",
                contract_address,
                account_id
            );
            return Ok(0);
        }
    };

    let entry_result = &entries[0];
    // Parse XDR string to LedgerEntry
    let entry = LedgerEntry::from_xdr_base64(&entry_result.xdr, Limits::none()).map_err(|e| {
        RelayerError::Internal(format!(
            "Failed to parse contract data ledger entry XDR: {}",
            e
        ))
    })?;

    match &entry.data {
        LedgerEntryData::ContractData(ContractDataEntry { val, .. }) => {
            // Extract balance from ScVal - support multiple types for robustness
            match val {
                ScVal::I128(parts) => {
                    // Most tokens use i128, check if it fits in u64
                    if parts.hi != 0 {
                        return Err(RelayerError::Internal(format!(
                            "Balance too large (i128 hi={}, lo={}) to fit in u64",
                            parts.hi, parts.lo
                        )));
                    }
                    let lo = parts.lo as i64;
                    if lo < 0 {
                        return Err(RelayerError::Internal(format!(
                            "Negative balance not allowed: i128 lo={}",
                            parts.lo
                        )));
                    }
                    Ok(lo as u64)
                }
                ScVal::U64(n) => Ok(*n),
                ScVal::I64(n) => {
                    if *n < 0 {
                        Err(RelayerError::Internal(format!(
                            "Negative balance not allowed: i64={}",
                            n
                        )))
                    } else {
                        Ok(*n as u64)
                    }
                }
                _ => Err(RelayerError::Internal(format!(
                    "Unexpected balance value type in contract data: {:?}. Expected I128, U64, or I64",
                    val
                ))),
            }
        }
        _ => Err(RelayerError::Internal(
            "Unexpected ledger entry type for contract data query".to_string(),
        )),
    }
}

// ============================================================================
// Status Check Utility Functions
// ============================================================================

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
