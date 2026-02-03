//! This module handles the preparation of Soroban gas abstraction transactions.
//! These are transactions where the user pays fees in tokens via the FeeForwarder contract.
//! The user signs an authorization entry, which is injected into the transaction before submission.
//! The relayer also signs its own authorization entry for the FeeForwarder contract.

use soroban_rs::xdr::{
    InvokeHostFunctionOp, Limits, Operation, OperationBody, ReadXdr, SorobanAuthorizationEntry,
    SorobanCredentials, SorobanResources, SorobanTransactionData, TransactionEnvelope,
    TransactionExt, WriteXdr,
};
use tracing::{debug, info};

use crate::{
    models::{StellarTransactionData, TransactionError, TransactionInput},
    repositories::TransactionCounterTrait,
    services::{
        provider::{StellarProvider, StellarProviderTrait},
        signer::StellarSignTrait,
        stellar_fee_forwarder::{FeeForwarderError, FeeForwarderService},
    },
};

use super::common::get_next_sequence;

/// Process a Soroban gas abstraction transaction.
///
/// This function:
/// 1. Parses the FeeForwarder transaction XDR
/// 2. Deserializes the user's signed authorization entry
/// 3. Signs the relayer's authorization entry using the provided signer
/// 4. Injects both signed auth entries into the transaction
/// 5. Re-simulates with signed auth entries to get accurate footprint
/// 6. Updates the transaction's sorobanData with accurate resources
/// 7. Updates the sequence number
/// 8. Returns the prepared transaction data for signing
///
/// # Arguments
///
/// * `counter_service` - Service for managing sequence numbers
/// * `relayer_id` - The relayer's ID for sequence tracking
/// * `relayer_address` - The relayer's Stellar address (source account for the transaction)
/// * `_signer` - Unused, kept for API compatibility
/// * `provider` - The Stellar provider for simulation
/// * `stellar_data` - The transaction data containing the XDR and signed auth entry
pub async fn process_soroban_gas_abstraction<C, S, P>(
    counter_service: &C,
    relayer_id: &str,
    relayer_address: &str,
    _signer: &S,
    provider: &P,
    mut stellar_data: StellarTransactionData,
) -> Result<StellarTransactionData, TransactionError>
where
    C: TransactionCounterTrait + Send + Sync,
    S: StellarSignTrait + Send + Sync,
    P: StellarProviderTrait + Send + Sync,
{
    // Extract XDR and signed auth entry from transaction input
    let (xdr, signed_auth_entry_xdr) = match &stellar_data.transaction_input {
        TransactionInput::SorobanGasAbstraction {
            xdr,
            signed_auth_entry,
        } => (xdr.clone(), signed_auth_entry.clone()),
        _ => {
            return Err(TransactionError::ValidationError(
                "Expected SorobanGasAbstraction transaction input".to_string(),
            ));
        }
    };

    debug!(
        "Processing Soroban gas abstraction: xdr_len={}, auth_entry_len={}",
        xdr.len(),
        signed_auth_entry_xdr.len()
    );

    // Parse the transaction envelope
    let mut envelope = TransactionEnvelope::from_xdr_base64(&xdr, Limits::none()).map_err(|e| {
        TransactionError::ValidationError(format!("Failed to parse transaction XDR: {e}"))
    })?;

    // Deserialize the user's signed authorization entry
    let signed_user_auth =
        FeeForwarderService::<StellarProvider>::deserialize_auth_entry(&signed_auth_entry_xdr)
            .map_err(|e| match e {
                FeeForwarderError::XdrError(msg) => TransactionError::ValidationError(msg),
                _ => TransactionError::ValidationError(format!(
                    "Failed to deserialize signed auth entry: {e}"
                )),
            })?;

    // Inject the user's signed auth entry and convert relayer's auth to SourceAccount
    let signed_auth_entries = inject_auth_entries_into_envelope(&mut envelope, signed_user_auth)?;

    // Re-simulate with signed auth entries to get accurate footprint.
    //
    // According to Soroban flow, after signing auth entries you must re-simulate:
    // 1. Simulation validates the signatures
    // 2. Calculates ledger resources accurately
    // 3. The footprint will include all accounts accessed via require_auth/require_auth_for_args
    // 4. Returns a fully-resourced transaction ready for submission
    //
    // The signed auth entries are used directly - this ensures the simulation executes
    // the full auth verification code path and captures the correct footprint.
    simulate_and_update_resources(&mut envelope, &signed_auth_entries, provider).await?;

    // Get the next sequence number for the relayer
    let sequence_number = get_next_sequence(counter_service, relayer_id, relayer_address).await?;

    // Update the sequence number in the envelope
    update_envelope_sequence(&mut envelope, sequence_number)?;

    // Serialize the updated envelope back to XDR
    let updated_xdr = envelope.to_xdr_base64(Limits::none()).map_err(|e| {
        TransactionError::UnexpectedError(format!("Failed to serialize updated envelope: {e}"))
    })?;

    // Update the transaction data with the new XDR and sequence number
    stellar_data.sequence_number = Some(sequence_number);
    stellar_data.transaction_input = TransactionInput::UnsignedXdr(updated_xdr);

    debug!(
        "Soroban gas abstraction prepared: sequence={}",
        sequence_number
    );

    Ok(stellar_data)
}

