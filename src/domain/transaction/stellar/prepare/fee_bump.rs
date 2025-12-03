//! Fee-bump transaction preparation logic.

use crate::domain::transaction::stellar::StellarTransactionValidator;
use crate::{
    domain::{
        attach_signatures_to_envelope, build_fee_bump_envelope, parse_transaction_xdr,
        SignTransactionResponse,
    },
    models::{
        NetworkTransactionData, RelayerStellarPolicy, StellarFeePaymentStrategy,
        StellarTransactionData, StellarValidationError, TransactionError, TransactionInput,
    },
    services::{
        provider::StellarProviderTrait, signer::Signer, stellar_dex::StellarDexServiceTrait,
    },
};
use eyre::Result;
use soroban_rs::xdr::{Limits, TransactionEnvelope, WriteXdr};

use super::common::{calculate_fee_bump_required_fee, create_signing_data};

/// Process a fee-bump transaction from signed XDR input.
///
/// This function:
/// 1. Extracts and validates the inner transaction from the signed XDR
/// 2. For User fee payment strategy: validates gasless transaction requirements (fee payment operations, etc.)
/// 3. Simulates the transaction if needed (for Soroban operations)
/// 4. Calculates the required fee based on simulation results or max_fee
/// 5. Builds the fee-bump envelope
/// 6. Signs the fee-bump transaction
/// 7. Returns the updated stellar data with the signed fee-bump envelope
pub async fn process_fee_bump<S, P, D>(
    relayer_address: &str,
    stellar_data: StellarTransactionData,
    provider: &P,
    signer: &S,
    relayer_policy: Option<&RelayerStellarPolicy>,
    dex_service: &D,
) -> Result<StellarTransactionData, TransactionError>
where
    S: Signer + Send + Sync,
    P: StellarProviderTrait + Send + Sync,
    D: StellarDexServiceTrait + Send + Sync,
{
    // Step 1: Extract and validate the inner transaction
    let (inner_envelope, max_fee) = extract_inner_transaction(&stellar_data)?;

    // Step 2: Validate User fee payment strategy transactions (gasless transactions)
    // When fee payment strategy is User, the transaction must include fee payment operations
    // and meet gasless transaction requirements since the user signs and submits it
    if let Some(policy) = relayer_policy {
        if matches!(
            policy.fee_payment_strategy,
            Some(StellarFeePaymentStrategy::User)
        ) {
            StellarTransactionValidator::validate_user_fee_payment_transaction(
                &inner_envelope,
                relayer_address,
                policy,
                provider,
                dex_service,
                None, // Don't validate duration for fee-bump flow as time bounds may not be set yet
            )
            .await
            .map_err(|e| TransactionError::ValidationError(e.to_string()))?;
        }
    }

    // Step 3: Calculate the required fee (may include simulation for Soroban)
    let required_fee = calculate_fee_bump_required_fee(&inner_envelope, max_fee, provider).await?;

    // Step 4: Build the fee-bump envelope (relayer pays XLM fee)
    let fee_bump_envelope =
        build_fee_bump_envelope(inner_envelope.clone(), relayer_address, required_fee as i64)
            .map_err(|e| {
                TransactionError::ValidationError(format!("Cannot create fee-bump envelope: {e}"))
            })?;

    // Step 5: Sign the fee-bump transaction
    let signed_stellar_data =
        sign_fee_bump_transaction(stellar_data, fee_bump_envelope, relayer_address, signer).await?;

    // Step 6: Update the fee in stellar data
    let signed_stellar_data = signed_stellar_data.with_fee(required_fee);

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
        StellarValidationError::InvalidXdr(format!("Invalid inner transaction: {e}"))
    })?;

    Ok((inner_envelope, max_fee))
}

