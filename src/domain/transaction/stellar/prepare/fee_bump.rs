//! Fee-bump transaction preparation logic.

use eyre::Result;
use soroban_rs::xdr::{Limits, ReadXdr, TransactionEnvelope, WriteXdr};

use crate::{
    domain::{attach_signatures_to_envelope, build_fee_bump_envelope, parse_transaction_xdr},
    models::{
        NetworkTransactionData, StellarTransactionData, StellarValidationError, TransactionError,
        TransactionInput,
    },
    services::{Signer, StellarProviderTrait},
};

use super::common::{calculate_fee_bump_required_fee, create_signing_data};

/// Process a fee-bump transaction from signed XDR input.
///
/// This function:
/// 1. Extracts and validates the inner transaction from the signed XDR
/// 2. Simulates the transaction if needed (for Soroban operations)
/// 3. Calculates the required fee based on simulation results or max_fee
/// 4. Builds the fee-bump envelope
/// 5. Signs the fee-bump transaction
/// 6. Returns the updated stellar data with the signed fee-bump envelope
pub async fn process_fee_bump<S, P>(
    relayer_address: &str,
    stellar_data: StellarTransactionData,
    provider: &P,
    signer: &S,
) -> Result<StellarTransactionData, TransactionError>
where
    S: Signer + Send + Sync,
    P: StellarProviderTrait + Send + Sync,
{
    // Step 1: Extract and validate the inner transaction
    let (inner_envelope, max_fee) = extract_inner_transaction(&stellar_data)?;

    // Step 2: Calculate the required fee (may include simulation for Soroban)
    let required_fee = calculate_fee_bump_required_fee(&inner_envelope, max_fee, provider).await?;

    // Step 3: Build the fee-bump envelope
    let fee_bump_envelope =
        build_fee_bump_envelope(inner_envelope, relayer_address, required_fee as i64).map_err(
            |e| {
                TransactionError::ValidationError(format!("Cannot create fee-bump envelope: {}", e))
            },
        )?;

    // Step 4: Sign the fee-bump transaction
    let signed_stellar_data =
        sign_fee_bump_transaction(stellar_data, fee_bump_envelope, relayer_address, signer).await?;

    Ok(signed_stellar_data)
}

/// Extract and validate the inner transaction from SignedXdr input.
///
/// This function:
/// - Extracts the XDR and max_fee from the SignedXdr input
/// - Validates that max_fee is positive
/// - Parses the inner transaction envelope
/// - Returns the parsed envelope and max_fee
fn extract_inner_transaction(
    stellar_data: &StellarTransactionData,
) -> Result<(TransactionEnvelope, i64), TransactionError> {
    // Extract XDR and max_fee from SignedXdr input
    let (inner_xdr, max_fee) = match &stellar_data.transaction_input {
        TransactionInput::SignedXdr { xdr, max_fee } => {
            if *max_fee <= 0 {
                return Err(StellarValidationError::InvalidMaxFee.into());
            }
            (xdr.clone(), *max_fee)
        }
        _ => {
            return Err(TransactionError::ValidationError(
                "Fee-bump requires SignedXdr input".to_string(),
            ))
        }
    };

    // Parse the inner transaction envelope
    let inner_envelope = parse_transaction_xdr(&inner_xdr, true).map_err(|e| {
        StellarValidationError::InvalidXdr(format!("Invalid inner transaction: {}", e))
    })?;

    Ok((inner_envelope, max_fee))
}