/// Inject signed authorization entries into the transaction envelope.
///
/// For FeeForwarder transactions, there are two auth entries:
/// 1. User's auth entry (first) - already signed by the user
/// 2. Relayer's auth entry (second) - uses SourceAccount credentials (no separate signature needed)
///
/// This function:
/// - Replaces the first auth entry with the user's signed version
/// - Converts the relayer's auth entry to use SourceAccount credentials
/// - Returns the auth entries for use in simulation
fn inject_auth_entries_into_envelope(
    envelope: &mut TransactionEnvelope,
    signed_user_auth: SorobanAuthorizationEntry,
) -> Result<Vec<SorobanAuthorizationEntry>, TransactionError> {
    let tx = match envelope {
        TransactionEnvelope::Tx(v1) => &mut v1.tx,
        TransactionEnvelope::TxV0(_) => {
            return Err(TransactionError::ValidationError(
                "V0 transactions are not supported for Soroban".to_string(),
            ));
        }
        TransactionEnvelope::TxFeeBump(_) => {
            return Err(TransactionError::ValidationError(
                "Fee bump transactions should not be used for Soroban gas abstraction".to_string(),
            ));
        }
    };

    // Get the first operation (should be InvokeHostFunction for FeeForwarder)
    let operations: Vec<_> = tx.operations.to_vec();
    if operations.is_empty() {
        return Err(TransactionError::ValidationError(
            "Transaction has no operations".to_string(),
        ));
    }

    let first_op = &operations[0];
    let invoke_op = match &first_op.body {
        OperationBody::InvokeHostFunction(invoke) => invoke.clone(),
        _ => {
            return Err(TransactionError::ValidationError(
                "First operation is not InvokeHostFunction".to_string(),
            ));
        }
    };

    // The auth entries should have user's auth entry as the first entry, relayer's as second
    let mut auth_entries: Vec<SorobanAuthorizationEntry> = invoke_op.auth.to_vec();

    if auth_entries.is_empty() {
        // If there are no auth entries, just add the user's signed one
        auth_entries.push(signed_user_auth);
    } else {
        // Replace the first auth entry (user's) with the signed version
        auth_entries[0] = signed_user_auth;

        // Convert the relayer's auth entry (second entry) to use SourceAccount credentials.
        // Since the relayer is the transaction source account, the transaction signature
        // already authorizes this entry - no separate auth entry signature is needed.
        if auth_entries.len() > 1 {
            let relayer_auth = &auth_entries[1];
            let source_account_auth = SorobanAuthorizationEntry {
                credentials: SorobanCredentials::SourceAccount,
                root_invocation: relayer_auth.root_invocation.clone(),
            };
            auth_entries[1] = source_account_auth;
            debug!("Converted relayer auth entry to SourceAccount credentials");
        }
    }

    // Clone auth_entries before consuming them in try_into (we need to return them)
    let result_auth_entries = auth_entries.clone();

    // Create the updated InvokeHostFunction operation
    let updated_invoke = soroban_rs::xdr::InvokeHostFunctionOp {
        host_function: invoke_op.host_function,
        auth: auth_entries.try_into().map_err(|_| {
            TransactionError::UnexpectedError("Failed to create auth entries vector".to_string())
        })?,
    };

    // Create the updated operation
    let updated_op = soroban_rs::xdr::Operation {
        source_account: first_op.source_account.clone(),
        body: OperationBody::InvokeHostFunction(updated_invoke),
    };

    // Replace the first operation with the updated one
    let mut updated_operations = operations;
    updated_operations[0] = updated_op;

    // Update the transaction's operations
    tx.operations = updated_operations.try_into().map_err(|_| {
        TransactionError::UnexpectedError("Failed to update operations vector".to_string())
    })?;

    debug!("Successfully injected signed auth entries into transaction");

    Ok(result_auth_entries)
}

