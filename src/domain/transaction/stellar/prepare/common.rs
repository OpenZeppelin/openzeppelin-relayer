//! Common functionality shared across preparation modules.

use eyre::Result;
use soroban_rs::{
    stellar_rpc_client::SimulateTransactionResponse,
    xdr::{Limits, TransactionEnvelope, WriteXdr},
};
use tracing::{debug, error, info, warn};

use crate::{
    constants::STELLAR_DEFAULT_TRANSACTION_FEE,
    domain::{
        stellar::i64_from_u64,
        xdr_utils::{
            extract_operations, extract_soroban_resource_fee, update_xdr_fee, update_xdr_sequence,
            xdr_needs_simulation,
        },
        SignTransactionResponse,
    },
    jobs::{JobProducerTrait, TransactionSend},
    models::{
        produce_transaction_update_notification_payload, NetworkTransactionData,
        StellarTransactionData, TransactionError, TransactionInput,
    },
    models::{TransactionRepoModel, TransactionStatus, TransactionUpdateRequest},
    repositories::TransactionCounterTrait,
    repositories::TransactionRepository,
    services::{provider::StellarProviderTrait, signer::Signer},
    utils::calculate_scheduled_timestamp,
};

/// Common helper functions for transaction preparation
/// Apply a sequence number to a transaction envelope.
///
/// This function updates the sequence number in the provided envelope and returns
/// the updated XDR string.
pub async fn apply_sequence(
    envelope: &mut TransactionEnvelope,
    sequence: i64,
) -> Result<String, TransactionError> {
    update_xdr_sequence(envelope, sequence).map_err(|e| {
        TransactionError::ValidationError(format!("Failed to update sequence: {e}"))
    })?;

    envelope.to_xdr_base64(Limits::none()).map_err(|e| {
        TransactionError::ValidationError(format!("Failed to serialize envelope: {e}"))
    })
}

/// Simulate a transaction if it contains operations that require simulation.
///
/// This function checks if the envelope needs simulation (contains Soroban operations)
/// and if so, performs the simulation using the provided Stellar provider.
pub async fn simulate_if_needed<P>(
    envelope: &TransactionEnvelope,
    provider: &P,
) -> Result<Option<SimulateTransactionResponse>, TransactionError>
where
    P: StellarProviderTrait + Send + Sync,
{
    // Check if the envelope needs simulation
    if xdr_needs_simulation(envelope).unwrap_or(false) {
        debug!("Transaction contains Soroban operations, simulating...");

        let resp = provider
            .simulate_transaction_envelope(envelope)
            .await
            .map_err(TransactionError::from)?;

        if let Some(err_msg) = resp.error.clone() {
            warn!(error = %err_msg, "stellar simulation failed");
            return Err(TransactionError::SimulationFailed(err_msg));
        }

        return Ok(Some(resp));
    }

    Ok(None)
}

/// Sign a Stellar transaction using the provided signer.
///
/// This function signs the transaction data and returns the updated stellar data
/// with the signature attached and the signed envelope XDR stored.
pub async fn sign_stellar_transaction<S>(
    signer: &S,
    stellar_data: StellarTransactionData,
) -> Result<StellarTransactionData, TransactionError>
where
    S: Signer + Send + Sync,
{
    // Sign the transaction with the data as-is
    // The signer knows how to handle all TransactionInput types
    let sig_resp = signer
        .sign_transaction(NetworkTransactionData::Stellar(stellar_data.clone()))
        .await?;

    let signature = match sig_resp {
        SignTransactionResponse::Stellar(s) => s.signature,
        _ => {
            return Err(TransactionError::InvalidType(
                "Expected Stellar signature".into(),
            ));
        }
    };

    // Attach the signature to the stellar data
    let mut signed_stellar_data = stellar_data.attach_signature(signature);

    // Build the signed envelope and store its XDR
    let signed_envelope = signed_stellar_data
        .get_envelope_for_submission()
        .map_err(|e| {
            TransactionError::SignerError(format!("Failed to build signed envelope: {e}"))
        })?;
    let signed_xdr = signed_envelope.to_xdr_base64(Limits::none()).map_err(|e| {
        TransactionError::SignerError(format!("Failed to serialize signed envelope: {e}"))
    })?;
    signed_stellar_data.signed_envelope_xdr = Some(signed_xdr);

    Ok(signed_stellar_data)
}

