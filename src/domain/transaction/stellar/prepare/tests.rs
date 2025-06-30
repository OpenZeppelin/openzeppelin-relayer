//! Tests for the prepare module

use super::*;
use crate::{
    domain::SignTransactionResponse,
    models::{
        NetworkTransactionData, RepositoryError, StellarTransactionData, TransactionInput,
        TransactionStatus,
    },
};
use soroban_rs::xdr::{Limits, ReadXdr, TransactionEnvelope, WriteXdr};

use crate::domain::transaction::stellar::prepare::common::{
    send_submit_transaction_job, update_and_notify_transaction,
};
use crate::domain::transaction::stellar::test_helpers::*;

mod prepare_transaction_tests {
    use super::*;

    #[tokio::test]
    async fn prepare_transaction_happy_path() {
        let relayer = create_test_relayer();
        let mut mocks = default_test_mocks();

        // sequence counter
        mocks
            .counter
            .expect_get_and_increment()
            .returning(|_, _| Ok(1));

        // signer
        mocks.signer.expect_sign_transaction().returning(|_| {
            Box::pin(async {
                Ok(SignTransactionResponse::Stellar(
                    crate::domain::SignTransactionResponseStellar {
                        signature: dummy_signature(),
                    },
                ))
            })
        });

        mocks
            .tx_repo
            .expect_partial_update()
            .withf(|_, upd| {
                upd.status == Some(TransactionStatus::Sent) && upd.network_data.is_some()
            })
            .returning(|id, upd| {
                let mut tx = create_test_transaction("relayer-1");
                tx.id = id;
                tx.status = upd.status.unwrap();
                tx.network_data = upd.network_data.unwrap();
                Ok::<_, RepositoryError>(tx)
            });

        // submit-job + notification
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
        let tx = create_test_transaction(&relayer.id);

        assert!(handler.prepare_transaction_impl(tx).await.is_ok());
    }

    #[tokio::test]
    async fn prepare_transaction_stores_signed_envelope_xdr() {
        let relayer = create_test_relayer();
        let mut mocks = default_test_mocks();

        // sequence counter
        mocks
            .counter
            .expect_get_and_increment()
            .returning(|_, _| Ok(1));

        // signer
        mocks.signer.expect_sign_transaction().returning(|_| {
            Box::pin(async {
                Ok(SignTransactionResponse::Stellar(
                    crate::domain::SignTransactionResponseStellar {
                        signature: dummy_signature(),
                    },
                ))
            })
        });

        mocks
            .tx_repo
            .expect_partial_update()
            .withf(|_, upd| {
                upd.status == Some(TransactionStatus::Sent) && upd.network_data.is_some()
            })
            .returning(move |id, upd| {
                let mut tx = create_test_transaction("relayer-1");
                tx.id = id;
                tx.status = upd.status.unwrap();
                tx.network_data = upd.network_data.clone().unwrap();
                Ok::<_, RepositoryError>(tx)
            });

        // submit-job + notification
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
        let tx = create_test_transaction(&relayer.id);

        let result = handler.prepare_transaction_impl(tx).await;
        assert!(result.is_ok());

        // Verify the signed_envelope_xdr was populated
        if let Ok(prepared_tx) = result {
            if let NetworkTransactionData::Stellar(stellar_data) = &prepared_tx.network_data {
                assert!(
                    stellar_data.signed_envelope_xdr.is_some(),
                    "signed_envelope_xdr should be populated"
                );

                // Verify it's valid XDR by attempting to parse it
                let xdr = stellar_data.signed_envelope_xdr.as_ref().unwrap();
                let envelope_result = TransactionEnvelope::from_xdr_base64(xdr, Limits::none());
                assert!(
                    envelope_result.is_ok(),
                    "signed_envelope_xdr should be valid XDR"
                );

                // Verify the envelope has signatures
                if let Ok(envelope) = envelope_result {
                    match envelope {
                        TransactionEnvelope::Tx(ref e) => {
                            assert!(!e.signatures.is_empty(), "Envelope should have signatures");
                        }
                        _ => panic!("Expected Tx envelope type"),
                    }
                }
            } else {
                panic!("Expected Stellar transaction data");
            }
        }
    }

    #[tokio::test]
    async fn prepare_transaction_sequence_failure_cleans_up_lane() {
        let relayer = create_test_relayer();
        let mut mocks = default_test_mocks();

        // Mock sequence counter to fail
        mocks.counter.expect_get_and_increment().returning(|_, _| {
            Err(crate::repositories::TransactionCounterError::NotFound(
                "Counter service failure".to_string(),
            ))
        });

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
        let tx = create_test_transaction(&relayer.id);

        // Verify that lane is claimed initially
        assert!(lane_gate::claim(&relayer.id, &tx.id));

        let result = handler.prepare_transaction_impl(tx.clone()).await;

        // Should return error but lane should be cleaned up
        assert!(result.is_err());

        // Verify lane is released - another transaction should be able to claim it
        let another_tx_id = "another-tx";
        assert!(lane_gate::claim(&relayer.id, another_tx_id));
        lane_gate::free(&relayer.id, another_tx_id)
    }