/// Update the sequence number in a transaction envelope.
fn update_envelope_sequence(
    envelope: &mut TransactionEnvelope,
    sequence: i64,
) -> Result<(), TransactionError> {
    match envelope {
        TransactionEnvelope::Tx(v1) => {
            v1.tx.seq_num = soroban_rs::xdr::SequenceNumber(sequence);
            Ok(())
        }
        TransactionEnvelope::TxV0(_) => Err(TransactionError::ValidationError(
            "V0 transactions are not supported".to_string(),
        )),
        TransactionEnvelope::TxFeeBump(_) => Err(TransactionError::ValidationError(
            "Cannot update sequence number on fee bump transaction".to_string(),
        )),
    }
}

/// Apply a buffer to Soroban resources to account for simulation variance.
///
/// Simulation can be slightly inaccurate due to timing differences or other factors.
/// Adding a 15% buffer prevents "exceeded limit" errors during execution.
fn apply_resource_buffer(resources: &mut SorobanResources) {
    const RESOURCE_BUFFER_PERCENT: u32 = 15;
    const BUFFER_MULTIPLIER: u32 = 100 + RESOURCE_BUFFER_PERCENT; // 115

    resources.instructions =
        (resources.instructions as u64 * BUFFER_MULTIPLIER as u64 / 100) as u32;
    resources.disk_read_bytes =
        (resources.disk_read_bytes as u64 * BUFFER_MULTIPLIER as u64 / 100) as u32;
    resources.write_bytes = (resources.write_bytes as u64 * BUFFER_MULTIPLIER as u64 / 100) as u32;
}

/// Re-simulate the transaction with signed auth entries and update resources.
///
/// This function:
/// 1. Builds a simulation envelope with the actual signed auth entries
/// 2. Simulates to get accurate footprint and resources
/// 3. Updates the original envelope's sorobanData with the accurate values
///
/// Using the actual signed auth entries allows the simulation to:
/// - Verify signatures (they should be valid since values haven't changed)
/// - Execute the full auth verification code path
/// - Capture the correct footprint including accounts accessed via require_auth
async fn simulate_and_update_resources<P>(
    envelope: &mut TransactionEnvelope,
    signed_auth_entries: &[SorobanAuthorizationEntry],
    provider: &P,
) -> Result<(), TransactionError>
where
    P: StellarProviderTrait + Send + Sync,
{
    info!("Re-simulating transaction with signed auth entries for accurate footprint");

    // Use the actual signed auth entries for simulation
    // This allows the simulation to verify signatures and capture the correct footprint
    // including all accounts accessed via require_auth/require_auth_for_args
    let simulation_auth_entries: Vec<SorobanAuthorizationEntry> = signed_auth_entries.to_vec();

    // Build simulation envelope (clone the original and replace auth entries)
    let simulation_envelope = build_simulation_envelope(envelope, &simulation_auth_entries)?;

    // Simulate the transaction
    let sim_response = provider
        .simulate_transaction_envelope(&simulation_envelope)
        .await
        .map_err(|e| {
            TransactionError::UnexpectedError(format!("Failed to simulate transaction: {e}"))
        })?;

    // Check for simulation errors
    if let Some(err) = &sim_response.error {
        return Err(TransactionError::UnexpectedError(format!(
            "Simulation failed: {err}"
        )));
    }

    // Parse the new transaction data from simulation
    let mut new_tx_data =
        SorobanTransactionData::from_xdr_base64(&sim_response.transaction_data, Limits::none())
            .map_err(|e| {
                TransactionError::UnexpectedError(format!(
                    "Failed to parse simulation transaction_data: {e}"
                ))
            })?;

    // Log the resource values from simulation (before buffer)
    info!(
        "Simulation complete: instructions={}, read_bytes={}, write_bytes={}",
        new_tx_data.resources.instructions,
        new_tx_data.resources.disk_read_bytes,
        new_tx_data.resources.write_bytes
    );

    // Apply buffer to resources to account for simulation variance
    apply_resource_buffer(&mut new_tx_data.resources);

    // Log the resource values after buffer
    info!(
        "Resources after buffer: instructions={}, read_bytes={}, write_bytes={}",
        new_tx_data.resources.instructions,
        new_tx_data.resources.disk_read_bytes,
        new_tx_data.resources.write_bytes
    );

    // Update the original envelope's sorobanData with accurate resources
    // Keep the original fee (already calculated and validated at /build time)
    match envelope {
        TransactionEnvelope::Tx(ref mut env) => {
            let original_fee = env.tx.fee;

            // Update the transaction extension with new soroban data
            env.tx.ext = TransactionExt::V1(new_tx_data);

            // Preserve the original fee
            env.tx.fee = original_fee;

            debug!(
                "Updated transaction sorobanData with simulation results, preserved fee={}",
                original_fee
            );
            Ok(())
        }
        _ => Err(TransactionError::ValidationError(
            "Expected V1 transaction envelope".to_string(),
        )),
    }
}