/// Get the next sequence number for a relayer.
///
/// This function retrieves and increments the sequence counter for the given relayer,
/// converting it from u64 to i64 with proper error handling.
pub async fn get_next_sequence<C>(
    counter_service: &C,
    relayer_id: &str,
    relayer_address: &str,
) -> Result<i64, TransactionError>
where
    C: TransactionCounterTrait + Send + Sync,
{
    let sequence_u64 = counter_service
        .get_and_increment(relayer_id, relayer_address)
        .await
        .map_err(|e| TransactionError::UnexpectedError(e.to_string()))?;

    i64_from_u64(sequence_u64).map_err(|relayer_err| {
        let msg = format!("Sequence conversion error for {sequence_u64}: {relayer_err}");
        TransactionError::ValidationError(msg)
    })
}

/// Create signing data for a transaction envelope.
///
/// This function creates a minimal StellarTransactionData structure suitable for signing,
/// containing only the necessary fields.
pub fn create_signing_data(
    source_account: String,
    envelope_xdr: String,
    network_passphrase: String,
) -> StellarTransactionData {
    StellarTransactionData {
        source_account,
        transaction_input: TransactionInput::UnsignedXdr(envelope_xdr),
        network_passphrase,
        // All other fields can be default/empty as they're not used for XDR signing
        fee: None,
        sequence_number: None,
        memo: None,
        valid_until: None,
        signatures: vec![],
        hash: None,
        simulation_transaction_data: None,
        signed_envelope_xdr: None,
        transaction_result_xdr: None,
    }
}

/// Ensure a transaction envelope has at least the minimum required fee.
///
/// This function checks the current fee against the minimum required fee
/// (100 stroops per operation) and updates it if necessary.
pub async fn ensure_minimum_fee(
    envelope: &mut TransactionEnvelope,
) -> Result<(), TransactionError> {
    // Get current fee and operation count
    let (current_fee, op_count) = match envelope {
        TransactionEnvelope::TxV0(e) => (e.tx.fee, e.tx.operations.len()),
        TransactionEnvelope::Tx(e) => (e.tx.fee, e.tx.operations.len()),
        _ => {
            return Err(TransactionError::ValidationError(
                "Unexpected envelope type for fee validation".to_string(),
            ))
        }
    };

    // Calculate minimum required fee (100 stroops per operation)
    let min_fee = STELLAR_DEFAULT_TRANSACTION_FEE * op_count as u32;

    // Update fee if it's below minimum
    if current_fee < min_fee {
        info!(
            "Updating transaction fee from {} to minimum {} stroops",
            current_fee, min_fee
        );
        update_xdr_fee(envelope, min_fee)
            .map_err(|e| TransactionError::ValidationError(format!("Failed to update fee: {e}")))?;
    }

    Ok(())
}

/// Calculate the outer fee-bump fee bid for a transaction.
///
/// Per CAP-0015, the fee-bump fee must cover `inner_num_ops + 1` operations and
/// the fee rate must be at least the inner transaction's fee rate. For Soroban,
/// the resource fee is carried once and only the inclusion-fee rate scales with
/// the extra fee-bump operation.
///
/// Returns `max_fee` as u32 when sufficient, or an error if it falls below the
/// required fee-bump minimum.
pub async fn calculate_fee_bump_required_fee<P>(
    inner_envelope: &TransactionEnvelope,
    max_fee: i64,
    provider: &P,
) -> Result<u32, TransactionError>
where
    P: StellarProviderTrait + Send + Sync,
{
    let inner_num_ops = extract_operations(inner_envelope)
        .map(|ops| ops.len() as i64)
        .unwrap_or(1)
        .max(1);
    let fee_bump_num_ops = inner_num_ops + 1; // CAP-0015

    let inner_tx_fee = extract_inner_transaction_fee(inner_envelope);

    // Check if the inner transaction already has SorobanTransactionData with resource fee.
    // This allows skipping simulation for pre-simulated signed transactions.
    if let Some(existing_resource_fee) = extract_soroban_resource_fee(inner_envelope) {
        let required_fee = calculate_fee_bump_fee(
            inner_tx_fee,
            existing_resource_fee,
            inner_num_ops,
            fee_bump_num_ops,
        )?;

        debug!(
            "Using existing Soroban resource fee for fee-bump calculation. \
             Inner tx fee: {}, Resource fee: {}, Fee-bump num_ops: {} (inner: {}), \
             Required fee-bump fee: {}",
            inner_tx_fee, existing_resource_fee, fee_bump_num_ops, inner_num_ops, required_fee
        );

        return validated_max_fee_as_u32(max_fee, required_fee);
    }

    let resource_fee = match simulate_if_needed(inner_envelope, provider).await? {
        Some(sim_resp) => {
            let resource_fee = i64::try_from(sim_resp.min_resource_fee).map_err(|_| {
                TransactionError::ValidationError(format!(
                    "Resource fee conversion overflow: min_resource_fee ({}) exceeds i64::MAX",
                    sim_resp.min_resource_fee
                ))
            })?;
            debug!(
                "Simulation complete. Inner tx fee: {}, Simulated resource fee: {}, \
                 Fee-bump num_ops: {} (inner: {})",
                inner_tx_fee, resource_fee, fee_bump_num_ops, inner_num_ops,
            );
            resource_fee
        }
        None => 0,
    };

    let required_fee =
        calculate_fee_bump_fee(inner_tx_fee, resource_fee, inner_num_ops, fee_bump_num_ops)?;
    validated_max_fee_as_u32(max_fee, required_fee)
}