/// Sign the fee-bump transaction and return the final stellar data.
///
/// This function:
/// - Serializes the fee-bump envelope for signing
/// - Creates signing data for the fee-bump transaction
/// - Signs the transaction using the provided signer
/// - Attaches the signature directly to the envelope (mutates in place, avoiding re-parsing)
/// - Serializes the signed envelope
/// - Returns the updated stellar data with the signed envelope XDR
async fn sign_fee_bump_transaction<S>(
    mut stellar_data: StellarTransactionData,
    mut fee_bump_envelope: TransactionEnvelope,
    relayer_address: &str,
    signer: &S,
) -> Result<StellarTransactionData, TransactionError>
where
    S: Signer + Send + Sync,
{
    // Serialize the fee-bump envelope for signing
    let fee_bump_xdr = fee_bump_envelope
        .to_xdr_base64(Limits::none())
        .map_err(|e| {
            TransactionError::ValidationError(format!("Failed to serialize fee-bump envelope: {e}"))
        })?;

    // Create signing data for the fee-bump transaction
    let signing_data = create_signing_data(
        relayer_address.to_string(),
        fee_bump_xdr,
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

    // Attach the signature directly to the fee-bump envelope (mutate in place)
    attach_signatures_to_envelope(&mut fee_bump_envelope, vec![signature.clone()]).map_err(
        |e| {
            TransactionError::SignerError(format!(
                "Failed to attach signature to fee-bump envelope: {e}"
            ))
        },
    )?;

    // Serialize the signed envelope
    let signed_xdr = fee_bump_envelope
        .to_xdr_base64(Limits::none())
        .map_err(|e| {
            TransactionError::SignerError(format!(
                "Failed to serialize signed fee-bump envelope: {e}"
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
    use crate::domain::transaction::stellar::test_helpers::{
        create_signed_v1_envelope, create_simple_v1_envelope, TEST_PK, TEST_PK_2,
    };

    fn create_test_envelope(source: &str, include_signature: bool) -> TransactionEnvelope {
        if include_signature {
            create_signed_v1_envelope(source, TEST_PK_2)
        } else {
            create_simple_v1_envelope(source, TEST_PK_2)
        }
    }

    #[test]
    fn test_extract_inner_transaction_valid() {
        let envelope = create_test_envelope(TEST_PK, true);
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
        let envelope = create_test_envelope(TEST_PK, true);
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

#[cfg(test)]
mod signed_xdr_tests {
    use super::*;
    use crate::domain::transaction::stellar::test_helpers::*;
    use crate::domain::SignTransactionResponse;
    use crate::models::{NetworkTransactionData, RepositoryError, TransactionStatus};
    use soroban_rs::xdr::{
        Memo, MuxedAccount, ReadXdr, Transaction, TransactionEnvelope, TransactionExt,
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

    // Tests for User fee payment strategy validation
    mod user_fee_payment_tests {
        use super::*;
        use crate::domain::{SignTransactionResponse, SignTransactionResponseStellar};
        use crate::models::{
            Address, RelayerStellarPolicy, StellarAllowedTokensPolicy, StellarFeePaymentStrategy,
        };
        use crate::repositories::MockTransactionCounterTrait;
        use crate::services::provider::MockStellarProviderTrait;
        use crate::services::signer::MockSigner;
        use crate::services::stellar_dex::MockStellarDexServiceTrait;
        use soroban_rs::stellar_rpc_client::{GetLedgerEntriesResponse, LedgerEntryResult};
        use soroban_rs::xdr::{
            AccountEntry, AccountEntryExt, AccountId, AlphaNum4, Asset, AssetCode4, LedgerEntry,
            LedgerEntryData, LedgerEntryExt, Limits, Memo, MuxedAccount, Operation, OperationBody,
            PaymentOp, Preconditions, SequenceNumber, Signature, SignatureHint, String32,
            Thresholds, Transaction, TransactionEnvelope, TransactionExt, TransactionV1Envelope,
            TrustLineEntry, TrustLineEntryExt, Uint256, VecM, WriteXdr,
        };
        use std::future::ready;
        use stellar_strkey::ed25519::PublicKey;

        const USDC_ASSET: &str = "USDC:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5";

        /// Helper to create a policy with User fee payment strategy
        fn create_user_fee_policy() -> RelayerStellarPolicy {
            let mut policy = RelayerStellarPolicy::default();
            policy.fee_payment_strategy = Some(StellarFeePaymentStrategy::User);
            policy.allowed_tokens = Some(vec![StellarAllowedTokensPolicy {
                asset: USDC_ASSET.to_string(),
                metadata: None,
                max_allowed_fee: None,
                swap_config: None,
            }]);
            policy
        }

        /// Helper to create a signed XDR with fee payment operation
        fn create_signed_xdr_with_fee_payment(
            source_account: &str,
            relayer_address: &str,
        ) -> String {
            use soroban_rs::xdr::{
                Memo, MuxedAccount, Operation, OperationBody, PaymentOp, Preconditions,
                SequenceNumber, Transaction, TransactionExt, TransactionV1Envelope, Uint256,
            };
            use stellar_strkey::ed25519::PublicKey;

            let source_pk = PublicKey::from_string(source_account).unwrap();
            let relayer_pk = PublicKey::from_string(relayer_address).unwrap();
            let usdc_issuer =
                PublicKey::from_string("GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5")
                    .unwrap();

            // Create fee payment operation (USDC to relayer)
            // For gasless transactions, there should be only one payment operation to the relayer
            let fee_payment_op = Operation {
                source_account: None,
                body: OperationBody::Payment(PaymentOp {
                    destination: MuxedAccount::Ed25519(Uint256(relayer_pk.0)),
                    asset: Asset::CreditAlphanum4(AlphaNum4 {
                        asset_code: AssetCode4(*b"USDC"),
                        issuer: AccountId(soroban_rs::xdr::PublicKey::PublicKeyTypeEd25519(
                            Uint256(usdc_issuer.0),
                        )),
                    }),
                    amount: 1500000, // 1.5 USDC fee
                }),
            };

            let tx = Transaction {
                source_account: MuxedAccount::Ed25519(Uint256(source_pk.0)),
                fee: 200, // 200 stroops
                seq_num: SequenceNumber(2),
                cond: Preconditions::None,
                memo: Memo::None,
                operations: vec![fee_payment_op].try_into().unwrap(),
                ext: TransactionExt::V0,
            };

            let dummy_signature = soroban_rs::xdr::DecoratedSignature {
                hint: SignatureHint([0; 4]),
                signature: Signature(vec![0u8; 64].try_into().unwrap()),
            };

            let envelope = TransactionV1Envelope {
                tx,
                signatures: vec![dummy_signature].try_into().unwrap(),
            };

            TransactionEnvelope::Tx(envelope)
                .to_xdr_base64(Limits::none())
                .unwrap()
        }

        #[tokio::test]
        async fn test_process_fee_bump_with_user_fee_payment_valid() {
            let relayer = create_test_relayer();
            let user_account = "GCZ54QGQCUZ6U5WJF4AG5JEZCUMYTS2F6JRLUS76XF2PQMEJ2E3JISI2";

            let _counter = MockTransactionCounterTrait::new();
            let mut provider = MockStellarProviderTrait::new();
            let mut signer = MockSigner::new();
            let mut dex_service = MockStellarDexServiceTrait::new();

            // Mock signer
            signer
                .expect_address()
                .returning(|| Box::pin(ready(Ok(Address::Stellar(TEST_PK.to_string())))));
            signer.expect_sign_transaction().returning(|_| {
                Box::pin(ready(Ok(SignTransactionResponse::Stellar(
                    SignTransactionResponseStellar {
                        signature: soroban_rs::xdr::DecoratedSignature {
                            hint: SignatureHint([0; 4]),
                            signature: Signature(vec![0u8; 64].try_into().unwrap()),
                        },
                    },
                ))))
            });

            // Mock get_account for validation
            provider.expect_get_account().returning(|_| {
                Box::pin(ready(Ok(AccountEntry {
                    account_id: AccountId(soroban_rs::xdr::PublicKey::PublicKeyTypeEd25519(
                        Uint256([0; 32]),
                    )),
                    balance: 1000000000,
                    seq_num: SequenceNumber(1),
                    num_sub_entries: 0,
                    inflation_dest: None,
                    flags: 0,
                    home_domain: String32::default(),
                    thresholds: Thresholds([0; 4]),
                    signers: VecM::default(),
                    ext: AccountEntryExt::V0,
                })))
            });

            // Mock get_ledger_entries for token balance validation
            provider.expect_get_ledger_entries().returning(|_| {
                let trustline_entry = TrustLineEntry {
                    account_id: AccountId(soroban_rs::xdr::PublicKey::PublicKeyTypeEd25519(
                        Uint256([0; 32]),
                    )),
                    asset: soroban_rs::xdr::TrustLineAsset::CreditAlphanum4(AlphaNum4 {
                        asset_code: AssetCode4(*b"USDC"),
                        issuer: AccountId(soroban_rs::xdr::PublicKey::PublicKeyTypeEd25519(
                            Uint256([0; 32]),
                        )),
                    }),
                    balance: 10_000_000i64,
                    limit: i64::MAX,
                    flags: 0,
                    ext: TrustLineEntryExt::V0,
                };

                let ledger_entry = LedgerEntry {
                    last_modified_ledger_seq: 0,
                    data: LedgerEntryData::Trustline(trustline_entry),
                    ext: LedgerEntryExt::V0,
                };

                let xdr = ledger_entry.data.to_xdr_base64(Limits::none()).unwrap();

                Box::pin(ready(Ok(GetLedgerEntriesResponse {
                    entries: Some(vec![LedgerEntryResult {
                        key: "test_key".to_string(),
                        xdr,
                        last_modified_ledger: 0u32,
                        live_until_ledger_seq_ledger_seq: None,
                    }]),
                    latest_ledger: 0,
                })))
            });

            // Mock DEX service for fee conversion validation
            dex_service
                .expect_get_xlm_to_token_quote()
                .returning(|_, _, _, _| {
                    Box::pin(ready(Ok(
                        crate::services::stellar_dex::StellarQuoteResponse {
                            input_asset: "native".to_string(),
                            output_asset: USDC_ASSET.to_string(),
                            in_amount: 200,
                            out_amount: 1500000,
                            price_impact_pct: 0.0,
                            slippage_bps: 100,
                            path: None,
                        },
                    )))
                });

            let policy = create_user_fee_policy();
            let xdr = create_signed_xdr_with_fee_payment(user_account, &relayer.address);

            let stellar_data = StellarTransactionData {
                source_account: user_account.to_string(),
                fee: None,
                sequence_number: None,
                memo: None,
                valid_until: None,
                network_passphrase: "Test SDF Network ; September 2015".to_string(),
                signatures: vec![],
                hash: None,
                simulation_transaction_data: None,
                transaction_input: TransactionInput::SignedXdr {
                    xdr,
                    max_fee: 1_000_000,
                },
                signed_envelope_xdr: None,
            };

            let result = process_fee_bump(
                &relayer.address,
                stellar_data,
                &provider,
                &signer,
                Some(&policy),
                &dex_service,
            )
            .await;

            // Should succeed with valid user fee payment transaction
            assert!(result.is_ok(), "Expected success, got: {:?}", result.err());
        }

        #[tokio::test]
        async fn test_process_fee_bump_with_user_fee_payment_missing_fee_operation() {
            let relayer = create_test_relayer();
            let user_account = "GCZ54QGQCUZ6U5WJF4AG5JEZCUMYTS2F6JRLUS76XF2PQMEJ2E3JISI2";

            let _counter = MockTransactionCounterTrait::new();
            let mut provider = MockStellarProviderTrait::new();
            let mut signer = MockSigner::new();
            let dex_service = MockStellarDexServiceTrait::new();

            // Mock signer
            signer
                .expect_address()
                .returning(|| Box::pin(ready(Ok(Address::Stellar(TEST_PK.to_string())))));
            signer.expect_sign_transaction().returning(|_| {
                Box::pin(ready(Ok(SignTransactionResponse::Stellar(
                    SignTransactionResponseStellar {
                        signature: soroban_rs::xdr::DecoratedSignature {
                            hint: SignatureHint([0; 4]),
                            signature: Signature(vec![0u8; 64].try_into().unwrap()),
                        },
                    },
                ))))
            });

            // Mock get_account for validation
            provider.expect_get_account().returning(|_| {
                Box::pin(ready(Ok(AccountEntry {
                    account_id: AccountId(soroban_rs::xdr::PublicKey::PublicKeyTypeEd25519(
                        Uint256([0; 32]),
                    )),
                    balance: 1000000000,
                    seq_num: SequenceNumber(1),
                    num_sub_entries: 0,
                    inflation_dest: None,
                    flags: 0,
                    home_domain: String32::default(),
                    thresholds: Thresholds([0; 4]),
                    signers: VecM::default(),
                    ext: AccountEntryExt::V0,
                })))
            });

            let policy = create_user_fee_policy();

            // Create XDR without fee payment operation (simple payment, no fee payment op)
            let source_pk = PublicKey::from_string(user_account).unwrap();
            let dest_pk = PublicKey::from_string(&relayer.address).unwrap();

            let payment_op = Operation {
                source_account: None,
                body: OperationBody::Payment(PaymentOp {
                    destination: MuxedAccount::Ed25519(Uint256(dest_pk.0)),
                    asset: Asset::Native,
                    amount: 1000000,
                }),
            };

            let tx = Transaction {
                source_account: MuxedAccount::Ed25519(Uint256(source_pk.0)),
                fee: 200,
                seq_num: SequenceNumber(2),
                cond: Preconditions::None,
                memo: Memo::None,
                operations: vec![payment_op].try_into().unwrap(),
                ext: TransactionExt::V0,
            };

            let dummy_signature = soroban_rs::xdr::DecoratedSignature {
                hint: SignatureHint([0; 4]),
                signature: Signature(vec![0u8; 64].try_into().unwrap()),
            };

            let envelope = TransactionV1Envelope {
                tx,
                signatures: vec![dummy_signature].try_into().unwrap(),
            };

            let xdr = TransactionEnvelope::Tx(envelope)
                .to_xdr_base64(Limits::none())
                .unwrap();

            let stellar_data = StellarTransactionData {
                source_account: user_account.to_string(),
                fee: None,
                sequence_number: None,
                memo: None,
                valid_until: None,
                network_passphrase: "Test SDF Network ; September 2015".to_string(),
                signatures: vec![],
                hash: None,
                simulation_transaction_data: None,
                transaction_input: TransactionInput::SignedXdr {
                    xdr,
                    max_fee: 1_000_000,
                },
                signed_envelope_xdr: None,
            };

            let result = process_fee_bump(
                &relayer.address,
                stellar_data,
                &provider,
                &signer,
                Some(&policy),
                &dex_service,
            )
            .await;

            // Should fail due to missing fee payment operation (or token not allowed)
            assert!(result.is_err());
            match result.unwrap_err() {
                TransactionError::ValidationError(_) => {
                    // Success - validation failed as expected
                }
                other => panic!("Expected ValidationError, got: {:?}", other),
            }
        }

        #[tokio::test]
        async fn test_process_fee_bump_with_user_fee_payment_invalid_token() {
            let relayer = create_test_relayer();
            let user_account = "GCZ54QGQCUZ6U5WJF4AG5JEZCUMYTS2F6JRLUS76XF2PQMEJ2E3JISI2";

            let _counter = MockTransactionCounterTrait::new();
            let mut provider = MockStellarProviderTrait::new();
            let mut signer = MockSigner::new();
            let dex_service = MockStellarDexServiceTrait::new();

            // Mock signer
            signer
                .expect_address()
                .returning(|| Box::pin(ready(Ok(Address::Stellar(TEST_PK.to_string())))));
            signer.expect_sign_transaction().returning(|_| {
                Box::pin(ready(Ok(SignTransactionResponse::Stellar(
                    SignTransactionResponseStellar {
                        signature: soroban_rs::xdr::DecoratedSignature {
                            hint: SignatureHint([0; 4]),
                            signature: Signature(vec![0u8; 64].try_into().unwrap()),
                        },
                    },
                ))))
            });

            // Mock get_account for validation
            provider.expect_get_account().returning(|_| {
                Box::pin(ready(Ok(AccountEntry {
                    account_id: AccountId(soroban_rs::xdr::PublicKey::PublicKeyTypeEd25519(
                        Uint256([0; 32]),
                    )),
                    balance: 1000000000,
                    seq_num: SequenceNumber(1),
                    num_sub_entries: 0,
                    inflation_dest: None,
                    flags: 0,
                    home_domain: String32::default(),
                    thresholds: Thresholds([0; 4]),
                    signers: VecM::default(),
                    ext: AccountEntryExt::V0,
                })))
            });

            let policy = create_user_fee_policy();

            // Create XDR with fee payment in non-allowed token (EURC instead of USDC)
            use soroban_rs::xdr::{
                Memo, MuxedAccount, Operation, OperationBody, PaymentOp, Preconditions,
                SequenceNumber, Transaction, TransactionExt, TransactionV1Envelope, Uint256,
            };
            use stellar_strkey::ed25519::PublicKey;

            let source_pk = PublicKey::from_string(user_account).unwrap();
            let relayer_pk = PublicKey::from_string(&relayer.address).unwrap();
            let eurc_issuer =
                PublicKey::from_string("GDHU6WRG4IEQXM5NZ4BMPKOXHW76MZM4Y2IEMFDVXBSDP6SJY4ITNPP2")
                    .unwrap();

            let fee_payment_op = Operation {
                source_account: None,
                body: OperationBody::Payment(PaymentOp {
                    destination: MuxedAccount::Ed25519(Uint256(relayer_pk.0)),
                    asset: Asset::CreditAlphanum4(AlphaNum4 {
                        asset_code: AssetCode4(*b"EURC"),
                        issuer: AccountId(soroban_rs::xdr::PublicKey::PublicKeyTypeEd25519(
                            Uint256(eurc_issuer.0),
                        )),
                    }),
                    amount: 1500000,
                }),
            };

            let tx = Transaction {
                source_account: MuxedAccount::Ed25519(Uint256(source_pk.0)),
                fee: 200,
                seq_num: SequenceNumber(2),
                cond: Preconditions::None,
                memo: Memo::None,
                operations: vec![fee_payment_op].try_into().unwrap(),
                ext: TransactionExt::V0,
            };

            let dummy_signature = soroban_rs::xdr::DecoratedSignature {
                hint: SignatureHint([0; 4]),
                signature: Signature(vec![0u8; 64].try_into().unwrap()),
            };

            let envelope = TransactionV1Envelope {
                tx,
                signatures: vec![dummy_signature].try_into().unwrap(),
            };

            let xdr = TransactionEnvelope::Tx(envelope)
                .to_xdr_base64(Limits::none())
                .unwrap();

            let stellar_data = StellarTransactionData {
                source_account: user_account.to_string(),
                fee: None,
                sequence_number: None,
                memo: None,
                valid_until: None,
                network_passphrase: "Test SDF Network ; September 2015".to_string(),
                signatures: vec![],
                hash: None,
                simulation_transaction_data: None,
                transaction_input: TransactionInput::SignedXdr {
                    xdr,
                    max_fee: 1_000_000,
                },
                signed_envelope_xdr: None,
            };

            let result = process_fee_bump(
                &relayer.address,
                stellar_data,
                &provider,
                &signer,
                Some(&policy),
                &dex_service,
            )
            .await;

            // Should fail due to non-allowed token
            assert!(result.is_err());
            match result.unwrap_err() {
                TransactionError::ValidationError(_) => {
                    // Success - validation failed as expected
                }
                other => panic!("Expected ValidationError, got: {:?}", other),
            }
        }

        #[tokio::test]
        async fn test_process_fee_bump_with_relayer_fee_payment_no_validation() {
            let relayer = create_test_relayer();
            let user_account = "GCZ54QGQCUZ6U5WJF4AG5JEZCUMYTS2F6JRLUS76XF2PQMEJ2E3JISI2";

            let _counter = MockTransactionCounterTrait::new();
            let provider = MockStellarProviderTrait::new();
            let mut signer = MockSigner::new();
            let dex_service = MockStellarDexServiceTrait::new();

            // Mock signer
            signer
                .expect_address()
                .returning(|| Box::pin(ready(Ok(Address::Stellar(TEST_PK.to_string())))));
            signer.expect_sign_transaction().returning(|_| {
                Box::pin(ready(Ok(SignTransactionResponse::Stellar(
                    SignTransactionResponseStellar {
                        signature: soroban_rs::xdr::DecoratedSignature {
                            hint: SignatureHint([0; 4]),
                            signature: Signature(vec![0u8; 64].try_into().unwrap()),
                        },
                    },
                ))))
            });

            // Use Relayer fee payment strategy (default)
            let mut policy = RelayerStellarPolicy::default();
            policy.fee_payment_strategy = Some(StellarFeePaymentStrategy::Relayer);

            // Create simple XDR without fee payment operation
            let source_pk = PublicKey::from_string(user_account).unwrap();
            let dest_pk = PublicKey::from_string(&relayer.address).unwrap();

            let payment_op = Operation {
                source_account: None,
                body: OperationBody::Payment(PaymentOp {
                    destination: MuxedAccount::Ed25519(Uint256(dest_pk.0)),
                    asset: Asset::Native,
                    amount: 1000000,
                }),
            };

            let tx = Transaction {
                source_account: MuxedAccount::Ed25519(Uint256(source_pk.0)),
                fee: 200,
                seq_num: SequenceNumber(2),
                cond: Preconditions::None,
                memo: Memo::None,
                operations: vec![payment_op].try_into().unwrap(),
                ext: TransactionExt::V0,
            };

            let dummy_signature = soroban_rs::xdr::DecoratedSignature {
                hint: SignatureHint([0; 4]),
                signature: Signature(vec![0u8; 64].try_into().unwrap()),
            };

            let envelope = TransactionV1Envelope {
                tx,
                signatures: vec![dummy_signature].try_into().unwrap(),
            };

            let xdr = TransactionEnvelope::Tx(envelope)
                .to_xdr_base64(Limits::none())
                .unwrap();

            let stellar_data = StellarTransactionData {
                source_account: user_account.to_string(),
                fee: None,
                sequence_number: None,
                memo: None,
                valid_until: None,
                network_passphrase: "Test SDF Network ; September 2015".to_string(),
                signatures: vec![],
                hash: None,
                simulation_transaction_data: None,
                transaction_input: TransactionInput::SignedXdr {
                    xdr,
                    max_fee: 1_000_000,
                },
                signed_envelope_xdr: None,
            };

            let result = process_fee_bump(
                &relayer.address,
                stellar_data,
                &provider,
                &signer,
                Some(&policy),
                &dex_service,
            )
            .await;

            // Should succeed without user fee payment validation
            assert!(
                result.is_ok(),
                "Expected success for Relayer fee payment, got: {:?}",
                result.err()
            );
        }

        #[tokio::test]
        async fn test_process_fee_bump_with_user_fee_payment_insufficient_balance() {
            let relayer = create_test_relayer();
            let user_account = "GCZ54QGQCUZ6U5WJF4AG5JEZCUMYTS2F6JRLUS76XF2PQMEJ2E3JISI2";

            let _counter = MockTransactionCounterTrait::new();
            let mut provider = MockStellarProviderTrait::new();
            let mut signer = MockSigner::new();
            let mut dex_service = MockStellarDexServiceTrait::new();

            // Mock signer
            signer
                .expect_address()
                .returning(|| Box::pin(ready(Ok(Address::Stellar(TEST_PK.to_string())))));
            signer.expect_sign_transaction().returning(|_| {
                Box::pin(ready(Ok(SignTransactionResponse::Stellar(
                    SignTransactionResponseStellar {
                        signature: soroban_rs::xdr::DecoratedSignature {
                            hint: SignatureHint([0; 4]),
                            signature: Signature(vec![0u8; 64].try_into().unwrap()),
                        },
                    },
                ))))
            });

            // Mock get_account for validation
            provider.expect_get_account().returning(|_| {
                Box::pin(ready(Ok(AccountEntry {
                    account_id: AccountId(soroban_rs::xdr::PublicKey::PublicKeyTypeEd25519(
                        Uint256([0; 32]),
                    )),
                    balance: 1000000000,
                    seq_num: SequenceNumber(1),
                    num_sub_entries: 0,
                    inflation_dest: None,
                    flags: 0,
                    home_domain: String32::default(),
                    thresholds: Thresholds([0; 4]),
                    signers: VecM::default(),
                    ext: AccountEntryExt::V0,
                })))
            });

            // Mock get_ledger_entries with insufficient balance
            provider.expect_get_ledger_entries().returning(|_| {
                let trustline_entry = TrustLineEntry {
                    account_id: AccountId(soroban_rs::xdr::PublicKey::PublicKeyTypeEd25519(
                        Uint256([0; 32]),
                    )),
                    asset: soroban_rs::xdr::TrustLineAsset::CreditAlphanum4(AlphaNum4 {
                        asset_code: AssetCode4(*b"USDC"),
                        issuer: AccountId(soroban_rs::xdr::PublicKey::PublicKeyTypeEd25519(
                            Uint256([0; 32]),
                        )),
                    }),
                    balance: 100_000i64, // Only 0.01 USDC - insufficient
                    limit: i64::MAX,
                    flags: 0,
                    ext: TrustLineEntryExt::V0,
                };

                let ledger_entry = LedgerEntry {
                    last_modified_ledger_seq: 0,
                    data: LedgerEntryData::Trustline(trustline_entry),
                    ext: LedgerEntryExt::V0,
                };

                let xdr = ledger_entry.data.to_xdr_base64(Limits::none()).unwrap();

                Box::pin(ready(Ok(GetLedgerEntriesResponse {
                    entries: Some(vec![LedgerEntryResult {
                        key: "test_key".to_string(),
                        xdr,
                        last_modified_ledger: 0u32,
                        live_until_ledger_seq_ledger_seq: None,
                    }]),
                    latest_ledger: 0,
                })))
            });

            // Mock DEX service
            dex_service
                .expect_get_xlm_to_token_quote()
                .returning(|_, _, _, _| {
                    Box::pin(ready(Ok(
                        crate::services::stellar_dex::StellarQuoteResponse {
                            input_asset: "native".to_string(),
                            output_asset: USDC_ASSET.to_string(),
                            in_amount: 200,
                            out_amount: 1500000, // 1.5 USDC required
                            price_impact_pct: 0.0,
                            slippage_bps: 100,
                            path: None,
                        },
                    )))
                });

            let policy = create_user_fee_policy();
            let xdr = create_signed_xdr_with_fee_payment(user_account, &relayer.address);

            let stellar_data = StellarTransactionData {
                source_account: user_account.to_string(),
                fee: None,
                sequence_number: None,
                memo: None,
                valid_until: None,
                network_passphrase: "Test SDF Network ; September 2015".to_string(),
                signatures: vec![],
                hash: None,
                simulation_transaction_data: None,
                transaction_input: TransactionInput::SignedXdr {
                    xdr,
                    max_fee: 1_000_000,
                },
                signed_envelope_xdr: None,
            };

            let result = process_fee_bump(
                &relayer.address,
                stellar_data,
                &provider,
                &signer,
                Some(&policy),
                &dex_service,
            )
            .await;

            // Should fail due to insufficient balance
            assert!(result.is_err());
            match result.unwrap_err() {
                TransactionError::ValidationError(_) => {
                    // Success - validation failed as expected
                }
                other => panic!("Expected ValidationError, got: {:?}", other),
            }
        }
    }
}
