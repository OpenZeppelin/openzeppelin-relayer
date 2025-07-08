//! XDR utility functions for Stellar transaction processing.
//!
//! This module provides utilities for parsing, validating, and manipulating
//! Stellar transaction XDR (External Data Representation) structures. It includes
//! support for regular transactions, fee-bump transactions, and various transaction
//! formats (V0, V1).

use crate::models::StellarValidationError;
use eyre::{eyre, Result};
use soroban_rs::xdr::{
    DecoratedSignature, FeeBumpTransaction, FeeBumpTransactionEnvelope, FeeBumpTransactionInnerTx,
    Limits, MuxedAccount, Operation, OperationBody, ReadXdr, TransactionEnvelope,
    TransactionV1Envelope, Uint256, VecM, WriteXdr,
};
use stellar_strkey::ed25519::PublicKey;

/// Parse a transaction XDR string into a TransactionEnvelope
pub fn parse_transaction_xdr(xdr: &str, expect_signed: bool) -> Result<TransactionEnvelope> {
    let envelope = TransactionEnvelope::from_xdr_base64(xdr, Limits::none())
        .map_err(|e| StellarValidationError::InvalidXdr(e.to_string()))?;

    if expect_signed && !is_signed(&envelope) {
        return Err(StellarValidationError::UnexpectedUnsignedXdr.into());
    }

    Ok(envelope)
}

/// Check if a transaction envelope is signed
pub fn is_signed(envelope: &TransactionEnvelope) -> bool {
    match envelope {
        TransactionEnvelope::TxV0(e) => !e.signatures.is_empty(),
        TransactionEnvelope::Tx(TransactionV1Envelope { signatures, .. }) => !signatures.is_empty(),
        TransactionEnvelope::TxFeeBump(FeeBumpTransactionEnvelope { signatures, .. }) => {
            !signatures.is_empty()
        }
    }
}

/// Check if a transaction envelope is a fee-bump transaction
pub fn is_fee_bump(envelope: &TransactionEnvelope) -> bool {
    matches!(envelope, TransactionEnvelope::TxFeeBump(_))
}

/// Extract the source account from a transaction envelope
pub fn extract_source_account(envelope: &TransactionEnvelope) -> Result<String> {
    let muxed_account = match envelope {
        TransactionEnvelope::TxV0(e) => {
            // For V0 transactions, the source account is Ed25519 only
            let bytes: [u8; 32] = e.tx.source_account_ed25519.0;
            let pk = PublicKey(bytes);
            return Ok(pk.to_string());
        }
        TransactionEnvelope::Tx(TransactionV1Envelope { tx, .. }) => &tx.source_account,
        TransactionEnvelope::TxFeeBump(FeeBumpTransactionEnvelope { tx, .. }) => &tx.fee_source,
    };

    muxed_account_to_string(muxed_account)
}

/// Validate that the source account of a transaction matches the expected account
pub fn validate_source_account(envelope: &TransactionEnvelope, expected: &str) -> Result<()> {
    let source = extract_source_account(envelope)?;
    if source != expected {
        return Err(eyre!(
            "Source account mismatch: expected {}, got {}",
            expected,
            source
        ));
    }
    Ok(())
}

/// Build a fee-bump transaction envelope
pub fn build_fee_bump_envelope(
    inner_envelope: TransactionEnvelope,
    fee_source: &str,
    max_fee: i64,
) -> Result<TransactionEnvelope> {
    // Validate that the inner transaction is signed
    if !is_signed(&inner_envelope) {
        return Err(eyre!("Inner transaction must be signed before fee-bumping"));
    }

    // Extract inner transaction source to ensure it's different from fee source
    let inner_source = extract_source_account(&inner_envelope)?;
    if inner_source == fee_source {
        return Err(eyre!(
            "Fee-bump source cannot be the same as inner transaction source"
        ));
    }

    // Convert fee source to MuxedAccount
    let fee_source_muxed = string_to_muxed_account(fee_source)?;

    // Create the inner transaction wrapper
    let inner_tx = match inner_envelope {
        TransactionEnvelope::TxV0(v0_envelope) => {
            // Convert V0 to V1 envelope for fee-bump
            FeeBumpTransactionInnerTx::Tx(convert_v0_to_v1_envelope(v0_envelope))
        }
        TransactionEnvelope::Tx(e) => FeeBumpTransactionInnerTx::Tx(e),
        TransactionEnvelope::TxFeeBump(_) => {
            return Err(eyre!("Cannot fee-bump a fee-bump transaction"));
        }
    };

    // Create the fee-bump transaction
    let fee_bump_tx = FeeBumpTransaction {
        fee_source: fee_source_muxed,
        fee: max_fee,
        inner_tx,
        ext: soroban_rs::xdr::FeeBumpTransactionExt::V0,
    };

    // Create the fee-bump envelope (unsigned initially)
    let fee_bump_envelope = FeeBumpTransactionEnvelope {
        tx: fee_bump_tx,
        signatures: vec![].try_into()?,
    };

    Ok(TransactionEnvelope::TxFeeBump(fee_bump_envelope))
}