fn extract_inner_transaction_fee(inner_envelope: &TransactionEnvelope) -> i64 {
    match inner_envelope {
        TransactionEnvelope::TxV0(e) => i64::from(e.tx.fee),
        TransactionEnvelope::Tx(e) => i64::from(e.tx.fee),
        TransactionEnvelope::TxFeeBump(fb) => {
            let soroban_rs::xdr::FeeBumpTransactionInnerTx::Tx(inner) = &fb.tx.inner_tx;
            i64::from(inner.tx.fee)
        }
    }
}

/// Compute the minimum total fee-bump fee that satisfies CAP-0015.
///
/// The fee-bump operation scales the inner transaction's inclusion-fee rate over
/// `inner_num_ops + 1` operations. Soroban `resource_fee` is carried once.
fn calculate_fee_bump_fee(
    inner_tx_fee: i64,
    resource_fee: i64,
    inner_num_ops: i64,
    fee_bump_num_ops: i64,
) -> Result<i64, TransactionError> {
    if inner_num_ops <= 0 || fee_bump_num_ops <= 0 {
        return Err(TransactionError::ValidationError(format!(
            "Invalid operation counts: inner_num_ops={inner_num_ops}, fee_bump_num_ops={fee_bump_num_ops}"
        )));
    }
    if inner_tx_fee < 0 || resource_fee < 0 {
        return Err(TransactionError::ValidationError(format!(
            "Invalid fee inputs: inner_tx_fee={inner_tx_fee}, resource_fee={resource_fee}"
        )));
    }

    let inner_inclusion_total = inner_tx_fee.saturating_sub(resource_fee);
    let inner_fee_rate =
        ceil_div(inner_inclusion_total, inner_num_ops).max(STELLAR_DEFAULT_TRANSACTION_FEE as i64);

    inner_fee_rate
        .checked_mul(fee_bump_num_ops)
        .ok_or_else(|| {
            TransactionError::ValidationError(format!(
                "Fee overflow computing fee-bump base fee for {inner_fee_rate} * {fee_bump_num_ops}"
            ))
        })?
        .checked_add(resource_fee)
        .ok_or_else(|| {
            TransactionError::ValidationError(format!(
                "Fee overflow adding resource fee ({resource_fee}) to fee-bump fee"
            ))
        })
}

/// Ceiling division for a non-negative divisor.
/// Caller must ensure `divisor > 0`.
fn ceil_div(value: i64, divisor: i64) -> i64 {
    (value + divisor - 1) / divisor
}

fn validated_max_fee_as_u32(max_fee: i64, required_fee: i64) -> Result<u32, TransactionError> {
    if max_fee < required_fee {
        return Err(TransactionError::ValidationError(format!(
            "max_fee ({max_fee}) is insufficient. Required fee-bump fee: {required_fee}"
        )));
    }
    u32::try_from(max_fee).map_err(|_| {
        TransactionError::ValidationError(format!(
            "Fee conversion overflow: max_fee ({max_fee}) exceeds u32::MAX"
        ))
    })
}

// Additional helper methods for transaction preparation

/// Send a submit-transaction job for the given transaction.
pub async fn send_submit_transaction_job<J>(
    job_producer: &J,
    tx: &TransactionRepoModel,
    delay_seconds: Option<i64>,
) -> Result<(), TransactionError>
where
    J: JobProducerTrait + Send + Sync,
{
    let job = TransactionSend::submit(tx.id.clone(), tx.relayer_id.clone());
    let scheduled_on = delay_seconds.map(calculate_scheduled_timestamp);
    debug!(
        tx_id = %tx.id,
        relayer_id = %tx.relayer_id,
        delay_seconds = ?delay_seconds,
        "enqueueing submit transaction job"
    );
    job_producer
        .produce_submit_transaction_job(job, scheduled_on)
        .await?;
    Ok(())
}

