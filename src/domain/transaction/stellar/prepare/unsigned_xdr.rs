//! This module handles the preparation of unsigned XDR transactions.
//! It includes XDR parsing, validation, sequence updating, and fee updating.

use eyre::Result;
use log::info;
use soroban_rs::xdr::{Limits, ReadXdr, TransactionEnvelope, WriteXdr};

use crate::{
    constants::STELLAR_DEFAULT_TRANSACTION_FEE,
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
    let sequence = get_next_sequence(counter_service, relayer_id, relayer_address).await?;
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

    // Step 6: Sign the transaction
    sign_stellar_transaction(signer, stellar_data_with_sim).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domain::SignTransactionResponse,
        models::{DecoratedSignature, NetworkTransactionData, RepositoryError},
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

    #[async_trait::async_trait]
    impl TransactionCounterTrait for MockCounter {
        async fn get_and_increment(
            &self,
            _relayer_id: &str,
            _address: &str,
        ) -> Result<u64, RepositoryError> {
            Ok(self.sequence)
        }

        async fn get(
            &self,
            _relayer_id: &str,
            _address: &str,
        ) -> Result<Option<u64>, RepositoryError> {
            Ok(Some(self.sequence))
        }

        async fn decrement(
            &self,
            _relayer_id: &str,
            _address: &str,
        ) -> Result<u64, RepositoryError> {
            Ok(self.sequence - 1)
        }

        async fn set(
            &self,
            _relayer_id: &str,
            _address: &str,
            _value: u64,
        ) -> Result<(), RepositoryError> {
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

#[cfg(test)]
mod xdr_transaction_tests {
    use std::future::ready;

    use super::*;
    use crate::constants::STELLAR_DEFAULT_TRANSACTION_FEE;
    use crate::domain::transaction::stellar::test_helpers::*;
    use crate::domain::SignTransactionResponse;
    use crate::models::{NetworkTransactionData, RepositoryError, TransactionStatus};
    use soroban_rs::xdr::{
        Memo, MuxedAccount, Transaction, TransactionEnvelope, TransactionExt,
        TransactionV1Envelope, Uint256, VecM,
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
}
