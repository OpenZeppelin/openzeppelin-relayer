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
        if fee_token.starts_with('C') {
            if fee_token.len() == 56 {
                // Validate it's a valid contract address using StrKey
                if stellar_strkey::Contract::from_string(fee_token).is_ok() {
                    return Ok(());
                }
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
                    // Convert amount from i64 to u64 (amounts are always positive)
                    let amount_u64 = (*amount) as u64;
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
    /// The transaction sequence must be >= the account's current sequence number.
    /// Future sequence numbers are allowed (user can queue transactions).
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

        // Validate that transaction sequence number is valid (>= account's current sequence)
        // The user can set a future sequence number, but not a past one
        if tx_seq_num < account_seq_num {
            return Err(StellarTransactionValidationError::ValidationError(format!(
                "Transaction sequence number {tx_seq_num} is too old. Account's current sequence is {account_seq_num}. \
                The transaction sequence must be >= the account's current sequence."
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
    use crate::models::AssetSpec;

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
                destination: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string(),
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
                contract_address: "CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC"
                    .to_string(),
                function_name: "test".to_string(),
                args: vec![],
                auth: None,
            },
            OperationSpec::CreateContract {
                source: crate::models::ContractSource::Address {
                    address: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string(),
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
                contract_address: "CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC"
                    .to_string(),
                function_name: "test".to_string(),
                args: vec![],
                auth: None,
            },
            OperationSpec::Payment {
                destination: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string(),
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
            contract_address: "CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC"
                .to_string(),
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
}