/// Update transaction status and send notifications.
pub async fn update_and_notify_transaction<T, J>(
    transaction_repository: &T,
    job_producer: &J,
    tx_id: String,
    stellar_data: StellarTransactionData,
    notification_id: Option<&str>,
) -> Result<TransactionRepoModel, TransactionError>
where
    T: TransactionRepository + Send + Sync,
    J: JobProducerTrait + Send + Sync,
{
    debug!(
        tx_id = %tx_id,
        "updating transaction status to Sent"
    );

    // Update the transaction with the final stellar data
    let update_req = TransactionUpdateRequest {
        status: Some(TransactionStatus::Sent),
        network_data: Some(NetworkTransactionData::Stellar(stellar_data)),
        ..Default::default()
    };

    let saved_tx = transaction_repository
        .partial_update(tx_id, update_req)
        .await?;

    debug!(
        tx_id = %saved_tx.id,
        relayer_id = %saved_tx.relayer_id,
        status = ?saved_tx.status,
        "transaction updated, enqueueing submit job"
    );

    send_submit_transaction_job(job_producer, &saved_tx, None).await?;

    // Send notification if notification_id is provided
    if let Some(notification_id) = notification_id {
        let notification =
            produce_transaction_update_notification_payload(notification_id, &saved_tx);
        if let Err(e) = job_producer
            .produce_send_notification_job(notification, None)
            .await
        {
            error!(error = %e, "failed to produce notification job");
        }
    }

    Ok(saved_tx)
}

/// Sign and finalize a transaction with common logic.
pub async fn sign_and_finalize_transaction<S>(
    signer: &S,
    tx: TransactionRepoModel,
    stellar_data: StellarTransactionData,
) -> Result<(TransactionRepoModel, StellarTransactionData), TransactionError>
where
    S: Signer + Send + Sync,
{
    // Sign the transaction
    let sig_resp = signer
        .sign_transaction(NetworkTransactionData::Stellar(stellar_data.clone()))
        .await?;

    let signature = match sig_resp {
        SignTransactionResponse::Stellar(s) => s.signature,
        _ => {
            return Err(TransactionError::InvalidType(
                "Expected Stellar signature".into(),
            ));
        }
    };

    let mut final_stellar_data = stellar_data.attach_signature(signature);

    // Build the signed envelope and store its XDR
    let signed_envelope = final_stellar_data
        .get_envelope_for_submission()
        .map_err(|e| {
            TransactionError::SignerError(format!("Failed to build signed envelope: {e}"))
        })?;
    let signed_xdr = signed_envelope.to_xdr_base64(Limits::none()).map_err(|e| {
        TransactionError::SignerError(format!("Failed to serialize signed envelope: {e}"))
    })?;
    final_stellar_data.signed_envelope_xdr = Some(signed_xdr);

    Ok((tx, final_stellar_data))
}

#[cfg(test)]
mod tests {
    use std::future::ready;

    use super::*;
    use soroban_rs::xdr::{
        Memo, MuxedAccount, SequenceNumber, Transaction, TransactionExt, TransactionV1Envelope,
        Uint256, VecM,
    };
    use stellar_strkey::ed25519::PublicKey;

