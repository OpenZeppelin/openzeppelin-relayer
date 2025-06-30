//! This module contains the preparation-related functionality for Stellar transactions.
//! It includes methods for preparing transactions with robust error handling,
//! ensuring lanes are always properly cleaned up on failure.

// Declare submodules from the prepare/ directory
pub mod common;
pub mod fee_bump;
pub mod operations;
pub mod unsigned_xdr;

#[cfg(test)]
mod tests;

use eyre::Result;
use log::{info, warn};

use super::{lane_gate, StellarRelayerTransaction};
use crate::models::RelayerRepoModel;
use crate::{
    jobs::JobProducerTrait,
    models::{TransactionError, TransactionInput, TransactionRepoModel, TransactionStatus},
    repositories::{Repository, TransactionCounterTrait, TransactionRepository},
    services::{Signer, StellarProviderTrait},
};

use common::{sign_and_finalize_transaction, update_and_notify_transaction};

impl<R, T, J, S, P, C> StellarRelayerTransaction<R, T, J, S, P, C>
where
    R: Repository<RelayerRepoModel, String> + Send + Sync,
    T: TransactionRepository + Send + Sync,
    J: JobProducerTrait + Send + Sync,
    S: Signer + Send + Sync,
    P: StellarProviderTrait + Send + Sync,
    C: TransactionCounterTrait + Send + Sync,
{
    /// Main preparation method with robust error handling and guaranteed lane cleanup.
    pub async fn prepare_transaction_impl(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        if !lane_gate::claim(&self.relayer().id, &tx.id) {
            info!(
                "Relayer {} already has a transaction in flight – {} must wait.",
                self.relayer().id,
                tx.id
            );
            return Ok(tx);
        }

        info!("Preparing transaction: {:?}", tx.id);

        // Call core preparation logic with error handling
        match self.prepare_core(tx.clone()).await {
            Ok(prepared_tx) => Ok(prepared_tx),
            Err(error) => {
                // Always cleanup on failure - this is the critical safety mechanism
                self.handle_prepare_failure(tx, error).await
            }
        }
    }

    /// Core preparation logic
    async fn prepare_core(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        let stellar_data = tx.network_data.get_stellar_transaction_data()?;

        // Simple dispatch to appropriate processing function based on input type
        match &stellar_data.transaction_input {
            TransactionInput::Operations(_) => {
                info!("Preparing operations-based transaction {}", tx.id);
                let stellar_data_with_sim = operations::process_operations(
                    self.transaction_counter_service(),
                    &self.relayer().id,
                    &self.relayer().address,
                    &tx,
                    stellar_data,
                    self.provider(),
                    self.signer(),
                )
                .await?;
                self.finalize_with_signature(tx, stellar_data_with_sim)
                    .await
            }
            TransactionInput::UnsignedXdr(_) => {
                info!("Preparing unsigned XDR transaction {}", tx.id);
                let stellar_data_with_sim = unsigned_xdr::process_unsigned_xdr(
                    self.transaction_counter_service(),
                    &self.relayer().id,
                    &self.relayer().address,
                    stellar_data,
                    self.provider(),
                    self.signer(),
                )
                .await?;
                self.finalize_with_signature(tx, stellar_data_with_sim)
                    .await
            }
            TransactionInput::SignedXdr { .. } => {
                info!("Preparing fee-bump transaction {}", tx.id);
                let stellar_data_with_fee_bump = fee_bump::process_fee_bump(
                    &self.relayer().address,
                    stellar_data,
                    self.provider(),
                    self.signer(),
                )
                .await?;
                update_and_notify_transaction(
                    self.transaction_repository(),
                    self.job_producer(),
                    tx.id,
                    stellar_data_with_fee_bump,
                    self.relayer().notification_id.as_deref(),
                )
                .await
            }
        }
    }

    /// Helper to sign and finalize transactions for Operations and UnsignedXdr inputs.
    async fn finalize_with_signature(
        &self,
        tx: TransactionRepoModel,
        stellar_data: crate::models::StellarTransactionData,
    ) -> Result<TransactionRepoModel, TransactionError> {
        let (tx, final_stellar_data) =
            sign_and_finalize_transaction(self.signer(), tx, stellar_data).await?;
        update_and_notify_transaction(
            self.transaction_repository(),
            self.job_producer(),
            tx.id,
            final_stellar_data,
            self.relayer().notification_id.as_deref(),
        )
        .await
    }

    /// Handles preparation failures with comprehensive cleanup and error reporting.
    /// This method ensures lanes are never left claimed after any failure.
    async fn handle_prepare_failure(
        &self,
        tx: TransactionRepoModel,
        error: TransactionError,
    ) -> Result<TransactionRepoModel, TransactionError> {
        let error_reason = format!("Preparation failed: {}", error);
        let tx_id = tx.id.clone(); // Clone the ID before moving tx
        warn!("Transaction {} preparation failed: {}", tx_id, error_reason);

        // Step 1: Mark transaction as Failed with detailed reason
        let _failed_tx = match self
            .finalize_transaction_state(
                tx_id.clone(),
                TransactionStatus::Failed,
                Some(error_reason.clone()),
                None,
            )
            .await
        {
            Ok(updated_tx) => updated_tx,
            Err(finalize_error) => {
                warn!(
                    "Failed to mark transaction {} as failed: {}. Proceeding with lane cleanup.",
                    tx_id, finalize_error
                );
                // Continue with cleanup even if we can't update the transaction
                tx
            }
        };

        // Step 2: Attempt to enqueue next pending transaction or release lane
        if let Err(enqueue_error) = self.enqueue_next_pending_transaction(&tx_id).await {
            warn!(
                "Failed to enqueue next pending transaction after {} failure: {}. Releasing lane directly.",
                tx_id, enqueue_error
            );
            // Fallback: release lane directly if we can't hand it over
            lane_gate::free(&self.relayer().id, &tx_id);
        }

        // Step 3: Log failure for monitoring (prepare_fail_total metric would go here)
        info!(
            "Transaction {} preparation failure handled. Lane cleaned up. Error: {}",
            tx_id, error_reason
        );

        // Step 4: Return original error to maintain API compatibility
        Err(error)
    }
}