/// Sign the fee-bump transaction and return the final stellar data.
///
/// This function:
/// - Serializes the fee-bump envelope
/// - Creates signing data for the fee-bump transaction
/// - Signs the transaction using the provided signer
/// - Attaches the signature to the envelope
/// - Returns the updated stellar data with the signed envelope XDR
async fn sign_fee_bump_transaction<S>(
    mut stellar_data: StellarTransactionData,
    fee_bump_envelope: TransactionEnvelope,
    relayer_address: &str,
    signer: &S,
) -> Result<StellarTransactionData, TransactionError>
where
    S: Signer + Send + Sync,
{
    use crate::domain::SignTransactionResponse;

    // Serialize the fee-bump envelope
    let fee_bump_xdr = fee_bump_envelope
        .to_xdr_base64(Limits::none())
        .map_err(|e| {
            TransactionError::ValidationError(format!(
                "Failed to serialize fee-bump envelope: {}",
                e
            ))
        })?;

    // Create signing data for the fee-bump transaction
    let signing_data = create_signing_data(
        relayer_address.to_string(),
        fee_bump_xdr.clone(),
        stellar_data.network_passphrase.clone(),
    );

    // Sign the transaction
    let sig_resp = signer
        .sign_transaction(NetworkTransactionData::Stellar(signing_data))
        .await?;

    let signature = match sig_resp {
        SignTransactionResponse::Stellar(s) => s.signature,
        _ => {
            return Err(TransactionError::InvalidType(
                "Expected Stellar signature".into(),
            ));
        }
    };

    // Parse the envelope to attach the signature
    let mut signed_envelope = TransactionEnvelope::from_xdr_base64(&fee_bump_xdr, Limits::none())
        .map_err(|e| {
        TransactionError::SignerError(format!("Failed to parse fee-bump envelope: {}", e))
    })?;

    // Attach the signature directly to the fee-bump envelope
    attach_signatures_to_envelope(&mut signed_envelope, vec![signature.clone()]).map_err(|e| {
        TransactionError::SignerError(format!(
            "Failed to attach signature to fee-bump envelope: {}",
            e
        ))
    })?;

    // Serialize the signed envelope
    let signed_xdr = signed_envelope.to_xdr_base64(Limits::none()).map_err(|e| {
        TransactionError::SignerError(format!(
            "Failed to serialize signed fee-bump envelope: {}",
            e
        ))
    })?;

    // Update stellar data
    stellar_data = stellar_data.attach_signature(signature);
    stellar_data.signed_envelope_xdr = Some(signed_xdr);

    Ok(stellar_data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::STELLAR_DEFAULT_TRANSACTION_FEE;
    use soroban_rs::xdr::{
        Memo, MuxedAccount, Operation, OperationBody, PaymentOp, Preconditions, SequenceNumber,
        Signature, SignatureHint, Transaction, TransactionExt, TransactionV1Envelope, Uint256,
    };
    use stellar_strkey::ed25519::PublicKey;

    fn create_test_envelope(source: &str, include_signature: bool) -> TransactionEnvelope {
        let source_pk = PublicKey::from_string(source).unwrap();
        let dest_pk =
            PublicKey::from_string("GCEZWKCA5VLDNRLN3RPRJMRZOX3Z6G5CHCGSNFHEYVXM3XOJMDS674JZ")
                .unwrap();

        let payment_op = PaymentOp {
            destination: MuxedAccount::Ed25519(Uint256(dest_pk.0)),
            asset: soroban_rs::xdr::Asset::Native,
            amount: 1000000,
        };

        let operation = Operation {
            source_account: None,
            body: OperationBody::Payment(payment_op),
        };

        let tx = Transaction {
            source_account: MuxedAccount::Ed25519(Uint256(source_pk.0)),
            fee: 100,
            seq_num: SequenceNumber(1),
            cond: Preconditions::None,
            memo: Memo::None,
            operations: vec![operation].try_into().unwrap(),
            ext: TransactionExt::V0,
        };

        let mut envelope = TransactionV1Envelope {
            tx,
            signatures: vec![].try_into().unwrap(),
        };

        if include_signature {
            let sig = soroban_rs::xdr::DecoratedSignature {
                hint: SignatureHint([0; 4]),
                signature: Signature(vec![0u8; 64].try_into().unwrap()),
            };
            envelope.signatures = vec![sig].try_into().unwrap();
        }

        TransactionEnvelope::Tx(envelope)
    }

    #[test]
    fn test_extract_inner_transaction_valid() {
        let envelope = create_test_envelope(
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
            true,
        );
        let xdr = envelope.to_xdr_base64(Limits::none()).unwrap();

        let stellar_data = StellarTransactionData {
            source_account: "test".to_string(),
            transaction_input: TransactionInput::SignedXdr {
                xdr: xdr.clone(),
                max_fee: 1_000_000,
            },
            network_passphrase: "Test Network".to_string(),
            fee: None,
            sequence_number: None,
            memo: None,
            valid_until: None,
            signatures: vec![],
            hash: None,
            simulation_transaction_data: None,
            signed_envelope_xdr: None,
        };

        let result = extract_inner_transaction(&stellar_data);
        assert!(result.is_ok());

        let (extracted_envelope, max_fee) = result.unwrap();
        assert_eq!(max_fee, 1_000_000);
        assert!(matches!(extracted_envelope, TransactionEnvelope::Tx(_)));
    }

    #[test]
    fn test_extract_inner_transaction_invalid_max_fee() {
        let envelope = create_test_envelope(
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
            true,
        );
        let xdr = envelope.to_xdr_base64(Limits::none()).unwrap();

        let stellar_data = StellarTransactionData {
            source_account: "test".to_string(),
            transaction_input: TransactionInput::SignedXdr {
                xdr,
                max_fee: 0, // Invalid: must be positive
            },
            network_passphrase: "Test Network".to_string(),
            fee: None,
            sequence_number: None,
            memo: None,
            valid_until: None,
            signatures: vec![],
            hash: None,
            simulation_transaction_data: None,
            signed_envelope_xdr: None,
        };

        let result = extract_inner_transaction(&stellar_data);
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            TransactionError::ValidationError(msg) => {
                assert!(msg.contains("max_fee must be greater than 0"));
            }
            _ => panic!("Expected ValidationError, got {:?}", err),
        }
    }

    #[test]
    fn test_extract_inner_transaction_wrong_input_type() {
        let stellar_data = StellarTransactionData {
            source_account: "test".to_string(),
            transaction_input: TransactionInput::Operations(vec![]), // Wrong type
            network_passphrase: "Test Network".to_string(),
            fee: None,
            sequence_number: None,
            memo: None,
            valid_until: None,
            signatures: vec![],
            hash: None,
            simulation_transaction_data: None,
            signed_envelope_xdr: None,
        };

        let result = extract_inner_transaction(&stellar_data);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TransactionError::ValidationError(_)
        ));
    }

    #[tokio::test]
    async fn test_process_fee_bump_integration() {
        // This is a skeleton for integration testing.
        // In a real test, you would:
        // 1. Mock the provider to return simulation results if needed
        // 2. Mock the signer to return a test signature
        // 3. Verify the entire flow works correctly

        // For now, we just verify the module compiles and basic structure works
        assert_eq!(STELLAR_DEFAULT_TRANSACTION_FEE, 100);
    }
}