    fn create_test_envelope() -> TransactionEnvelope {
        let pk = PublicKey([0; 32]);
        let source = MuxedAccount::Ed25519(Uint256(pk.0));

        let tx = Transaction {
            source_account: source,
            fee: 100,
            seq_num: SequenceNumber(1),
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
    async fn test_apply_sequence() {
        let mut envelope = create_test_envelope();
        let new_sequence = 42i64;

        let result = apply_sequence(&mut envelope, new_sequence).await;
        assert!(result.is_ok());

        // Verify the sequence was updated
        match &envelope {
            TransactionEnvelope::Tx(e) => {
                assert_eq!(e.tx.seq_num.0, new_sequence);
            }
            _ => panic!("Unexpected envelope type"),
        }

        // Verify we got valid XDR back
        let xdr = result.unwrap();
        assert!(!xdr.is_empty());
    }

    #[tokio::test]
    async fn test_get_next_sequence() {
        use crate::repositories::MockTransactionCounterTrait;

        let mut counter_service = MockTransactionCounterTrait::new();
        counter_service
            .expect_get_and_increment()
            .returning(|_, _| Box::pin(ready(Ok(100))));

        let result = get_next_sequence(&counter_service, "relayer-1", "GTEST").await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 100i64);
    }

    #[tokio::test]
    async fn test_get_next_sequence_overflow() {
        use crate::repositories::MockTransactionCounterTrait;

        let mut counter_service = MockTransactionCounterTrait::new();
        counter_service
            .expect_get_and_increment()
            .returning(|_, _| Box::pin(ready(Ok(u64::MAX))));

        let result = get_next_sequence(&counter_service, "relayer-1", "GTEST").await;

        assert!(result.is_err());
        match result.unwrap_err() {
            TransactionError::ValidationError(msg) => {
                assert!(msg.contains("Sequence conversion error"));
            }
            _ => panic!("Expected ValidationError"),
        }
    }

    #[test]
    fn test_create_signing_data() {
        let source = "GTEST".to_string();
        let xdr = "test-xdr".to_string();
        let passphrase = "Test Network".to_string();

        let data = create_signing_data(source.clone(), xdr.clone(), passphrase.clone());

        assert_eq!(data.source_account, source);
        assert!(matches!(
            data.transaction_input,
            TransactionInput::UnsignedXdr(ref x) if x == &xdr
        ));
        assert_eq!(data.network_passphrase, passphrase);
        assert!(data.fee.is_none());
        assert!(data.sequence_number.is_none());
        assert!(data.signatures.is_empty());
    }

    #[tokio::test]
    async fn test_ensure_minimum_fee() {
        let mut envelope = create_test_envelope();

        // Add an operation to test fee calculation
        let payment_op = soroban_rs::xdr::Operation {
            source_account: None,
            body: soroban_rs::xdr::OperationBody::Payment(soroban_rs::xdr::PaymentOp {
                destination: MuxedAccount::Ed25519(Uint256([0; 32])),
                asset: soroban_rs::xdr::Asset::Native,
                amount: 1000000,
            }),
        };

        match &mut envelope {
            TransactionEnvelope::Tx(ref mut e) => {
                e.tx.fee = 50; // Below minimum
                e.tx.operations = vec![payment_op].try_into().unwrap();
            }
            _ => panic!("Unexpected envelope type"),
        }

        let result = ensure_minimum_fee(&mut envelope).await;
        assert!(result.is_ok());

        // Verify fee was updated to minimum
        match &envelope {
            TransactionEnvelope::Tx(e) => {
                assert_eq!(e.tx.fee, STELLAR_DEFAULT_TRANSACTION_FEE);
            }
            _ => panic!("Unexpected envelope type"),
        }
    }

    #[tokio::test]
    async fn test_ensure_minimum_fee_already_sufficient() {
        let mut envelope = create_test_envelope();

        match &mut envelope {
            TransactionEnvelope::Tx(ref mut e) => {
                e.tx.fee = 200; // Above minimum
            }
            _ => panic!("Unexpected envelope type"),
        }

        let result = ensure_minimum_fee(&mut envelope).await;
        assert!(result.is_ok());

        // Verify fee was not changed
        match &envelope {
            TransactionEnvelope::Tx(e) => {
                assert_eq!(e.tx.fee, 200);
            }
            _ => panic!("Unexpected envelope type"),
        }
    }
}

#[cfg(test)]
mod send_submit_transaction_job_tests {
    use super::*;
    use crate::domain::transaction::stellar::test_helpers::*;

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
                job.transaction_id == "tx-1"
                    && job.relayer_id == "relayer-1"
                    && delay.is_some()
                    && delay.unwrap() > chrono::Utc::now().timestamp()
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

#[cfg(test)]
mod calculate_fee_bump_required_fee_tests {
    use super::*;
    use crate::models::TransactionError;
    use crate::services::provider::MockStellarProviderTrait;
    use soroban_rs::xdr::{
        Hash, HostFunction, InvokeContractArgs, InvokeHostFunctionOp, LedgerFootprint, Memo,
        MuxedAccount, Operation, OperationBody, PaymentOp, ScAddress, SequenceNumber,
        SorobanResources, SorobanTransactionData, SorobanTransactionDataExt, Transaction,
        TransactionV1Envelope, Uint256, VecM,
    };
    use stellar_strkey::ed25519::PublicKey;

    fn create_soroban_envelope_with_existing_data(resource_fee: i64) -> TransactionEnvelope {
        let pk = PublicKey([0; 32]);
        let source = MuxedAccount::Ed25519(Uint256(pk.0));

        let soroban_data = SorobanTransactionData {
            ext: SorobanTransactionDataExt::V0,
            resources: SorobanResources {
                footprint: LedgerFootprint {
                    read_only: vec![].try_into().unwrap(),
                    read_write: vec![].try_into().unwrap(),
                },
                instructions: 1000,
                disk_read_bytes: 100,
                write_bytes: 50,
            },
            resource_fee,
        };

        let invoke_op = Operation {
            source_account: None,
            body: OperationBody::InvokeHostFunction(InvokeHostFunctionOp {
                host_function: HostFunction::InvokeContract(InvokeContractArgs {
                    contract_address: ScAddress::Contract(soroban_rs::xdr::ContractId(Hash(
                        [0u8; 32],
                    ))),
                    function_name: "test".try_into().unwrap(),
                    args: vec![].try_into().unwrap(),
                }),
                auth: vec![].try_into().unwrap(),
            }),
        };

        let tx = Transaction {
            source_account: source,
            fee: u32::try_from(resource_fee + STELLAR_DEFAULT_TRANSACTION_FEE as i64).unwrap(),
            seq_num: SequenceNumber(1),
            cond: soroban_rs::xdr::Preconditions::None,
            memo: Memo::None,
            operations: vec![invoke_op].try_into().unwrap(),
            ext: soroban_rs::xdr::TransactionExt::V1(soroban_data),
        };

        TransactionEnvelope::Tx(TransactionV1Envelope {
            tx,
            signatures: VecM::default(),
        })
    }

