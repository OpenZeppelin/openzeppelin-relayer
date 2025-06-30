//! Operations-based transaction preparation logic.

use eyre::Result;
use log::info;

use super::common::{get_next_sequence, sign_stellar_transaction, simulate_if_needed};
use crate::{
    domain::extract_operations,
    models::{StellarTransactionData, TransactionError, TransactionRepoModel},
    repositories::TransactionCounterTrait,
    services::{Signer, StellarProviderTrait},
};

/// Process operations-based transaction.
///
/// This function:
/// 1. Gets the next sequence number for the relayer
/// 2. Updates the stellar data with the sequence number
/// 3. Builds the unsigned envelope from operations
/// 4. Simulates the transaction if needed (for Soroban operations)
/// 5. Signs the transaction envelope
///
/// # Arguments
/// * `counter_service` - Service for managing transaction sequence numbers
/// * `relayer_id` - The relayer's ID
/// * `relayer_address` - The relayer's Stellar address
/// * `tx` - The transaction model to process
/// * `stellar_data` - The stellar-specific transaction data containing operations
/// * `provider` - Provider for Stellar RPC operations
/// * `signer` - Service for signing transactions
///
/// # Returns
/// The updated stellar data with simulation results (if applicable) and signature
pub async fn process_operations<C, P, S>(
    counter_service: &C,
    relayer_id: &str,
    relayer_address: &str,
    tx: &TransactionRepoModel,
    stellar_data: StellarTransactionData,
    provider: &P,
    signer: &S,
) -> Result<StellarTransactionData, TransactionError>
where
    C: TransactionCounterTrait + Send + Sync,
    P: StellarProviderTrait + Send + Sync,
    S: Signer + Send + Sync,
{
    // Get the next sequence number
    let sequence_i64 = get_next_sequence(counter_service, relayer_id, relayer_address)?;

    info!(
        "Using sequence number {} for operations transaction {}",
        sequence_i64, tx.id
    );

    // Update stellar data with sequence
    let stellar_data = stellar_data.with_sequence_number(sequence_i64);

    // Build the unsigned envelope
    let unsigned_env = stellar_data
        .get_envelope_for_simulation()
        .map_err(TransactionError::from)?;

    // Check if simulation is needed and apply results
    let stellar_data_with_sim = match simulate_if_needed(&unsigned_env, provider).await? {
        Some(sim_resp) => {
            info!("Applying simulation results to operations transaction");
            // Get operation count from the envelope
            let op_count = extract_operations(&unsigned_env)?.len() as u64;
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

    // Sign the transaction
    // The signer will build the envelope from operations and sign it
    sign_stellar_transaction(signer, stellar_data_with_sim).await
}