    #[tokio::test]
    async fn prepare_transaction_signer_failure_cleans_up_lane() {
        let relayer = create_test_relayer();
        let mut mocks = default_test_mocks();

        // sequence counter succeeds
        mocks
            .counter
            .expect_get_and_increment()
            .returning(|_, _| Ok(1));

        // signer fails
        mocks.signer.expect_sign_transaction().returning(|_| {
            Box::pin(async {
                Err(crate::models::SignerError::SigningError(
                    "Signer failure".to_string(),
                ))
            })
        });

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
        let tx = create_test_transaction(&relayer.id);

        let result = handler.prepare_transaction_impl(tx.clone()).await;

        // Should return error but lane should be cleaned up
        assert!(result.is_err());

        // Verify lane is released
        let another_tx_id = "another-tx";
        assert!(lane_gate::claim(&relayer.id, another_tx_id));
        lane_gate::free(&relayer.id, another_tx_id); // cleanup
    }

    #[tokio::test]
    async fn prepare_transaction_already_claimed_lane_returns_original() {
        let mut relayer = create_test_relayer();
        relayer.id = "unique-relayer-for-lane-test".to_string(); // Use unique relayer ID
        let mocks = default_test_mocks();

        let handler = make_stellar_tx_handler(relayer.clone(), mocks);
        let tx = create_test_transaction(&relayer.id);

        // Claim lane with different transaction
        assert!(lane_gate::claim(&relayer.id, "other-tx"));

        let result = handler.prepare_transaction_impl(tx.clone()).await;

        // Should return Ok with original transaction (waiting)
        assert!(result.is_ok());
        let returned_tx = result.unwrap();
        assert_eq!(returned_tx.id, tx.id);
        assert_eq!(returned_tx.status, tx.status);

        // Cleanup
        lane_gate::free(&relayer.id, "other-tx");
    }
}

mod xdr_transaction_tests {
    use super::*;
    use crate::constants::STELLAR_DEFAULT_TRANSACTION_FEE;
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
            .returning(move |_, _| Ok(expected_sequence as u64));

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