    fn create_classic_envelope(fee: u32, op_count: usize) -> TransactionEnvelope {
        let pk = PublicKey([0; 32]);
        let source = MuxedAccount::Ed25519(Uint256(pk.0));

        let operations: Vec<_> = (0..op_count)
            .map(|_| Operation {
                source_account: None,
                body: OperationBody::Payment(PaymentOp {
                    destination: MuxedAccount::Ed25519(Uint256([1; 32])),
                    asset: soroban_rs::xdr::Asset::Native,
                    amount: 1_000_000,
                }),
            })
            .collect();

        let tx = Transaction {
            source_account: source,
            fee,
            seq_num: SequenceNumber(1),
            cond: soroban_rs::xdr::Preconditions::None,
            memo: Memo::None,
            operations: operations.try_into().unwrap(),
            ext: soroban_rs::xdr::TransactionExt::V0,
        };

        TransactionEnvelope::Tx(TransactionV1Envelope {
            tx,
            signatures: VecM::default(),
        })
    }

    #[tokio::test]
    async fn test_calculate_fee_bump_skips_simulation_with_existing_data() {
        let resource_fee = 50000i64;
        let envelope = create_soroban_envelope_with_existing_data(resource_fee);
        assert!(xdr_needs_simulation(&envelope).unwrap());

        // No mock expectation set — panics if simulation is called
        let provider = MockStellarProviderTrait::new();

        // required = 100 * 2 + 50000 = 50200
        let max_fee = 200000i64;
        let result = calculate_fee_bump_required_fee(&envelope, max_fee, &provider).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), max_fee as u32);
    }

