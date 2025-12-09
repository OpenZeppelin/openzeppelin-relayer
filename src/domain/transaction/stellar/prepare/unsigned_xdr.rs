//! This module handles the preparation of unsigned XDR transactions.
//! It includes XDR parsing, validation, sequence updating, and fee updating.

use eyre::Result;
use soroban_rs::xdr::{Limits, OperationBody, ReadXdr, TransactionEnvelope, WriteXdr};
use tracing::debug;

use crate::{
    constants::STELLAR_DEFAULT_TRANSACTION_FEE,
    domain::transaction::stellar::token::get_token_balance,
    domain::transaction::stellar::utils::{asset_to_asset_id, change_trust_asset_to_asset_id},
    domain::transaction::stellar::StellarTransactionValidator,
    domain::{extract_operations, extract_source_account, muxed_account_to_string},
    models::{
        RelayerStellarPolicy, StellarFeePaymentStrategy, StellarTransactionData,
        StellarValidationError, TransactionError, TransactionInput,
    },
    repositories::TransactionCounterTrait,
    services::{
        provider::StellarProviderTrait, signer::Signer, stellar_dex::StellarDexServiceTrait,
    },
};

use super::common::{
    apply_sequence, ensure_minimum_fee, get_next_sequence, sign_stellar_transaction,
    simulate_if_needed,
};

/// Check if a transaction contains only ChangeTrust operations for allowed tokens without existing trustlines
///
/// This is an exception that allows certain transactions in user fee payment mode via unsigned_xdr path.
/// Returns true if:
/// - All operations are ChangeTrust operations
/// - Each ChangeTrust is for a token in the allowed_tokens list
/// - The trustline doesn't already exist for the relayer account
///
/// Note: In Stellar, ChangeTrust operations always create trustlines for the transaction source account.
/// This function only allows trustlines to be created for the relayer account.
async fn is_change_trust_for_allowed_token_without_trustline<P>(
    envelope: &TransactionEnvelope,
    relayer_address: &str,
    policy: &RelayerStellarPolicy,
    provider: &P,
) -> Result<bool, TransactionError>
where
    P: StellarProviderTrait + Send + Sync,
{
    let operations = extract_operations(envelope).map_err(|e| {
        TransactionError::ValidationError(format!("Failed to extract operations: {e}"))
    })?;

    // Must have at least one operation
    if operations.is_empty() {
        return Ok(false);
    }

    // Check if all operations are ChangeTrust
    for op in operations.iter() {
        if !matches!(op.body, OperationBody::ChangeTrust(_)) {
            return Ok(false);
        }
    }

    // Extract assets from ChangeTrust operations and validate
    for op in operations.iter() {
        if let OperationBody::ChangeTrust(change_trust) = &op.body {
            // Extract asset from ChangeTrust and convert to asset_id string
            let asset_id =
                match change_trust_asset_to_asset_id(&change_trust.line).map_err(|e| {
                    TransactionError::ValidationError(format!(
                        "Failed to convert ChangeTrust asset to asset_id: {e}"
                    ))
                })? {
                    None => {
                        // Native or PoolShare assets don't need trustlines
                        return Ok(false);
                    }
                    Some(asset_id) => asset_id,
                };

            // Check if asset is in allowed_tokens list
            StellarTransactionValidator::validate_allowed_token(&asset_id, policy).map_err(
                |e| {
                    TransactionError::ValidationError(format!(
                        "ChangeTrust asset {asset_id} is not in allowed_tokens list: {e}"
                    ))
                },
            )?;

            // Check if trustline already exists for the relayer account
            // (if get_token_balance succeeds, trustline exists)
            match get_token_balance(provider, relayer_address, &asset_id).await {
                Ok(_) => {
                    // Trustline already exists, this is not allowed
                    return Ok(false);
                }
                Err(_) => {
                    // Trustline doesn't exist (error is expected), this is allowed
                    // Continue checking other operations
                }
            }
        }
    }

    // All operations are ChangeTrust for allowed tokens without trustlines
    Ok(true)
}

/// Check if a transaction is a valid swap transaction (PathPaymentStrictSend from allowed token to XLM)
///
/// This is an exception that allows swap transactions in user fee payment mode via unsigned_xdr path.
/// Returns true if:
/// - Transaction contains exactly one PathPaymentStrictSend operation
/// - Source asset is an allowed token
/// - Destination asset is native/XLM
/// - Destination account is the relayer account
/// - Source account is the relayer account (already validated before this check)
///
/// This ensures only legitimate swap transactions initiated by the relayer are allowed,
/// preventing malicious external requests from swapping arbitrary tokens or sending to other accounts.
fn is_valid_swap_transaction(
    envelope: &TransactionEnvelope,
    relayer_address: &str,
    policy: &RelayerStellarPolicy,
) -> Result<bool, TransactionError> {
    let operations = extract_operations(envelope).map_err(|e| {
        TransactionError::ValidationError(format!("Failed to extract operations: {e}"))
    })?;

    // Must have exactly one operation for swap transactions
    if operations.len() != 1 {
        return Ok(false);
    }

    // Check if the operation is PathPaymentStrictSend
    let op = &operations[0];
    let path_payment = match &op.body {
        OperationBody::PathPaymentStrictSend(path_payment_op) => path_payment_op,
        _ => return Ok(false),
    };

    // Validate source asset is an allowed token
    let source_asset_id = asset_to_asset_id(&path_payment.send_asset).map_err(|e| {
        TransactionError::ValidationError(format!(
            "Failed to convert source asset to asset_id: {e}"
        ))
    })?;

    // Source asset must be in allowed_tokens list (not native/XLM)
    if source_asset_id == "native" {
        return Ok(false);
    }

    StellarTransactionValidator::validate_allowed_token(&source_asset_id, policy).map_err(|e| {
        TransactionError::ValidationError(format!(
            "Swap source asset {source_asset_id} is not in allowed_tokens list: {e}"
        ))
    })?;

    // Validate destination asset is native/XLM
    let dest_asset_id = asset_to_asset_id(&path_payment.dest_asset).map_err(|e| {
        TransactionError::ValidationError(format!(
            "Failed to convert destination asset to asset_id: {e}"
        ))
    })?;

    if dest_asset_id != "native" {
        return Ok(false);
    }

    // Validate destination account is the relayer account
    let dest_account_str = muxed_account_to_string(&path_payment.destination).map_err(|e| {
        TransactionError::ValidationError(format!(
            "Failed to convert destination account to string: {e}"
        ))
    })?;

    if dest_account_str != relayer_address {
        return Ok(false);
    }

    // All validations passed - this is a valid swap transaction
    Ok(true)
}