        // Mock counter service (will be called before validation fails)
        mocks
            .counter
            .expect_get_and_increment()
            .returning(|_, _| Ok(1));

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
    async fn test_signed_xdr_without_fee_bump() {
        let relayer = create_test_relayer();
        let mut mocks = default_test_mocks();

        let mut tx = create_test_transaction(&relayer.id);
        let mut stellar_data = tx
            .network_data
            .get_stellar_transaction_data()
            .unwrap()
            .clone();

        // Create signed XDR (has signatures)
        let different_account = "GBCFR5QVA3K7JKIPT7WFULRXQVNTDZQLZHTUTGONFSTS5KCEGS6O5AZB";
        let mut envelope = create_unsigned_xdr_envelope(different_account);
        if let TransactionEnvelope::Tx(ref mut e) = envelope {
            e.signatures = vec![soroban_rs::xdr::DecoratedSignature {
                hint: soroban_rs::xdr::SignatureHint([0; 4]),
                signature: soroban_rs::xdr::Signature(vec![0; 64].try_into().unwrap()),
            }]
            .try_into()
            .unwrap();
        }
        let xdr = envelope.to_xdr_base64(Limits::none()).unwrap();
        // Since SignedXdr always implies fee_bump now, we can't test the false case
        // Instead, let's verify that SignedXdr works correctly
        stellar_data.transaction_input = TransactionInput::SignedXdr {
            xdr: xdr.clone(),
            max_fee: 1_000_000,
        };

        // Update the transaction with the modified stellar data
        tx.network_data = NetworkTransactionData::Stellar(stellar_data);

        // This test now verifies that signed XDR from a different source gets fee-bumped
        // For this test to work, we need to mock the signer
        mocks.signer.expect_sign_transaction().returning(|_| {
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

        let result = handler.prepare_transaction_impl(tx).await;

        // Should succeed since SignedXdr always does fee-bump
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_signed_xdr_with_fee_bump() {
        let relayer = create_test_relayer();
        let mut mocks = default_test_mocks();

        // Mock signer for fee-bump transaction
        mocks.signer.expect_sign_transaction().returning(|_| {
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

        // Create signed XDR
        let different_account = "GBCFR5QVA3K7JKIPT7WFULRXQVNTDZQLZHTUTGONFSTS5KCEGS6O5AZB";
        let mut envelope = create_unsigned_xdr_envelope(different_account);
        if let TransactionEnvelope::Tx(ref mut e) = envelope {
            e.signatures = vec![soroban_rs::xdr::DecoratedSignature {
                hint: soroban_rs::xdr::SignatureHint([0; 4]),
                signature: soroban_rs::xdr::Signature(vec![0; 64].try_into().unwrap()),
            }]
            .try_into()
            .unwrap();
        }
        let xdr = envelope.to_xdr_base64(Limits::none()).unwrap();
        stellar_data.transaction_input = TransactionInput::SignedXdr {
            xdr: xdr.clone(),
            max_fee: 2_000_000, // 0.2 XLM
        };

        // Update the transaction with the modified stellar data
        tx.network_data = NetworkTransactionData::Stellar(stellar_data);

        let result = handler.prepare_transaction_impl(tx).await;
        assert!(result.is_ok());

        let updated_tx = result.unwrap();
        if let NetworkTransactionData::Stellar(data) = &updated_tx.network_data {
            // Verify it's a SignedXdr transaction (which always implies fee-bump)
            assert!(matches!(
                data.transaction_input,
                TransactionInput::SignedXdr { .. }
            ));
            // Verify the signed_envelope_xdr was populated
            assert!(
                data.signed_envelope_xdr.is_some(),
                "signed_envelope_xdr should be populated for fee-bump transactions"
            );

            // Verify it's valid XDR by attempting to parse it
            let envelope_xdr = data.signed_envelope_xdr.as_ref().unwrap();
            let envelope_result =
                TransactionEnvelope::from_xdr_base64(envelope_xdr, Limits::none());
            assert!(
                envelope_result.is_ok(),
                "signed_envelope_xdr should be valid XDR"
            );

            // Verify it's a fee-bump envelope
            if let Ok(envelope) = envelope_result {
                assert!(
                    matches!(envelope, TransactionEnvelope::TxFeeBump(_)),
                    "Should be a fee-bump envelope"
                );
            }
        } else {
            panic!("Expected Stellar transaction data");
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
            .returning(move |_, _| Ok(expected_sequence as u64));

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

mod send_submit_transaction_job_tests {
    use super::*;

    #[tokio::test]
    async fn send_submit_transaction_job_success() {
        let relayer = create_test_relayer();
        let mut mocks = default_test_mocks();

        // Mock successful job production
        mocks
            .job_producer
            .expect_produce_submit_transaction_job()
            .withf(|job, delay| {
                job.transaction_id == "tx-1" && job.relayer_id == "relayer-1" && delay.is_none()
            })
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let handler = make_stellar_tx_handler(relayer.clone(), mocks);
        let tx = create_test_transaction(&relayer.id);

        let result = send_submit_transaction_job(handler.job_producer(), &tx, None).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn send_submit_transaction_job_with_delay() {
        let relayer = create_test_relayer();
        let mut mocks = default_test_mocks();

        // Mock successful job production with delay
        mocks
            .job_producer
            .expect_produce_submit_transaction_job()
            .withf(|job, delay| {
                job.transaction_id == "tx-1" && job.relayer_id == "relayer-1" && delay == &Some(30)
            })
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let handler = make_stellar_tx_handler(relayer.clone(), mocks);
        let tx = create_test_transaction(&relayer.id);

        let result = send_submit_transaction_job(handler.job_producer(), &tx, Some(30)).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn send_submit_transaction_job_handles_producer_error() {
        let relayer = create_test_relayer();
        let mut mocks = default_test_mocks();

        // Mock job producer failure
        mocks
            .job_producer
            .expect_produce_submit_transaction_job()
            .times(1)
            .returning(|_, _| {
                Box::pin(async {
                    Err(crate::jobs::JobProducerError::QueueError(
                        "Job queue is full".to_string(),
                    ))
                })
            });

        let handler = make_stellar_tx_handler(relayer.clone(), mocks);
        let tx = create_test_transaction(&relayer.id);

        let result = send_submit_transaction_job(handler.job_producer(), &tx, None).await;
        assert!(result.is_err());
    }
}

mod refactoring_tests {
    use super::*;

    #[tokio::test]
    async fn test_update_and_notify_transaction_consistency() {
        let relayer = create_test_relayer();
        let mut mocks = default_test_mocks();

        // Mock the repository update
        let expected_stellar_data = StellarTransactionData {
            source_account: TEST_PK.to_string(),
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            fee: Some(100),
            sequence_number: Some(1),
            transaction_input: TransactionInput::Operations(vec![]),
            memo: None,
            valid_until: None,
            signatures: vec![],
            hash: None,
            simulation_transaction_data: None,
            signed_envelope_xdr: Some("test-xdr".to_string()),
        };

        let expected_xdr = expected_stellar_data.signed_envelope_xdr.clone();
        mocks
            .tx_repo
            .expect_partial_update()
            .withf(move |id, upd| {
                id == "tx-1"
                    && upd.status == Some(TransactionStatus::Sent)
                    && if let Some(NetworkTransactionData::Stellar(ref data)) = upd.network_data {
                        data.signed_envelope_xdr == expected_xdr
                    } else {
                        false
                    }
            })
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

        // Test update_and_notify_transaction directly
        let result = update_and_notify_transaction(
            handler.transaction_repository(),
            handler.job_producer(),
            "tx-1".to_string(),
            expected_stellar_data,
            handler.relayer().notification_id.as_deref(),
        )
        .await;

        assert!(result.is_ok());
        let updated_tx = result.unwrap();
        assert_eq!(updated_tx.status, TransactionStatus::Sent);

        if let NetworkTransactionData::Stellar(data) = &updated_tx.network_data {
            assert_eq!(data.signed_envelope_xdr, Some("test-xdr".to_string()));
        } else {
            panic!("Expected Stellar transaction data");
        }
    }
}