/// Extract the inner transaction hash from a fee-bump envelope
pub fn extract_inner_transaction_hash(envelope: &TransactionEnvelope) -> Result<String> {
    match envelope {
        TransactionEnvelope::TxFeeBump(fb_envelope) => {
            let FeeBumpTransactionInnerTx::Tx(inner_tx) = &fb_envelope.tx.inner_tx;

            // Calculate the hash of the inner transaction
            let inner_envelope = TransactionEnvelope::Tx(inner_tx.clone());
            let hash = calculate_transaction_hash(&inner_envelope)?;
            Ok(hash)
        }
        _ => Err(eyre!("Not a fee-bump transaction")),
    }
}

/// Calculate the hash of a transaction envelope
pub fn calculate_transaction_hash(envelope: &TransactionEnvelope) -> Result<String> {
    use sha2::{Digest, Sha256};

    let xdr_bytes = envelope
        .to_xdr(Limits::none())
        .map_err(|e| eyre!("Failed to serialize transaction: {}", e))?;

    let mut hasher = Sha256::new();
    hasher.update(&xdr_bytes);
    let hash = hasher.finalize();

    Ok(hex::encode(hash))
}

/// Convert a MuxedAccount to a string representation
pub fn muxed_account_to_string(muxed: &MuxedAccount) -> Result<String> {
    match muxed {
        MuxedAccount::Ed25519(key) => {
            let bytes: [u8; 32] = key.0;
            let pk = PublicKey(bytes);
            Ok(pk.to_string())
        }
        MuxedAccount::MuxedEd25519(m) => {
            // For muxed accounts, we need to extract the underlying ed25519 key
            let bytes: [u8; 32] = m.ed25519.0;
            let pk = PublicKey(bytes);
            Ok(pk.to_string())
        }
    }
}

/// Convert a string address to a MuxedAccount
pub fn string_to_muxed_account(address: &str) -> Result<MuxedAccount> {
    let pk =
        PublicKey::from_string(address).map_err(|e| eyre!("Failed to decode account ID: {}", e))?;

    let key = Uint256(pk.0);
    Ok(MuxedAccount::Ed25519(key))
}

/// Extract operations from a transaction envelope
pub fn extract_operations(envelope: &TransactionEnvelope) -> Result<&VecM<Operation, 100>> {
    match envelope {
        TransactionEnvelope::TxV0(e) => Ok(&e.tx.operations),
        TransactionEnvelope::Tx(e) => Ok(&e.tx.operations),
        TransactionEnvelope::TxFeeBump(e) => {
            // For fee-bump transactions, extract operations from inner transaction
            match &e.tx.inner_tx {
                FeeBumpTransactionInnerTx::Tx(inner) => Ok(&inner.tx.operations),
            }
        }
    }
}

/// Check if a transaction envelope contains operations that require simulation
pub fn xdr_needs_simulation(envelope: &TransactionEnvelope) -> Result<bool> {
    let operations = extract_operations(envelope)?;

    // Check if any operation is a Soroban operation
    for op in operations.iter() {
        if matches!(op.body, OperationBody::InvokeHostFunction(_)) {
            return Ok(true);
        }
    }

    Ok(false)
}

/// Attach signatures to a transaction envelope
/// This function handles all envelope types (V0, V1, and FeeBump)
pub fn attach_signatures_to_envelope(
    envelope: &mut TransactionEnvelope,
    signatures: Vec<DecoratedSignature>,
) -> Result<()> {
    let signatures_vec: VecM<DecoratedSignature, 20> = signatures
        .try_into()
        .map_err(|_| eyre!("Too many signatures (max 20)"))?;

    match envelope {
        TransactionEnvelope::TxV0(ref mut v0_env) => {
            v0_env.signatures = signatures_vec;
        }
        TransactionEnvelope::Tx(ref mut v1_env) => {
            v1_env.signatures = signatures_vec;
        }
        TransactionEnvelope::TxFeeBump(ref mut fb_env) => {
            fb_env.signatures = signatures_vec;
        }
    }

    Ok(())
}

/// Convert a V0 transaction envelope to V1 format
/// This is required for fee-bump transactions as they only support V1 inner transactions
fn convert_v0_to_v1_envelope(
    v0_envelope: soroban_rs::xdr::TransactionV0Envelope,
) -> TransactionV1Envelope {
    let v0_tx = &v0_envelope.tx;
    let source_bytes: [u8; 32] = v0_tx.source_account_ed25519.0;

    // Create V1 transaction from V0 data
    let tx = soroban_rs::xdr::Transaction {
        source_account: MuxedAccount::Ed25519(Uint256(source_bytes)),
        fee: v0_tx.fee,
        seq_num: v0_tx.seq_num.clone(),
        cond: match v0_tx.time_bounds.clone() {
            Some(tb) => soroban_rs::xdr::Preconditions::Time(tb),
            None => soroban_rs::xdr::Preconditions::None,
        },
        memo: v0_tx.memo.clone(),
        operations: v0_tx.operations.clone(),
        ext: soroban_rs::xdr::TransactionExt::V0,
    };

    // Create V1 envelope with V0 signatures
    TransactionV1Envelope {
        tx,
        signatures: v0_envelope.signatures.clone(),
    }
}