/// Process an unsigned XDR transaction.
///
/// This function:
/// 1. Parses the unsigned XDR from the transaction input
/// 2. Rejects User fee payment strategy transactions (must use fee_bump path)
/// 3. Validates source account is relayer's account
/// 4. Gets the next sequence number and updates the envelope
/// 5. Ensures the transaction has at least the minimum required fee
/// 6. Simulates the transaction if it contains Soroban operations
/// 7. Signs the transaction and returns the updated stellar data
///
/// Note: Gasless transactions (User fee payment strategy) are not supported via this path.
/// They must be processed via fee_bump path (SignedXdr) where validations are performed.
///
/// For relayer-paid transactions:
/// - Source account must be relayer's account
/// - Uses relayer's sequence number from counter
#[allow(clippy::too_many_arguments)]
pub async fn process_unsigned_xdr<C, P, S, D>(
    counter_service: &C,
    relayer_id: &str,
    relayer_address: &str,
    stellar_data: StellarTransactionData,
    provider: &P,
    signer: &S,
    relayer_policy: Option<&RelayerStellarPolicy>,
    _dex_service: &D,
) -> Result<StellarTransactionData, TransactionError>
where
    C: TransactionCounterTrait + Send + Sync,
    P: StellarProviderTrait + Send + Sync,
    S: Signer + Send + Sync,
    D: StellarDexServiceTrait + Send + Sync,
{
    // Step 1: Parse the XDR first (needed for validation)
    // Step 2: Check if User fee payment strategy transactions should be allowed
    // (Allow ChangeTrust operations for allowed tokens without existing trustlines)
    let xdr = match &stellar_data.transaction_input {
        TransactionInput::UnsignedXdr(xdr) => xdr,
        _ => {
            return Err(TransactionError::UnexpectedError(
                "Expected UnsignedXdr input".into(),
            ))
        }
    };

    let mut envelope = TransactionEnvelope::from_xdr_base64(xdr, Limits::none())
        .map_err(|e| StellarValidationError::InvalidXdr(e.to_string()))?;

    // Step 2: Extract source account (needed for validation)
    let source_account = extract_source_account(&envelope).map_err(|e| {
        TransactionError::ValidationError(format!("Failed to extract source account: {e}"))
    })?;

    // Step 3: Check if User fee payment strategy transactions should be allowed
    // Allow ChangeTrust operations for allowed tokens without existing trustlines
    // Also allow swap transactions (PathPaymentStrictSend from allowed tokens to XLM)
    if let Some(policy) = relayer_policy {
        if matches!(
            policy.fee_payment_strategy,
            Some(StellarFeePaymentStrategy::User)
        ) {
            // Check if this is an allowed exception: ChangeTrust for allowed tokens without trustlines
            // Note: This only allows trustlines for the relayer account (source_account must be relayer)
            let is_change_trust_exception = is_change_trust_for_allowed_token_without_trustline(
                &envelope,
                relayer_address,
                policy,
                provider,
            )
            .await
            .map_err(|e| {
                TransactionError::ValidationError(format!(
                    "Failed to validate ChangeTrust exception: {e}"
                ))
            })?;

            // Check if this is a valid swap transaction (PathPaymentStrictSend from allowed token to XLM)
            let is_swap_exception = is_valid_swap_transaction(&envelope, relayer_address, policy)
                .map_err(|e| {
                TransactionError::ValidationError(format!(
                    "Failed to validate swap transaction exception: {e}"
                ))
            })?;

            if !is_change_trust_exception && !is_swap_exception {
                return Err(TransactionError::ValidationError(
                    "Gasless transactions (User fee payment strategy) are not supported via unsigned_xdr path. \
                     Please use fee_bump path (SignedXdr) for gasless transactions.".to_string(),
                ));
            }
        }
    }

    // Step 4: Validate source account - must be relayer for unsigned_xdr path

    if source_account != relayer_address {
        return Err(StellarValidationError::SourceAccountMismatch {
            expected: relayer_address.to_string(),
            actual: source_account,
        }
        .into());
    }

    // Step 4: Get the next sequence number and update the envelope
    // For relayer-paid transactions, use relayer's sequence number
    let sequence = get_next_sequence(counter_service, relayer_id, relayer_address).await?;

    debug!(
        "Using sequence number {} for relayer-paid transaction",
        sequence
    );

    // Apply sequence updates the envelope in-place and returns the XDR
    let _updated_xdr = apply_sequence(&mut envelope, sequence).await?;

    // Update stellar data with sequence number
    let mut stellar_data = stellar_data.with_sequence_number(sequence);

    // Step 5: Ensure minimum fee
    ensure_minimum_fee(&mut envelope).await?;

    // Re-serialize the envelope after fee update
    let updated_xdr = envelope.to_xdr_base64(Limits::none()).map_err(|e| {
        TransactionError::ValidationError(format!("Failed to serialize updated envelope: {e}"))
    })?;

    // Update stellar data with new XDR
    stellar_data.transaction_input = TransactionInput::UnsignedXdr(updated_xdr.clone());

    // Step 6: Check if simulation is needed
    let stellar_data_with_sim = match simulate_if_needed(&envelope, provider).await? {
        Some(sim_resp) => {
            debug!("Applying simulation results to unsigned XDR transaction");
            // Get operation count from the envelope
            let op_count = extract_operations(&envelope)?.len() as u64;
            stellar_data
                .with_simulation_data(sim_resp, op_count)
                .map_err(|e| {
                    TransactionError::ValidationError(format!(
                        "Failed to apply simulation data: {e}"
                    ))
                })?
        }
        None => {
            // For non-simulated transactions, ensure fee is set from the envelope
            let fee = match &envelope {
                TransactionEnvelope::TxV0(e) => e.tx.fee,
                TransactionEnvelope::Tx(e) => e.tx.fee,
                _ => STELLAR_DEFAULT_TRANSACTION_FEE,
            };
            stellar_data.with_fee(fee)
        }
    };

    // Step 7: Sign the transaction
    sign_stellar_transaction(signer, stellar_data_with_sim).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domain::{transaction::stellar::test_helpers::*, SignTransactionResponse},
        models::DecoratedSignature,
        services::{signer::MockSigner, stellar_dex::MockStellarDexServiceTrait},
    };
    use soroban_rs::xdr::{BytesM, Signature, SignatureHint};
    use std::future::ready;

    use crate::repositories::MockTransactionCounterTrait;
    use crate::services::provider::MockStellarProviderTrait;

    fn create_test_envelope(source_account: &str) -> TransactionEnvelope {
        create_simple_v1_envelope(source_account, TEST_PK_2)
    }

    #[tokio::test]
    async fn test_process_unsigned_xdr_valid_source() {
        let relayer_address = TEST_PK;
        let relayer_id = "test-relayer";
        let expected_sequence = 42i64;

        let mut counter = MockTransactionCounterTrait::new();
        counter
            .expect_get_and_increment()
            .returning(move |_, _| Box::pin(ready(Ok(expected_sequence as u64))));

        let mut provider = MockStellarProviderTrait::new();
        provider
            .expect_simulate_transaction_envelope()
            .returning(|_| {
                Box::pin(ready(Ok(
                    soroban_rs::stellar_rpc_client::SimulateTransactionResponse {
                        min_resource_fee: 0,
                        transaction_data: String::new(),
                        ..Default::default()
                    },
                )))
            });

        let mut signer = MockSigner::new();

        // Mock signer expectations
        signer.expect_address().returning(|| {
            Box::pin(ready(Ok(crate::models::Address::Stellar(
                "test-signer-address".to_string(),
            ))))
        });
        signer.expect_sign_transaction().returning(|_| {
            let sig_bytes: Vec<u8> = vec![1u8; 64];
            let sig_bytes_m: BytesM<64> = sig_bytes.try_into().unwrap();
            Box::pin(ready(Ok(SignTransactionResponse::Stellar(
                crate::domain::SignTransactionResponseStellar {
                    signature: DecoratedSignature {
                        hint: SignatureHint([0; 4]),
                        signature: Signature(sig_bytes_m),
                    },
                },
            ))))
        });

        // Create envelope with matching source
        let envelope = create_test_envelope(relayer_address);
        let xdr = envelope.to_xdr_base64(Limits::none()).unwrap();

        let stellar_data = StellarTransactionData {
            source_account: relayer_address.to_string(),
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            fee: None,
            sequence_number: None,
            transaction_input: TransactionInput::UnsignedXdr(xdr),
            memo: None,
            valid_until: None,
            signatures: vec![],
            hash: None,
            simulation_transaction_data: None,
            signed_envelope_xdr: None,
        };

        let dex_service = MockStellarDexServiceTrait::new();
        let result = process_unsigned_xdr(
            &counter,
            relayer_id,
            relayer_address,
            stellar_data,
            &provider,
            &signer,
            None,
            &dex_service,
        )
        .await;

        assert!(result.is_ok());
        let updated_data = result.unwrap();
        assert_eq!(updated_data.sequence_number, Some(expected_sequence));
        assert!(updated_data.signed_envelope_xdr.is_some());
        assert!(!updated_data.signatures.is_empty());
    }

    #[tokio::test]
    async fn test_process_unsigned_xdr_invalid_source() {
        let relayer_address = TEST_PK;
        let different_address = "GCEZWKCA5VLDNRLN3RPRJMRZOX3Z6G5CHCGSNFHEYVXM3XOJMDS674JZ";
        let relayer_id = "test-relayer";

        let mut counter = MockTransactionCounterTrait::new();
        counter
            .expect_get_and_increment()
            .returning(|_, _| Box::pin(ready(Ok(42))));

        let mut provider = MockStellarProviderTrait::new();
        provider
            .expect_simulate_transaction_envelope()
            .returning(|_| {
                Box::pin(ready(Ok(
                    soroban_rs::stellar_rpc_client::SimulateTransactionResponse {
                        min_resource_fee: 0,
                        transaction_data: String::new(),
                        ..Default::default()
                    },
                )))
            });

        let mut signer = MockSigner::new();

        // Mock signer expectations
        signer.expect_address().returning(|| {
            Box::pin(ready(Ok(crate::models::Address::Stellar(
                "test-signer-address".to_string(),
            ))))
        });
        signer.expect_sign_transaction().returning(|_| {
            let sig_bytes: Vec<u8> = vec![1u8; 64];
            let sig_bytes_m: BytesM<64> = sig_bytes.try_into().unwrap();
            Box::pin(ready(Ok(SignTransactionResponse::Stellar(
                crate::domain::SignTransactionResponseStellar {
                    signature: DecoratedSignature {
                        hint: SignatureHint([0; 4]),
                        signature: Signature(sig_bytes_m),
                    },
                },
            ))))
        });

        // Create envelope with different source
        let envelope = create_test_envelope(different_address);
        let xdr = envelope.to_xdr_base64(Limits::none()).unwrap();

        let stellar_data = StellarTransactionData {
            source_account: relayer_address.to_string(),
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            fee: None,
            sequence_number: None,
            transaction_input: TransactionInput::UnsignedXdr(xdr),
            memo: None,
            valid_until: None,
            signatures: vec![],
            hash: None,
            simulation_transaction_data: None,
            signed_envelope_xdr: None,
        };

        let dex_service = MockStellarDexServiceTrait::new();
        let result = process_unsigned_xdr(
            &counter,
            relayer_id,
            relayer_address,
            stellar_data,
            &provider,
            &signer,
            None,
            &dex_service,
        )
        .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            TransactionError::ValidationError(msg) => {
                assert!(msg.contains("does not match relayer account"));
            }
            _ => panic!("Expected ValidationError"),
        }
    }

    #[tokio::test]
    async fn test_process_unsigned_xdr_fee_update() {
        let relayer_address = TEST_PK;
        let relayer_id = "test-relayer";

        let mut counter = MockTransactionCounterTrait::new();
        counter
            .expect_get_and_increment()
            .returning(|_, _| Box::pin(ready(Ok(42))));

        let mut provider = MockStellarProviderTrait::new();
        provider
            .expect_simulate_transaction_envelope()
            .returning(|_| {
                Box::pin(ready(Ok(
                    soroban_rs::stellar_rpc_client::SimulateTransactionResponse {
                        min_resource_fee: 0,
                        transaction_data: String::new(),
                        ..Default::default()
                    },
                )))
            });

        let mut signer = MockSigner::new();

        // Mock signer expectations
        signer.expect_address().returning(|| {
            Box::pin(ready(Ok(crate::models::Address::Stellar(
                "test-signer-address".to_string(),
            ))))
        });
        signer.expect_sign_transaction().returning(|_| {
            let sig_bytes: Vec<u8> = vec![1u8; 64];
            let sig_bytes_m: BytesM<64> = sig_bytes.try_into().unwrap();
            Box::pin(ready(Ok(SignTransactionResponse::Stellar(
                crate::domain::SignTransactionResponseStellar {
                    signature: DecoratedSignature {
                        hint: SignatureHint([0; 4]),
                        signature: Signature(sig_bytes_m),
                    },
                },
            ))))
        });

        // Create envelope with low fee
        let mut envelope = create_test_envelope(relayer_address);
        if let TransactionEnvelope::Tx(ref mut e) = envelope {
            e.tx.fee = 50; // Below minimum
        }
        let xdr = envelope.to_xdr_base64(Limits::none()).unwrap();

        let stellar_data = StellarTransactionData {
            source_account: relayer_address.to_string(),
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            fee: None,
            sequence_number: None,
            transaction_input: TransactionInput::UnsignedXdr(xdr),
            memo: None,
            valid_until: None,
            signatures: vec![],
            hash: None,
            simulation_transaction_data: None,
            signed_envelope_xdr: None,
        };

        let dex_service = MockStellarDexServiceTrait::new();
        let result = process_unsigned_xdr(
            &counter,
            relayer_id,
            relayer_address,
            stellar_data,
            &provider,
            &signer,
            None,
            &dex_service,
        )
        .await;

        assert!(result.is_ok());
        let updated_data = result.unwrap();

        // Parse the updated XDR to verify fee was updated
        if let TransactionInput::UnsignedXdr(updated_xdr) = &updated_data.transaction_input {
            let updated_envelope =
                TransactionEnvelope::from_xdr_base64(updated_xdr, Limits::none()).unwrap();
            if let TransactionEnvelope::Tx(e) = updated_envelope {
                assert!(e.tx.fee >= 100); // Minimum fee
            } else {
                panic!("Expected Tx envelope");
            }
        } else {
            panic!("Expected UnsignedXdr input");
        }
    }

    #[tokio::test]
    async fn test_process_unsigned_xdr_wrong_input_type() {
        let relayer_address = TEST_PK;
        let relayer_id = "test-relayer";

        let counter = MockTransactionCounterTrait::new();
        let provider = MockStellarProviderTrait::new();
        let mut signer = MockSigner::new();

        // Mock signer expectations
        signer.expect_address().returning(|| {
            Box::pin(ready(Ok(crate::models::Address::Stellar(
                "test-signer-address".to_string(),
            ))))
        });
        signer.expect_sign_transaction().returning(|_| {
            let sig_bytes: Vec<u8> = vec![1u8; 64];
            let sig_bytes_m: BytesM<64> = sig_bytes.try_into().unwrap();
            Box::pin(ready(Ok(SignTransactionResponse::Stellar(
                crate::domain::SignTransactionResponseStellar {
                    signature: DecoratedSignature {
                        hint: SignatureHint([0; 4]),
                        signature: Signature(sig_bytes_m),
                    },
                },
            ))))
        });

        // Create stellar data with wrong input type
        let stellar_data = StellarTransactionData {
            source_account: relayer_address.to_string(),
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            fee: None,
            sequence_number: None,
            transaction_input: TransactionInput::Operations(vec![]), // Wrong type
            memo: None,
            valid_until: None,
            signatures: vec![],
            hash: None,
            simulation_transaction_data: None,
            signed_envelope_xdr: None,
        };

        let dex_service = MockStellarDexServiceTrait::new();
        let result = process_unsigned_xdr(
            &counter,
            relayer_id,
            relayer_address,
            stellar_data,
            &provider,
            &signer,
            None,
            &dex_service,
        )
        .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            TransactionError::UnexpectedError(msg) => {
                assert_eq!(msg, "Expected UnsignedXdr input");
            }
            _ => panic!("Expected UnexpectedError"),
        }
    }

    #[tokio::test]
    async fn test_process_unsigned_xdr_invalid_xdr() {
        let relayer_address = TEST_PK;
        let relayer_id = "test-relayer";

        let counter = MockTransactionCounterTrait::new();
        let provider = MockStellarProviderTrait::new();
        let mut signer = MockSigner::new();

        signer.expect_address().returning(|| {
            Box::pin(ready(Ok(crate::models::Address::Stellar(
                "test-signer-address".to_string(),
            ))))
        });

        // Create stellar data with invalid XDR
        let stellar_data = StellarTransactionData {
            source_account: relayer_address.to_string(),
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            fee: None,
            sequence_number: None,
            transaction_input: TransactionInput::UnsignedXdr("invalid-xdr".to_string()),
            memo: None,
            valid_until: None,
            signatures: vec![],
            hash: None,
            simulation_transaction_data: None,
            signed_envelope_xdr: None,
        };

        let dex_service = MockStellarDexServiceTrait::new();
        let result = process_unsigned_xdr(
            &counter,
            relayer_id,
            relayer_address,
            stellar_data,
            &provider,
            &signer,
            None,
            &dex_service,
        )
        .await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TransactionError::ValidationError(_)
        ));
    }

    #[tokio::test]
    async fn test_process_unsigned_xdr_with_policy_relayer_strategy() {
        let relayer_address = TEST_PK;
        let relayer_id = "test-relayer";

        let mut counter = MockTransactionCounterTrait::new();
        counter
            .expect_get_and_increment()
            .returning(|_, _| Box::pin(ready(Ok(42))));

        let mut provider = MockStellarProviderTrait::new();
        provider
            .expect_simulate_transaction_envelope()
            .returning(|_| {
                Box::pin(ready(Ok(
                    soroban_rs::stellar_rpc_client::SimulateTransactionResponse {
                        min_resource_fee: 0,
                        transaction_data: String::new(),
                        ..Default::default()
                    },
                )))
            });

        let mut signer = MockSigner::new();

        signer.expect_address().returning(|| {
            Box::pin(ready(Ok(crate::models::Address::Stellar(
                "test-signer-address".to_string(),
            ))))
        });
        signer.expect_sign_transaction().returning(|_| {
            let sig_bytes: Vec<u8> = vec![1u8; 64];
            let sig_bytes_m: BytesM<64> = sig_bytes.try_into().unwrap();
            Box::pin(ready(Ok(SignTransactionResponse::Stellar(
                crate::domain::SignTransactionResponseStellar {
                    signature: DecoratedSignature {
                        hint: SignatureHint([0; 4]),
                        signature: Signature(sig_bytes_m),
                    },
                },
            ))))
        });

        let envelope = create_test_envelope(relayer_address);
        let xdr = envelope.to_xdr_base64(Limits::none()).unwrap();

        let stellar_data = StellarTransactionData {
            source_account: relayer_address.to_string(),
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            fee: None,
            sequence_number: None,
            transaction_input: TransactionInput::UnsignedXdr(xdr),
            memo: None,
            valid_until: None,
            signatures: vec![],
            hash: None,
            simulation_transaction_data: None,
            signed_envelope_xdr: None,
        };

        let policy = RelayerStellarPolicy {
            fee_payment_strategy: Some(StellarFeePaymentStrategy::Relayer),
            ..Default::default()
        };

        let dex_service = MockStellarDexServiceTrait::new();
        let result = process_unsigned_xdr(
            &counter,
            relayer_id,
            relayer_address,
            stellar_data,
            &provider,
            &signer,
            Some(&policy),
            &dex_service,
        )
        .await;

        assert!(result.is_ok());
    }

    #[test]
    fn test_is_valid_swap_transaction_with_valid_swap() {
        use soroban_rs::xdr::{
            Asset, Operation, OperationBody, PathPaymentStrictSendOp, Transaction, TransactionExt,
            TransactionV1Envelope, VecM,
        };

        let relayer_address = TEST_PK;

        // Create a PathPaymentStrictSend operation from USDC to XLM
        let usdc_asset = Asset::CreditAlphanum4(soroban_rs::xdr::AlphaNum4 {
            asset_code: soroban_rs::xdr::AssetCode4(*b"USDC"),
            issuer: create_account_id(TEST_PK_2),
        });

        let path_payment_op = PathPaymentStrictSendOp {
            send_asset: usdc_asset,
            send_amount: 10000000,
            destination: create_muxed_account(relayer_address),
            dest_asset: Asset::Native,
            dest_min: 9000000,
            path: VecM::default(),
        };

        let op = Operation {
            source_account: None,
            body: OperationBody::PathPaymentStrictSend(path_payment_op),
        };

        let tx = Transaction {
            source_account: create_muxed_account(relayer_address),
            fee: 100,
            seq_num: soroban_rs::xdr::SequenceNumber(1),
            cond: soroban_rs::xdr::Preconditions::None,
            memo: soroban_rs::xdr::Memo::None,
            operations: vec![op].try_into().unwrap(),
            ext: TransactionExt::V0,
        };

        let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
            tx,
            signatures: vec![].try_into().unwrap(),
        });

        let mut policy = RelayerStellarPolicy::default();
        policy.allowed_tokens = Some(vec![crate::models::StellarAllowedTokensPolicy {
            asset: format!("USDC:{}", TEST_PK_2),
            metadata: None,
            swap_config: None,
            max_allowed_fee: None,
        }]);

        let result = is_valid_swap_transaction(&envelope, relayer_address, &policy);
        assert!(result.is_ok());
        assert!(result.unwrap());
    }

    #[test]
    fn test_is_valid_swap_transaction_with_multiple_operations() {
        use soroban_rs::xdr::{Operation, Transaction, TransactionExt, TransactionV1Envelope};

        let relayer_address = TEST_PK;

        let operations: Vec<Operation> = vec![
            create_native_payment_operation(TEST_PK_2, 1000000),
            create_native_payment_operation(TEST_PK_2, 1000000),
        ];

        let tx = Transaction {
            source_account: create_muxed_account(relayer_address),
            fee: 100,
            seq_num: soroban_rs::xdr::SequenceNumber(1),
            cond: soroban_rs::xdr::Preconditions::None,
            memo: soroban_rs::xdr::Memo::None,
            operations: operations.try_into().unwrap(),
            ext: TransactionExt::V0,
        };

        let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
            tx,
            signatures: vec![].try_into().unwrap(),
        });

        let policy = RelayerStellarPolicy::default();
        let result = is_valid_swap_transaction(&envelope, relayer_address, &policy);
        assert!(result.is_ok());
        assert!(!result.unwrap()); // Should return false for multiple operations
    }

    #[test]
    fn test_is_valid_swap_transaction_with_non_path_payment() {
        let relayer_address = TEST_PK;
        let envelope = create_simple_v1_envelope(relayer_address, TEST_PK_2);

        let policy = RelayerStellarPolicy::default();
        let result = is_valid_swap_transaction(&envelope, relayer_address, &policy);
        assert!(result.is_ok());
        assert!(!result.unwrap()); // Should return false for non-PathPaymentStrictSend
    }

    #[test]
    fn test_is_valid_swap_transaction_with_native_source() {
        use soroban_rs::xdr::{
            Asset, Operation, OperationBody, PathPaymentStrictSendOp, Transaction, TransactionExt,
            TransactionV1Envelope, VecM,
        };

        let relayer_address = TEST_PK;

        // Create a PathPaymentStrictSend with native source (should be rejected)
        let path_payment_op = PathPaymentStrictSendOp {
            send_asset: Asset::Native,
            send_amount: 10000000,
            destination: create_muxed_account(relayer_address),
            dest_asset: Asset::Native,
            dest_min: 9000000,
            path: VecM::default(),
        };

        let op = Operation {
            source_account: None,
            body: OperationBody::PathPaymentStrictSend(path_payment_op),
        };

        let tx = Transaction {
            source_account: create_muxed_account(relayer_address),
            fee: 100,
            seq_num: soroban_rs::xdr::SequenceNumber(1),
            cond: soroban_rs::xdr::Preconditions::None,
            memo: soroban_rs::xdr::Memo::None,
            operations: vec![op].try_into().unwrap(),
            ext: TransactionExt::V0,
        };

        let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
            tx,
            signatures: vec![].try_into().unwrap(),
        });

        let policy = RelayerStellarPolicy::default();
        let result = is_valid_swap_transaction(&envelope, relayer_address, &policy);
        assert!(result.is_ok());
        assert!(!result.unwrap()); // Should return false for native source
    }

    #[test]
    fn test_is_valid_swap_transaction_with_wrong_destination() {
        use soroban_rs::xdr::{
            Asset, Operation, OperationBody, PathPaymentStrictSendOp, Transaction, TransactionExt,
            TransactionV1Envelope, VecM,
        };

        let relayer_address = TEST_PK;

        let usdc_asset = Asset::CreditAlphanum4(soroban_rs::xdr::AlphaNum4 {
            asset_code: soroban_rs::xdr::AssetCode4(*b"USDC"),
            issuer: create_account_id(TEST_PK_2),
        });

        // Destination is different from relayer
        let path_payment_op = PathPaymentStrictSendOp {
            send_asset: usdc_asset,
            send_amount: 10000000,
            destination: create_muxed_account(TEST_PK_2), // Different destination
            dest_asset: Asset::Native,
            dest_min: 9000000,
            path: VecM::default(),
        };

        let op = Operation {
            source_account: None,
            body: OperationBody::PathPaymentStrictSend(path_payment_op),
        };

        let tx = Transaction {
            source_account: create_muxed_account(relayer_address),
            fee: 100,
            seq_num: soroban_rs::xdr::SequenceNumber(1),
            cond: soroban_rs::xdr::Preconditions::None,
            memo: soroban_rs::xdr::Memo::None,
            operations: vec![op].try_into().unwrap(),
            ext: TransactionExt::V0,
        };

        let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
            tx,
            signatures: vec![].try_into().unwrap(),
        });

        let mut policy = RelayerStellarPolicy::default();
        policy.allowed_tokens = Some(vec![crate::models::StellarAllowedTokensPolicy {
            asset: format!("USDC:{}", TEST_PK_2),
            metadata: None,
            swap_config: None,
            max_allowed_fee: None,
        }]);

        let result = is_valid_swap_transaction(&envelope, relayer_address, &policy);
        assert!(result.is_ok());
        assert!(!result.unwrap()); // Should return false for wrong destination
    }
}

