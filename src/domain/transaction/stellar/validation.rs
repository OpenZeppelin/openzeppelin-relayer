//! Validation logic for Stellar transactions
//!
//! This module focuses on business logic validations that aren't
//! already handled by XDR parsing or the type system.

use crate::constants::STELLAR_DEFAULT_TRANSACTION_FEE;
use crate::constants::STELLAR_MAX_OPERATIONS;
use crate::domain::relayer::xdr_utils::{
    extract_operations, extract_source_account, muxed_account_to_string,
};
use crate::domain::transaction::stellar::token::get_token_balance;
use crate::domain::transaction::stellar::utils::{
    asset_to_asset_id, convert_xlm_fee_to_token, estimate_fee, extract_time_bounds,
};
use crate::domain::xdr_needs_simulation;
use crate::models::RelayerStellarPolicy;
use crate::models::{MemoSpec, OperationSpec, StellarValidationError, TransactionError};
use crate::services::provider::StellarProviderTrait;
use crate::services::stellar_dex::StellarDexServiceTrait;
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use soroban_rs::xdr::{
    AccountId, HostFunction, InvokeHostFunctionOp, LedgerKey, OperationBody, PaymentOp,
    PublicKey as XdrPublicKey, ScAddress, SorobanCredentials, TransactionEnvelope,
};
use stellar_strkey::ed25519::PublicKey;
use thiserror::Error;
#[derive(Debug, Error, Serialize)]
pub enum StellarTransactionValidationError {
    #[error("Validation error: {0}")]
    ValidationError(String),
    #[error("Policy violation: {0}")]
    PolicyViolation(String),
    #[error("Invalid asset identifier: {0}")]
    InvalidAssetIdentifier(String),
    #[error("Token not allowed: {0}")]
    TokenNotAllowed(String),
    #[error("Insufficient token payment: expected {0}, got {1}")]
    InsufficientTokenPayment(u64, u64),
    #[error("Max fee exceeded: {0}")]
    MaxFeeExceeded(u64),
}

/// Validate operations for business rules
pub fn validate_operations(ops: &[OperationSpec]) -> Result<(), TransactionError> {
    // Basic sanity checks
    if ops.is_empty() {
        return Err(StellarValidationError::EmptyOperations.into());
    }

    if ops.len() > STELLAR_MAX_OPERATIONS {
        return Err(StellarValidationError::TooManyOperations {
            count: ops.len(),
            max: STELLAR_MAX_OPERATIONS,
        }
        .into());
    }

    // Check Soroban exclusivity - this is a specific business rule
    validate_soroban_exclusivity(ops)?;

    Ok(())
}

/// Validate that Soroban operations are exclusive
fn validate_soroban_exclusivity(ops: &[OperationSpec]) -> Result<(), TransactionError> {
    let soroban_ops = ops.iter().filter(|op| is_soroban_operation(op)).count();

    if soroban_ops > 1 {
        return Err(StellarValidationError::MultipleSorobanOperations.into());
    }

    if soroban_ops == 1 && ops.len() > 1 {
        return Err(StellarValidationError::SorobanNotExclusive.into());
    }

    Ok(())
}

/// Check if an operation is a Soroban operation
fn is_soroban_operation(op: &OperationSpec) -> bool {
    matches!(
        op,
        OperationSpec::InvokeContract { .. }
            | OperationSpec::CreateContract { .. }
            | OperationSpec::UploadWasm { .. }
    )
}

/// Validate that Soroban operations don't have a non-None memo
pub fn validate_soroban_memo_restriction(
    ops: &[OperationSpec],
    memo: &Option<MemoSpec>,
) -> Result<(), TransactionError> {
    let has_soroban = ops.iter().any(is_soroban_operation);

    if has_soroban && memo.is_some() && !matches!(memo, Some(MemoSpec::None)) {
        return Err(StellarValidationError::SorobanWithMemo.into());
    }

    Ok(())
}

/// Validator for Stellar transactions and policies
pub struct StellarTransactionValidator;

impl StellarTransactionValidator {
    /// Validate fee_token structure
    ///
    /// Validates that the fee_token is in a valid format:
    /// - "native" or "XLM" for native XLM
    /// - "CODE:ISSUER" for classic assets (CODE: 1-12 chars, ISSUER: 56 chars starting with 'G')
    /// - Contract address starting with "C" (56 chars) for Soroban contract tokens
    pub fn validate_fee_token_structure(
        fee_token: &str,
    ) -> Result<(), StellarTransactionValidationError> {
        // Handle native XLM
        if fee_token == "native" || fee_token == "XLM" || fee_token.is_empty() {
            return Ok(());
        }

        // Check if it's a contract address (starts with 'C', 56 chars)
        if fee_token.starts_with('C') && fee_token.len() == 56 && !fee_token.contains(':') {
            // Validate it's a valid contract address using StrKey
            if stellar_strkey::Contract::from_string(fee_token).is_ok() {
                return Ok(());
            }
            return Err(StellarTransactionValidationError::InvalidAssetIdentifier(
                format!(
                    "Invalid contract address format: {fee_token} (must be 56 characters and valid StrKey)"
                ),
            ));
        }

        // Otherwise, must be CODE:ISSUER format
        let parts: Vec<&str> = fee_token.split(':').collect();
        if parts.len() != 2 {
            return Err(StellarTransactionValidationError::InvalidAssetIdentifier(format!(
                "Invalid fee_token format: {fee_token}. Expected 'native', 'CODE:ISSUER', or contract address (C...)"
            )));
        }

        let code = parts[0];
        let issuer = parts[1];

        // Validate CODE length (1-12 characters)
        if code.is_empty() || code.len() > 12 {
            return Err(StellarTransactionValidationError::InvalidAssetIdentifier(
                format!("Invalid asset code length: {code} (must be 1-12 characters)"),
            ));
        }

        // Validate ISSUER format (56 chars, starts with 'G')
        if issuer.len() != 56 {
            return Err(StellarTransactionValidationError::InvalidAssetIdentifier(
                format!("Invalid issuer address length: {issuer} (must be 56 characters)"),
            ));
        }

        if !issuer.starts_with('G') {
            return Err(StellarTransactionValidationError::InvalidAssetIdentifier(
                format!("Invalid issuer address prefix: {issuer} (must start with 'G')"),
            ));
        }

        // Validate issuer is a valid Stellar public key
        if stellar_strkey::ed25519::PublicKey::from_string(issuer).is_err() {
            return Err(StellarTransactionValidationError::InvalidAssetIdentifier(
                format!(
                    "Invalid issuer address format: {issuer} (must be a valid Stellar public key)"
                ),
            ));
        }

        Ok(())
    }

    /// Validate that an asset identifier is in the allowed tokens list
    pub fn validate_allowed_token(
        asset: &str,
        policy: &RelayerStellarPolicy,
    ) -> Result<(), StellarTransactionValidationError> {
        let allowed_tokens = policy.get_allowed_tokens();

        if allowed_tokens.is_empty() {
            // If no allowed tokens specified, all tokens are allowed
            return Ok(());
        }

        // Check if native XLM is allowed
        if asset == "native" || asset.is_empty() {
            let native_allowed = allowed_tokens
                .iter()
                .any(|token| token.asset == "native" || token.asset.is_empty());
            if !native_allowed {
                return Err(StellarTransactionValidationError::TokenNotAllowed(
                    "Native XLM not in allowed tokens list".to_string(),
                ));
            }
            return Ok(());
        }

        // Check if the asset is in the allowed list
        let is_allowed = allowed_tokens.iter().any(|token| token.asset == asset);

        if !is_allowed {
            return Err(StellarTransactionValidationError::TokenNotAllowed(format!(
                "Token {asset} not in allowed tokens list"
            )));
        }

        Ok(())
    }

    /// Validate that a fee amount doesn't exceed the maximum allowed fee
    pub fn validate_max_fee(
        fee: u64,
        policy: &RelayerStellarPolicy,
    ) -> Result<(), StellarTransactionValidationError> {
        if let Some(max_fee) = policy.max_fee {
            if fee > max_fee as u64 {
                return Err(StellarTransactionValidationError::MaxFeeExceeded(fee));
            }
        }

        Ok(())
    }

    /// Validate that a specific token's max_allowed_fee is not exceeded
    pub fn validate_token_max_fee(
        asset_id: &str,
        fee: u64,
        policy: &RelayerStellarPolicy,
    ) -> Result<(), StellarTransactionValidationError> {
        if let Some(token_entry) = policy.get_allowed_token_entry(asset_id) {
            if let Some(max_allowed_fee) = token_entry.max_allowed_fee {
                if fee > max_allowed_fee {
                    return Err(StellarTransactionValidationError::MaxFeeExceeded(fee));
                }
            }
        }

        Ok(())
    }

    /// Extract payment operations from a transaction envelope that pay to the relayer
    ///
    /// Returns a vector of (asset_id, amount) tuples for payments to the relayer
    pub fn extract_relayer_payments(
        envelope: &TransactionEnvelope,
        relayer_address: &str,
    ) -> Result<Vec<(String, u64)>, StellarTransactionValidationError> {
        let operations = extract_operations(envelope).map_err(|e| {
            StellarTransactionValidationError::ValidationError(format!(
                "Failed to extract operations: {e}"
            ))
        })?;

        let mut payments = Vec::new();

        for op in operations.iter() {
            if let OperationBody::Payment(PaymentOp {
                destination,
                asset,
                amount,
            }) = &op.body
            {
                // Convert destination to string
                let dest_str = muxed_account_to_string(destination).map_err(|e| {
                    StellarTransactionValidationError::ValidationError(format!(
                        "Failed to parse destination: {e}"
                    ))
                })?;

                // Check if payment is to relayer
                if dest_str == relayer_address {
                    // Convert asset to identifier string
                    let asset_id = asset_to_asset_id(asset).map_err(|e| {
                        StellarTransactionValidationError::InvalidAssetIdentifier(format!(
                            "Failed to convert asset to asset_id: {e}"
                        ))
                    })?;
                    // Validate amount is non-negative before converting from i64 to u64
                    if *amount < 0 {
                        return Err(StellarTransactionValidationError::ValidationError(
                            "Negative payment amount".to_string(),
                        ));
                    }
                    let amount_u64 = *amount as u64;
                    payments.push((asset_id, amount_u64));
                }
            }
        }

        Ok(payments)
    }