    #[tokio::test]
    async fn test_calculate_fee_bump_rejects_bid_that_only_covers_inner_soroban_fee() {
        let resource_fee = 50000i64;
        let envelope = create_soroban_envelope_with_existing_data(resource_fee);

        let provider = MockStellarProviderTrait::new();

        let max_fee = 50100i64; // Same as inner_fee

        let result = calculate_fee_bump_required_fee(&envelope, max_fee, &provider).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, TransactionError::ValidationError(msg) if msg.contains("50200")));
    }

    #[tokio::test]
    async fn test_calculate_fee_bump_uses_actual_inner_fee_rate_when_higher() {
        // tx.fee diverges from sorobanData.resourceFee when assembleTransaction()
        // sets fee = inclusionFee + minResourceFee (which can be >> sorobanData.resourceFee)
        let resource_fee = 33048i64;
        let mut envelope = create_soroban_envelope_with_existing_data(resource_fee);
        match &mut envelope {
            TransactionEnvelope::Tx(ref mut e) => e.tx.fee = 66196,
            _ => panic!("Expected Tx envelope"),
        }

        let provider = MockStellarProviderTrait::new();

        // inclusion = 66196 - 33048 = 33148, required = 33148 * 2 + 33048 = 99344
        let max_fee = 99_344i64;

        let result = calculate_fee_bump_required_fee(&envelope, max_fee, &provider).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 99_344u32);
    }

    #[tokio::test]
    async fn test_calculate_fee_bump_rejects_classic_bid_below_minimum_fee_bump_fee() {
        let envelope = create_classic_envelope(100, 1);

        let provider = MockStellarProviderTrait::new();

        let max_fee = 100i64;
        let result = calculate_fee_bump_required_fee(&envelope, max_fee, &provider).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, TransactionError::ValidationError(msg) if msg.contains("200")));
    }

    #[test]
    fn test_calculate_fee_bump_fee_scales_inclusion_rate_only() {
        let required = calculate_fee_bump_fee(25_202, 25_102, 1, 2).unwrap();
        assert_eq!(required, 25_302);
    }

    #[test]
    fn test_calculate_fee_bump_fee_uses_ceil_division_for_classic_multi_op() {
        let required = calculate_fee_bump_fee(201, 0, 2, 3).unwrap();
        assert_eq!(required, 303);
    }

    #[test]
    fn test_calculate_fee_bump_fee_rejects_invalid_inputs() {
        let err = calculate_fee_bump_fee(-1, 0, 1, 2).unwrap_err();
        assert!(matches!(err, TransactionError::ValidationError(_)));

        let err = calculate_fee_bump_fee(100, 0, 0, 1).unwrap_err();
        assert!(matches!(err, TransactionError::ValidationError(_)));
    }

    fn create_soroban_envelope_without_existing_data(fee: u32) -> TransactionEnvelope {
        let pk = PublicKey([0; 32]);
        let source = MuxedAccount::Ed25519(Uint256(pk.0));

        let invoke_op = Operation {
            source_account: None,
            body: OperationBody::InvokeHostFunction(InvokeHostFunctionOp {
                host_function: HostFunction::InvokeContract(InvokeContractArgs {
                    contract_address: ScAddress::Contract(soroban_rs::xdr::ContractId(Hash(
                        [0u8; 32],
                    ))),
                    function_name: "test".try_into().unwrap(),
                    args: vec![].try_into().unwrap(),
                }),
                auth: vec![].try_into().unwrap(),
            }),
        };

        let tx = Transaction {
            source_account: source,
            fee,
            seq_num: SequenceNumber(1),
            cond: soroban_rs::xdr::Preconditions::None,
            memo: Memo::None,
            operations: vec![invoke_op].try_into().unwrap(),
            ext: soroban_rs::xdr::TransactionExt::V0,
        };

        TransactionEnvelope::Tx(TransactionV1Envelope {
            tx,
            signatures: VecM::default(),
        })
    }

    #[tokio::test]
    async fn test_calculate_fee_bump_simulates_when_no_existing_soroban_data() {
        let envelope = create_soroban_envelope_without_existing_data(100);

        let mut provider = MockStellarProviderTrait::new();
        provider
            .expect_simulate_transaction_envelope()
            .times(1)
            .returning(|_| {
                Box::pin(std::future::ready(Ok(SimulateTransactionResponse {
                    error: None,
                    min_resource_fee: 50000,
                    ..Default::default()
                })))
            });

        // required = 100 * 2 + 50000 = 50200
        let max_fee = 200_000i64;
        let result = calculate_fee_bump_required_fee(&envelope, max_fee, &provider).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 200_000u32);
    }

    #[tokio::test]
    async fn test_calculate_fee_bump_simulation_uses_inner_fee_rate_when_higher() {
        let envelope = create_soroban_envelope_without_existing_data(70_000);

        let mut provider = MockStellarProviderTrait::new();
        provider
            .expect_simulate_transaction_envelope()
            .times(1)
            .returning(|_| {
                Box::pin(std::future::ready(Ok(SimulateTransactionResponse {
                    error: None,
                    min_resource_fee: 30_000,
                    ..Default::default()
                })))
            });

        // inclusion = 70000 - 30000 = 40000, required = 40000 * 2 + 30000 = 110000
        let max_fee = 110_000i64;
        let result = calculate_fee_bump_required_fee(&envelope, max_fee, &provider).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 110_000u32);
    }

    #[tokio::test]
    async fn test_calculate_fee_bump_simulation_required_fee_less_than_max_fee() {
        let envelope = create_soroban_envelope_without_existing_data(100);

        let mut provider = MockStellarProviderTrait::new();
        provider
            .expect_simulate_transaction_envelope()
            .times(1)
            .returning(|_| {
                Box::pin(std::future::ready(Ok(SimulateTransactionResponse {
                    error: None,
                    min_resource_fee: 5_000,
                    ..Default::default()
                })))
            });

        // required = 100 * 2 + 5000 = 5200
        let max_fee = 500_000i64;
        let result = calculate_fee_bump_required_fee(&envelope, max_fee, &provider).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 500_000u32);
    }

    #[tokio::test]
    async fn test_calculate_fee_bump_simulation_failure_propagates_error() {
        let envelope = create_soroban_envelope_without_existing_data(100);

        let mut provider = MockStellarProviderTrait::new();
        provider
            .expect_simulate_transaction_envelope()
            .times(1)
            .returning(|_| {
                Box::pin(std::future::ready(Ok(SimulateTransactionResponse {
                    error: Some("Insufficient resources for operation".to_string()),
                    min_resource_fee: 0,
                    ..Default::default()
                })))
            });

        let max_fee = 200_000i64;
        let result = calculate_fee_bump_required_fee(&envelope, max_fee, &provider).await;

        assert!(result.is_err());
        match result.unwrap_err() {
            TransactionError::SimulationFailed(msg) => {
                assert!(msg.contains("Insufficient resources"));
            }
            other => panic!("Expected SimulationFailed, got: {other:?}"),
        }
    }
}

