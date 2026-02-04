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
/// * `provider` - The Stellar provider for simulation
/// * `stellar_data` - The transaction data containing the XDR and signed auth entry
pub async fn process_soroban_gas_abstraction<C, P>(
    counter_service: &C,
    relayer_id: &str,
    relayer_address: &str,
    provider: &P,
    mut stellar_data: StellarTransactionData,
) -> Result<StellarTransactionData, TransactionError>
where
    C: TransactionCounterTrait + Send + Sync,
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
/// Uses saturating arithmetic to prevent silent truncation if scaled values exceed u32::MAX.
fn apply_resource_buffer(resources: &mut SorobanResources) {
    const BUFFER_MULTIPLIER: u64 = 115;

    let scale = |value: u32| -> u32 {
        ((value as u64).saturating_mul(BUFFER_MULTIPLIER) / 100).min(u32::MAX as u64) as u32
    };

    resources.instructions = scale(resources.instructions);
    resources.disk_read_bytes = scale(resources.disk_read_bytes);
    resources.write_bytes = scale(resources.write_bytes);
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
    use soroban_rs::xdr::{
        ContractId, FeeBumpTransaction, FeeBumpTransactionEnvelope, FeeBumpTransactionExt,
        FeeBumpTransactionInnerTx, Hash, HostFunction, InvokeContractArgs, InvokeHostFunctionOp,
        Memo, MuxedAccount, Preconditions, ScAddress, ScSymbol, ScVal, SequenceNumber,
        SorobanAddressCredentials, SorobanAuthorizedFunction, SorobanAuthorizedInvocation,
        Transaction, TransactionExt, TransactionV0, TransactionV0Envelope, TransactionV0Ext,
        TransactionV1Envelope, Uint256, VecM,
    };

    // Helper to create a minimal V1 transaction envelope
    fn create_minimal_v1_envelope() -> TransactionEnvelope {
        let tx = Transaction {
            source_account: MuxedAccount::Ed25519(Uint256([0u8; 32])),
            fee: 100,
            seq_num: SequenceNumber(0),
            cond: Preconditions::None,
            memo: Memo::None,
            operations: VecM::default(),
            ext: TransactionExt::V0,
        };

        TransactionEnvelope::Tx(TransactionV1Envelope {
            tx,
            signatures: VecM::default(),
        })
    }

    // Helper to create a V0 transaction envelope
    fn create_v0_envelope() -> TransactionEnvelope {
        let tx = TransactionV0 {
            source_account_ed25519: Uint256([0u8; 32]),
            fee: 100,
            seq_num: SequenceNumber(0),
            time_bounds: None,
            memo: Memo::None,
            operations: VecM::default(),
            ext: TransactionV0Ext::V0,
        };

        TransactionEnvelope::TxV0(TransactionV0Envelope {
            tx,
            signatures: VecM::default(),
        })
    }

    // Helper to create a fee bump transaction envelope
    fn create_fee_bump_envelope() -> TransactionEnvelope {
        let inner_tx = Transaction {
            source_account: MuxedAccount::Ed25519(Uint256([0u8; 32])),
            fee: 100,
            seq_num: SequenceNumber(0),
            cond: Preconditions::None,
            memo: Memo::None,
            operations: VecM::default(),
            ext: TransactionExt::V0,
        };

        let inner_envelope = TransactionV1Envelope {
            tx: inner_tx,
            signatures: VecM::default(),
        };

        let fee_bump_tx = FeeBumpTransaction {
            fee_source: MuxedAccount::Ed25519(Uint256([1u8; 32])),
            fee: 200,
            inner_tx: FeeBumpTransactionInnerTx::Tx(inner_envelope),
            ext: FeeBumpTransactionExt::V0,
        };

        TransactionEnvelope::TxFeeBump(FeeBumpTransactionEnvelope {
            tx: fee_bump_tx,
            signatures: VecM::default(),
        })
    }

    // Helper to create a V1 envelope with an InvokeHostFunction operation
    fn create_invoke_host_function_envelope(
        auth_entries: Vec<SorobanAuthorizationEntry>,
    ) -> TransactionEnvelope {
        let invoke_op = InvokeHostFunctionOp {
            host_function: HostFunction::InvokeContract(InvokeContractArgs {
                contract_address: ScAddress::Contract(ContractId(Hash([0u8; 32]))),
                function_name: ScSymbol("test".try_into().unwrap()),
                args: VecM::default(),
            }),
            auth: auth_entries.try_into().unwrap_or_default(),
        };

        let operation = Operation {
            source_account: None,
            body: OperationBody::InvokeHostFunction(invoke_op),
        };

        let tx = Transaction {
            source_account: MuxedAccount::Ed25519(Uint256([0u8; 32])),
            fee: 100,
            seq_num: SequenceNumber(0),
            cond: Preconditions::None,
            memo: Memo::None,
            operations: vec![operation].try_into().unwrap(),
            ext: TransactionExt::V0,
        };

        TransactionEnvelope::Tx(TransactionV1Envelope {
            tx,
            signatures: VecM::default(),
        })
    }

    // Helper to create a mock SorobanAuthorizationEntry with Address credentials
    fn create_address_auth_entry() -> SorobanAuthorizationEntry {
        SorobanAuthorizationEntry {
            credentials: SorobanCredentials::Address(SorobanAddressCredentials {
                address: ScAddress::Contract(ContractId(Hash([0u8; 32]))),
                nonce: 0,
                signature_expiration_ledger: 100,
                signature: ScVal::Void,
            }),
            root_invocation: SorobanAuthorizedInvocation {
                function: SorobanAuthorizedFunction::ContractFn(InvokeContractArgs {
                    contract_address: ScAddress::Contract(ContractId(Hash([0u8; 32]))),
                    function_name: ScSymbol("test".try_into().unwrap()),
                    args: VecM::default(),
                }),
                sub_invocations: VecM::default(),
            },
        }
    }

    // Helper to create a mock SorobanAuthorizationEntry with SourceAccount credentials
    fn create_source_account_auth_entry() -> SorobanAuthorizationEntry {
        SorobanAuthorizationEntry {
            credentials: SorobanCredentials::SourceAccount,
            root_invocation: SorobanAuthorizedInvocation {
                function: SorobanAuthorizedFunction::ContractFn(InvokeContractArgs {
                    contract_address: ScAddress::Contract(ContractId(Hash([1u8; 32]))),
                    function_name: ScSymbol("relayer_fn".try_into().unwrap()),
                    args: VecM::default(),
                }),
                sub_invocations: VecM::default(),
            },
        }
    }

    // ==================== update_envelope_sequence tests ====================

    #[test]
    fn test_update_envelope_sequence() {
        let mut envelope = create_minimal_v1_envelope();
        update_envelope_sequence(&mut envelope, 12345).unwrap();

        if let TransactionEnvelope::Tx(v1) = &envelope {
            assert_eq!(v1.tx.seq_num.0, 12345);
        } else {
            panic!("Expected Tx envelope");
        }
    }

    #[test]
    fn test_update_envelope_sequence_v0_returns_error() {
        let mut envelope = create_v0_envelope();
        let result = update_envelope_sequence(&mut envelope, 12345);

        assert!(result.is_err());
        match result.unwrap_err() {
            TransactionError::ValidationError(msg) => {
                assert!(msg.contains("V0 transactions are not supported"));
            }
            _ => panic!("Expected ValidationError"),
        }
    }

    #[test]
    fn test_update_envelope_sequence_fee_bump_returns_error() {
        let mut envelope = create_fee_bump_envelope();
        let result = update_envelope_sequence(&mut envelope, 12345);

        assert!(result.is_err());
        match result.unwrap_err() {
            TransactionError::ValidationError(msg) => {
                assert!(msg.contains("Cannot update sequence number on fee bump transaction"));
            }
            _ => panic!("Expected ValidationError"),
        }
    }

    #[test]
    fn test_update_envelope_sequence_zero() {
        let mut envelope = create_minimal_v1_envelope();
        update_envelope_sequence(&mut envelope, 0).unwrap();

        if let TransactionEnvelope::Tx(v1) = &envelope {
            assert_eq!(v1.tx.seq_num.0, 0);
        } else {
            panic!("Expected Tx envelope");
        }
    }

    #[test]
    fn test_update_envelope_sequence_max_value() {
        let mut envelope = create_minimal_v1_envelope();
        update_envelope_sequence(&mut envelope, i64::MAX).unwrap();

        if let TransactionEnvelope::Tx(v1) = &envelope {
            assert_eq!(v1.tx.seq_num.0, i64::MAX);
        } else {
            panic!("Expected Tx envelope");
        }
    }

    // ==================== apply_resource_buffer tests ====================

    #[test]
    fn test_apply_resource_buffer_standard_values() {
        let mut resources = SorobanResources {
            footprint: soroban_rs::xdr::LedgerFootprint {
                read_only: VecM::default(),
                read_write: VecM::default(),
            },
            instructions: 1000,
            disk_read_bytes: 2000,
            write_bytes: 500,
        };

        apply_resource_buffer(&mut resources);

        // 15% buffer: value * 115 / 100
        assert_eq!(resources.instructions, 1150);
        assert_eq!(resources.disk_read_bytes, 2300);
        assert_eq!(resources.write_bytes, 575);
    }

    #[test]
    fn test_apply_resource_buffer_zero_values() {
        let mut resources = SorobanResources {
            footprint: soroban_rs::xdr::LedgerFootprint {
                read_only: VecM::default(),
                read_write: VecM::default(),
            },
            instructions: 0,
            disk_read_bytes: 0,
            write_bytes: 0,
        };

        apply_resource_buffer(&mut resources);

        assert_eq!(resources.instructions, 0);
        assert_eq!(resources.disk_read_bytes, 0);
        assert_eq!(resources.write_bytes, 0);
    }

    #[test]
    fn test_apply_resource_buffer_large_values_no_overflow() {
        let large_value = u32::MAX - 1000;
        let mut resources = SorobanResources {
            footprint: soroban_rs::xdr::LedgerFootprint {
                read_only: VecM::default(),
                read_write: VecM::default(),
            },
            instructions: large_value,
            disk_read_bytes: large_value,
            write_bytes: large_value,
        };

        apply_resource_buffer(&mut resources);

        // Should saturate at u32::MAX, not overflow
        assert!(resources.instructions <= u32::MAX);
        assert!(resources.disk_read_bytes <= u32::MAX);
        assert!(resources.write_bytes <= u32::MAX);
    }

    #[test]
    fn test_apply_resource_buffer_max_value_saturates() {
        let mut resources = SorobanResources {
            footprint: soroban_rs::xdr::LedgerFootprint {
                read_only: VecM::default(),
                read_write: VecM::default(),
            },
            instructions: u32::MAX,
            disk_read_bytes: u32::MAX,
            write_bytes: u32::MAX,
        };

        apply_resource_buffer(&mut resources);

        // Should saturate at u32::MAX
        assert_eq!(resources.instructions, u32::MAX);
        assert_eq!(resources.disk_read_bytes, u32::MAX);
        assert_eq!(resources.write_bytes, u32::MAX);
    }

    #[test]
    fn test_apply_resource_buffer_preserves_footprint() {
        use soroban_rs::xdr::{LedgerFootprint, LedgerKey, LedgerKeyAccount};

        let account_key = LedgerKey::Account(LedgerKeyAccount {
            account_id: soroban_rs::xdr::AccountId(
                soroban_rs::xdr::PublicKey::PublicKeyTypeEd25519(Uint256([0u8; 32])),
            ),
        });

        let mut resources = SorobanResources {
            footprint: LedgerFootprint {
                read_only: vec![account_key.clone()].try_into().unwrap(),
                read_write: VecM::default(),
            },
            instructions: 1000,
            disk_read_bytes: 1000,
            write_bytes: 1000,
        };

        apply_resource_buffer(&mut resources);

        // Footprint should be unchanged
        assert_eq!(resources.footprint.read_only.len(), 1);
    }

    // ==================== inject_auth_entries_into_envelope tests ====================

    #[test]
    fn test_inject_auth_entries_v0_envelope_returns_error() {
        let mut envelope = create_v0_envelope();
        let signed_user_auth = create_address_auth_entry();

        let result = inject_auth_entries_into_envelope(&mut envelope, signed_user_auth);

        assert!(result.is_err());
        match result.unwrap_err() {
            TransactionError::ValidationError(msg) => {
                assert!(msg.contains("V0 transactions are not supported for Soroban"));
            }
            _ => panic!("Expected ValidationError"),
        }
    }

    #[test]
    fn test_inject_auth_entries_fee_bump_returns_error() {
        let mut envelope = create_fee_bump_envelope();
        let signed_user_auth = create_address_auth_entry();

        let result = inject_auth_entries_into_envelope(&mut envelope, signed_user_auth);

        assert!(result.is_err());
        match result.unwrap_err() {
            TransactionError::ValidationError(msg) => {
                assert!(msg.contains("Fee bump transactions should not be used"));
            }
            _ => panic!("Expected ValidationError"),
        }
    }

    #[test]
    fn test_inject_auth_entries_no_operations_returns_error() {
        let mut envelope = create_minimal_v1_envelope();
        let signed_user_auth = create_address_auth_entry();

        let result = inject_auth_entries_into_envelope(&mut envelope, signed_user_auth);

        assert!(result.is_err());
        match result.unwrap_err() {
            TransactionError::ValidationError(msg) => {
                assert!(msg.contains("Transaction has no operations"));
            }
            _ => panic!("Expected ValidationError"),
        }
    }

    #[test]
    fn test_inject_auth_entries_non_invoke_host_function_returns_error() {
        use soroban_rs::xdr::{Asset, PaymentOp};

        // Create envelope with a Payment operation (not InvokeHostFunction)
        let payment_op = Operation {
            source_account: None,
            body: OperationBody::Payment(PaymentOp {
                destination: MuxedAccount::Ed25519(Uint256([0u8; 32])),
                asset: Asset::Native,
                amount: 1000,
            }),
        };

        let tx = Transaction {
            source_account: MuxedAccount::Ed25519(Uint256([0u8; 32])),
            fee: 100,
            seq_num: SequenceNumber(0),
            cond: Preconditions::None,
            memo: Memo::None,
            operations: vec![payment_op].try_into().unwrap(),
            ext: TransactionExt::V0,
        };

        let mut envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
            tx,
            signatures: VecM::default(),
        });

        let signed_user_auth = create_address_auth_entry();
        let result = inject_auth_entries_into_envelope(&mut envelope, signed_user_auth);

        assert!(result.is_err());
        match result.unwrap_err() {
            TransactionError::ValidationError(msg) => {
                assert!(msg.contains("First operation is not InvokeHostFunction"));
            }
            _ => panic!("Expected ValidationError"),
        }
    }

    #[test]
    fn test_inject_auth_entries_empty_auth_adds_user_entry() {
        let mut envelope = create_invoke_host_function_envelope(vec![]);
        let signed_user_auth = create_address_auth_entry();

        let result = inject_auth_entries_into_envelope(&mut envelope, signed_user_auth.clone());

        assert!(result.is_ok());
        let auth_entries = result.unwrap();
        assert_eq!(auth_entries.len(), 1);
    }

    #[test]
    fn test_inject_auth_entries_replaces_first_entry() {
        let original_auth = create_address_auth_entry();
        let mut envelope = create_invoke_host_function_envelope(vec![original_auth]);

        let signed_user_auth = create_address_auth_entry();
        let result = inject_auth_entries_into_envelope(&mut envelope, signed_user_auth);

        assert!(result.is_ok());
        let auth_entries = result.unwrap();
        assert_eq!(auth_entries.len(), 1);
    }

    #[test]
    fn test_inject_auth_entries_converts_relayer_to_source_account() {
        let user_auth = create_address_auth_entry();
        let relayer_auth = create_address_auth_entry();
        let mut envelope = create_invoke_host_function_envelope(vec![user_auth, relayer_auth]);

        let signed_user_auth = create_address_auth_entry();
        let result = inject_auth_entries_into_envelope(&mut envelope, signed_user_auth);

        assert!(result.is_ok());
        let auth_entries = result.unwrap();
        assert_eq!(auth_entries.len(), 2);

        // Second entry should have SourceAccount credentials
        match &auth_entries[1].credentials {
            SorobanCredentials::SourceAccount => {} // expected
            _ => panic!("Expected SourceAccount credentials for relayer auth entry"),
        }
    }

    // ==================== build_simulation_envelope tests ====================

    #[test]
    fn test_build_simulation_envelope_v0_returns_error() {
        let envelope = create_v0_envelope();
        let auth_entries = vec![create_address_auth_entry()];

        let result = build_simulation_envelope(&envelope, &auth_entries);

        assert!(result.is_err());
        match result.unwrap_err() {
            TransactionError::ValidationError(msg) => {
                assert!(msg.contains("Expected V1 transaction envelope"));
            }
            _ => panic!("Expected ValidationError"),
        }
    }

    #[test]
    fn test_build_simulation_envelope_fee_bump_returns_error() {
        let envelope = create_fee_bump_envelope();
        let auth_entries = vec![create_address_auth_entry()];

        let result = build_simulation_envelope(&envelope, &auth_entries);

        assert!(result.is_err());
        match result.unwrap_err() {
            TransactionError::ValidationError(msg) => {
                assert!(msg.contains("Expected V1 transaction envelope"));
            }
            _ => panic!("Expected ValidationError"),
        }
    }

    #[test]
    fn test_build_simulation_envelope_no_operations_returns_error() {
        let envelope = create_minimal_v1_envelope();
        let auth_entries = vec![create_address_auth_entry()];

        let result = build_simulation_envelope(&envelope, &auth_entries);

        assert!(result.is_err());
        match result.unwrap_err() {
            TransactionError::ValidationError(msg) => {
                assert!(msg.contains("Transaction has no operations"));
            }
            _ => panic!("Expected ValidationError"),
        }
    }

    #[test]
    fn test_build_simulation_envelope_non_invoke_host_function_returns_error() {
        use soroban_rs::xdr::{Asset, PaymentOp};

        let payment_op = Operation {
            source_account: None,
            body: OperationBody::Payment(PaymentOp {
                destination: MuxedAccount::Ed25519(Uint256([0u8; 32])),
                asset: Asset::Native,
                amount: 1000,
            }),
        };

        let tx = Transaction {
            source_account: MuxedAccount::Ed25519(Uint256([0u8; 32])),
            fee: 100,
            seq_num: SequenceNumber(0),
            cond: Preconditions::None,
            memo: Memo::None,
            operations: vec![payment_op].try_into().unwrap(),
            ext: TransactionExt::V0,
        };

        let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
            tx,
            signatures: VecM::default(),
        });

        let auth_entries = vec![create_address_auth_entry()];
        let result = build_simulation_envelope(&envelope, &auth_entries);

        assert!(result.is_err());
        match result.unwrap_err() {
            TransactionError::ValidationError(msg) => {
                assert!(msg.contains("First operation is not InvokeHostFunction"));
            }
            _ => panic!("Expected ValidationError"),
        }
    }

    #[test]
    fn test_build_simulation_envelope_success() {
        let original_auth = create_address_auth_entry();
        let envelope = create_invoke_host_function_envelope(vec![original_auth]);

        let new_auth = create_source_account_auth_entry();
        let result = build_simulation_envelope(&envelope, &vec![new_auth]);

        assert!(result.is_ok());

        // Verify the simulation envelope has the new auth entries
        let sim_envelope = result.unwrap();
        if let TransactionEnvelope::Tx(v1) = sim_envelope {
            assert_eq!(v1.signatures.len(), 0); // Simulation envelope should have no signatures
            assert_eq!(v1.tx.operations.len(), 1);
        } else {
            panic!("Expected Tx envelope");
        }
    }

    #[test]
    fn test_build_simulation_envelope_preserves_transaction_fields() {
        let original_auth = create_address_auth_entry();
        let mut envelope = create_invoke_host_function_envelope(vec![original_auth]);

        // Modify some fields to verify they're preserved
        if let TransactionEnvelope::Tx(ref mut v1) = envelope {
            v1.tx.fee = 500;
            v1.tx.seq_num = SequenceNumber(42);
        }

        let new_auth = create_source_account_auth_entry();
        let result = build_simulation_envelope(&envelope, &vec![new_auth]);

        assert!(result.is_ok());
        let sim_envelope = result.unwrap();

        if let TransactionEnvelope::Tx(v1) = sim_envelope {
            assert_eq!(v1.tx.fee, 500);
            assert_eq!(v1.tx.seq_num.0, 42);
        } else {
            panic!("Expected Tx envelope");
        }
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::models::TransactionInput;
    use crate::repositories::MockTransactionCounterTrait;
    use crate::services::provider::MockStellarProviderTrait;
    use soroban_rs::stellar_rpc_client::SimulateTransactionResponse;
    use soroban_rs::xdr::{
        ContractId, Hash, HostFunction, InvokeContractArgs, InvokeHostFunctionOp, Memo,
        MuxedAccount, Operation, Preconditions, ScAddress, ScSymbol, ScVal, SequenceNumber,
        SorobanAddressCredentials, SorobanAuthorizationEntry, SorobanAuthorizedFunction,
        SorobanAuthorizedInvocation, SorobanCredentials, SorobanTransactionData, Transaction,
        TransactionExt, TransactionV1Envelope, Uint256, VecM,
    };
    use std::future::ready;

    fn create_gas_abstraction_envelope() -> TransactionEnvelope {
        let user_auth = SorobanAuthorizationEntry {
            credentials: SorobanCredentials::Address(SorobanAddressCredentials {
                address: ScAddress::Contract(ContractId(Hash([1u8; 32]))),
                nonce: 12345,
                signature_expiration_ledger: 1000,
                signature: ScVal::Void,
            }),
            root_invocation: SorobanAuthorizedInvocation {
                function: SorobanAuthorizedFunction::ContractFn(InvokeContractArgs {
                    contract_address: ScAddress::Contract(ContractId(Hash([0u8; 32]))),
                    function_name: ScSymbol("forward".try_into().unwrap()),
                    args: VecM::default(),
                }),
                sub_invocations: VecM::default(),
            },
        };

        let relayer_auth = SorobanAuthorizationEntry {
            credentials: SorobanCredentials::Address(SorobanAddressCredentials {
                address: ScAddress::Contract(ContractId(Hash([2u8; 32]))),
                nonce: 67890,
                signature_expiration_ledger: 1000,
                signature: ScVal::Void,
            }),
            root_invocation: SorobanAuthorizedInvocation {
                function: SorobanAuthorizedFunction::ContractFn(InvokeContractArgs {
                    contract_address: ScAddress::Contract(ContractId(Hash([0u8; 32]))),
                    function_name: ScSymbol("collect".try_into().unwrap()),
                    args: VecM::default(),
                }),
                sub_invocations: VecM::default(),
            },
        };

        let invoke_op = InvokeHostFunctionOp {
            host_function: HostFunction::InvokeContract(InvokeContractArgs {
                contract_address: ScAddress::Contract(ContractId(Hash([0u8; 32]))),
                function_name: ScSymbol("forward".try_into().unwrap()),
                args: VecM::default(),
            }),
            auth: vec![user_auth, relayer_auth].try_into().unwrap(),
        };

        let operation = Operation {
            source_account: None,
            body: OperationBody::InvokeHostFunction(invoke_op),
        };

        let tx = Transaction {
            source_account: MuxedAccount::Ed25519(Uint256([0u8; 32])),
            fee: 100,
            seq_num: SequenceNumber(0),
            cond: Preconditions::None,
            memo: Memo::None,
            operations: vec![operation].try_into().unwrap(),
            ext: TransactionExt::V0,
        };

        TransactionEnvelope::Tx(TransactionV1Envelope {
            tx,
            signatures: VecM::default(),
        })
    }

    fn create_valid_soroban_tx_data_xdr() -> String {
        use soroban_rs::xdr::SorobanTransactionDataExt;

        let tx_data = SorobanTransactionData {
            ext: SorobanTransactionDataExt::V0,
            resources: soroban_rs::xdr::SorobanResources {
                footprint: soroban_rs::xdr::LedgerFootprint {
                    read_only: VecM::default(),
                    read_write: VecM::default(),
                },
                instructions: 1000,
                disk_read_bytes: 500,
                write_bytes: 200,
            },
            resource_fee: 100,
        };
        tx_data.to_xdr_base64(Limits::none()).unwrap()
    }

    #[tokio::test]
    async fn test_process_soroban_gas_abstraction_invalid_input_type() {
        let counter = MockTransactionCounterTrait::new();
        let provider = MockStellarProviderTrait::new();

        let stellar_data = StellarTransactionData {
            source_account: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string(),
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            fee: Some(100),
            sequence_number: None,
            transaction_input: TransactionInput::Operations(vec![]), // Wrong type
            memo: None,
            valid_until: None,
            signatures: vec![],
            hash: None,
            simulation_transaction_data: None,
            signed_envelope_xdr: None,
            transaction_result_xdr: None,
        };

        let result = process_soroban_gas_abstraction(
            &counter,
            "relayer-1",
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
            &provider,
            stellar_data,
        )
        .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            TransactionError::ValidationError(msg) => {
                assert!(msg.contains("Expected SorobanGasAbstraction"));
            }
            _ => panic!("Expected ValidationError"),
        }
    }

    #[tokio::test]
    async fn test_process_soroban_gas_abstraction_invalid_xdr() {
        let counter = MockTransactionCounterTrait::new();
        let provider = MockStellarProviderTrait::new();

        let stellar_data = StellarTransactionData {
            source_account: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string(),
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            fee: Some(100),
            sequence_number: None,
            transaction_input: TransactionInput::SorobanGasAbstraction {
                xdr: "invalid-xdr".to_string(),
                signed_auth_entry: "also-invalid".to_string(),
            },
            memo: None,
            valid_until: None,
            signatures: vec![],
            hash: None,
            simulation_transaction_data: None,
            signed_envelope_xdr: None,
            transaction_result_xdr: None,
        };

        let result = process_soroban_gas_abstraction(
            &counter,
            "relayer-1",
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
            &provider,
            stellar_data,
        )
        .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            TransactionError::ValidationError(msg) => {
                assert!(msg.contains("Failed to parse transaction XDR"));
            }
            _ => panic!("Expected ValidationError"),
        }
    }

    #[tokio::test]
    async fn test_process_soroban_gas_abstraction_invalid_auth_entry() {
        let counter = MockTransactionCounterTrait::new();
        let provider = MockStellarProviderTrait::new();

        let envelope = create_gas_abstraction_envelope();
        let xdr = envelope.to_xdr_base64(Limits::none()).unwrap();

        let stellar_data = StellarTransactionData {
            source_account: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string(),
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            fee: Some(100),
            sequence_number: None,
            transaction_input: TransactionInput::SorobanGasAbstraction {
                xdr,
                signed_auth_entry: "invalid-auth-entry".to_string(),
            },
            memo: None,
            valid_until: None,
            signatures: vec![],
            hash: None,
            simulation_transaction_data: None,
            signed_envelope_xdr: None,
            transaction_result_xdr: None,
        };

        let result = process_soroban_gas_abstraction(
            &counter,
            "relayer-1",
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
            &provider,
            stellar_data,
        )
        .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            TransactionError::ValidationError(msg) => {
                assert!(
                    msg.contains("Failed to deserialize") || msg.contains("XdrError"),
                    "Unexpected error message: {}",
                    msg
                );
            }
            other => panic!("Expected ValidationError, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_process_soroban_gas_abstraction_simulation_error() {
        let mut counter = MockTransactionCounterTrait::new();
        counter
            .expect_get_and_increment()
            .returning(|_, _| Box::pin(ready(Ok(42u64))));

        let mut provider = MockStellarProviderTrait::new();
        provider
            .expect_simulate_transaction_envelope()
            .returning(|_| {
                Box::pin(ready(Ok(SimulateTransactionResponse {
                    error: Some("Simulation failed: insufficient resources".to_string()),
                    min_resource_fee: 0,
                    transaction_data: String::new(),
                    ..Default::default()
                })))
            });

        let envelope = create_gas_abstraction_envelope();
        let xdr = envelope.to_xdr_base64(Limits::none()).unwrap();

        // Create a valid signed auth entry
        let signed_auth = SorobanAuthorizationEntry {
            credentials: SorobanCredentials::Address(SorobanAddressCredentials {
                address: ScAddress::Contract(ContractId(Hash([1u8; 32]))),
                nonce: 12345,
                signature_expiration_ledger: 1000,
                signature: ScVal::Void,
            }),
            root_invocation: SorobanAuthorizedInvocation {
                function: SorobanAuthorizedFunction::ContractFn(InvokeContractArgs {
                    contract_address: ScAddress::Contract(ContractId(Hash([0u8; 32]))),
                    function_name: ScSymbol("forward".try_into().unwrap()),
                    args: VecM::default(),
                }),
                sub_invocations: VecM::default(),
            },
        };
        let signed_auth_xdr = signed_auth.to_xdr_base64(Limits::none()).unwrap();

        let stellar_data = StellarTransactionData {
            source_account: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string(),
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            fee: Some(100),
            sequence_number: None,
            transaction_input: TransactionInput::SorobanGasAbstraction {
                xdr,
                signed_auth_entry: signed_auth_xdr,
            },
            memo: None,
            valid_until: None,
            signatures: vec![],
            hash: None,
            simulation_transaction_data: None,
            signed_envelope_xdr: None,
            transaction_result_xdr: None,
        };

        let result = process_soroban_gas_abstraction(
            &counter,
            "relayer-1",
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
            &provider,
            stellar_data,
        )
        .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            TransactionError::UnexpectedError(msg) => {
                assert!(msg.contains("Simulation failed"));
            }
            other => panic!("Expected UnexpectedError, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_process_soroban_gas_abstraction_success() {
        let mut counter = MockTransactionCounterTrait::new();
        counter
            .expect_get_and_increment()
            .returning(|_, _| Box::pin(ready(Ok(42u64))));

        let mut provider = MockStellarProviderTrait::new();
        provider
            .expect_simulate_transaction_envelope()
            .returning(|_| {
                Box::pin(ready(Ok(SimulateTransactionResponse {
                    error: None,
                    min_resource_fee: 1000,
                    transaction_data: create_valid_soroban_tx_data_xdr(),
                    ..Default::default()
                })))
            });

        let envelope = create_gas_abstraction_envelope();
        let xdr = envelope.to_xdr_base64(Limits::none()).unwrap();

        let signed_auth = SorobanAuthorizationEntry {
            credentials: SorobanCredentials::Address(SorobanAddressCredentials {
                address: ScAddress::Contract(ContractId(Hash([1u8; 32]))),
                nonce: 12345,
                signature_expiration_ledger: 1000,
                signature: ScVal::Void,
            }),
            root_invocation: SorobanAuthorizedInvocation {
                function: SorobanAuthorizedFunction::ContractFn(InvokeContractArgs {
                    contract_address: ScAddress::Contract(ContractId(Hash([0u8; 32]))),
                    function_name: ScSymbol("forward".try_into().unwrap()),
                    args: VecM::default(),
                }),
                sub_invocations: VecM::default(),
            },
        };
        let signed_auth_xdr = signed_auth.to_xdr_base64(Limits::none()).unwrap();

        let stellar_data = StellarTransactionData {
            source_account: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string(),
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            fee: Some(100),
            sequence_number: None,
            transaction_input: TransactionInput::SorobanGasAbstraction {
                xdr,
                signed_auth_entry: signed_auth_xdr,
            },
            memo: None,
            valid_until: None,
            signatures: vec![],
            hash: None,
            simulation_transaction_data: None,
            signed_envelope_xdr: None,
            transaction_result_xdr: None,
        };

        let result = process_soroban_gas_abstraction(
            &counter,
            "relayer-1",
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
            &provider,
            stellar_data,
        )
        .await;

        assert!(result.is_ok());
        let prepared = result.unwrap();

        assert_eq!(prepared.sequence_number, Some(42));
        match prepared.transaction_input {
            TransactionInput::UnsignedXdr(_) => {} // Expected
            _ => panic!("Expected UnsignedXdr transaction input"),
        }
    }
}