/// Build a simulation envelope with the provided auth entries.
///
/// This creates a copy of the envelope with the specified auth entries.
/// The auth entries should be the actual signed entries to ensure proper
/// signature verification and footprint capture during simulation.
fn build_simulation_envelope(
    original: &TransactionEnvelope,
    simulation_auth_entries: &[SorobanAuthorizationEntry],
) -> Result<TransactionEnvelope, TransactionError> {
    match original {
        TransactionEnvelope::Tx(env) => {
            let mut sim_tx = env.tx.clone();

            // Get the operations and update the auth entries
            let operations: Vec<_> = sim_tx.operations.to_vec();
            if operations.is_empty() {
                return Err(TransactionError::ValidationError(
                    "Transaction has no operations".to_string(),
                ));
            }

            let first_op = &operations[0];
            let invoke_op = match &first_op.body {
                OperationBody::InvokeHostFunction(invoke) => invoke.clone(),
                _ => {
                    return Err(TransactionError::ValidationError(
                        "First operation is not InvokeHostFunction".to_string(),
                    ));
                }
            };

            // Create updated invoke operation with simulation auth entries
            let updated_invoke = InvokeHostFunctionOp {
                host_function: invoke_op.host_function,
                auth: simulation_auth_entries.to_vec().try_into().map_err(|_| {
                    TransactionError::UnexpectedError(
                        "Failed to create simulation auth entries".to_string(),
                    )
                })?,
            };

            let updated_op = Operation {
                source_account: first_op.source_account.clone(),
                body: OperationBody::InvokeHostFunction(updated_invoke),
            };

            let mut updated_operations = operations;
            updated_operations[0] = updated_op;

            sim_tx.operations = updated_operations.try_into().map_err(|_| {
                TransactionError::UnexpectedError("Failed to update operations".to_string())
            })?;

            Ok(TransactionEnvelope::Tx(
                soroban_rs::xdr::TransactionV1Envelope {
                    tx: sim_tx,
                    signatures: Default::default(),
                },
            ))
        }
        _ => Err(TransactionError::ValidationError(
            "Expected V1 transaction envelope".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_rs::xdr::VecM;

    #[test]
    fn test_update_envelope_sequence() {
        use soroban_rs::xdr::{
            Memo, MuxedAccount, Preconditions, SequenceNumber, Transaction, TransactionExt,
            TransactionV1Envelope, Uint256,
        };

        // Create a minimal transaction envelope
        let tx = Transaction {
            source_account: MuxedAccount::Ed25519(Uint256([0u8; 32])),
            fee: 100,
            seq_num: SequenceNumber(0),
            cond: Preconditions::None,
            memo: Memo::None,
            operations: VecM::default(),
            ext: TransactionExt::V0,
        };

        let mut envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
            tx,
            signatures: VecM::default(),
        });

        // Update sequence number
        update_envelope_sequence(&mut envelope, 12345).unwrap();

        // Verify the update
        if let TransactionEnvelope::Tx(v1) = &envelope {
            assert_eq!(v1.tx.seq_num.0, 12345);
        } else {
            panic!("Expected Tx envelope");
        }
    }
}