#[cfg(test)]
mod xdr_transaction_tests {
    use std::future::ready;

    use super::*;
    use crate::constants::STELLAR_DEFAULT_TRANSACTION_FEE;
    use crate::domain::transaction::stellar::test_helpers::*;
    use crate::domain::SignTransactionResponse;
    use crate::models::{NetworkTransactionData, RepositoryError, TransactionStatus};
    use crate::services::stellar_dex::MockStellarDexServiceTrait;
    use soroban_rs::xdr::{
        Limits, Memo, MuxedAccount, Transaction, TransactionEnvelope, TransactionExt,
        TransactionV1Envelope, Uint256, VecM, WriteXdr,
    };
    use stellar_strkey::ed25519::PublicKey;

    fn create_unsigned_xdr_envelope(source_account: &str) -> TransactionEnvelope {
        let pk = match PublicKey::from_string(source_account) {
            Ok(pk) => pk,
            Err(_) => {
                // Create a dummy public key for tests - use a non-zero value
                let mut bytes = [0; 32];
                bytes[0] = 1; // This will create a different address
                PublicKey(bytes)
            }
        };
        let source = MuxedAccount::Ed25519(Uint256(pk.0));

        let tx = Transaction {
            source_account: source,
            fee: 100,
            seq_num: soroban_rs::xdr::SequenceNumber(1),
            cond: soroban_rs::xdr::Preconditions::None,
            memo: Memo::None,
            operations: VecM::default(),
            ext: TransactionExt::V0,
        };

        TransactionEnvelope::Tx(TransactionV1Envelope {
            tx,
            signatures: VecM::default(),
        })
    }