#[cfg(test)]
mod simulate_if_needed_tests {
    use super::*;
    use crate::services::provider::MockStellarProviderTrait;
    use soroban_rs::xdr::{
        Hash, HostFunction, InvokeContractArgs, InvokeHostFunctionOp, Memo, MuxedAccount,
        Operation, OperationBody, ScAddress, SequenceNumber, Transaction, TransactionV1Envelope,
        Uint256, VecM,
    };
    use stellar_strkey::ed25519::PublicKey;

    fn create_non_soroban_envelope() -> TransactionEnvelope {
        let pk = PublicKey([0; 32]);
        let source = MuxedAccount::Ed25519(Uint256(pk.0));

        let payment_op = Operation {
            source_account: None,
            body: OperationBody::Payment(soroban_rs::xdr::PaymentOp {
                destination: MuxedAccount::Ed25519(Uint256([0; 32])),
                asset: soroban_rs::xdr::Asset::Native,
                amount: 1_000_000,
            }),
        };

        let tx = Transaction {
            source_account: source,
            fee: 100,
            seq_num: SequenceNumber(1),
            cond: soroban_rs::xdr::Preconditions::None,
            memo: Memo::None,
            operations: vec![payment_op].try_into().unwrap(),
            ext: soroban_rs::xdr::TransactionExt::V0,
        };

        TransactionEnvelope::Tx(TransactionV1Envelope {
            tx,
            signatures: VecM::default(),
        })
    }

    fn create_soroban_envelope() -> TransactionEnvelope {
        let pk = PublicKey([0; 32]);
        let source = MuxedAccount::Ed25519(Uint256(pk.0));

        let invoke_op = Operation {
            source_account: None,
            body: OperationBody::InvokeHostFunction(InvokeHostFunctionOp {
                host_function: HostFunction::InvokeContract(InvokeContractArgs {
                    contract_address: ScAddress::Contract(soroban_rs::xdr::ContractId(Hash(
                        [0u8; 32],
                    ))),
                    function_name: "test".try_into().unwrap(),
                    args: vec![].try_into().unwrap(),
                }),
                auth: vec![].try_into().unwrap(),
            }),
        };

        let tx = Transaction {
            source_account: source,
            fee: 100,
            seq_num: SequenceNumber(1),
            cond: soroban_rs::xdr::Preconditions::None,
            memo: Memo::None,
            operations: vec![invoke_op].try_into().unwrap(),
            ext: soroban_rs::xdr::TransactionExt::V0,
        };

        TransactionEnvelope::Tx(TransactionV1Envelope {
            tx,
            signatures: VecM::default(),
        })
    }

    #[tokio::test]
    async fn test_returns_none_for_non_soroban_transaction() {
        let envelope = create_non_soroban_envelope();

        // Provider should NOT be called; no expectations set → panics if called
        let provider = MockStellarProviderTrait::new();

        let result = simulate_if_needed(&envelope, &provider).await;

        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_returns_response_on_successful_simulation() {
        let envelope = create_soroban_envelope();

        let mut provider = MockStellarProviderTrait::new();
        provider
            .expect_simulate_transaction_envelope()
            .times(1)
            .returning(|_| {
                Box::pin(std::future::ready(Ok(SimulateTransactionResponse {
                    error: None,
                    min_resource_fee: 42_000,
                    ..Default::default()
                })))
            });

        let result = simulate_if_needed(&envelope, &provider).await;

        assert!(result.is_ok());
        let resp = result.unwrap().expect("Expected Some(response)");
        assert_eq!(resp.min_resource_fee, 42_000);
        assert!(resp.error.is_none());
    }

    #[tokio::test]
    async fn test_returns_error_on_simulation_failure() {
        let envelope = create_soroban_envelope();

        let mut provider = MockStellarProviderTrait::new();
        provider
            .expect_simulate_transaction_envelope()
            .times(1)
            .returning(|_| {
                Box::pin(std::future::ready(Ok(SimulateTransactionResponse {
                    error: Some("tx simulation failed".to_string()),
                    min_resource_fee: 0,
                    ..Default::default()
                })))
            });

        let result = simulate_if_needed(&envelope, &provider).await;

        assert!(result.is_err());
        match result.unwrap_err() {
            TransactionError::SimulationFailed(msg) => {
                assert_eq!(msg, "tx simulation failed");
            }
            other => panic!("Expected SimulationFailed, got: {other:?}"),
        }
    }
}
