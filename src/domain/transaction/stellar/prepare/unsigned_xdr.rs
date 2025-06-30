//! This module handles the preparation of unsigned XDR transactions.
//! It includes XDR parsing, validation, sequence updating, and fee updating.

use eyre::Result;
use log::info;
use soroban_rs::xdr::{Limits, ReadXdr, TransactionEnvelope, WriteXdr};

use crate::{
    domain::{extract_operations, extract_source_account},
    models::{StellarTransactionData, StellarValidationError, TransactionError, TransactionInput},
    repositories::TransactionCounterTrait,
    services::{Signer, StellarProviderTrait},
};

use super::common::{
    apply_sequence, ensure_minimum_fee, get_next_sequence, sign_stellar_transaction,
    simulate_if_needed,
};

/// Process an unsigned XDR transaction.
///
/// This function:
/// 1. Parses the unsigned XDR from the transaction input
/// 2. Validates that the source account matches the relayer address
/// 3. Gets the next sequence number and updates the envelope
/// 4. Ensures the transaction has at least the minimum required fee
/// 5. Simulates the transaction if it contains Soroban operations
/// 6. Signs the transaction and returns the updated stellar data
pub async fn process_unsigned_xdr<C, P, S>(
    counter_service: &C,
    relayer_id: &str,
    relayer_address: &str,
    stellar_data: StellarTransactionData,
    provider: &P,
    signer: &S,
) -> Result<StellarTransactionData, TransactionError>
where
    C: TransactionCounterTrait + Send + Sync,
    P: StellarProviderTrait + Send + Sync,
    S: Signer + Send + Sync,
{
    // Step 1: Parse the XDR
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

    // Step 2: Validate source account matches relayer
    let source_account = extract_source_account(&envelope).map_err(|e| {
        TransactionError::ValidationError(format!("Failed to extract source account: {}", e))
    })?;

    if source_account != relayer_address {
        return Err(StellarValidationError::SourceAccountMismatch {
            expected: relayer_address.to_string(),
            actual: source_account,
        }
        .into());
    }

    // Step 3: Get the next sequence number and update the envelope
    let sequence = get_next_sequence(counter_service, relayer_id, relayer_address)?;
    info!(
        "Using sequence number {} for unsigned XDR transaction",
        sequence
    );

    // Apply sequence updates the envelope in-place and returns the XDR
    let _updated_xdr = apply_sequence(&mut envelope, sequence).await?;

    // Update stellar data with sequence number
    let mut stellar_data = stellar_data.with_sequence_number(sequence);

    // Step 4: Ensure minimum fee
    ensure_minimum_fee(&mut envelope).await?;

    // Re-serialize the envelope after fee update
    let updated_xdr = envelope.to_xdr_base64(Limits::none()).map_err(|e| {
        TransactionError::ValidationError(format!("Failed to serialize updated envelope: {}", e))
    })?;

    // Update stellar data with new XDR
    stellar_data.transaction_input = TransactionInput::UnsignedXdr(updated_xdr.clone());

    // Step 5: Check if simulation is needed
    let stellar_data_with_sim = match simulate_if_needed(&envelope, provider).await? {
        Some(sim_resp) => {
            info!("Applying simulation results to unsigned XDR transaction");
            // Get operation count from the envelope
            let op_count = extract_operations(&envelope)?.len() as u64;
            stellar_data
                .with_simulation_data(sim_resp, op_count)
                .map_err(|e| {
                    TransactionError::ValidationError(format!(
                        "Failed to apply simulation data: {}",
                        e
                    ))
                })?
        }
        None => stellar_data,
    };

    // Step 6: Sign the transaction
    sign_stellar_transaction(signer, stellar_data_with_sim).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domain::SignTransactionResponse,
        models::{DecoratedSignature, NetworkTransactionData},
        repositories::TransactionCounterError,
    };
    use soroban_rs::xdr::{
        BytesM, Memo, MuxedAccount, Operation, OperationBody, PaymentOp, Preconditions,
        SequenceNumber, Signature, SignatureHint, Transaction, TransactionExt,
        TransactionV1Envelope, Uint256, VecM, WriteXdr,
    };
    use stellar_strkey::ed25519::PublicKey;

    struct MockCounter {
        sequence: u64,
    }

    impl TransactionCounterTrait for MockCounter {
        fn get_and_increment(
            &self,
            _relayer_id: &str,
            _address: &str,
        ) -> Result<u64, TransactionCounterError> {
            Ok(self.sequence)
        }

        fn get(
            &self,
            _relayer_id: &str,
            _address: &str,
        ) -> Result<Option<u64>, TransactionCounterError> {
            Ok(Some(self.sequence))
        }

        fn decrement(
            &self,
            _relayer_id: &str,
            _address: &str,
        ) -> Result<u64, TransactionCounterError> {
            Ok(self.sequence - 1)
        }

        fn set(
            &self,
            _relayer_id: &str,
            _address: &str,
            _value: u64,
        ) -> Result<(), TransactionCounterError> {
            Ok(())
        }
    }

    struct MockProvider;

    #[async_trait::async_trait]
    impl StellarProviderTrait for MockProvider {
        async fn get_account(
            &self,
            _account_id: &str,
        ) -> Result<soroban_rs::xdr::AccountEntry, eyre::Error> {
            unimplemented!()
        }

        async fn simulate_transaction_envelope(
            &self,
            _envelope: &TransactionEnvelope,
        ) -> Result<soroban_rs::stellar_rpc_client::SimulateTransactionResponse, eyre::Error>
        {
            // Return a response indicating no simulation needed
            Ok(
                soroban_rs::stellar_rpc_client::SimulateTransactionResponse {
                    min_resource_fee: 0,
                    transaction_data: String::new(),
                    ..Default::default()
                },
            )
        }

        async fn send_transaction_polling(
            &self,
            _tx_envelope: &TransactionEnvelope,
        ) -> Result<soroban_rs::SorobanTransactionResponse, eyre::Error> {
            unimplemented!()
        }

        async fn get_network(
            &self,
        ) -> Result<soroban_rs::stellar_rpc_client::GetNetworkResponse, eyre::Error> {
            unimplemented!()
        }

        async fn get_latest_ledger(
            &self,
        ) -> Result<soroban_rs::stellar_rpc_client::GetLatestLedgerResponse, eyre::Error> {
            unimplemented!()
        }

        async fn send_transaction(
            &self,
            _tx_envelope: &TransactionEnvelope,
        ) -> Result<soroban_rs::xdr::Hash, eyre::Error> {
            unimplemented!()
        }

        async fn get_transaction(
            &self,
            _tx_id: &soroban_rs::xdr::Hash,
        ) -> Result<soroban_rs::stellar_rpc_client::GetTransactionResponse, eyre::Error> {
            unimplemented!()
        }

        async fn get_transactions(
            &self,
            _request: soroban_rs::stellar_rpc_client::GetTransactionsRequest,
        ) -> Result<soroban_rs::stellar_rpc_client::GetTransactionsResponse, eyre::Error> {
            unimplemented!()
        }

        async fn get_ledger_entries(
            &self,
            _keys: &[soroban_rs::xdr::LedgerKey],
        ) -> Result<soroban_rs::stellar_rpc_client::GetLedgerEntriesResponse, eyre::Error> {
            unimplemented!()
        }

        async fn get_events(
            &self,
            _request: crate::services::GetEventsRequest,
        ) -> Result<soroban_rs::stellar_rpc_client::GetEventsResponse, eyre::Error> {
            unimplemented!()
        }
    }

    struct MockSigner {
        address: String,
    }

    #[async_trait::async_trait]
    impl Signer for MockSigner {
        async fn address(&self) -> Result<crate::models::Address, crate::models::SignerError> {
            Ok(crate::models::Address::Stellar(self.address.clone()))
        }

        async fn sign_transaction(
            &self,
            _data: NetworkTransactionData,
        ) -> Result<SignTransactionResponse, crate::models::SignerError> {
            let sig_bytes: Vec<u8> = vec![1u8; 64];
            let sig_bytes_m: BytesM<64> = sig_bytes.try_into().unwrap();
            Ok(SignTransactionResponse::Stellar(
                crate::domain::SignTransactionResponseStellar {
                    signature: DecoratedSignature {
                        hint: SignatureHint([0; 4]),
                        signature: Signature(sig_bytes_m),
                    },
                },
            ))
        }
    }

    fn create_test_envelope(source_account: &str) -> TransactionEnvelope {
        let pk = PublicKey::from_string(source_account).unwrap();
        let source = MuxedAccount::Ed25519(Uint256(pk.0));

        let dest_pk =
            PublicKey::from_string("GCEZWKCA5VLDNRLN3RPRJMRZOX3Z6G5CHCGSNFHEYVXM3XOJMDS674JZ")
                .unwrap();

        // Create a payment operation
        let payment_op = PaymentOp {
            destination: MuxedAccount::Ed25519(Uint256(dest_pk.0)),
            asset: soroban_rs::xdr::Asset::Native,
            amount: 1000000,
        };

        let operation = Operation {
            source_account: None,
            body: OperationBody::Payment(payment_op),
        };

        let operations: VecM<Operation, 100> = vec![operation].try_into().unwrap();

        let tx = Transaction {
            source_account: source,
            fee: 100,
            seq_num: SequenceNumber(0), // Will be updated
            cond: Preconditions::None,
            memo: Memo::None,
            operations,
            ext: TransactionExt::V0,
        };

        TransactionEnvelope::Tx(TransactionV1Envelope {
            tx,
            signatures: VecM::default(),
        })
    }

    #[tokio::test]
    async fn test_process_unsigned_xdr_valid_source() {
        let relayer_address = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";
        let relayer_id = "test-relayer";
        let expected_sequence = 42i64;

        let counter = MockCounter {
            sequence: expected_sequence as u64,
        };
        let provider = MockProvider;
        let signer = MockSigner {
            address: "test-signer-address".to_string(),
        };

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

        let result = process_unsigned_xdr(
            &counter,
            relayer_id,
            relayer_address,
            stellar_data,
            &provider,
            &signer,
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
        let relayer_address = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";
        let different_address = "GCEZWKCA5VLDNRLN3RPRJMRZOX3Z6G5CHCGSNFHEYVXM3XOJMDS674JZ";
        let relayer_id = "test-relayer";

        let counter = MockCounter { sequence: 42 };
        let provider = MockProvider;
        let signer = MockSigner {
            address: "test-signer-address".to_string(),
        };

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

        let result = process_unsigned_xdr(
            &counter,
            relayer_id,
            relayer_address,
            stellar_data,
            &provider,
            &signer,
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
        let relayer_address = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";
        let relayer_id = "test-relayer";

        let counter = MockCounter { sequence: 42 };
        let provider = MockProvider;
        let signer = MockSigner {
            address: "test-signer-address".to_string(),
        };

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

        let result = process_unsigned_xdr(
            &counter,
            relayer_id,
            relayer_address,
            stellar_data,
            &provider,
            &signer,
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
        let relayer_address = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";
        let relayer_id = "test-relayer";

        let counter = MockCounter { sequence: 42 };
        let provider = MockProvider;
        let signer = MockSigner {
            address: "test-signer-address".to_string(),
        };

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

        let result = process_unsigned_xdr(
            &counter,
            relayer_id,
            relayer_address,
            stellar_data,
            &provider,
            &signer,
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
}