    #[tokio::test]
    async fn test_unsigned_xdr_valid_source() {
        let relayer = create_test_relayer();
        let mut mocks = default_test_mocks();

        // Mock the counter service to provide a sequence number
        let expected_sequence = 42i64;
        mocks
            .counter
            .expect_get_and_increment()
            .returning(move |_, _| Box::pin(ready(Ok(expected_sequence as u64))));

        // Mock signer for unsigned XDR
        mocks
            .signer
            .expect_sign_transaction()
            .withf(move |data| {
                // Verify that the transaction data has the updated sequence number
                if let NetworkTransactionData::Stellar(stellar_data) = data {
                    // Check that the XDR was updated
                    if let TransactionInput::UnsignedXdr(xdr) = &stellar_data.transaction_input {
                        // Parse the XDR to verify sequence number
                        if let Ok(env) = TransactionEnvelope::from_xdr_base64(xdr, Limits::none()) {
                            match env {
                                TransactionEnvelope::Tx(e) => e.tx.seq_num.0 == expected_sequence,
                                TransactionEnvelope::TxV0(e) => e.tx.seq_num.0 == expected_sequence,
                                _ => false,
                            }
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                } else {
                    false
                }
            })
            .returning(|_| {
                Box::pin(async {
                    Ok(SignTransactionResponse::Stellar(
                        crate::domain::SignTransactionResponseStellar {
                            signature: dummy_signature(),
                        },
                    ))
                })
            });

        // Mock the repository update
        mocks
            .tx_repo
            .expect_partial_update()
            .withf(|_, upd| upd.status == Some(TransactionStatus::Sent))
            .returning(|id, upd| {
                let mut tx = create_test_transaction("relayer-1");
                tx.id = id;
                tx.status = upd.status.unwrap();
                tx.network_data = upd.network_data.unwrap();
                Ok::<_, RepositoryError>(tx)
            });

        // Mock job production
        mocks
            .job_producer
            .expect_produce_submit_transaction_job()
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(()) }));