/// Update the sequence number in an XDR envelope
pub fn update_xdr_sequence(envelope: &mut TransactionEnvelope, sequence: i64) -> Result<()> {
    match envelope {
        TransactionEnvelope::TxV0(ref mut e) => {
            e.tx.seq_num = soroban_rs::xdr::SequenceNumber(sequence);
        }
        TransactionEnvelope::Tx(ref mut e) => {
            e.tx.seq_num = soroban_rs::xdr::SequenceNumber(sequence);
        }
        TransactionEnvelope::TxFeeBump(_) => {
            return Err(eyre!("Cannot set sequence number on fee-bump transaction"));
        }
    }
    Ok(())
}

/// Update the fee in an XDR envelope
pub fn update_xdr_fee(envelope: &mut TransactionEnvelope, fee: u32) -> Result<()> {
    match envelope {
        TransactionEnvelope::TxV0(ref mut e) => {
            e.tx.fee = fee;
        }
        TransactionEnvelope::Tx(ref mut e) => {
            e.tx.fee = fee;
        }
        TransactionEnvelope::TxFeeBump(_) => {
            return Err(eyre!(
                "Cannot set fee on fee-bump transaction - use max_fee instead"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_rs::xdr::{
        Asset, BytesM, DecoratedSignature, FeeBumpTransactionInnerTx, HostFunction,
        InvokeContractArgs, InvokeHostFunctionOp, Limits, Memo, MuxedAccount, Operation,
        OperationBody, PaymentOp, Preconditions, SequenceNumber, Signature, SignatureHint,
        Transaction, TransactionEnvelope, TransactionExt, TransactionV0, TransactionV0Envelope,
        TransactionV1Envelope, Uint256, VecM, WriteXdr,
    };
    use stellar_strkey::ed25519::PublicKey;

    // Helper function to create test XDR
    fn create_test_transaction_xdr(include_signature: bool) -> String {
        // Create a test account public key
        let source_pk =
            PublicKey::from_string("GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF")
                .unwrap();
        let dest_pk =
            PublicKey::from_string("GCEZWKCA5VLDNRLN3RPRJMRZOX3Z6G5CHCGSNFHEYVXM3XOJMDS674JZ")
                .unwrap();

        // Create a payment operation
        let payment_op = PaymentOp {
            destination: MuxedAccount::Ed25519(Uint256(dest_pk.0)),
            asset: Asset::Native,
            amount: 1000000, // 0.1 XLM
        };

        let operation = Operation {
            source_account: None,
            body: OperationBody::Payment(payment_op),
        };

        let operations: VecM<Operation, 100> = vec![operation].try_into().unwrap();

        // Create the transaction
        let tx = Transaction {
            source_account: MuxedAccount::Ed25519(Uint256(source_pk.0)),
            fee: 100,
            seq_num: SequenceNumber(1),
            cond: Preconditions::None,
            memo: Memo::None,
            operations,
            ext: TransactionExt::V0,
        };

        // Create the envelope
        let mut envelope = TransactionV1Envelope {
            tx,
            signatures: vec![].try_into().unwrap(),
        };

        if include_signature {
            // Add a dummy signature
            let hint = SignatureHint([0; 4]);
            let sig_bytes: Vec<u8> = vec![0u8; 64];
            let sig_bytes_m: BytesM<64> = sig_bytes.try_into().unwrap();
            let sig = DecoratedSignature {
                hint,
                signature: Signature(sig_bytes_m),
            };
            envelope.signatures = vec![sig].try_into().unwrap();
        }

        let tx_envelope = TransactionEnvelope::Tx(envelope);
        tx_envelope.to_xdr_base64(Limits::none()).unwrap()
    }

    // Helper to get test XDR
    fn get_unsigned_xdr() -> String {
        create_test_transaction_xdr(false)
    }

    fn get_signed_xdr() -> String {
        create_test_transaction_xdr(true)
    }

    const INVALID_XDR: &str = "INVALID_BASE64_XDR_DATA";

    #[test]
    fn test_parse_unsigned_xdr() {
        // This test should parse an unsigned transaction XDR successfully
        let unsigned_xdr = get_unsigned_xdr();
        let result = parse_transaction_xdr(&unsigned_xdr, false);
        assert!(result.is_ok(), "Failed to parse unsigned XDR");

        let envelope = result.unwrap();
        assert!(
            !is_signed(&envelope),
            "Unsigned XDR should not have signatures"
        );
    }

    #[test]
    fn test_parse_signed_xdr() {
        // This test should parse a signed transaction XDR successfully
        let signed_xdr = get_signed_xdr();
        let result = parse_transaction_xdr(&signed_xdr, true);
        assert!(result.is_ok(), "Failed to parse signed XDR");

        let envelope = result.unwrap();
        assert!(is_signed(&envelope), "Signed XDR should have signatures");
    }

    #[test]
    fn test_parse_invalid_xdr() {
        // This test should fail when parsing invalid XDR
        let result = parse_transaction_xdr(INVALID_XDR, false);
        assert!(result.is_err(), "Should fail to parse invalid XDR");
    }

    #[test]
    fn test_validate_unsigned_xdr_expecting_signed() {
        // This test should fail when unsigned XDR is provided but signed is expected
        let unsigned_xdr = get_unsigned_xdr();
        let result = parse_transaction_xdr(&unsigned_xdr, true);
        assert!(
            result.is_err(),
            "Should fail when expecting signed but got unsigned"
        );
    }

    #[test]
    fn test_extract_source_account_from_xdr() {
        // This test should extract the source account from the transaction
        let unsigned_xdr = get_unsigned_xdr();
        let envelope = parse_transaction_xdr(&unsigned_xdr, false).unwrap();
        let source_account = extract_source_account(&envelope).unwrap();
        assert!(!source_account.is_empty(), "Should extract source account");
        assert_eq!(
            source_account,
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF"
        );
    }

    #[test]
    fn test_validate_source_account() {
        // This test should validate that the source account matches expected
        let unsigned_xdr = get_unsigned_xdr();
        let envelope = parse_transaction_xdr(&unsigned_xdr, false).unwrap();
        let source_account = extract_source_account(&envelope).unwrap();

        // This should pass
        let result = validate_source_account(&envelope, &source_account);
        assert!(result.is_ok(), "Should validate matching source account");

        // This should fail
        let result = validate_source_account(&envelope, "DIFFERENT_ACCOUNT");
        assert!(
            result.is_err(),
            "Should fail with non-matching source account"
        );
    }

    #[test]
    fn test_build_fee_bump_envelope() {
        // This test should create a fee-bump transaction from a signed inner transaction
        let signed_xdr = get_signed_xdr();
        let inner_envelope = parse_transaction_xdr(&signed_xdr, true).unwrap();
        let fee_source = "GCEZWKCA5VLDNRLN3RPRJMRZOX3Z6G5CHCGSNFHEYVXM3XOJMDS674JZ";
        let max_fee = 10_000_000; // 1 XLM

        let result = build_fee_bump_envelope(inner_envelope, fee_source, max_fee);
        assert!(result.is_ok(), "Should build fee-bump envelope");

        let fee_bump_envelope = result.unwrap();
        assert!(
            is_fee_bump(&fee_bump_envelope),
            "Should be a fee-bump transaction"
        );
    }

    #[test]
    fn test_fee_bump_requires_different_source() {
        // This test should fail when trying to fee-bump with same source as inner tx
        let signed_xdr = get_signed_xdr();
        let inner_envelope = parse_transaction_xdr(&signed_xdr, true).unwrap();
        let inner_source = extract_source_account(&inner_envelope).unwrap();
        let max_fee = 10_000_000;

        let result = build_fee_bump_envelope(inner_envelope, &inner_source, max_fee);
        assert!(
            result.is_err(),
            "Should fail when fee-bump source equals inner source"
        );
    }

    #[test]
    fn test_extract_inner_transaction_hash() {
        // This test should extract the hash of the inner transaction from a fee-bump
        let signed_xdr = get_signed_xdr();
        let inner_envelope = parse_transaction_xdr(&signed_xdr, true).unwrap();
        let fee_source = "GCEZWKCA5VLDNRLN3RPRJMRZOX3Z6G5CHCGSNFHEYVXM3XOJMDS674JZ";
        let fee_bump_envelope =
            build_fee_bump_envelope(inner_envelope.clone(), fee_source, 10_000_000).unwrap();

        let inner_hash = extract_inner_transaction_hash(&fee_bump_envelope).unwrap();
        assert!(
            !inner_hash.is_empty(),
            "Should extract inner transaction hash"
        );
    }

    #[test]
    fn test_extract_operations_from_v1_envelope() {
        // Test extracting operations from a V1 envelope
        let envelope = create_test_transaction_xdr(false);
        let parsed = TransactionEnvelope::from_xdr_base64(envelope, Limits::none()).unwrap();

        let operations = extract_operations(&parsed).unwrap();
        assert_eq!(operations.len(), 1, "Should extract 1 operation");

        // Verify the operation details
        if let OperationBody::Payment(payment) = &operations[0].body {
            assert_eq!(payment.amount, 1000000, "Payment amount should be 0.1 XLM");
        } else {
            panic!("Expected payment operation");
        }
    }

    #[test]
    fn test_extract_operations_from_v0_envelope() {
        // Test extracting operations from a V0 envelope
        let source_pk =
            PublicKey::from_string("GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF")
                .unwrap();
        let dest_pk =
            PublicKey::from_string("GCEZWKCA5VLDNRLN3RPRJMRZOX3Z6G5CHCGSNFHEYVXM3XOJMDS674JZ")
                .unwrap();

        let payment_op = PaymentOp {
            destination: MuxedAccount::Ed25519(Uint256(dest_pk.0)),
            asset: Asset::Native,
            amount: 2000000, // 0.2 XLM
        };

        let operation = Operation {
            source_account: None,
            body: OperationBody::Payment(payment_op),
        };

        let operations: VecM<Operation, 100> = vec![operation].try_into().unwrap();

        // Create V0 transaction
        let tx_v0 = TransactionV0 {
            source_account_ed25519: Uint256(source_pk.0),
            fee: 100,
            seq_num: SequenceNumber(1),
            time_bounds: None,
            memo: Memo::None,
            operations,
            ext: soroban_rs::xdr::TransactionV0Ext::V0,
        };

        let envelope = TransactionEnvelope::TxV0(TransactionV0Envelope {
            tx: tx_v0,
            signatures: vec![].try_into().unwrap(),
        });

        let operations = extract_operations(&envelope).unwrap();
        assert_eq!(operations.len(), 1, "Should extract 1 operation from V0");

        if let OperationBody::Payment(payment) = &operations[0].body {
            assert_eq!(payment.amount, 2000000, "Payment amount should be 0.2 XLM");
        } else {
            panic!("Expected payment operation");
        }
    }

    #[test]
    fn test_extract_operations_from_fee_bump() {
        // Test extracting operations from a fee-bump envelope
        let signed_xdr = get_signed_xdr();
        let inner_envelope = parse_transaction_xdr(&signed_xdr, true).unwrap();
        let fee_source = "GCEZWKCA5VLDNRLN3RPRJMRZOX3Z6G5CHCGSNFHEYVXM3XOJMDS674JZ";
        let fee_bump_envelope =
            build_fee_bump_envelope(inner_envelope, fee_source, 10_000_000).unwrap();

        let operations = extract_operations(&fee_bump_envelope).unwrap();
        assert_eq!(
            operations.len(),
            1,
            "Should extract operations from inner tx"
        );

        if let OperationBody::Payment(payment) = &operations[0].body {
            assert_eq!(payment.amount, 1000000, "Payment amount should be 0.1 XLM");
        } else {
            panic!("Expected payment operation");
        }
    }

    #[test]
    fn test_xdr_needs_simulation_with_soroban_operation() {
        // Test that Soroban operations require simulation
        let source_pk =
            PublicKey::from_string("GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF")
                .unwrap();

        // Create a Soroban InvokeHostFunction operation
        let invoke_op = InvokeHostFunctionOp {
            host_function: HostFunction::InvokeContract(InvokeContractArgs {
                contract_address: soroban_rs::xdr::ScAddress::Contract([0u8; 32].into()),
                function_name: "test".try_into().unwrap(),
                args: vec![].try_into().unwrap(),
            }),
            auth: vec![].try_into().unwrap(),
        };

        let operation = Operation {
            source_account: None,
            body: OperationBody::InvokeHostFunction(invoke_op),
        };

        let operations: VecM<Operation, 100> = vec![operation].try_into().unwrap();

        let tx = Transaction {
            source_account: MuxedAccount::Ed25519(Uint256(source_pk.0)),
            fee: 100,
            seq_num: SequenceNumber(1),
            cond: Preconditions::None,
            memo: Memo::None,
            operations,
            ext: TransactionExt::V0,
        };

        let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
            tx,
            signatures: vec![].try_into().unwrap(),
        });

        let needs_sim = xdr_needs_simulation(&envelope).unwrap();
        assert!(needs_sim, "Soroban operations should require simulation");
    }

    #[test]
    fn test_xdr_needs_simulation_without_soroban() {
        // Test that non-Soroban operations don't require simulation
        let envelope = create_test_transaction_xdr(false);
        let parsed = TransactionEnvelope::from_xdr_base64(envelope, Limits::none()).unwrap();

        let needs_sim = xdr_needs_simulation(&parsed).unwrap();
        assert!(
            !needs_sim,
            "Payment operations should not require simulation"
        );
    }

    #[test]
    fn test_xdr_needs_simulation_with_multiple_operations() {
        // Test with multiple operations where at least one is Soroban
        let source_pk =
            PublicKey::from_string("GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF")
                .unwrap();
        let dest_pk =
            PublicKey::from_string("GCEZWKCA5VLDNRLN3RPRJMRZOX3Z6G5CHCGSNFHEYVXM3XOJMDS674JZ")
                .unwrap();

        // Create a payment operation
        let payment_op = Operation {
            source_account: None,
            body: OperationBody::Payment(PaymentOp {
                destination: MuxedAccount::Ed25519(Uint256(dest_pk.0)),
                asset: Asset::Native,
                amount: 1000000,
            }),
        };

        // Create a Soroban operation
        let soroban_op = Operation {
            source_account: None,
            body: OperationBody::InvokeHostFunction(InvokeHostFunctionOp {
                host_function: HostFunction::InvokeContract(InvokeContractArgs {
                    contract_address: soroban_rs::xdr::ScAddress::Contract([0u8; 32].into()),
                    function_name: "test".try_into().unwrap(),
                    args: vec![].try_into().unwrap(),
                }),
                auth: vec![].try_into().unwrap(),
            }),
        };

        let operations: VecM<Operation, 100> = vec![payment_op, soroban_op].try_into().unwrap();

        let tx = Transaction {
            source_account: MuxedAccount::Ed25519(Uint256(source_pk.0)),
            fee: 100,
            seq_num: SequenceNumber(1),
            cond: Preconditions::None,
            memo: Memo::None,
            operations,
            ext: TransactionExt::V0,
        };

        let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
            tx,
            signatures: vec![].try_into().unwrap(),
        });

        let needs_sim = xdr_needs_simulation(&envelope).unwrap();
        assert!(
            needs_sim,
            "Should require simulation when any operation is Soroban"
        );
    }

    #[test]
    fn test_calculate_transaction_hash() {
        // Test transaction hash calculation
        let envelope_xdr = get_signed_xdr();
        let envelope = parse_transaction_xdr(&envelope_xdr, true).unwrap();

        let hash1 = calculate_transaction_hash(&envelope).unwrap();
        let hash2 = calculate_transaction_hash(&envelope).unwrap();

        // Hash should be deterministic
        assert_eq!(hash1, hash2, "Hash should be deterministic");
        assert_eq!(hash1.len(), 64, "SHA256 hash should be 64 hex characters");

        // Verify it's valid hex
        assert!(
            hash1.chars().all(|c| c.is_ascii_hexdigit()),
            "Hash should be valid hex"
        );
    }

    #[test]
    fn test_muxed_account_conversion() {
        let address = "GCEZWKCA5VLDNRLN3RPRJMRZOX3Z6G5CHCGSNFHEYVXM3XOJMDS674JZ";
        let muxed = string_to_muxed_account(address).unwrap();
        let back = muxed_account_to_string(&muxed).unwrap();
        assert_eq!(address, back);
    }

    #[test]
    fn test_muxed_account_ed25519_variant() {
        // Test handling of regular Ed25519 accounts
        let address = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";
        let muxed = string_to_muxed_account(address).unwrap();

        match muxed {
            MuxedAccount::Ed25519(_) => (),
            _ => panic!("Expected Ed25519 variant"),
        }

        let back = muxed_account_to_string(&muxed).unwrap();
        assert_eq!(address, back);
    }

    #[test]
    fn test_muxed_account_muxed_ed25519_variant() {
        // Test handling of MuxedEd25519 accounts
        let pk = PublicKey::from_string("GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF")
            .unwrap();

        let muxed = MuxedAccount::MuxedEd25519(soroban_rs::xdr::MuxedAccountMed25519 {
            id: 123456789,
            ed25519: Uint256(pk.0),
        });

        let address = muxed_account_to_string(&muxed).unwrap();
        assert_eq!(
            address,
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF"
        );
    }

    #[test]
    fn test_v0_to_v1_conversion_in_fee_bump() {
        // Test the V0 to V1 conversion logic in build_fee_bump_envelope
        let source_pk =
            PublicKey::from_string("GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF")
                .unwrap();
        let dest_pk =
            PublicKey::from_string("GCEZWKCA5VLDNRLN3RPRJMRZOX3Z6G5CHCGSNFHEYVXM3XOJMDS674JZ")
                .unwrap();

        // Create V0 transaction with time bounds
        let time_bounds = soroban_rs::xdr::TimeBounds {
            min_time: soroban_rs::xdr::TimePoint(1000),
            max_time: soroban_rs::xdr::TimePoint(2000),
        };

        let payment_op = Operation {
            source_account: None,
            body: OperationBody::Payment(PaymentOp {
                destination: MuxedAccount::Ed25519(Uint256(dest_pk.0)),
                asset: Asset::Native,
                amount: 3000000,
            }),
        };

        let operations: VecM<Operation, 100> = vec![payment_op].try_into().unwrap();

        let tx_v0 = TransactionV0 {
            source_account_ed25519: Uint256(source_pk.0),
            fee: 200,
            seq_num: SequenceNumber(42),
            time_bounds: Some(time_bounds.clone()),
            memo: Memo::Text("Test memo".as_bytes().to_vec().try_into().unwrap()),
            operations: operations.clone(),
            ext: soroban_rs::xdr::TransactionV0Ext::V0,
        };

        // Add a signature to V0 envelope
        let sig = DecoratedSignature {
            hint: SignatureHint([1, 2, 3, 4]),
            signature: Signature(vec![5u8; 64].try_into().unwrap()),
        };

        let v0_envelope = TransactionEnvelope::TxV0(TransactionV0Envelope {
            tx: tx_v0,
            signatures: vec![sig.clone()].try_into().unwrap(),
        });

        // Build fee-bump from V0 envelope
        let fee_source = "GCEZWKCA5VLDNRLN3RPRJMRZOX3Z6G5CHCGSNFHEYVXM3XOJMDS674JZ";
        let fee_bump_envelope =
            build_fee_bump_envelope(v0_envelope, fee_source, 50_000_000).unwrap();

        // Verify it's a fee-bump envelope
        assert!(matches!(
            fee_bump_envelope,
            TransactionEnvelope::TxFeeBump(_)
        ));

        if let TransactionEnvelope::TxFeeBump(fb_env) = fee_bump_envelope {
            // Verify fee source
            let fb_source = muxed_account_to_string(&fb_env.tx.fee_source).unwrap();
            assert_eq!(fb_source, fee_source);
            assert_eq!(fb_env.tx.fee, 50_000_000);

            // Verify inner transaction was properly converted
            let FeeBumpTransactionInnerTx::Tx(inner_v1) = &fb_env.tx.inner_tx;
            // Check that V0 data was preserved in V1 format
            assert_eq!(inner_v1.tx.fee, 200);
            assert_eq!(inner_v1.tx.seq_num.0, 42);

            // Check time bounds conversion
            if let Preconditions::Time(tb) = &inner_v1.tx.cond {
                assert_eq!(tb.min_time.0, 1000);
                assert_eq!(tb.max_time.0, 2000);
            } else {
                panic!("Expected time bounds in preconditions");
            }

            // Check memo preservation
            if let Memo::Text(text) = &inner_v1.tx.memo {
                assert_eq!(text.as_slice(), "Test memo".as_bytes());
            } else {
                panic!("Expected text memo");
            }

            // Check operations preservation
            assert_eq!(inner_v1.tx.operations.len(), 1);
            // Check signatures were preserved
            assert_eq!(inner_v1.signatures.len(), 1);
            assert_eq!(inner_v1.signatures[0].hint, sig.hint);
        }
    }

    #[test]
    fn test_attach_signatures_to_envelope() {
        use soroban_rs::xdr::{
            DecoratedSignature, Memo, Operation, OperationBody, PaymentOp, SequenceNumber,
            Signature, SignatureHint, TransactionV0, TransactionV0Envelope,
        };
        use stellar_strkey::ed25519::PublicKey;

        let source_pk =
            PublicKey::from_string("GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF")
                .unwrap();
        let dest_pk =
            PublicKey::from_string("GCEZWKCA5VLDNRLN3RPRJMRZOX3Z6G5CHCGSNFHEYVXM3XOJMDS674JZ")
                .unwrap();

        // Create a test transaction
        let payment_op = Operation {
            source_account: None,
            body: OperationBody::Payment(PaymentOp {
                destination: MuxedAccount::Ed25519(Uint256(dest_pk.0)),
                asset: soroban_rs::xdr::Asset::Native,
                amount: 1000000,
            }),
        };

        let operations: VecM<Operation, 100> = vec![payment_op].try_into().unwrap();

        let tx_v0 = TransactionV0 {
            source_account_ed25519: Uint256(source_pk.0),
            fee: 100,
            seq_num: SequenceNumber(42),
            time_bounds: None,
            memo: Memo::None,
            operations,
            ext: soroban_rs::xdr::TransactionV0Ext::V0,
        };

        let mut envelope = TransactionEnvelope::TxV0(TransactionV0Envelope {
            tx: tx_v0,
            signatures: vec![].try_into().unwrap(),
        });

        // Create test signatures
        let sig1 = DecoratedSignature {
            hint: SignatureHint([1, 2, 3, 4]),
            signature: Signature(vec![1u8; 64].try_into().unwrap()),
        };
        let sig2 = DecoratedSignature {
            hint: SignatureHint([5, 6, 7, 8]),
            signature: Signature(vec![2u8; 64].try_into().unwrap()),
        };

        // Attach signatures
        let result = attach_signatures_to_envelope(&mut envelope, vec![sig1, sig2]);
        assert!(result.is_ok());

        // Verify signatures were attached
        match &envelope {
            TransactionEnvelope::TxV0(e) => {
                assert_eq!(e.signatures.len(), 2);
                assert_eq!(e.signatures[0].hint.0, [1, 2, 3, 4]);
                assert_eq!(e.signatures[1].hint.0, [5, 6, 7, 8]);
            }
            _ => panic!("Expected V0 envelope"),
        }
    }

    #[test]
    fn test_extract_operations() {
        use soroban_rs::xdr::{
            Memo, Operation, OperationBody, PaymentOp, SequenceNumber, Transaction, TransactionV0,
            TransactionV0Envelope, TransactionV1Envelope,
        };
        use stellar_strkey::ed25519::PublicKey;

        let source_pk =
            PublicKey::from_string("GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF")
                .unwrap();
        let dest_pk =
            PublicKey::from_string("GCEZWKCA5VLDNRLN3RPRJMRZOX3Z6G5CHCGSNFHEYVXM3XOJMDS674JZ")
                .unwrap();

        // Create test operation
        let payment_op = Operation {
            source_account: None,
            body: OperationBody::Payment(PaymentOp {
                destination: MuxedAccount::Ed25519(Uint256(dest_pk.0)),
                asset: soroban_rs::xdr::Asset::Native,
                amount: 1000000,
            }),
        };

        let operations: VecM<Operation, 100> = vec![payment_op.clone()].try_into().unwrap();

        // Test V0 envelope
        let tx_v0 = TransactionV0 {
            source_account_ed25519: Uint256(source_pk.0),
            fee: 100,
            seq_num: SequenceNumber(42),
            time_bounds: None,
            memo: Memo::None,
            operations: operations.clone(),
            ext: soroban_rs::xdr::TransactionV0Ext::V0,
        };

        let v0_envelope = TransactionEnvelope::TxV0(TransactionV0Envelope {
            tx: tx_v0,
            signatures: vec![].try_into().unwrap(),
        });

        let extracted_ops = extract_operations(&v0_envelope).unwrap();
        assert_eq!(extracted_ops.len(), 1);

        // Test V1 envelope
        let tx_v1 = Transaction {
            source_account: MuxedAccount::Ed25519(Uint256(source_pk.0)),
            fee: 100,
            seq_num: SequenceNumber(42),
            cond: soroban_rs::xdr::Preconditions::None,
            memo: Memo::None,
            operations: operations.clone(),
            ext: soroban_rs::xdr::TransactionExt::V0,
        };

        let v1_envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
            tx: tx_v1,
            signatures: vec![].try_into().unwrap(),
        });

        let extracted_ops = extract_operations(&v1_envelope).unwrap();
        assert_eq!(extracted_ops.len(), 1);
    }

    #[test]
    fn test_xdr_needs_simulation() {
        use soroban_rs::xdr::{
            HostFunction, InvokeHostFunctionOp, Memo, Operation, OperationBody, PaymentOp,
            ScSymbol, ScVal, SequenceNumber, Transaction, TransactionV1Envelope,
        };
        use stellar_strkey::ed25519::PublicKey;

        let source_pk =
            PublicKey::from_string("GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF")
                .unwrap();
        let dest_pk =
            PublicKey::from_string("GCEZWKCA5VLDNRLN3RPRJMRZOX3Z6G5CHCGSNFHEYVXM3XOJMDS674JZ")
                .unwrap();

        // Test with payment operation (should not need simulation)
        let payment_op = Operation {
            source_account: None,
            body: OperationBody::Payment(PaymentOp {
                destination: MuxedAccount::Ed25519(Uint256(dest_pk.0)),
                asset: soroban_rs::xdr::Asset::Native,
                amount: 1000000,
            }),
        };

        let operations: VecM<Operation, 100> = vec![payment_op].try_into().unwrap();

        let tx = Transaction {
            source_account: MuxedAccount::Ed25519(Uint256(source_pk.0)),
            fee: 100,
            seq_num: SequenceNumber(42),
            cond: soroban_rs::xdr::Preconditions::None,
            memo: Memo::None,
            operations,
            ext: soroban_rs::xdr::TransactionExt::V0,
        };

        let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
            tx,
            signatures: vec![].try_into().unwrap(),
        });

        assert!(!xdr_needs_simulation(&envelope).unwrap());

        // Test with InvokeHostFunction operation (should need simulation)
        let invoke_op = Operation {
            source_account: None,
            body: OperationBody::InvokeHostFunction(InvokeHostFunctionOp {
                host_function: HostFunction::InvokeContract(soroban_rs::xdr::InvokeContractArgs {
                    contract_address: soroban_rs::xdr::ScAddress::Contract([0u8; 32].into()),
                    function_name: ScSymbol("test".try_into().unwrap()),
                    args: vec![ScVal::U32(42)].try_into().unwrap(),
                }),
                auth: vec![].try_into().unwrap(),
            }),
        };

        let operations: VecM<Operation, 100> = vec![invoke_op].try_into().unwrap();

        let tx = Transaction {
            source_account: MuxedAccount::Ed25519(Uint256(source_pk.0)),
            fee: 100,
            seq_num: SequenceNumber(42),
            cond: soroban_rs::xdr::Preconditions::None,
            memo: Memo::None,
            operations,
            ext: soroban_rs::xdr::TransactionExt::V0,
        };

        let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
            tx,
            signatures: vec![].try_into().unwrap(),
        });

        assert!(xdr_needs_simulation(&envelope).unwrap());
    }

    #[test]
    fn test_v0_to_v1_conversion() {
        use soroban_rs::xdr::{
            Memo, Operation, OperationBody, PaymentOp, SequenceNumber, TimeBounds, TimePoint,
            TransactionV0, TransactionV0Envelope,
        };
        use stellar_strkey::ed25519::PublicKey;

        let source_pk =
            PublicKey::from_string("GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF")
                .unwrap();
        let dest_pk =
            PublicKey::from_string("GCEZWKCA5VLDNRLN3RPRJMRZOX3Z6G5CHCGSNFHEYVXM3XOJMDS674JZ")
                .unwrap();

        // Create test V0 transaction with various fields
        let time_bounds = TimeBounds {
            min_time: TimePoint(1000),
            max_time: TimePoint(2000),
        };

        let payment_op = Operation {
            source_account: None,
            body: OperationBody::Payment(PaymentOp {
                destination: MuxedAccount::Ed25519(Uint256(dest_pk.0)),
                asset: soroban_rs::xdr::Asset::Native,
                amount: 1000000,
            }),
        };

        let operations: VecM<Operation, 100> = vec![payment_op].try_into().unwrap();

        let tx_v0 = TransactionV0 {
            source_account_ed25519: Uint256(source_pk.0),
            fee: 100,
            seq_num: SequenceNumber(42),
            time_bounds: Some(time_bounds.clone()),
            memo: Memo::Text("Test".as_bytes().to_vec().try_into().unwrap()),
            operations: operations.clone(),
            ext: soroban_rs::xdr::TransactionV0Ext::V0,
        };

        let sig = soroban_rs::xdr::DecoratedSignature {
            hint: soroban_rs::xdr::SignatureHint([1, 2, 3, 4]),
            signature: soroban_rs::xdr::Signature(vec![0u8; 64].try_into().unwrap()),
        };

        let v0_envelope = TransactionV0Envelope {
            tx: tx_v0,
            signatures: vec![sig.clone()].try_into().unwrap(),
        };

        // Convert to V1
        let v1_envelope = convert_v0_to_v1_envelope(v0_envelope);

        // Verify conversion preserved all data
        assert_eq!(v1_envelope.tx.fee, 100);
        assert_eq!(v1_envelope.tx.seq_num.0, 42);
        assert_eq!(v1_envelope.tx.operations.len(), 1);
        assert_eq!(v1_envelope.signatures.len(), 1);

        // Check source account conversion
        if let MuxedAccount::Ed25519(key) = &v1_envelope.tx.source_account {
            assert_eq!(key.0, source_pk.0);
        } else {
            panic!("Expected Ed25519 source account");
        }

        // Check time bounds conversion
        if let soroban_rs::xdr::Preconditions::Time(tb) = &v1_envelope.tx.cond {
            assert_eq!(tb.min_time.0, 1000);
            assert_eq!(tb.max_time.0, 2000);
        } else {
            panic!("Expected time bounds in preconditions");
        }

        // Check memo preservation
        if let Memo::Text(text) = &v1_envelope.tx.memo {
            assert_eq!(text.as_slice(), "Test".as_bytes());
        } else {
            panic!("Expected text memo");
        }
    }
}