    /// Validate token payment in transaction
    ///
    /// Checks that:
    /// 1. Payment operation to relayer exists
    /// 2. Token is in allowed_tokens list
    /// 3. Payment amount matches expected fee (within tolerance)
    pub fn validate_token_payment(
        envelope: &TransactionEnvelope,
        relayer_address: &str,
        expected_fee_token: &str,
        expected_fee_amount: u64,
        policy: &RelayerStellarPolicy,
    ) -> Result<(), StellarTransactionValidationError> {
        // Extract payments to relayer
        let payments = Self::extract_relayer_payments(envelope, relayer_address)?;

        if payments.is_empty() {
            return Err(StellarTransactionValidationError::ValidationError(
                "No payment operation found to relayer".to_string(),
            ));
        }

        // Find payment matching the expected token
        let matching_payment = payments
            .iter()
            .find(|(asset_id, _)| asset_id == expected_fee_token);

        match matching_payment {
            Some((asset_id, amount)) => {
                // Validate token is allowed
                Self::validate_allowed_token(asset_id, policy)?;

                // Validate amount matches expected (allow 1% tolerance for rounding)
                let tolerance = (expected_fee_amount as f64 * 0.01) as u64;
                if *amount < expected_fee_amount.saturating_sub(tolerance) {
                    return Err(StellarTransactionValidationError::InsufficientTokenPayment(
                        expected_fee_amount,
                        *amount,
                    ));
                }

                // Validate max fee
                Self::validate_token_max_fee(asset_id, *amount, policy)?;

                Ok(())
            }
            None => Err(StellarTransactionValidationError::ValidationError(format!(
                "No payment found for expected token: {expected_fee_token}. Found payments: {payments:?}"
            ))),
        }
    }

    /// Validate that the source account is not the relayer address
    ///
    /// This prevents malicious attempts to drain the relayer's funds by
    /// using the relayer as the transaction source.
    fn validate_source_account_not_relayer(
        envelope: &TransactionEnvelope,
        relayer_address: &str,
    ) -> Result<(), StellarTransactionValidationError> {
        let source_account = extract_source_account(envelope).map_err(|e| {
            StellarTransactionValidationError::ValidationError(format!(
                "Failed to extract source account: {e}"
            ))
        })?;

        if source_account == relayer_address {
            return Err(StellarTransactionValidationError::ValidationError(
                "Transaction source account cannot be the relayer address. This is a security measure to prevent relayer fund drainage.".to_string(),
            ));
        }

        Ok(())
    }

    /// Validate transaction type
    ///
    /// Rejects fee-bump transactions as they are not suitable for gasless transactions.
    fn validate_transaction_type(
        envelope: &TransactionEnvelope,
    ) -> Result<(), StellarTransactionValidationError> {
        match envelope {
            soroban_rs::xdr::TransactionEnvelope::TxFeeBump(_) => {
                Err(StellarTransactionValidationError::ValidationError(
                    "Fee-bump transactions are not supported for gasless transactions".to_string(),
                ))
            }
            _ => Ok(()),
        }
    }

    /// Validate that operations don't target the relayer (except for fee payment)
    ///
    /// This prevents operations that could drain the relayer's funds or manipulate
    /// the relayer's account state. Fee payment operations are expected and allowed.
    fn validate_operations_not_targeting_relayer(
        envelope: &TransactionEnvelope,
        relayer_address: &str,
    ) -> Result<(), StellarTransactionValidationError> {
        let operations = extract_operations(envelope).map_err(|e| {
            StellarTransactionValidationError::ValidationError(format!(
                "Failed to extract operations: {e}"
            ))
        })?;

        for op in operations.iter() {
            match &op.body {
                OperationBody::Payment(PaymentOp { destination, .. }) => {
                    let dest_str = muxed_account_to_string(destination).map_err(|e| {
                        StellarTransactionValidationError::ValidationError(format!(
                            "Failed to parse destination: {e}"
                        ))
                    })?;

                    // Payment to relayer is allowed (for fee payment), but we log it
                    if dest_str == relayer_address {
                        // This is expected for fee payment, but we should ensure
                        // it's the last operation added by the relayer
                        continue;
                    }
                }
                OperationBody::AccountMerge(destination) => {
                    let dest_str = muxed_account_to_string(destination).map_err(|e| {
                        StellarTransactionValidationError::ValidationError(format!(
                            "Failed to parse merge destination: {e}"
                        ))
                    })?;

                    if dest_str == relayer_address {
                        return Err(StellarTransactionValidationError::ValidationError(
                            "Account merge operations targeting the relayer are not allowed"
                                .to_string(),
                        ));
                    }
                }
                OperationBody::SetOptions(_) => {
                    // SetOptions operations could potentially modify account settings
                    // We should reject them if they target relayer, but SetOptions doesn't have a target
                    // However, SetOptions on the source account could be problematic
                    // For now, we allow SetOptions but could add more specific checks
                }
                _ => {
                    // Other operation types are checked in validate_operation_types
                }
            }
        }

        Ok(())
    }

    /// Validate operations count
    ///
    /// Ensures the transaction has a reasonable number of operations.
    fn validate_operations_count(
        envelope: &TransactionEnvelope,
    ) -> Result<(), StellarTransactionValidationError> {
        let operations = extract_operations(envelope).map_err(|e| {
            StellarTransactionValidationError::ValidationError(format!(
                "Failed to extract operations: {e}"
            ))
        })?;

        if operations.is_empty() {
            return Err(StellarTransactionValidationError::ValidationError(
                "Transaction must contain at least one operation".to_string(),
            ));
        }

        if operations.len() > STELLAR_MAX_OPERATIONS {
            return Err(StellarTransactionValidationError::ValidationError(format!(
                "Transaction contains too many operations: {} (maximum is {})",
                operations.len(),
                STELLAR_MAX_OPERATIONS
            )));
        }

        Ok(())
    }

    /// Convert AccountId to string representation
    fn account_id_to_string(
        account_id: &AccountId,
    ) -> Result<String, StellarTransactionValidationError> {
        match &account_id.0 {
            XdrPublicKey::PublicKeyTypeEd25519(uint256) => {
                let bytes: [u8; 32] = uint256.0;
                let pk = PublicKey(bytes);
                Ok(pk.to_string())
            }
        }
    }

    /// Check if a footprint key targets relayer-owned storage
    #[allow(dead_code)]
    fn footprint_key_targets_relayer(
        key: &LedgerKey,
        relayer_address: &str,
    ) -> Result<bool, StellarTransactionValidationError> {
        match key {
            LedgerKey::Account(account_key) => {
                // Extract account ID from the key
                let account_str = Self::account_id_to_string(&account_key.account_id)?;
                Ok(account_str == relayer_address)
            }
            LedgerKey::Trustline(trustline_key) => {
                // Check if trustline belongs to relayer
                let account_str = Self::account_id_to_string(&trustline_key.account_id)?;
                Ok(account_str == relayer_address)
            }
            LedgerKey::ContractData(contract_data_key) => {
                // Check if contract data key references relayer account
                match &contract_data_key.contract {
                    ScAddress::Account(acc_id) => {
                        let account_str = Self::account_id_to_string(acc_id)?;
                        Ok(account_str == relayer_address)
                    }
                    ScAddress::Contract(_) => {
                        // Contract storage keys are allowed
                        Ok(false)
                    }
                    ScAddress::MuxedAccount(_)
                    | ScAddress::ClaimableBalance(_)
                    | ScAddress::LiquidityPool(_) => {
                        // These are not account addresses, so they're safe
                        Ok(false)
                    }
                }
            }
            LedgerKey::ContractCode(_) => {
                // Contract code keys are allowed
                Ok(false)
            }
            _ => {
                // Other ledger key types are allowed
                Ok(false)
            }
        }
    }