        mocks
            .job_producer
            .expect_produce_send_notification_job()
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let handler = make_stellar_tx_handler(relayer.clone(), mocks);

        let mut tx = create_test_transaction(&relayer.id);
        let mut stellar_data = tx
            .network_data
            .get_stellar_transaction_data()
            .unwrap()
            .clone();

        // Create unsigned XDR with relayer as source (with sequence 0)
        let envelope = create_unsigned_xdr_envelope(&relayer.address);
        let xdr = envelope.to_xdr_base64(Limits::none()).unwrap();
        stellar_data.transaction_input = TransactionInput::UnsignedXdr(xdr.clone());

        // Update the transaction with the modified stellar data
        tx.network_data = NetworkTransactionData::Stellar(stellar_data);

        let result = handler.prepare_transaction_impl(tx).await;
        assert!(result.is_ok());

        // Verify the resulting transaction has the correct sequence number
        if let Ok(prepared_tx) = result {
            if let NetworkTransactionData::Stellar(data) = &prepared_tx.network_data {
                assert_eq!(data.sequence_number, Some(expected_sequence));
            } else {
                panic!("Expected Stellar transaction data");
            }
        }
    }

    #[tokio::test]
    async fn test_unsigned_xdr_invalid_source() {
        let relayer = create_test_relayer();
        let mut mocks = default_test_mocks();

        // Don't expect counter to be called - validation fails before get_next_sequence

        // Mock sync_sequence_from_chain for error handling
        mocks.provider.expect_get_account().returning(|_| {
            Box::pin(async {
                use soroban_rs::xdr::{
                    AccountEntry, AccountEntryExt, AccountId, PublicKey, SequenceNumber, String32,
                    Thresholds, Uint256,
                };
                use stellar_strkey::ed25519;

                let pk = ed25519::PublicKey::from_string(TEST_PK).unwrap();
                let account_id = AccountId(PublicKey::PublicKeyTypeEd25519(Uint256(pk.0)));

                Ok(AccountEntry {
                    account_id,
                    balance: 1000000,
                    seq_num: SequenceNumber(0),
                    num_sub_entries: 0,
                    inflation_dest: None,
                    flags: 0,
                    home_domain: String32::default(),
                    thresholds: Thresholds([1, 1, 1, 1]),
                    signers: Default::default(),
                    ext: AccountEntryExt::V0,
                })
            })
        });

        mocks
            .counter
            .expect_set()
            .returning(|_, _, _| Box::pin(async { Ok(()) }));

        // Mock finalize_transaction_state for failure handling
        mocks
            .tx_repo
            .expect_partial_update()
            .withf(|_, upd| upd.status == Some(TransactionStatus::Failed))
            .returning(|id, upd| {
                let mut tx = create_test_transaction("relayer-1");
                tx.id = id;
                tx.status = upd.status.unwrap();
                Ok::<_, RepositoryError>(tx)
            });

        // Mock notification for failed transaction
        mocks
            .job_producer
            .expect_produce_send_notification_job()
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(()) }));

        // Mock find_by_status for enqueue_next_pending_transaction
        mocks
            .tx_repo
            .expect_find_by_status()
            .returning(|_, _| Ok(vec![])); // No pending transactions

        let handler = make_stellar_tx_handler(relayer.clone(), mocks);

        let mut tx = create_test_transaction(&relayer.id);
        let mut stellar_data = tx
            .network_data
            .get_stellar_transaction_data()
            .unwrap()
            .clone();

        // Remove sequence number since validation fails before it's set
        stellar_data.sequence_number = None;

        // Create unsigned XDR with different source
        let different_account = "GBCFR5QVA3K7JKIPT7WFULRXQVNTDZQLZHTUTGONFSTS5KCEGS6O5AZB";
        let envelope = create_unsigned_xdr_envelope(different_account);
        let xdr = envelope.to_xdr_base64(Limits::none()).unwrap();
        stellar_data.transaction_input = TransactionInput::UnsignedXdr(xdr.clone());

        // Update the transaction with the modified stellar data
        tx.network_data = NetworkTransactionData::Stellar(stellar_data);

        let result = handler.prepare_transaction_impl(tx).await;
        assert!(result.is_err());
        if let Err(TransactionError::ValidationError(msg)) = result {
            // The StellarValidationError formats differently - check for the expected/actual pattern
            assert!(
                msg.contains("does not match relayer account"),
                "Error message was: {}",
                msg
            );
        } else {
            panic!("Expected ValidationError, got {:?}", result);
        }
    }

    #[tokio::test]
    async fn test_unsigned_xdr_fee_update() {
        let relayer = create_test_relayer();
        let mut mocks = default_test_mocks();

        // Mock the counter service to provide a sequence number
        let expected_sequence = 42i64;
        let relayer_id = relayer.id.clone();
        mocks
            .counter
            .expect_get_and_increment()
            .withf(move |id, _| id == relayer_id)
            .returning(move |_, _| Box::pin(ready(Ok(expected_sequence as u64))));

        // Mock signer that verifies fee was updated
        mocks
            .signer
            .expect_sign_transaction()
            .withf(move |data| {
                match data {
                    NetworkTransactionData::Stellar(stellar_data) => {
                        // Also verify the fee field is set correctly
                        if stellar_data.fee != Some(100) {
                            return false;
                        }

                        if let TransactionInput::UnsignedXdr(xdr) = &stellar_data.transaction_input
                        {
                            if let Ok(env) =
                                TransactionEnvelope::from_xdr_base64(xdr, Limits::none())
                            {
                                match env {
                                    TransactionEnvelope::Tx(e) => {
                                        // Verify fee was updated to at least minimum
                                        e.tx.fee >= STELLAR_DEFAULT_TRANSACTION_FEE
                                    }
                                    TransactionEnvelope::TxV0(e) => {
                                        e.tx.fee >= STELLAR_DEFAULT_TRANSACTION_FEE
                                    }
                                    _ => false,
                                }
                            } else {
                                false
                            }
                        } else {
                            false
                        }
                    }
                    _ => false,
                }
            })
            .returning(move |_| {
                Box::pin(async move {
                    Ok(SignTransactionResponse::Stellar(
                        crate::domain::SignTransactionResponseStellar {
                            signature: crate::models::DecoratedSignature {
                                hint: soroban_rs::xdr::SignatureHint([0; 4]),
                                signature: soroban_rs::xdr::Signature(
                                    vec![1, 2, 3, 4].try_into().unwrap(),
                                ),
                            },
                        },
                    ))
                })
            });

        // Mock repository and job producer
        mocks.tx_repo.expect_partial_update().returning(|_, _| {
            let mut tx = create_test_transaction("test");
            if let NetworkTransactionData::Stellar(ref mut data) = tx.network_data {
                data.signed_envelope_xdr = Some("test-xdr".to_string());
            }
            Ok(tx)
        });

        mocks
            .job_producer
            .expect_produce_submit_transaction_job()
            .returning(|_, _| Box::pin(async { Ok(()) }));

        mocks
            .job_producer
            .expect_produce_send_notification_job()
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let handler = make_stellar_tx_handler(relayer.clone(), mocks);

        let mut tx = create_test_transaction(&relayer.id);
        let mut stellar_data = tx
            .network_data
            .get_stellar_transaction_data()
            .unwrap()
            .clone();

        // Create unsigned XDR with low fee (1 stroop) and a payment operation
        let mut envelope = create_unsigned_xdr_envelope(&relayer.address);

        // Add a payment operation so fee calculation works
        let payment_op = soroban_rs::xdr::Operation {
            source_account: None,
            body: soroban_rs::xdr::OperationBody::Payment(soroban_rs::xdr::PaymentOp {
                destination: soroban_rs::xdr::MuxedAccount::Ed25519(soroban_rs::xdr::Uint256(
                    [0; 32],
                )),
                asset: soroban_rs::xdr::Asset::Native,
                amount: 1000000,
            }),
        };

        match &mut envelope {
            TransactionEnvelope::Tx(ref mut e) => {
                e.tx.fee = 1;
                e.tx.operations = vec![payment_op].try_into().unwrap();
            }
            TransactionEnvelope::TxV0(ref mut e) => {
                e.tx.fee = 1;
                e.tx.operations = vec![payment_op].try_into().unwrap();
            }
            _ => panic!("Unexpected envelope type"),
        }

        let xdr = envelope.to_xdr_base64(Limits::none()).unwrap();
        stellar_data.transaction_input = TransactionInput::UnsignedXdr(xdr);

        // Update the transaction with the modified stellar data
        tx.network_data = NetworkTransactionData::Stellar(stellar_data);

        let result = handler.prepare_transaction_impl(tx).await;
        assert!(
            result.is_ok(),
            "Expected successful preparation, got: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn test_process_unsigned_xdr_rejects_user_fee_payment_strategy() {
        use crate::models::{RelayerStellarPolicy, StellarFeePaymentStrategy};

        let relayer = create_test_relayer();
        let mocks = default_test_mocks();

        let tx = create_test_transaction(&relayer.id);
        let mut stellar_data = tx
            .network_data
            .get_stellar_transaction_data()
            .unwrap()
            .clone();

        // Create unsigned XDR with a simple payment operation
        let envelope = create_unsigned_xdr_envelope(&relayer.address);
        let xdr = envelope.to_xdr_base64(Limits::none()).unwrap();
        stellar_data.transaction_input = TransactionInput::UnsignedXdr(xdr);

        // Create a policy with User fee payment strategy (gasless mode)
        let mut policy = RelayerStellarPolicy::default();
        policy.fee_payment_strategy = Some(StellarFeePaymentStrategy::User);

        // Call process_unsigned_xdr directly with the policy
        let dex_service = MockStellarDexServiceTrait::new();
        let result = process_unsigned_xdr(
            &mocks.counter,
            &relayer.id,
            &relayer.address,
            stellar_data,
            &mocks.provider,
            &mocks.signer,
            Some(&policy),
            &dex_service,
        )
        .await;

        // Should return a validation error
        assert!(result.is_err());

        match result.unwrap_err() {
            TransactionError::ValidationError(msg) => {
                assert!(msg.contains("Gasless transactions"));
                assert!(msg.contains("User fee payment strategy"));
                assert!(msg.contains("not supported via unsigned_xdr path"));
                assert!(msg.contains("fee_bump"));
            }
            other => panic!("Expected ValidationError, got: {:?}", other),
        }
    }

    // Note: Additional tests for ChangeTrust and swap exceptions, as well as testing the Relayer
    // fee payment strategy path, would require complex mocking of get_ledger_entries and proper
    // mock setup. The core validation logic for User fee payment strategy rejection is tested above.
    // Integration tests for these exception cases exist in the full test suite.
}
