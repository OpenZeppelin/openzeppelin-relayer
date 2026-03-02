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

/// Calculate the required fee for a fee-bump transaction.
///
/// Per CAP-0015, a fee-bump transaction has an effective number of operations
/// equal to `inner_num_ops + 1`. The fee-bump fee must satisfy:
///   `fee_bump_fee / (inner_num_ops + 1) >= inner_fee / inner_num_ops`
///
/// For Soroban transactions (always 1 operation), this means the fee-bump fee
/// must be at least `2 * inner_fee`.
///
/// If the inner transaction already has SorobanTransactionData with a resource fee,
/// that fee is used instead of re-simulating (useful for pre-simulated signed transactions).
/// For regular transactions, it uses the provided max_fee scaled by the operation count.
pub async fn calculate_fee_bump_required_fee<P>(
    inner_envelope: &TransactionEnvelope,
    max_fee: i64,
    provider: &P,
) -> Result<u32, TransactionError>
where
    P: StellarProviderTrait + Send + Sync,
{
    // CAP-0015: fee-bump effective num_ops = inner_num_ops + 1
    let inner_num_ops = extract_operations(inner_envelope)
        .map(|ops| ops.len() as i64)
        .unwrap_or(1)
        .max(1);
    let fee_bump_num_ops = inner_num_ops.checked_add(1).ok_or_else(|| {
        TransactionError::ValidationError(format!(
            "Operation count overflow: inner_num_ops ({inner_num_ops}) + 1 exceeds i64::MAX"
        ))
    })?;

    // Check if the inner transaction already has SorobanTransactionData with resource fee.
    // This allows skipping simulation for pre-simulated signed transactions.
    if let Some(existing_resource_fee) = extract_soroban_resource_fee(inner_envelope) {
        let inclusion_fee = STELLAR_DEFAULT_TRANSACTION_FEE as i64;
        let inner_fee = inclusion_fee.checked_add(existing_resource_fee).ok_or_else(|| {
            TransactionError::ValidationError(format!(
                "Fee overflow: inclusion fee ({inclusion_fee}) + resource fee ({existing_resource_fee}) exceeds i64::MAX"
            ))
        })?;

        // Scale fee for fee-bump using ceil division to satisfy CAP-0015 inequality.
        let required_fee = scale_fee_for_fee_bump(inner_fee, inner_num_ops, fee_bump_num_ops)?;

        debug!(
            "Using existing resource fee from inner transaction. \
             Inclusion fee: {}, Resource fee: {}, Inner fee: {}, \
             Fee-bump num_ops: {} (inner: {}), Required fee-bump fee: {}",
            inclusion_fee,
            existing_resource_fee,
            inner_fee,
            fee_bump_num_ops,
            inner_num_ops,
            required_fee
        );

        // Validate max_fee covers the required fee-bump fee
        if max_fee < required_fee {
            // Use required_fee instead of max_fee when max_fee is insufficient
            // The plugin's max_fee was calculated for the inner tx, not the fee-bump
            debug!(
                "max_fee ({max_fee}) is below required fee-bump fee ({required_fee}), using required fee"
            );
        }

        return u32::try_from(required_fee.max(max_fee)).map_err(|_| {
            TransactionError::ValidationError(format!(
                "Fee conversion overflow: required fee {} exceeds u32::MAX",
                required_fee.max(max_fee)
            ))
        });
    }

    // Check if the inner transaction needs simulation (Soroban operations)
    if xdr_needs_simulation(inner_envelope).unwrap_or(false) {
        debug!("Inner transaction contains Soroban operations, simulating to determine resource fee...");

        match simulate_if_needed(inner_envelope, provider).await? {
            Some(sim_resp) => {
                let inclusion_fee = STELLAR_DEFAULT_TRANSACTION_FEE as i64;
                let resource_fee = i64::try_from(sim_resp.min_resource_fee).map_err(|_| {
                    TransactionError::ValidationError(format!(
                        "Resource fee conversion overflow: min_resource_fee ({}) exceeds i64::MAX",
                        sim_resp.min_resource_fee
                    ))
                })?;
                let inner_fee = inclusion_fee.checked_add(resource_fee).ok_or_else(|| {
                    TransactionError::ValidationError(format!(
                        "Fee overflow: inclusion fee ({inclusion_fee}) + resource fee ({resource_fee}) exceeds i64::MAX"
                    ))
                })?;

                // Scale for fee-bump num_ops (CAP-0015).
                let required_fee =
                    scale_fee_for_fee_bump(inner_fee, inner_num_ops, fee_bump_num_ops)?;

                debug!(
                    "Simulation complete. Inclusion fee: {}, Resource fee: {}, Inner fee: {}, \
                     Fee-bump num_ops: {} (inner: {}), Required fee-bump fee: {}",
                    inclusion_fee,
                    resource_fee,
                    inner_fee,
                    fee_bump_num_ops,
                    inner_num_ops,
                    required_fee
                );

                u32::try_from(required_fee.max(max_fee)).map_err(|_| {
                    TransactionError::ValidationError(format!(
                        "Fee conversion overflow: required fee {} exceeds u32::MAX",
                        required_fee.max(max_fee)
                    ))
                })
            }
            None => {
                // No simulation needed, scale max_fee for fee-bump
                let required_fee =
                    scale_fee_for_fee_bump(max_fee, inner_num_ops, fee_bump_num_ops)?;
                u32::try_from(required_fee).map_err(|_| {
                    TransactionError::ValidationError(format!(
                        "Fee conversion overflow: required fee {required_fee} exceeds u32::MAX"
                    ))
                })
            }
        }
    } else {
        // No simulation needed, scale max_fee for fee-bump
        let required_fee = scale_fee_for_fee_bump(max_fee, inner_num_ops, fee_bump_num_ops)?;
        u32::try_from(required_fee).map_err(|_| {
            TransactionError::ValidationError(format!(
                "Fee conversion overflow: required fee {required_fee} exceeds u32::MAX"
            ))
        })
    }
}