    /// Validate contract invocation operation
    ///
    /// Performs comprehensive security validation for Soroban contract invocations:
    /// 1. Validates host function type is allowed
    /// 2. Validates Soroban auth entries don't require relayer
    fn validate_contract_invocation(
        invoke: &InvokeHostFunctionOp,
        op_idx: usize,
        relayer_address: &str,
        _policy: &RelayerStellarPolicy,
    ) -> Result<(), StellarTransactionValidationError> {
        // 1. Validate host function type
        match &invoke.host_function {
            HostFunction::InvokeContract(_) => {
                // Contract invocations are allowed by default
            }
            HostFunction::CreateContract(_) => {
                return Err(StellarTransactionValidationError::ValidationError(format!(
                    "Op {op_idx}: CreateContract not allowed for gasless transactions"
                )));
            }
            HostFunction::UploadContractWasm(_) => {
                return Err(StellarTransactionValidationError::ValidationError(format!(
                    "Op {op_idx}: UploadContractWasm not allowed for gasless transactions"
                )));
            }
            _ => {
                return Err(StellarTransactionValidationError::ValidationError(format!(
                    "Op {op_idx}: Unsupported host function"
                )));
            }
        }

        // Validate Soroban auth entries
        for (i, entry) in invoke.auth.iter().enumerate() {
            // Validate that relayer is NOT required signer
            match &entry.credentials {
                SorobanCredentials::SourceAccount => {
                    // We've already validated that the source account is not the relayer,
                    // so SourceAccount credentials are safe.
                }
                SorobanCredentials::Address(address_creds) => {
                    // Check if the address is the relayer
                    match &address_creds.address {
                        ScAddress::Account(acc_id) => {
                            // Convert account ID to string for comparison
                            let account_str = Self::account_id_to_string(acc_id)?;
                            if account_str == relayer_address {
                                return Err(StellarTransactionValidationError::ValidationError(
                                    format!(
                                        "Op {op_idx}: Soroban auth entry {i} requires relayer ({relayer_address}). Forbidden."
                                    ),
                                ));
                            }
                        }
                        ScAddress::Contract(_) => {
                            // Contract addresses in auth are allowed
                        }
                        ScAddress::MuxedAccount(_) => {
                            // Muxed accounts are allowed
                        }
                        ScAddress::ClaimableBalance(_) | ScAddress::LiquidityPool(_) => {
                            // These are not account addresses, so they're safe
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Validate operation types
    ///
    /// Ensures only allowed operation types are present in the transaction.
    /// Currently allows common operation types but can be extended based on policy.
    fn validate_operation_types(
        envelope: &TransactionEnvelope,
        relayer_address: &str,
        policy: &RelayerStellarPolicy,
    ) -> Result<(), StellarTransactionValidationError> {
        let operations = extract_operations(envelope).map_err(|e| {
            StellarTransactionValidationError::ValidationError(format!(
                "Failed to extract operations: {e}"
            ))
        })?;

        for (idx, op) in operations.iter().enumerate() {
            match &op.body {
                // Prevent account merges (could drain account before payment executes)
                OperationBody::AccountMerge(_) => {
                    return Err(StellarTransactionValidationError::ValidationError(format!(
                        "Operation {idx}: AccountMerge operations are not allowed"
                    )));
                }

                // Prevent SetOptions that could lock out the account
                OperationBody::SetOptions(_set_opts) => {
                    return Err(StellarTransactionValidationError::ValidationError(format!(
                        "Operation {idx}: SetOptions operations are not allowed"
                    )));
                }

                // Validate smart contract invocations
                OperationBody::InvokeHostFunction(invoke) => {
                    Self::validate_contract_invocation(invoke, idx, relayer_address, policy)?;
                }

                // Allow common operations
                OperationBody::Payment(_)
                | OperationBody::PathPaymentStrictReceive(_)
                | OperationBody::PathPaymentStrictSend(_)
                | OperationBody::ManageSellOffer(_)
                | OperationBody::ManageBuyOffer(_)
                | OperationBody::CreatePassiveSellOffer(_)
                | OperationBody::ChangeTrust(_)
                | OperationBody::ManageData(_)
                | OperationBody::BumpSequence(_)
                | OperationBody::CreateClaimableBalance(_)
                | OperationBody::ClaimClaimableBalance(_)
                | OperationBody::BeginSponsoringFutureReserves(_)
                | OperationBody::EndSponsoringFutureReserves
                | OperationBody::RevokeSponsorship(_)
                | OperationBody::Clawback(_)
                | OperationBody::ClawbackClaimableBalance(_)
                | OperationBody::SetTrustLineFlags(_)
                | OperationBody::LiquidityPoolDeposit(_)
                | OperationBody::LiquidityPoolWithdraw(_) => {
                    // These are generally safe
                }

                // Deprecated operations
                OperationBody::CreateAccount(_) | OperationBody::AllowTrust(_) => {
                    return Err(StellarTransactionValidationError::ValidationError(format!(
                        "Operation {idx}: Deprecated operation type not allowed"
                    )));
                }

                // Other operations
                OperationBody::Inflation
                | OperationBody::ExtendFootprintTtl(_)
                | OperationBody::RestoreFootprint(_) => {
                    // These are allowed
                }
            }
        }

        Ok(())
    }

    /// Validate sequence number
    ///
    /// Validates that the transaction sequence number is valid for the source account.
    /// Note: The relayer will fee-bump this transaction, so the relayer's sequence will be consumed.
    /// However, the inner transaction (user's tx) must still have a valid sequence number.
    ///
    /// The transaction sequence must be strictly greater than the account's current sequence number.
    /// Future sequence numbers are allowed (user can queue transactions), but equal sequences are rejected.
    pub async fn validate_sequence_number<P>(
        envelope: &TransactionEnvelope,
        provider: &P,
    ) -> Result<(), StellarTransactionValidationError>
    where
        P: StellarProviderTrait + Send + Sync,
    {
        // Extract source account
        let source_account = extract_source_account(envelope).map_err(|e| {
            StellarTransactionValidationError::ValidationError(format!(
                "Failed to extract source account: {e}"
            ))
        })?;

        // Get account's current sequence number from chain
        let account_entry = provider.get_account(&source_account).await.map_err(|e| {
            StellarTransactionValidationError::ValidationError(format!(
                "Failed to get account sequence: {e}"
            ))
        })?;
        let account_seq_num = account_entry.seq_num.0;

        // Extract transaction sequence number
        let tx_seq_num = match envelope {
            TransactionEnvelope::TxV0(e) => e.tx.seq_num.0,
            TransactionEnvelope::Tx(e) => e.tx.seq_num.0,
            TransactionEnvelope::TxFeeBump(_) => {
                return Err(StellarTransactionValidationError::ValidationError(
                    "Fee-bump transactions are not supported for gasless transactions".to_string(),
                ));
            }
        };

        // Validate that transaction sequence number is strictly greater than account's current sequence
        // Stellar requires tx_seq_num > account_seq_num (not >=). Equal sequences are invalid.
        // The user can set a future sequence number, but not a past or equal one
        if tx_seq_num <= account_seq_num {
            return Err(StellarTransactionValidationError::ValidationError(format!(
                "Transaction sequence number {tx_seq_num} is invalid. Account's current sequence is {account_seq_num}. \
                The transaction sequence must be strictly greater than the account's current sequence."
            )));
        }

        Ok(())
    }

    /// Comprehensive validation for gasless transactions
    ///
    /// Performs all security and policy validations on a transaction envelope
    /// before it's processed for gasless execution.
    ///
    /// This includes:
    /// - Validating source account is not relayer
    /// - Validating transaction type
    /// - Validating operations don't target relayer (except fee payment)
    /// - Validating operations count
    /// - Validating operation types
    /// - Validating sequence number
    /// - Validating transaction validity duration (if max_validity_duration is provided)
    ///
    /// # Arguments
    /// * `envelope` - The transaction envelope to validate
    /// * `relayer_address` - The relayer's Stellar address
    /// * `policy` - The relayer policy
    /// * `provider` - Provider for Stellar RPC operations
    /// * `max_validity_duration` - Optional maximum allowed transaction validity duration. If provided,
    ///   validates that the transaction's time bounds don't exceed this duration. This protects against
    ///   price fluctuations for user-paid fee transactions.
    pub async fn gasless_transaction_validation<P>(
        envelope: &TransactionEnvelope,
        relayer_address: &str,
        policy: &RelayerStellarPolicy,
        provider: &P,
        max_validity_duration: Option<Duration>,
    ) -> Result<(), StellarTransactionValidationError>
    where
        P: StellarProviderTrait + Send + Sync,
    {
        Self::validate_source_account_not_relayer(envelope, relayer_address)?;
        Self::validate_transaction_type(envelope)?;
        Self::validate_operations_not_targeting_relayer(envelope, relayer_address)?;
        Self::validate_operations_count(envelope)?;
        Self::validate_operation_types(envelope, relayer_address, policy)?;
        Self::validate_sequence_number(envelope, provider).await?;

        // Validate that transaction time bounds are not expired
        Self::validate_time_bounds_not_expired(envelope)?;

        // Validate transaction validity duration if max_validity_duration is provided
        if let Some(max_duration) = max_validity_duration {
            Self::validate_transaction_validity_duration(envelope, max_duration)?;
        }

        Ok(())
    }

    /// Validate that transaction time bounds are valid and not expired
    ///
    /// Checks that:
    /// 1. Time bounds exist (if envelope has them)
    /// 2. Current time is within the bounds (min_time <= now <= max_time)
    /// 3. Transaction has not expired (now <= max_time)
    ///
    /// # Arguments
    /// * `envelope` - The transaction envelope to validate
    ///
    /// # Returns
    /// Ok(()) if validation passes, StellarTransactionValidationError if validation fails
    pub fn validate_time_bounds_not_expired(
        envelope: &TransactionEnvelope,
    ) -> Result<(), StellarTransactionValidationError> {
        let time_bounds = extract_time_bounds(envelope);

        if let Some(bounds) = time_bounds {
            let now = Utc::now().timestamp() as u64;
            let min_time = bounds.min_time.0;
            let max_time = bounds.max_time.0;

            // Check if transaction has expired
            if now > max_time {
                return Err(StellarTransactionValidationError::ValidationError(format!(
                    "Transaction has expired: max_time={max_time}, current_time={now}"
                )));
            }

            // Check if transaction is not yet valid (optional check, but good to have)
            if min_time > 0 && now < min_time {
                return Err(StellarTransactionValidationError::ValidationError(format!(
                    "Transaction is not yet valid: min_time={min_time}, current_time={now}"
                )));
            }
        }
        // If no time bounds are set, we don't fail here (some transactions may not have them)
        // The caller can decide if time bounds are required

        Ok(())
    }

    /// Validate that transaction validity duration is within the maximum allowed time
    ///
    /// This prevents price fluctuations and protects the relayer from losses.
    /// The transaction must have time bounds set and the validity duration must not exceed
    /// the maximum allowed duration.
    ///
    /// # Arguments
    /// * `envelope` - The transaction envelope to validate
    /// * `max_duration` - Maximum allowed validity duration
    ///
    /// # Returns
    /// Ok(()) if validation passes, StellarTransactionValidationError if validation fails
    pub fn validate_transaction_validity_duration(
        envelope: &TransactionEnvelope,
        max_duration: Duration,
    ) -> Result<(), StellarTransactionValidationError> {
        let time_bounds = extract_time_bounds(envelope);

        if let Some(bounds) = time_bounds {
            let max_time =
                DateTime::from_timestamp(bounds.max_time.0 as i64, 0).ok_or_else(|| {
                    StellarTransactionValidationError::ValidationError(
                        "Invalid max_time in time bounds".to_string(),
                    )
                })?;
            let now = Utc::now();
            let duration = max_time - now;

            if duration > max_duration {
                return Err(StellarTransactionValidationError::ValidationError(format!(
                    "Transaction validity duration ({duration:?}) exceeds maximum allowed duration ({max_duration:?})"
                )));
            }
        } else {
            return Err(StellarTransactionValidationError::ValidationError(
                "Transaction must have time bounds set".to_string(),
            ));
        }

        Ok(())
    }

    /// Comprehensive validation for user fee payment transactions
    ///
    /// This function performs all validations required for user-paid fee transactions.
    /// It validates:
    /// 1. Transaction structure and operations (via gasless_transaction_validation)
    /// 2. Fee payment operations exist and are valid
    /// 3. Allowed token validation
    /// 4. Token max fee validation
    /// 5. Payment amount is sufficient (compares with required fee including margin)
    /// 6. Transaction validity duration (if max_validity_duration is provided)
    ///
    /// This function is used by both fee-bump and sign-transaction flows.
    /// For sign-transaction flows, pass `max_validity_duration` to enforce time bounds.
    /// For fee-bump flows, pass `None` as transactions may not have time bounds set yet.
    ///
    /// # Arguments
    /// * `envelope` - The transaction envelope to validate
    /// * `relayer_address` - The relayer's Stellar address
    /// * `policy` - The relayer policy containing fee payment strategy and token settings
    /// * `provider` - Provider for Stellar RPC operations
    /// * `dex_service` - DEX service for fetching quotes to validate payment amounts
    /// * `max_validity_duration` - Optional maximum allowed transaction validity duration.
    ///   If provided, validates that the transaction's time bounds don't exceed this duration.
    ///   This protects against price fluctuations for user-paid fee transactions when signing.
    ///   Pass `None` for fee-bump flows where time bounds may not be set yet.
    ///
    /// # Returns
    /// Ok(()) if validation passes, StellarTransactionValidationError if validation fails
    pub async fn validate_user_fee_payment_transaction<P, D>(
        envelope: &TransactionEnvelope,
        relayer_address: &str,
        policy: &RelayerStellarPolicy,
        provider: &P,
        dex_service: &D,
        max_validity_duration: Option<Duration>,
    ) -> Result<(), StellarTransactionValidationError>
    where
        P: StellarProviderTrait + Send + Sync,
        D: StellarDexServiceTrait + Send + Sync,
    {
        // Step 1: Comprehensive security validation for gasless transactions
        // Include duration validation if max_validity_duration is provided
        Self::gasless_transaction_validation(
            envelope,
            relayer_address,
            policy,
            provider,
            max_validity_duration,
        )
        .await?;

        // Step 2: Validate fee payment amounts
        Self::validate_user_fee_payment_amounts(
            envelope,
            relayer_address,
            policy,
            provider,
            dex_service,
        )
        .await?;

        Ok(())
    }

    /// Validate fee payment amounts for user-paid fee transactions
    ///
    /// This function validates that the fee payment operation exists, is valid,
    /// and the payment amount is sufficient. It's separated from the core validation
    /// to allow reuse in different flows.
    ///
    /// # Arguments
    /// * `envelope` - The transaction envelope to validate
    /// * `relayer_address` - The relayer's Stellar address
    /// * `policy` - The relayer policy containing fee payment strategy and token settings
    /// * `provider` - Provider for Stellar RPC operations
    /// * `dex_service` - DEX service for fetching quotes to validate payment amounts
    ///
    /// # Returns
    /// Ok(()) if validation passes, StellarTransactionValidationError if validation fails
    async fn validate_user_fee_payment_amounts<P, D>(
        envelope: &TransactionEnvelope,
        relayer_address: &str,
        policy: &RelayerStellarPolicy,
        provider: &P,
        dex_service: &D,
    ) -> Result<(), StellarTransactionValidationError>
    where
        P: StellarProviderTrait + Send + Sync,
        D: StellarDexServiceTrait + Send + Sync,
    {
        // Extract the fee payment for amount validation
        let payments = Self::extract_relayer_payments(envelope, relayer_address)?;
        if payments.is_empty() {
            return Err(StellarTransactionValidationError::ValidationError(
                "Gasless transactions must include a fee payment operation to the relayer"
                    .to_string(),
            ));
        }

        // Validate only one fee payment operation
        if payments.len() > 1 {
            return Err(StellarTransactionValidationError::ValidationError(format!(
                "Gasless transactions must include exactly one fee payment operation to the relayer, found {}",
                payments.len()
            )));
        }

        // Extract the single payment
        let (asset_id, amount) = &payments[0];

        // Validate fee payment token
        Self::validate_allowed_token(asset_id, policy)?;

        // Validate max fee
        Self::validate_token_max_fee(asset_id, *amount, policy)?;

        // Calculate required XLM fee using estimate_fee (handles Soroban transactions correctly)

        let mut required_xlm_fee = estimate_fee(envelope, provider, None).await.map_err(|e| {
            StellarTransactionValidationError::ValidationError(format!(
                "Failed to estimate fee: {e}",
            ))
        })?;

        let is_soroban = xdr_needs_simulation(envelope).unwrap_or(false);
        if !is_soroban {
            // For regular transactions, fee-bump needs base fee (100 stroops)
            required_xlm_fee += STELLAR_DEFAULT_TRANSACTION_FEE as u64;
        }

        let fee_quote = convert_xlm_fee_to_token(dex_service, policy, required_xlm_fee, asset_id)
            .await
            .map_err(|e| {
                StellarTransactionValidationError::ValidationError(format!(
                    "Failed to convert XLM fee to token {asset_id}: {e}",
                ))
            })?;

        // Compare payment amount with required token amount (from convert_xlm_fee_to_token which includes margin)
        if *amount < fee_quote.fee_in_token {
            return Err(StellarTransactionValidationError::InsufficientTokenPayment(
                fee_quote.fee_in_token,
                *amount,
            ));
        }

        // Validate user token balance
        Self::validate_user_token_balance(envelope, asset_id, fee_quote.fee_in_token, provider)
            .await?;

        Ok(())
    }

    /// Validate that user has sufficient token balance to pay the transaction fee
    ///
    /// This function checks that the user's account has enough balance of the specified
    /// fee token to cover the required transaction fee. This prevents users from getting
    /// quotes or building transactions they cannot afford.
    ///
    /// # Arguments
    /// * `envelope` - The transaction envelope to extract source account from
    /// * `fee_token` - The token identifier (e.g., "native" or "USDC:GA5Z...")
    /// * `required_fee_amount` - The required fee amount in token's smallest unit (stroops)
    /// * `provider` - Provider for Stellar RPC operations to fetch balance
    ///
    /// # Returns
    /// Ok(()) if validation passes, StellarTransactionValidationError if validation fails
    pub async fn validate_user_token_balance<P>(
        envelope: &TransactionEnvelope,
        fee_token: &str,
        required_fee_amount: u64,
        provider: &P,
    ) -> Result<(), StellarTransactionValidationError>
    where
        P: StellarProviderTrait + Send + Sync,
    {
        // Extract source account from envelope
        let source_account = extract_source_account(envelope).map_err(|e| {
            StellarTransactionValidationError::ValidationError(format!(
                "Failed to extract source account: {e}"
            ))
        })?;

        // Fetch user's token balance
        let user_balance = get_token_balance(provider, &source_account, fee_token)
            .await
            .map_err(|e| {
                StellarTransactionValidationError::ValidationError(format!(
                    "Failed to fetch user balance for token {fee_token}: {e}",
                ))
            })?;

        // Check if balance is sufficient
        if user_balance < required_fee_amount {
            return Err(StellarTransactionValidationError::ValidationError(format!(
                "Insufficient balance: user has {user_balance} {fee_token} but needs {required_fee_amount} {fee_token} for transaction fee"
            )));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::transaction::stellar::test_helpers::{
        create_account_id, create_muxed_account, create_native_payment_operation,
        create_simple_v1_envelope, TEST_CONTRACT, TEST_PK, TEST_PK_2,
    };
    use crate::models::{AssetSpec, StellarAllowedTokensPolicy};
    use crate::services::provider::MockStellarProviderTrait;
    use crate::services::stellar_dex::MockStellarDexServiceTrait;
    use futures::future::ready;
    use soroban_rs::xdr::{
        AccountEntry, AccountEntryExt, Asset as XdrAsset, ChangeTrustAsset, ChangeTrustOp,
        HostFunction, InvokeContractArgs, InvokeHostFunctionOp, Operation, OperationBody,
        ScAddress, ScSymbol, SequenceNumber, SorobanAuthorizationEntry, SorobanAuthorizedFunction,
        SorobanCredentials, Thresholds, TimeBounds, TimePoint, Transaction, TransactionEnvelope,
        TransactionExt, TransactionV1Envelope,
    };

    #[test]
    fn test_empty_operations_rejected() {
        let result = validate_operations(&[]);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("at least one operation"));
    }

    #[test]
    fn test_too_many_operations_rejected() {
        let ops = vec![
            OperationSpec::Payment {
                destination: TEST_PK.to_string(),
                amount: 1000,
                asset: AssetSpec::Native,
            };
            101
        ];
        let result = validate_operations(&ops);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("maximum allowed is 100"));
    }

    #[test]
    fn test_soroban_exclusivity_enforced() {
        // Multiple Soroban operations should fail
        let ops = vec![
            OperationSpec::InvokeContract {
                contract_address: TEST_CONTRACT.to_string(),
                function_name: "test".to_string(),
                args: vec![],
                auth: None,
            },
            OperationSpec::CreateContract {
                source: crate::models::ContractSource::Address {
                    address: TEST_PK.to_string(),
                },
                wasm_hash: "abc123".to_string(),
                salt: None,
                constructor_args: None,
                auth: None,
            },
        ];
        let result = validate_operations(&ops);
        assert!(result.is_err());

        // Soroban mixed with non-Soroban should fail
        let ops = vec![
            OperationSpec::InvokeContract {
                contract_address: TEST_CONTRACT.to_string(),
                function_name: "test".to_string(),
                args: vec![],
                auth: None,
            },
            OperationSpec::Payment {
                destination: TEST_PK.to_string(),
                amount: 1000,
                asset: AssetSpec::Native,
            },
        ];
        let result = validate_operations(&ops);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Soroban operations must be exclusive"));
    }

    #[test]
    fn test_soroban_memo_restriction() {
        let soroban_op = vec![OperationSpec::InvokeContract {
            contract_address: TEST_CONTRACT.to_string(),
            function_name: "test".to_string(),
            args: vec![],
            auth: None,
        }];

        // Soroban with text memo should fail
        let result = validate_soroban_memo_restriction(
            &soroban_op,
            &Some(MemoSpec::Text {
                value: "test".to_string(),
            }),
        );
        assert!(result.is_err());

        // Soroban with MemoNone should succeed
        let result = validate_soroban_memo_restriction(&soroban_op, &Some(MemoSpec::None));
        assert!(result.is_ok());

        // Soroban with no memo should succeed
        let result = validate_soroban_memo_restriction(&soroban_op, &None);
        assert!(result.is_ok());
    }

    mod validate_fee_token_structure_tests {
        use super::*;

        #[test]
        fn test_native_xlm_valid() {
            assert!(StellarTransactionValidator::validate_fee_token_structure("native").is_ok());
            assert!(StellarTransactionValidator::validate_fee_token_structure("XLM").is_ok());
            assert!(StellarTransactionValidator::validate_fee_token_structure("").is_ok());
        }

        #[test]
        fn test_contract_address_valid() {
            assert!(
                StellarTransactionValidator::validate_fee_token_structure(TEST_CONTRACT).is_ok()
            );
        }

        #[test]
        fn test_contract_address_invalid_length() {
            let result = StellarTransactionValidator::validate_fee_token_structure("C123");
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("Invalid fee_token format"));
        }

        #[test]
        fn test_classic_asset_valid() {
            let result = StellarTransactionValidator::validate_fee_token_structure(&format!(
                "USDC:{}",
                TEST_PK
            ));
            assert!(result.is_ok());
        }

        #[test]
        fn test_classic_asset_code_too_long() {
            let result = StellarTransactionValidator::validate_fee_token_structure(&format!(
                "VERYLONGCODE1:{}",
                TEST_PK
            ));
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("Invalid asset code length"));
        }

        #[test]
        fn test_classic_asset_invalid_issuer_length() {
            let result = StellarTransactionValidator::validate_fee_token_structure("USDC:GSHORT");
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("Invalid issuer address length"));
        }

        #[test]
        fn test_classic_asset_invalid_issuer_prefix() {
            let result = StellarTransactionValidator::validate_fee_token_structure(
                "USDC:SAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
            );
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("Invalid issuer address prefix"));
        }

        #[test]
        fn test_invalid_format_multiple_colons() {
            let result =
                StellarTransactionValidator::validate_fee_token_structure("USDC:ISSUER:EXTRA");
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("Invalid fee_token format"));
        }
    }

    mod validate_allowed_token_tests {
        use super::*;

        #[test]
        fn test_empty_allowed_list_allows_all() {
            let policy = RelayerStellarPolicy::default();
            assert!(StellarTransactionValidator::validate_allowed_token("native", &policy).is_ok());
            assert!(
                StellarTransactionValidator::validate_allowed_token(TEST_CONTRACT, &policy).is_ok()
            );
        }

        #[test]
        fn test_native_allowed() {
            let mut policy = RelayerStellarPolicy::default();
            policy.allowed_tokens = Some(vec![StellarAllowedTokensPolicy {
                asset: "native".to_string(),
                metadata: None,
                swap_config: None,
                max_allowed_fee: None,
            }]);
            assert!(StellarTransactionValidator::validate_allowed_token("native", &policy).is_ok());
            assert!(StellarTransactionValidator::validate_allowed_token("", &policy).is_ok());
        }

        #[test]
        fn test_native_not_allowed() {
            let mut policy = RelayerStellarPolicy::default();
            policy.allowed_tokens = Some(vec![StellarAllowedTokensPolicy {
                asset: format!("USDC:{}", TEST_PK),
                metadata: None,
                swap_config: None,
                max_allowed_fee: None,
            }]);
            let result = StellarTransactionValidator::validate_allowed_token("native", &policy);
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("Native XLM not in allowed tokens list"));
        }

        #[test]
        fn test_token_allowed() {
            let token = format!("USDC:{}", TEST_PK);
            let mut policy = RelayerStellarPolicy::default();
            policy.allowed_tokens = Some(vec![StellarAllowedTokensPolicy {
                asset: token.clone(),
                metadata: None,
                swap_config: None,
                max_allowed_fee: None,
            }]);
            assert!(StellarTransactionValidator::validate_allowed_token(&token, &policy).is_ok());
        }

        #[test]
        fn test_token_not_allowed() {
            let mut policy = RelayerStellarPolicy::default();
            policy.allowed_tokens = Some(vec![StellarAllowedTokensPolicy {
                asset: format!("USDC:{}", TEST_PK),
                metadata: None,
                swap_config: None,
                max_allowed_fee: None,
            }]);
            let result = StellarTransactionValidator::validate_allowed_token(
                &format!("AQUA:{}", TEST_PK_2),
                &policy,
            );
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("not in allowed tokens list"));
        }
    }

    mod validate_max_fee_tests {
        use super::*;

        #[test]
        fn test_no_max_fee_allows_any() {
            let policy = RelayerStellarPolicy::default();
            assert!(StellarTransactionValidator::validate_max_fee(1_000_000, &policy).is_ok());
        }

        #[test]
        fn test_fee_within_limit() {
            let mut policy = RelayerStellarPolicy::default();
            policy.max_fee = Some(1_000_000);
            assert!(StellarTransactionValidator::validate_max_fee(500_000, &policy).is_ok());
        }

        #[test]
        fn test_fee_exceeds_limit() {
            let mut policy = RelayerStellarPolicy::default();
            policy.max_fee = Some(1_000_000);
            let result = StellarTransactionValidator::validate_max_fee(2_000_000, &policy);
            assert!(result.is_err());
            assert!(result.unwrap_err().to_string().contains("Max fee exceeded"));
        }
    }

    mod validate_token_max_fee_tests {
        use super::*;

        #[test]
        fn test_no_token_entry() {
            let policy = RelayerStellarPolicy::default();
            assert!(StellarTransactionValidator::validate_token_max_fee(
                "USDC:ISSUER",
                1_000_000,
                &policy
            )
            .is_ok());
        }

        #[test]
        fn test_no_max_allowed_fee_in_entry() {
            let mut policy = RelayerStellarPolicy::default();
            policy.allowed_tokens = Some(vec![StellarAllowedTokensPolicy {
                asset: "USDC:ISSUER".to_string(),
                metadata: None,
                swap_config: None,
                max_allowed_fee: None,
            }]);
            assert!(StellarTransactionValidator::validate_token_max_fee(
                "USDC:ISSUER",
                1_000_000,
                &policy
            )
            .is_ok());
        }

        #[test]
        fn test_fee_within_token_limit() {
            let mut policy = RelayerStellarPolicy::default();
            policy.allowed_tokens = Some(vec![StellarAllowedTokensPolicy {
                asset: "USDC:ISSUER".to_string(),
                metadata: None,
                swap_config: None,
                max_allowed_fee: Some(1_000_000),
            }]);
            assert!(StellarTransactionValidator::validate_token_max_fee(
                "USDC:ISSUER",
                500_000,
                &policy
            )
            .is_ok());
        }

        #[test]
        fn test_fee_exceeds_token_limit() {
            let mut policy = RelayerStellarPolicy::default();
            policy.allowed_tokens = Some(vec![StellarAllowedTokensPolicy {
                asset: "USDC:ISSUER".to_string(),
                metadata: None,
                swap_config: None,
                max_allowed_fee: Some(1_000_000),
            }]);
            let result = StellarTransactionValidator::validate_token_max_fee(
                "USDC:ISSUER",
                2_000_000,
                &policy,
            );
            assert!(result.is_err());
            assert!(result.unwrap_err().to_string().contains("Max fee exceeded"));
        }
    }

    mod extract_relayer_payments_tests {
        use super::*;

        #[test]
        fn test_extract_single_payment() {
            let envelope = create_simple_v1_envelope(TEST_PK, TEST_PK_2);
            let payments =
                StellarTransactionValidator::extract_relayer_payments(&envelope, TEST_PK_2)
                    .unwrap();
            assert_eq!(payments.len(), 1);
            assert_eq!(payments[0].0, "native");
            assert_eq!(payments[0].1, 1_000_000);
        }

        #[test]
        fn test_extract_no_payments_to_relayer() {
            let envelope = create_simple_v1_envelope(TEST_PK, TEST_PK_2);
            let payments =
                StellarTransactionValidator::extract_relayer_payments(&envelope, TEST_PK).unwrap();
            assert_eq!(payments.len(), 0);
        }

        #[test]
        fn test_extract_negative_amount_rejected() {
            let payment_op = Operation {
                source_account: None,
                body: OperationBody::Payment(soroban_rs::xdr::PaymentOp {
                    destination: create_muxed_account(TEST_PK_2),
                    asset: XdrAsset::Native,
                    amount: -100, // Negative amount
                }),
            };

            let tx = Transaction {
                source_account: create_muxed_account(TEST_PK),
                fee: 100,
                seq_num: SequenceNumber(1),
                cond: soroban_rs::xdr::Preconditions::None,
                memo: soroban_rs::xdr::Memo::None,
                operations: vec![payment_op].try_into().unwrap(),
                ext: TransactionExt::V0,
            };

            let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
                tx,
                signatures: vec![].try_into().unwrap(),
            });

            let result =
                StellarTransactionValidator::extract_relayer_payments(&envelope, TEST_PK_2);
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("Negative payment amount"));
        }
    }

    mod validate_time_bounds_tests {
        use super::*;

        #[test]
        fn test_no_time_bounds_is_ok() {
            let envelope = create_simple_v1_envelope(TEST_PK, TEST_PK_2);
            assert!(
                StellarTransactionValidator::validate_time_bounds_not_expired(&envelope).is_ok()
            );
        }

        #[test]
        fn test_valid_time_bounds() {
            let now = Utc::now().timestamp() as u64;
            let payment_op = create_native_payment_operation(TEST_PK_2, 1_000_000);

            let tx = Transaction {
                source_account: create_muxed_account(TEST_PK),
                fee: 100,
                seq_num: SequenceNumber(1),
                cond: soroban_rs::xdr::Preconditions::Time(TimeBounds {
                    min_time: TimePoint(now - 60),
                    max_time: TimePoint(now + 60),
                }),
                memo: soroban_rs::xdr::Memo::None,
                operations: vec![payment_op].try_into().unwrap(),
                ext: TransactionExt::V0,
            };

            let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
                tx,
                signatures: vec![].try_into().unwrap(),
            });

            assert!(
                StellarTransactionValidator::validate_time_bounds_not_expired(&envelope).is_ok()
            );
        }

        #[test]
        fn test_expired_transaction() {
            let now = Utc::now().timestamp() as u64;
            let payment_op = create_native_payment_operation(TEST_PK_2, 1_000_000);

            let tx = Transaction {
                source_account: create_muxed_account(TEST_PK),
                fee: 100,
                seq_num: SequenceNumber(1),
                cond: soroban_rs::xdr::Preconditions::Time(TimeBounds {
                    min_time: TimePoint(now - 120),
                    max_time: TimePoint(now - 60), // Expired
                }),
                memo: soroban_rs::xdr::Memo::None,
                operations: vec![payment_op].try_into().unwrap(),
                ext: TransactionExt::V0,
            };

            let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
                tx,
                signatures: vec![].try_into().unwrap(),
            });

            let result = StellarTransactionValidator::validate_time_bounds_not_expired(&envelope);
            assert!(result.is_err());
            assert!(result.unwrap_err().to_string().contains("has expired"));
        }

        #[test]
        fn test_not_yet_valid_transaction() {
            let now = Utc::now().timestamp() as u64;
            let payment_op = create_native_payment_operation(TEST_PK_2, 1_000_000);

            let tx = Transaction {
                source_account: create_muxed_account(TEST_PK),
                fee: 100,
                seq_num: SequenceNumber(1),
                cond: soroban_rs::xdr::Preconditions::Time(TimeBounds {
                    min_time: TimePoint(now + 60), // Not yet valid
                    max_time: TimePoint(now + 120),
                }),
                memo: soroban_rs::xdr::Memo::None,
                operations: vec![payment_op].try_into().unwrap(),
                ext: TransactionExt::V0,
            };

            let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
                tx,
                signatures: vec![].try_into().unwrap(),
            });

            let result = StellarTransactionValidator::validate_time_bounds_not_expired(&envelope);
            assert!(result.is_err());
            assert!(result.unwrap_err().to_string().contains("not yet valid"));
        }
    }

    mod validate_transaction_validity_duration_tests {
        use super::*;

        #[test]
        fn test_duration_within_limit() {
            let now = Utc::now().timestamp() as u64;
            let payment_op = create_native_payment_operation(TEST_PK_2, 1_000_000);

            let tx = Transaction {
                source_account: create_muxed_account(TEST_PK),
                fee: 100,
                seq_num: SequenceNumber(1),
                cond: soroban_rs::xdr::Preconditions::Time(TimeBounds {
                    min_time: TimePoint(0),
                    max_time: TimePoint(now + 60), // 1 minute from now
                }),
                memo: soroban_rs::xdr::Memo::None,
                operations: vec![payment_op].try_into().unwrap(),
                ext: TransactionExt::V0,
            };

            let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
                tx,
                signatures: vec![].try_into().unwrap(),
            });

            let max_duration = Duration::minutes(5);
            assert!(
                StellarTransactionValidator::validate_transaction_validity_duration(
                    &envelope,
                    max_duration
                )
                .is_ok()
            );
        }

        #[test]
        fn test_duration_exceeds_limit() {
            let now = Utc::now().timestamp() as u64;
            let payment_op = create_native_payment_operation(TEST_PK_2, 1_000_000);

            let tx = Transaction {
                source_account: create_muxed_account(TEST_PK),
                fee: 100,
                seq_num: SequenceNumber(1),
                cond: soroban_rs::xdr::Preconditions::Time(TimeBounds {
                    min_time: TimePoint(0),
                    max_time: TimePoint(now + 600), // 10 minutes from now
                }),
                memo: soroban_rs::xdr::Memo::None,
                operations: vec![payment_op].try_into().unwrap(),
                ext: TransactionExt::V0,
            };

            let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
                tx,
                signatures: vec![].try_into().unwrap(),
            });

            let max_duration = Duration::minutes(5);
            let result = StellarTransactionValidator::validate_transaction_validity_duration(
                &envelope,
                max_duration,
            );
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("exceeds maximum allowed duration"));
        }

        #[test]
        fn test_no_time_bounds_rejected() {
            let envelope = create_simple_v1_envelope(TEST_PK, TEST_PK_2);
            let max_duration = Duration::minutes(5);
            let result = StellarTransactionValidator::validate_transaction_validity_duration(
                &envelope,
                max_duration,
            );
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("must have time bounds set"));
        }
    }

    mod validate_sequence_number_tests {
        use super::*;

        #[tokio::test]
        async fn test_valid_sequence_number() {
            let envelope = create_simple_v1_envelope(TEST_PK, TEST_PK_2);

            let mut provider = MockStellarProviderTrait::new();
            provider.expect_get_account().returning(|_| {
                Box::pin(ready(Ok(AccountEntry {
                    account_id: create_account_id(TEST_PK),
                    balance: 1_000_000_000,
                    seq_num: SequenceNumber(0), // Current sequence is 0, tx sequence is 1
                    num_sub_entries: 0,
                    inflation_dest: None,
                    flags: 0,
                    home_domain: Default::default(),
                    thresholds: Thresholds([0; 4]),
                    signers: Default::default(),
                    ext: AccountEntryExt::V0,
                })))
            });

            assert!(
                StellarTransactionValidator::validate_sequence_number(&envelope, &provider)
                    .await
                    .is_ok()
            );
        }

        #[tokio::test]
        async fn test_equal_sequence_rejected() {
            let envelope = create_simple_v1_envelope(TEST_PK, TEST_PK_2);

            let mut provider = MockStellarProviderTrait::new();
            provider.expect_get_account().returning(|_| {
                Box::pin(ready(Ok(AccountEntry {
                    account_id: create_account_id(TEST_PK),
                    balance: 1_000_000_000,
                    seq_num: SequenceNumber(1), // Same as tx sequence
                    num_sub_entries: 0,
                    inflation_dest: None,
                    flags: 0,
                    home_domain: Default::default(),
                    thresholds: Thresholds([0; 4]),
                    signers: Default::default(),
                    ext: AccountEntryExt::V0,
                })))
            });

            let result =
                StellarTransactionValidator::validate_sequence_number(&envelope, &provider).await;
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("strictly greater than"));
        }

        #[tokio::test]
        async fn test_past_sequence_rejected() {
            let envelope = create_simple_v1_envelope(TEST_PK, TEST_PK_2);

            let mut provider = MockStellarProviderTrait::new();
            provider.expect_get_account().returning(|_| {
                Box::pin(ready(Ok(AccountEntry {
                    account_id: create_account_id(TEST_PK),
                    balance: 1_000_000_000,
                    seq_num: SequenceNumber(10), // Higher than tx sequence
                    num_sub_entries: 0,
                    inflation_dest: None,
                    flags: 0,
                    home_domain: Default::default(),
                    thresholds: Thresholds([0; 4]),
                    signers: Default::default(),
                    ext: AccountEntryExt::V0,
                })))
            });

            let result =
                StellarTransactionValidator::validate_sequence_number(&envelope, &provider).await;
            assert!(result.is_err());
            assert!(result.unwrap_err().to_string().contains("is invalid"));
        }
    }

    mod validate_operations_count_tests {
        use super::*;

        #[test]
        fn test_valid_operations_count() {
            let envelope = create_simple_v1_envelope(TEST_PK, TEST_PK_2);
            assert!(StellarTransactionValidator::validate_operations_count(&envelope).is_ok());
        }

        #[test]
        fn test_too_many_operations() {
            // VecM has a max of 100, so we can't actually create an envelope with 101 operations
            // Instead, we test that the validation logic works correctly by checking the limit
            // This test verifies the validation function would reject if it could receive such an envelope

            // Create an envelope with exactly 100 operations (the maximum)
            let operations: Vec<Operation> = (0..100)
                .map(|_| create_native_payment_operation(TEST_PK_2, 100))
                .collect();

            let tx = Transaction {
                source_account: create_muxed_account(TEST_PK),
                fee: 100,
                seq_num: SequenceNumber(1),
                cond: soroban_rs::xdr::Preconditions::None,
                memo: soroban_rs::xdr::Memo::None,
                operations: operations.try_into().unwrap(),
                ext: TransactionExt::V0,
            };

            let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
                tx,
                signatures: vec![].try_into().unwrap(),
            });

            // 100 operations should be OK
            let result = StellarTransactionValidator::validate_operations_count(&envelope);
            assert!(result.is_ok());
        }
    }

    mod validate_source_account_tests {
        use super::*;

        #[test]
        fn test_source_account_not_relayer() {
            let envelope = create_simple_v1_envelope(TEST_PK, TEST_PK_2);
            assert!(
                StellarTransactionValidator::validate_source_account_not_relayer(
                    &envelope, TEST_PK_2
                )
                .is_ok()
            );
        }

        #[test]
        fn test_source_account_is_relayer_rejected() {
            let envelope = create_simple_v1_envelope(TEST_PK, TEST_PK_2);
            let result = StellarTransactionValidator::validate_source_account_not_relayer(
                &envelope, TEST_PK,
            );
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("cannot be the relayer address"));
        }
    }

    mod validate_operation_types_tests {
        use super::*;

        #[test]
        fn test_payment_operation_allowed() {
            let envelope = create_simple_v1_envelope(TEST_PK, TEST_PK_2);
            let policy = RelayerStellarPolicy::default();
            assert!(StellarTransactionValidator::validate_operation_types(
                &envelope, TEST_PK_2, &policy
            )
            .is_ok());
        }

        #[test]
        fn test_account_merge_rejected() {
            let operation = Operation {
                source_account: None,
                body: OperationBody::AccountMerge(create_muxed_account(TEST_PK_2)),
            };

            let tx = Transaction {
                source_account: create_muxed_account(TEST_PK),
                fee: 100,
                seq_num: SequenceNumber(1),
                cond: soroban_rs::xdr::Preconditions::None,
                memo: soroban_rs::xdr::Memo::None,
                operations: vec![operation].try_into().unwrap(),
                ext: TransactionExt::V0,
            };

            let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
                tx,
                signatures: vec![].try_into().unwrap(),
            });

            let policy = RelayerStellarPolicy::default();
            let result = StellarTransactionValidator::validate_operation_types(
                &envelope, TEST_PK_2, &policy,
            );
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("AccountMerge operations are not allowed"));
        }

        #[test]
        fn test_set_options_rejected() {
            let operation = Operation {
                source_account: None,
                body: OperationBody::SetOptions(soroban_rs::xdr::SetOptionsOp {
                    inflation_dest: None,
                    clear_flags: None,
                    set_flags: None,
                    master_weight: None,
                    low_threshold: None,
                    med_threshold: None,
                    high_threshold: None,
                    home_domain: None,
                    signer: None,
                }),
            };

            let tx = Transaction {
                source_account: create_muxed_account(TEST_PK),
                fee: 100,
                seq_num: SequenceNumber(1),
                cond: soroban_rs::xdr::Preconditions::None,
                memo: soroban_rs::xdr::Memo::None,
                operations: vec![operation].try_into().unwrap(),
                ext: TransactionExt::V0,
            };

            let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
                tx,
                signatures: vec![].try_into().unwrap(),
            });

            let policy = RelayerStellarPolicy::default();
            let result = StellarTransactionValidator::validate_operation_types(
                &envelope, TEST_PK_2, &policy,
            );
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("SetOptions operations are not allowed"));
        }

        #[test]
        fn test_change_trust_allowed() {
            let operation = Operation {
                source_account: None,
                body: OperationBody::ChangeTrust(ChangeTrustOp {
                    line: ChangeTrustAsset::CreditAlphanum4(soroban_rs::xdr::AlphaNum4 {
                        asset_code: soroban_rs::xdr::AssetCode4(*b"USDC"),
                        issuer: create_account_id(TEST_PK_2),
                    }),
                    limit: 1_000_000_000,
                }),
            };

            let tx = Transaction {
                source_account: create_muxed_account(TEST_PK),
                fee: 100,
                seq_num: SequenceNumber(1),
                cond: soroban_rs::xdr::Preconditions::None,
                memo: soroban_rs::xdr::Memo::None,
                operations: vec![operation].try_into().unwrap(),
                ext: TransactionExt::V0,
            };

            let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
                tx,
                signatures: vec![].try_into().unwrap(),
            });

            let policy = RelayerStellarPolicy::default();
            assert!(StellarTransactionValidator::validate_operation_types(
                &envelope, TEST_PK_2, &policy
            )
            .is_ok());
        }
    }

    mod validate_token_payment_tests {
        use super::*;

        #[test]
        fn test_valid_native_payment() {
            let envelope = create_simple_v1_envelope(TEST_PK, TEST_PK_2);
            let policy = RelayerStellarPolicy::default();

            let result = StellarTransactionValidator::validate_token_payment(
                &envelope, TEST_PK_2, "native", 1_000_000, &policy,
            );
            assert!(result.is_ok());
        }

        #[test]
        fn test_no_payment_to_relayer() {
            let envelope = create_simple_v1_envelope(TEST_PK, TEST_PK_2);
            let policy = RelayerStellarPolicy::default();

            // Wrong relayer address - no payments will match
            let result = StellarTransactionValidator::validate_token_payment(
                &envelope, TEST_PK, // Different from destination
                "native", 1_000_000, &policy,
            );
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("No payment operation found to relayer"));
        }

        #[test]
        fn test_wrong_token_in_payment() {
            let envelope = create_simple_v1_envelope(TEST_PK, TEST_PK_2);
            let policy = RelayerStellarPolicy::default();

            // Expecting USDC but envelope has native payment
            let result = StellarTransactionValidator::validate_token_payment(
                &envelope,
                TEST_PK_2,
                &format!("USDC:{}", TEST_PK),
                1_000_000,
                &policy,
            );
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("No payment found for expected token"));
        }

        #[test]
        fn test_insufficient_payment_amount() {
            let envelope = create_simple_v1_envelope(TEST_PK, TEST_PK_2);
            let policy = RelayerStellarPolicy::default();

            // Expecting 2M but envelope has 1M payment
            let result = StellarTransactionValidator::validate_token_payment(
                &envelope, TEST_PK_2, "native", 2_000_000, &policy,
            );
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("Insufficient token payment"));
        }

        #[test]
        fn test_payment_within_tolerance() {
            let envelope = create_simple_v1_envelope(TEST_PK, TEST_PK_2);
            let policy = RelayerStellarPolicy::default();

            let result = StellarTransactionValidator::validate_token_payment(
                &envelope, TEST_PK_2, "native", 990_000, &policy,
            );
            assert!(result.is_ok());
        }

        #[test]
        fn test_token_not_in_allowed_list() {
            let envelope = create_simple_v1_envelope(TEST_PK, TEST_PK_2);
            let mut policy = RelayerStellarPolicy::default();
            policy.allowed_tokens = Some(vec![StellarAllowedTokensPolicy {
                asset: format!("USDC:{}", TEST_PK),
                metadata: None,
                swap_config: None,
                max_allowed_fee: None,
            }]);

            // Native payment but only USDC is allowed
            let result = StellarTransactionValidator::validate_token_payment(
                &envelope, TEST_PK_2, "native", 1_000_000, &policy,
            );
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("not in allowed tokens list"));
        }

        #[test]
        fn test_payment_exceeds_token_max_fee() {
            let envelope = create_simple_v1_envelope(TEST_PK, TEST_PK_2);
            let mut policy = RelayerStellarPolicy::default();
            policy.allowed_tokens = Some(vec![StellarAllowedTokensPolicy {
                asset: "native".to_string(),
                metadata: None,
                swap_config: None,
                max_allowed_fee: Some(500_000), // Max 0.5 XLM
            }]);

            // Payment is 1M but max allowed is 500K
            let result = StellarTransactionValidator::validate_token_payment(
                &envelope, TEST_PK_2, "native", 1_000_000, &policy,
            );
            assert!(result.is_err());
            assert!(result.unwrap_err().to_string().contains("Max fee exceeded"));
        }

        #[test]
        fn test_classic_asset_payment() {
            let usdc_asset = format!("USDC:{}", TEST_PK);
            let payment_op = Operation {
                source_account: None,
                body: OperationBody::Payment(soroban_rs::xdr::PaymentOp {
                    destination: create_muxed_account(TEST_PK_2),
                    asset: XdrAsset::CreditAlphanum4(soroban_rs::xdr::AlphaNum4 {
                        asset_code: soroban_rs::xdr::AssetCode4(*b"USDC"),
                        issuer: create_account_id(TEST_PK),
                    }),
                    amount: 1_000_000,
                }),
            };

            let tx = Transaction {
                source_account: create_muxed_account(TEST_PK),
                fee: 100,
                seq_num: SequenceNumber(1),
                cond: soroban_rs::xdr::Preconditions::None,
                memo: soroban_rs::xdr::Memo::None,
                operations: vec![payment_op].try_into().unwrap(),
                ext: TransactionExt::V0,
            };

            let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
                tx,
                signatures: vec![].try_into().unwrap(),
            });

            let mut policy = RelayerStellarPolicy::default();
            policy.allowed_tokens = Some(vec![StellarAllowedTokensPolicy {
                asset: usdc_asset.clone(),
                metadata: None,
                swap_config: None,
                max_allowed_fee: None,
            }]);

            let result = StellarTransactionValidator::validate_token_payment(
                &envelope,
                TEST_PK_2,
                &usdc_asset,
                1_000_000,
                &policy,
            );
            assert!(result.is_ok());
        }

        #[test]
        fn test_multiple_payments_finds_correct_token() {
            // Create envelope with two payments: one USDC to relayer, one XLM to someone else
            let usdc_asset = format!("USDC:{}", TEST_PK);
            let usdc_payment = Operation {
                source_account: None,
                body: OperationBody::Payment(soroban_rs::xdr::PaymentOp {
                    destination: create_muxed_account(TEST_PK_2),
                    asset: XdrAsset::CreditAlphanum4(soroban_rs::xdr::AlphaNum4 {
                        asset_code: soroban_rs::xdr::AssetCode4(*b"USDC"),
                        issuer: create_account_id(TEST_PK),
                    }),
                    amount: 500_000,
                }),
            };

            let xlm_payment = create_native_payment_operation(TEST_PK, 1_000_000);

            let tx = Transaction {
                source_account: create_muxed_account(TEST_PK),
                fee: 100,
                seq_num: SequenceNumber(1),
                cond: soroban_rs::xdr::Preconditions::None,
                memo: soroban_rs::xdr::Memo::None,
                operations: vec![xlm_payment, usdc_payment].try_into().unwrap(),
                ext: TransactionExt::V0,
            };

            let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
                tx,
                signatures: vec![].try_into().unwrap(),
            });

            let policy = RelayerStellarPolicy::default();

            // Should find the USDC payment to TEST_PK_2
            let result = StellarTransactionValidator::validate_token_payment(
                &envelope,
                TEST_PK_2,
                &usdc_asset,
                500_000,
                &policy,
            );
            assert!(result.is_ok());
        }
    }

    mod validate_user_fee_payment_amounts_tests {
        use super::*;
        use soroban_rs::stellar_rpc_client::{
            GetLatestLedgerResponse, SimulateTransactionResponse,
        };
        use soroban_rs::xdr::WriteXdr;

        const USDC_ISSUER: &str = TEST_PK;

        fn create_usdc_payment_envelope(
            source: &str,
            destination: &str,
            amount: i64,
        ) -> TransactionEnvelope {
            let payment_op = Operation {
                source_account: None,
                body: OperationBody::Payment(PaymentOp {
                    destination: create_muxed_account(destination),
                    asset: soroban_rs::xdr::Asset::CreditAlphanum4(soroban_rs::xdr::AlphaNum4 {
                        asset_code: soroban_rs::xdr::AssetCode4(*b"USDC"),
                        issuer: create_account_id(USDC_ISSUER),
                    }),
                    amount,
                }),
            };

            let tx = Transaction {
                source_account: create_muxed_account(source),
                fee: 100,
                seq_num: SequenceNumber(1),
                cond: soroban_rs::xdr::Preconditions::None,
                memo: soroban_rs::xdr::Memo::None,
                operations: vec![payment_op].try_into().unwrap(),
                ext: TransactionExt::V0,
            };

            TransactionEnvelope::Tx(TransactionV1Envelope {
                tx,
                signatures: vec![].try_into().unwrap(),
            })
        }

        fn create_usdc_policy() -> RelayerStellarPolicy {
            let usdc_asset = format!("USDC:{}", USDC_ISSUER);
            let mut policy = RelayerStellarPolicy::default();
            policy.allowed_tokens = Some(vec![StellarAllowedTokensPolicy {
                asset: usdc_asset,
                metadata: None,
                swap_config: None,
                max_allowed_fee: None,
            }]);
            policy
        }

        fn create_mock_provider_with_balance(balance: i64) -> MockStellarProviderTrait {
            let mut provider = MockStellarProviderTrait::new();

            // Mock get_account for source account
            provider.expect_get_account().returning(move |_| {
                Box::pin(ready(Ok(AccountEntry {
                    account_id: create_account_id(TEST_PK),
                    balance,
                    seq_num: SequenceNumber(1),
                    num_sub_entries: 0,
                    inflation_dest: None,
                    flags: 0,
                    home_domain: Default::default(),
                    thresholds: Thresholds([0; 4]),
                    signers: Default::default(),
                    ext: AccountEntryExt::V0,
                })))
            });

            // Mock get_latest_ledger for fee estimation
            provider.expect_get_latest_ledger().returning(|| {
                Box::pin(ready(Ok(GetLatestLedgerResponse {
                    id: "test".to_string(),
                    protocol_version: 20,
                    sequence: 1000,
                })))
            });

            // Mock simulate_transaction_envelope for Soroban fee estimation
            provider
                .expect_simulate_transaction_envelope()
                .returning(|_| {
                    Box::pin(ready(Ok(SimulateTransactionResponse {
                        min_resource_fee: 100,
                        transaction_data: String::new(),
                        ..Default::default()
                    })))
                });

            // Mock get_ledger_entries for trustline balance check
            provider.expect_get_ledger_entries().returning(|_| {
                use soroban_rs::stellar_rpc_client::{GetLedgerEntriesResponse, LedgerEntryResult};
                use soroban_rs::xdr::{
                    LedgerEntry, LedgerEntryData, LedgerEntryExt, TrustLineAsset, TrustLineEntry,
                    TrustLineEntryExt,
                };

                let trustline_entry = TrustLineEntry {
                    account_id: create_account_id(TEST_PK),
                    asset: TrustLineAsset::CreditAlphanum4(soroban_rs::xdr::AlphaNum4 {
                        asset_code: soroban_rs::xdr::AssetCode4(*b"USDC"),
                        issuer: create_account_id(TEST_PK_2),
                    }),
                    balance: 10_000_000, // 10 USDC
                    limit: 1_000_000_000,
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

            provider
        }

        fn create_mock_dex_service() -> MockStellarDexServiceTrait {
            let mut dex_service = MockStellarDexServiceTrait::new();
            dex_service
                .expect_get_xlm_to_token_quote()
                .returning(|_, _, _, _| {
                    Box::pin(ready(Ok(
                        crate::services::stellar_dex::StellarQuoteResponse {
                            input_asset: "native".to_string(),
                            output_asset: format!("USDC:{}", USDC_ISSUER),
                            in_amount: 100,
                            out_amount: 1_000_000, // 0.1 USDC
                            price_impact_pct: 0.0,
                            slippage_bps: 100,
                            path: None,
                        },
                    )))
                });
            dex_service
        }

        #[tokio::test]
        async fn test_valid_fee_payment() {
            let envelope = create_usdc_payment_envelope(TEST_PK, TEST_PK_2, 1_000_000);
            let policy = create_usdc_policy();
            let provider = create_mock_provider_with_balance(10_000_000_000);
            let dex_service = create_mock_dex_service();

            let result = StellarTransactionValidator::validate_user_fee_payment_amounts(
                &envelope,
                TEST_PK_2,
                &policy,
                &provider,
                &dex_service,
            )
            .await;

            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn test_no_fee_payment() {
            // Envelope with payment to different address (not the relayer)
            let envelope = create_usdc_payment_envelope(TEST_PK, TEST_PK, 1_000_000);
            let policy = create_usdc_policy();
            let provider = create_mock_provider_with_balance(10_000_000_000);
            let dex_service = create_mock_dex_service();

            let result = StellarTransactionValidator::validate_user_fee_payment_amounts(
                &envelope,
                TEST_PK_2, // Different from destination
                &policy,
                &provider,
                &dex_service,
            )
            .await;

            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("must include a fee payment operation to the relayer"));
        }

        #[tokio::test]
        async fn test_multiple_fee_payments_rejected() {
            // Create envelope with two USDC payments to relayer
            let payment1 = Operation {
                source_account: None,
                body: OperationBody::Payment(PaymentOp {
                    destination: create_muxed_account(TEST_PK_2),
                    asset: soroban_rs::xdr::Asset::CreditAlphanum4(soroban_rs::xdr::AlphaNum4 {
                        asset_code: soroban_rs::xdr::AssetCode4(*b"USDC"),
                        issuer: create_account_id(USDC_ISSUER),
                    }),
                    amount: 500_000,
                }),
            };
            let payment2 = Operation {
                source_account: None,
                body: OperationBody::Payment(PaymentOp {
                    destination: create_muxed_account(TEST_PK_2),
                    asset: soroban_rs::xdr::Asset::CreditAlphanum4(soroban_rs::xdr::AlphaNum4 {
                        asset_code: soroban_rs::xdr::AssetCode4(*b"USDC"),
                        issuer: create_account_id(USDC_ISSUER),
                    }),
                    amount: 500_000,
                }),
            };

            let tx = Transaction {
                source_account: create_muxed_account(TEST_PK),
                fee: 100,
                seq_num: SequenceNumber(1),
                cond: soroban_rs::xdr::Preconditions::None,
                memo: soroban_rs::xdr::Memo::None,
                operations: vec![payment1, payment2].try_into().unwrap(),
                ext: TransactionExt::V0,
            };

            let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
                tx,
                signatures: vec![].try_into().unwrap(),
            });

            let policy = create_usdc_policy();
            let provider = create_mock_provider_with_balance(10_000_000_000);
            let dex_service = create_mock_dex_service();

            let result = StellarTransactionValidator::validate_user_fee_payment_amounts(
                &envelope,
                TEST_PK_2,
                &policy,
                &provider,
                &dex_service,
            )
            .await;

            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("exactly one fee payment operation"));
        }

        #[tokio::test]
        async fn test_token_not_allowed() {
            // Create envelope with EURC payment (not in allowed list)
            let payment_op = Operation {
                source_account: None,
                body: OperationBody::Payment(PaymentOp {
                    destination: create_muxed_account(TEST_PK_2),
                    asset: soroban_rs::xdr::Asset::CreditAlphanum4(soroban_rs::xdr::AlphaNum4 {
                        asset_code: soroban_rs::xdr::AssetCode4(*b"EURC"),
                        issuer: create_account_id(TEST_PK),
                    }),
                    amount: 1_000_000,
                }),
            };

            let tx = Transaction {
                source_account: create_muxed_account(TEST_PK),
                fee: 100,
                seq_num: SequenceNumber(1),
                cond: soroban_rs::xdr::Preconditions::None,
                memo: soroban_rs::xdr::Memo::None,
                operations: vec![payment_op].try_into().unwrap(),
                ext: TransactionExt::V0,
            };

            let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
                tx,
                signatures: vec![].try_into().unwrap(),
            });

            let policy = create_usdc_policy(); // Only USDC is allowed

            let provider = create_mock_provider_with_balance(10_000_000_000);
            let dex_service = create_mock_dex_service();

            let result = StellarTransactionValidator::validate_user_fee_payment_amounts(
                &envelope,
                TEST_PK_2,
                &policy,
                &provider,
                &dex_service,
            )
            .await;

            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("not in allowed tokens list"));
        }

        #[tokio::test]
        async fn test_fee_exceeds_token_max() {
            let envelope = create_usdc_payment_envelope(TEST_PK, TEST_PK_2, 1_000_000);
            let usdc_asset = format!("USDC:{}", USDC_ISSUER);
            let mut policy = RelayerStellarPolicy::default();
            policy.allowed_tokens = Some(vec![StellarAllowedTokensPolicy {
                asset: usdc_asset,
                metadata: None,
                swap_config: None,
                max_allowed_fee: Some(500_000), // Lower than payment amount
            }]);

            let provider = create_mock_provider_with_balance(10_000_000_000);
            let dex_service = create_mock_dex_service();

            let result = StellarTransactionValidator::validate_user_fee_payment_amounts(
                &envelope,
                TEST_PK_2,
                &policy,
                &provider,
                &dex_service,
            )
            .await;

            assert!(result.is_err());
            assert!(result.unwrap_err().to_string().contains("Max fee exceeded"));
        }

        #[tokio::test]
        async fn test_insufficient_payment_amount() {
            let envelope = create_usdc_payment_envelope(TEST_PK, TEST_PK_2, 1_000_000);
            let policy = create_usdc_policy();
            let provider = create_mock_provider_with_balance(10_000_000_000);

            // Mock DEX to require more than the payment amount
            let mut dex_service = MockStellarDexServiceTrait::new();
            dex_service
                .expect_get_xlm_to_token_quote()
                .returning(|_, _, _, _| {
                    Box::pin(ready(Ok(
                        crate::services::stellar_dex::StellarQuoteResponse {
                            input_asset: "native".to_string(),
                            output_asset: "USDC:...".to_string(),
                            in_amount: 200,
                            out_amount: 2_000_000, // More than the 1M payment
                            price_impact_pct: 0.0,
                            slippage_bps: 100,
                            path: None,
                        },
                    )))
                });

            let result = StellarTransactionValidator::validate_user_fee_payment_amounts(
                &envelope,
                TEST_PK_2,
                &policy,
                &provider,
                &dex_service,
            )
            .await;

            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("Insufficient token payment"));
        }

        #[tokio::test]
        async fn test_insufficient_user_balance() {
            let envelope = create_usdc_payment_envelope(TEST_PK, TEST_PK_2, 1_000_000);
            let policy = create_usdc_policy();

            // Create provider with low USDC trustline balance
            let mut provider = MockStellarProviderTrait::new();

            provider.expect_get_account().returning(move |_| {
                Box::pin(ready(Ok(AccountEntry {
                    account_id: create_account_id(TEST_PK),
                    balance: 10_000_000_000,
                    seq_num: SequenceNumber(1),
                    num_sub_entries: 0,
                    inflation_dest: None,
                    flags: 0,
                    home_domain: Default::default(),
                    thresholds: Thresholds([0; 4]),
                    signers: Default::default(),
                    ext: AccountEntryExt::V0,
                })))
            });

            provider.expect_get_latest_ledger().returning(|| {
                Box::pin(ready(Ok(GetLatestLedgerResponse {
                    id: "test".to_string(),
                    protocol_version: 20,
                    sequence: 1000,
                })))
            });

            provider
                .expect_simulate_transaction_envelope()
                .returning(|_| {
                    Box::pin(ready(Ok(SimulateTransactionResponse {
                        min_resource_fee: 100,
                        transaction_data: String::new(),
                        ..Default::default()
                    })))
                });

            // Mock get_ledger_entries with low USDC balance
            provider.expect_get_ledger_entries().returning(|_| {
                use soroban_rs::stellar_rpc_client::{GetLedgerEntriesResponse, LedgerEntryResult};
                use soroban_rs::xdr::{
                    LedgerEntry, LedgerEntryData, LedgerEntryExt, TrustLineAsset, TrustLineEntry,
                    TrustLineEntryExt,
                };

                let trustline_entry = TrustLineEntry {
                    account_id: create_account_id(TEST_PK),
                    asset: TrustLineAsset::CreditAlphanum4(soroban_rs::xdr::AlphaNum4 {
                        asset_code: soroban_rs::xdr::AssetCode4(*b"USDC"),
                        issuer: create_account_id(USDC_ISSUER),
                    }),
                    balance: 500_000, // Only 0.05 USDC - insufficient
                    limit: 1_000_000_000,
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

            let dex_service = create_mock_dex_service();

            let result = StellarTransactionValidator::validate_user_fee_payment_amounts(
                &envelope,
                TEST_PK_2,
                &policy,
                &provider,
                &dex_service,
            )
            .await;

            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("Insufficient balance"));
        }

        #[tokio::test]
        async fn test_valid_fee_payment_with_usdc() {
            let envelope = create_usdc_payment_envelope(TEST_PK, TEST_PK_2, 1_000_000);
            let policy = create_usdc_policy();
            let provider = create_mock_provider_with_balance(10_000_000_000);
            let dex_service = create_mock_dex_service();

            let result = StellarTransactionValidator::validate_user_fee_payment_amounts(
                &envelope,
                TEST_PK_2,
                &policy,
                &provider,
                &dex_service,
            )
            .await;

            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn test_dex_conversion_failure() {
            let envelope = create_usdc_payment_envelope(TEST_PK, TEST_PK_2, 1_000_000);
            let policy = create_usdc_policy();
            let provider = create_mock_provider_with_balance(10_000_000_000);

            let mut dex_service = MockStellarDexServiceTrait::new();
            dex_service
                .expect_get_xlm_to_token_quote()
                .returning(|_, _, _, _| {
                    Box::pin(ready(Err(
                        crate::services::stellar_dex::StellarDexServiceError::UnknownError(
                            "DEX unavailable".to_string(),
                        ),
                    )))
                });

            let result = StellarTransactionValidator::validate_user_fee_payment_amounts(
                &envelope,
                TEST_PK_2,
                &policy,
                &provider,
                &dex_service,
            )
            .await;

            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("Failed to convert XLM fee to token"));
        }
    }

    mod validate_contract_invocation_tests {
        use super::*;

        #[test]
        fn test_invoke_contract_allowed() {
            let invoke_op = InvokeHostFunctionOp {
                host_function: HostFunction::InvokeContract(InvokeContractArgs {
                    contract_address: ScAddress::Contract(soroban_rs::xdr::ContractId(
                        soroban_rs::xdr::Hash([0u8; 32]),
                    )),
                    function_name: ScSymbol("test".try_into().unwrap()),
                    args: Default::default(),
                }),
                auth: Default::default(),
            };

            let policy = RelayerStellarPolicy::default();
            assert!(StellarTransactionValidator::validate_contract_invocation(
                &invoke_op, 0, TEST_PK_2, &policy
            )
            .is_ok());
        }

        #[test]
        fn test_create_contract_rejected() {
            let invoke_op = InvokeHostFunctionOp {
                host_function: HostFunction::CreateContract(soroban_rs::xdr::CreateContractArgs {
                    contract_id_preimage: soroban_rs::xdr::ContractIdPreimage::Address(
                        soroban_rs::xdr::ContractIdPreimageFromAddress {
                            address: ScAddress::Account(create_account_id(TEST_PK)),
                            salt: soroban_rs::xdr::Uint256([0u8; 32]),
                        },
                    ),
                    executable: soroban_rs::xdr::ContractExecutable::Wasm(soroban_rs::xdr::Hash(
                        [0u8; 32],
                    )),
                }),
                auth: Default::default(),
            };

            let policy = RelayerStellarPolicy::default();
            let result = StellarTransactionValidator::validate_contract_invocation(
                &invoke_op, 0, TEST_PK_2, &policy,
            );
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("CreateContract not allowed"));
        }

        #[test]
        fn test_upload_wasm_rejected() {
            let invoke_op = InvokeHostFunctionOp {
                host_function: HostFunction::UploadContractWasm(vec![0u8; 100].try_into().unwrap()),
                auth: Default::default(),
            };

            let policy = RelayerStellarPolicy::default();
            let result = StellarTransactionValidator::validate_contract_invocation(
                &invoke_op, 0, TEST_PK_2, &policy,
            );
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("UploadContractWasm not allowed"));
        }

        #[test]
        fn test_relayer_in_auth_rejected() {
            let auth_entry = SorobanAuthorizationEntry {
                credentials: SorobanCredentials::Address(
                    soroban_rs::xdr::SorobanAddressCredentials {
                        address: ScAddress::Account(create_account_id(TEST_PK_2)),
                        nonce: 0,
                        signature_expiration_ledger: 0,
                        signature: soroban_rs::xdr::ScVal::Void,
                    },
                ),
                root_invocation: soroban_rs::xdr::SorobanAuthorizedInvocation {
                    function: SorobanAuthorizedFunction::ContractFn(
                        soroban_rs::xdr::InvokeContractArgs {
                            contract_address: ScAddress::Contract(soroban_rs::xdr::ContractId(
                                soroban_rs::xdr::Hash([0u8; 32]),
                            )),
                            function_name: ScSymbol("test".try_into().unwrap()),
                            args: Default::default(),
                        },
                    ),
                    sub_invocations: Default::default(),
                },
            };

            let invoke_op = InvokeHostFunctionOp {
                host_function: HostFunction::InvokeContract(InvokeContractArgs {
                    contract_address: ScAddress::Contract(soroban_rs::xdr::ContractId(
                        soroban_rs::xdr::Hash([0u8; 32]),
                    )),
                    function_name: ScSymbol("test".try_into().unwrap()),
                    args: Default::default(),
                }),
                auth: vec![auth_entry].try_into().unwrap(),
            };

            let policy = RelayerStellarPolicy::default();
            let result = StellarTransactionValidator::validate_contract_invocation(
                &invoke_op, 0, TEST_PK_2, // Relayer address matches auth entry
                &policy,
            );
            assert!(result.is_err());
            assert!(result.unwrap_err().to_string().contains("requires relayer"));
        }
    }
}