/// Scale inner fee for fee-bump operation count using ceil division:
/// `ceil(inner_fee * fee_bump_num_ops / inner_num_ops)`.
fn scale_fee_for_fee_bump(
    inner_fee: i64,
    inner_num_ops: i64,
    fee_bump_num_ops: i64,
) -> Result<i64, TransactionError> {
    if inner_num_ops <= 0 || fee_bump_num_ops <= 0 {
        return Err(TransactionError::ValidationError(format!(
            "Invalid operation counts: inner_num_ops={inner_num_ops}, fee_bump_num_ops={fee_bump_num_ops}"
        )));
    }
    if inner_fee < 0 {
        return Err(TransactionError::ValidationError(format!(
            "Invalid inner fee: {inner_fee} (must be non-negative)"
        )));
    }

    // Ceil division: ceil(a / b) = (a + b - 1) / b for positive integers.
    let numerator = inner_fee
        .checked_mul(fee_bump_num_ops)
        .ok_or_else(|| {
            TransactionError::ValidationError(format!(
                "Fee overflow computing fee-bump required fee for {inner_fee} * {fee_bump_num_ops}"
            ))
        })?
        .checked_add(inner_num_ops - 1)
        .ok_or_else(|| {
            TransactionError::ValidationError(format!(
                "Fee overflow computing ceil adjustment for numerator + ({inner_num_ops} - 1)"
            ))
        })?;

    numerator.checked_div(inner_num_ops).ok_or_else(|| {
        TransactionError::ValidationError(format!(
            "Division error computing fee-bump required fee for numerator {numerator} / {inner_num_ops}"
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
        MuxedAccount, Operation, OperationBody, ScAddress, SequenceNumber, SorobanResources,
        SorobanTransactionData, SorobanTransactionDataExt, Transaction, TransactionV1Envelope,
        Uint256, VecM,
    };
    use stellar_strkey::ed25519::PublicKey;

    /// Creates a Soroban transaction envelope with existing SorobanTransactionData.
    /// Includes an InvokeHostFunction operation so xdr_needs_simulation returns true.
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

        // Create an InvokeHostFunction operation so xdr_needs_simulation returns true
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
            ext: soroban_rs::xdr::TransactionExt::V1(soroban_data),
        };

        TransactionEnvelope::Tx(TransactionV1Envelope {
            tx,
            signatures: VecM::default(),
        })
    }

    fn create_envelope_without_soroban_data() -> TransactionEnvelope {
        let pk = PublicKey([0; 32]);
        let source = MuxedAccount::Ed25519(Uint256(pk.0));

        let tx = Transaction {
            source_account: source,
            fee: 100,
            seq_num: SequenceNumber(1),
            cond: soroban_rs::xdr::Preconditions::None,
            memo: Memo::None,
            operations: VecM::default(),
            ext: soroban_rs::xdr::TransactionExt::V0,
        };

        TransactionEnvelope::Tx(TransactionV1Envelope {
            tx,
            signatures: VecM::default(),
        })
    }

    #[tokio::test]
    async fn test_calculate_fee_bump_skips_simulation_with_existing_data() {
        // Create a Soroban transaction envelope with existing SorobanTransactionData
        // This includes an InvokeHostFunction op, so xdr_needs_simulation would return true
        // if we didn't short-circuit on the existing resource fee
        let resource_fee = 50000i64;
        let envelope = create_soroban_envelope_with_existing_data(resource_fee);

        // Verify that the envelope does need simulation (has Soroban op)
        assert!(xdr_needs_simulation(&envelope).unwrap());

        // Provider should NOT be called for simulation since we have existing data
        // If simulate_transaction_envelope is called, mockall will panic because
        // no expectation is set
        let provider = MockStellarProviderTrait::new();

        // inner_fee = 100 (inclusion) + 50000 (resource) = 50100
        // CAP-0015: fee_bump_num_ops = 1 + 1 = 2
        // required_fee = 50100 * 2 / 1 = 100200
        let max_fee = 200000i64;
        let result = calculate_fee_bump_required_fee(&envelope, max_fee, &provider).await;

        assert!(result.is_ok());
        // Should return required_fee (100200) since max_fee (200000) > required_fee
        // max(required_fee, max_fee) = max(100200, 200000) = 200000
        assert_eq!(result.unwrap(), max_fee as u32);
    }

    #[tokio::test]
    async fn test_calculate_fee_bump_accounts_for_cap0015_num_ops() {
        // Verify that fee-bump fee is scaled by (inner_ops + 1) / inner_ops per CAP-0015
        let resource_fee = 50000i64;
        let envelope = create_soroban_envelope_with_existing_data(resource_fee);

        let provider = MockStellarProviderTrait::new();

        // inner_fee = 100 + 50000 = 50100 (for 1 Soroban operation)
        // CAP-0015: fee_bump_num_ops = 1 + 1 = 2
        // required_fee = 50100 * 2 / 1 = 100200
        // Plugin sends max_fee based on inner fee calculation (doesn't know about +1)
        let max_fee = 50100i64; // Same as inner_fee

        let result = calculate_fee_bump_required_fee(&envelope, max_fee, &provider).await;

        // Should succeed with the properly scaled fee, not error
        assert!(result.is_ok());
        // Fee should be doubled for 1-op Soroban tx
        assert_eq!(result.unwrap(), 100200u32);
    }

    #[tokio::test]
    async fn test_calculate_fee_bump_falls_back_to_max_fee_without_soroban_data() {
        // Create a transaction envelope without SorobanTransactionData (V0 ext)
        let envelope = create_envelope_without_soroban_data();

        let provider = MockStellarProviderTrait::new();

        // No soroban data, no simulation needed.
        // max_fee is scaled by fee_bump_num_ops / inner_num_ops.
        // The envelope has 0 ops but we clamp to 1, so fee_bump_num_ops = 2.
        // required = 100000 * 2 / 1 = 200000
        let max_fee = 100000i64;
        let result = calculate_fee_bump_required_fee(&envelope, max_fee, &provider).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 200000u32);
    }

    #[test]
    fn test_scale_fee_for_fee_bump_uses_ceil_division() {
        // ceil(101 * 3 / 2) = ceil(303 / 2) = 152
        let scaled = scale_fee_for_fee_bump(101, 2, 3).unwrap();
        assert_eq!(scaled, 152);
    }

    #[test]
    fn test_scale_fee_for_fee_bump_rejects_invalid_inputs() {
        let err = scale_fee_for_fee_bump(-1, 1, 2).unwrap_err();
        assert!(matches!(err, TransactionError::ValidationError(_)));

        let err = scale_fee_for_fee_bump(100, 0, 1).unwrap_err();
        assert!(matches!(err, TransactionError::ValidationError(_)));
    }
}
