//! This module contains the status-related functionality for EVM transactions.
//! It includes methods for checking transaction status, determining when to resubmit
//! or replace transactions with NOOPs, and updating transaction status in the repository.

use alloy::network::ReceiptResponse;
use chrono::{DateTime, Duration, Utc};
use eyre::Result;
use tracing::{debug, error, info, warn};

use super::EvmRelayerTransaction;
use super::{
    ensure_status, get_age_since_status_change, has_enough_confirmations, is_noop,
    is_too_early_to_resubmit, is_transaction_valid, make_noop, too_many_attempts,
    too_many_noop_attempts,
};
use crate::constants::{
    get_evm_min_age_for_hash_recovery, get_evm_pending_recovery_trigger_timeout,
    get_evm_prepare_timeout, get_evm_resend_timeout, ARBITRUM_TIME_TO_RESUBMIT,
    EVM_MIN_HASHES_FOR_RECOVERY,
};
use crate::domain::transaction::common::{
    get_age_of_sent_at, is_final_state, is_pending_transaction,
};
use crate::domain::transaction::util::get_age_since_created;
use crate::models::{EvmNetwork, NetworkRepoModel, NetworkType};
use crate::repositories::{NetworkRepository, RelayerRepository};
use crate::{
    domain::transaction::evm::price_calculator::PriceCalculatorTrait,
    jobs::JobProducerTrait,
    models::{
        NetworkTransactionData, RelayerRepoModel, TransactionError, TransactionRepoModel,
        TransactionStatus, TransactionUpdateRequest,
    },
    repositories::{Repository, TransactionCounterTrait, TransactionRepository},
    services::{provider::EvmProviderTrait, signer::Signer},
    utils::{get_resubmit_timeout_for_speed, get_resubmit_timeout_with_backoff},
};

impl<P, RR, NR, TR, J, S, TCR, PC> EvmRelayerTransaction<P, RR, NR, TR, J, S, TCR, PC>
where
    P: EvmProviderTrait + Send + Sync,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    J: JobProducerTrait + Send + Sync + 'static,
    S: Signer + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PC: PriceCalculatorTrait + Send + Sync,
{
    pub(super) async fn check_transaction_status(
        &self,
        tx: &TransactionRepoModel,
    ) -> Result<TransactionStatus, TransactionError> {
        // Early return if transaction is already in a final state
        if is_final_state(&tx.status) {
            return Ok(tx.status.clone());
        }

        // Early return for Pending/Sent states - these are DB-only states
        // that don't require on-chain queries and may not have a hash yet
        match tx.status {
            TransactionStatus::Pending | TransactionStatus::Sent => {
                return Ok(tx.status.clone());
            }
            _ => {}
        }

        let evm_data = tx.network_data.get_evm_transaction_data()?;
        let tx_hash = evm_data
            .hash
            .as_ref()
            .ok_or(TransactionError::UnexpectedError(
                "Transaction hash is missing".to_string(),
            ))?;

        let receipt_result = self.provider().get_transaction_receipt(tx_hash).await?;

        if let Some(receipt) = receipt_result {
            if !receipt.inner.status() {
                return Ok(TransactionStatus::Failed);
            }
            let last_block_number = self.provider().get_block_number().await?;
            let tx_block_number = receipt
                .block_number
                .ok_or(TransactionError::UnexpectedError(
                    "Transaction receipt missing block number".to_string(),
                ))?;

            let network_model = self
                .network_repository()
                .get_by_chain_id(NetworkType::Evm, evm_data.chain_id)
                .await?
                .ok_or(TransactionError::UnexpectedError(format!(
                    "Network with chain id {} not found",
                    evm_data.chain_id
                )))?;

            let network = EvmNetwork::try_from(network_model).map_err(|e| {
                TransactionError::UnexpectedError(format!(
                    "Error converting network model to EvmNetwork: {e}"
                ))
            })?;

            if !has_enough_confirmations(
                tx_block_number,
                last_block_number,
                network.required_confirmations,
            ) {
                debug!(tx_hash = %tx_hash, "transaction mined but not confirmed");
                return Ok(TransactionStatus::Mined);
            }
            Ok(TransactionStatus::Confirmed)
        } else {
            debug!(tx_hash = %tx_hash, "transaction not yet mined");

            // FALLBACK: Try to find transaction by checking all historical hashes
            // Only do this for transactions that have multiple resubmission attempts
            // and have been stuck in Submitted for a while
            if tx.hashes.len() > 1 && self.should_try_hash_recovery(tx)? {
                if let Some(recovered_tx) = self
                    .try_recover_with_historical_hashes(tx, &evm_data)
                    .await?
                {
                    // Return the status from the recovered (updated) transaction
                    return Ok(recovered_tx.status);
                }
            }

            Ok(TransactionStatus::Submitted)
        }
    }

    /// Determines if a transaction should be resubmitted.
    pub(super) async fn should_resubmit(
        &self,
        tx: &TransactionRepoModel,
    ) -> Result<bool, TransactionError> {
        // Validate transaction is in correct state for resubmission
        ensure_status(tx, TransactionStatus::Submitted, Some("should_resubmit"))?;

        let evm_data = tx.network_data.get_evm_transaction_data()?;
        let age = get_age_of_sent_at(tx)?;

        // Check if network lacks mempool and determine appropriate timeout
        let network_model = self
            .network_repository()
            .get_by_chain_id(NetworkType::Evm, evm_data.chain_id)
            .await?
            .ok_or(TransactionError::UnexpectedError(format!(
                "Network with chain id {} not found",
                evm_data.chain_id
            )))?;

        let network = EvmNetwork::try_from(network_model).map_err(|e| {
            TransactionError::UnexpectedError(format!(
                "Error converting network model to EvmNetwork: {e}"
            ))
        })?;

        let timeout = match network.is_arbitrum() {
            true => ARBITRUM_TIME_TO_RESUBMIT,
            false => get_resubmit_timeout_for_speed(&evm_data.speed),
        };

        let timeout_with_backoff = match network.is_arbitrum() {
            true => timeout, // Use base timeout without backoff for Arbitrum
            false => get_resubmit_timeout_with_backoff(timeout, tx.hashes.len()),
        };

        if age > Duration::milliseconds(timeout_with_backoff) {
            info!("Transaction has been pending for too long, resubmitting");
            return Ok(true);
        }
        Ok(false)
    }

    /// Determines if a transaction should be replaced with a NOOP transaction.
    ///
    /// Returns a tuple `(should_noop, reason)` where:
    /// - `should_noop`: `true` if transaction should be replaced with NOOP
    /// - `reason`: Optional reason string explaining why NOOP is needed (only set when `should_noop` is `true`)
    ///
    /// # Arguments
    ///
    /// * `tx` - The transaction to check
    pub(super) async fn should_noop(
        &self,
        tx: &TransactionRepoModel,
    ) -> Result<(bool, Option<String>), TransactionError> {
        if too_many_noop_attempts(tx) {
            info!("Transaction has too many NOOP attempts already");
            return Ok((false, None));
        }

        let evm_data = tx.network_data.get_evm_transaction_data()?;
        if is_noop(&evm_data) {
            return Ok((false, None));
        }

        let network_model = self
            .network_repository()
            .get_by_chain_id(NetworkType::Evm, evm_data.chain_id)
            .await?
            .ok_or(TransactionError::UnexpectedError(format!(
                "Network with chain id {} not found",
                evm_data.chain_id
            )))?;

        let network = EvmNetwork::try_from(network_model).map_err(|e| {
            TransactionError::UnexpectedError(format!(
                "Error converting network model to EvmNetwork: {e}"
            ))
        })?;

        if network.is_rollup() && too_many_attempts(tx) {
            let reason =
                "Rollup transaction has too many attempts. Replacing with NOOP.".to_string();
            info!("{}", reason);
            return Ok((true, Some(reason)));
        }

        if !is_transaction_valid(&tx.created_at, &tx.valid_until) {
            let reason = "Transaction is expired. Replacing with NOOP.".to_string();
            info!("{}", reason);
            return Ok((true, Some(reason)));
        }

        if tx.status == TransactionStatus::Pending {
            let created_at = &tx.created_at;
            let created_time = DateTime::parse_from_rfc3339(created_at)
                .map_err(|e| {
                    TransactionError::UnexpectedError(format!("Invalid created_at timestamp: {e}"))
                })?
                .with_timezone(&Utc);
            let age = Utc::now().signed_duration_since(created_time);
            if age > get_evm_prepare_timeout() {
                let reason = format!(
                    "Transaction in Pending state for over {} minutes. Replacing with NOOP.",
                    get_evm_prepare_timeout().num_minutes()
                );
                info!("{}", reason);
                return Ok((true, Some(reason)));
            }
        }

        let latest_block = self.provider().get_block_by_number().await;
        if let Ok(block) = latest_block {
            let block_gas_limit = block.header.gas_limit;
            if let Some(gas_limit) = evm_data.gas_limit {
                if gas_limit > block_gas_limit {
                    let reason = format!(
                                "Transaction gas limit ({gas_limit}) exceeds block gas limit ({block_gas_limit}). Replacing with NOOP.",
                            );
                    warn!(
                        tx_id = %tx.id,
                        tx_gas_limit = %gas_limit,
                        block_gas_limit = %block_gas_limit,
                        "transaction gas limit exceeds block gas limit, replacing with NOOP"
                    );
                    return Ok((true, Some(reason)));
                }
            }
        }

        Ok((false, None))
    }

    /// Helper method that updates transaction status only if it's different from the current status.
    pub(super) async fn update_transaction_status_if_needed(
        &self,
        tx: TransactionRepoModel,
        new_status: TransactionStatus,
    ) -> Result<TransactionRepoModel, TransactionError> {
        if tx.status != new_status {
            return self.update_transaction_status(tx, new_status).await;
        }
        Ok(tx)
    }

    /// Prepares a NOOP transaction update request.
    pub(super) async fn prepare_noop_update_request(
        &self,
        tx: &TransactionRepoModel,
        is_cancellation: bool,
        reason: Option<String>,
    ) -> Result<TransactionUpdateRequest, TransactionError> {
        let mut evm_data = tx.network_data.get_evm_transaction_data()?;
        let network_model = self
            .network_repository()
            .get_by_chain_id(NetworkType::Evm, evm_data.chain_id)
            .await?
            .ok_or(TransactionError::UnexpectedError(format!(
                "Network with chain id {} not found",
                evm_data.chain_id
            )))?;

        let network = EvmNetwork::try_from(network_model).map_err(|e| {
            TransactionError::UnexpectedError(format!(
                "Error converting network model to EvmNetwork: {e}"
            ))
        })?;

        make_noop(&mut evm_data, &network, Some(self.provider())).await?;

        let noop_count = tx.noop_count.unwrap_or(0) + 1;
        let update_request = TransactionUpdateRequest {
            network_data: Some(NetworkTransactionData::Evm(evm_data)),
            noop_count: Some(noop_count),
            status_reason: reason,
            is_canceled: if is_cancellation {
                Some(true)
            } else {
                tx.is_canceled
            },
            ..Default::default()
        };
        Ok(update_request)
    }

    /// Handles transactions in the Submitted state.
    async fn handle_submitted_state(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        if self.should_resubmit(&tx).await? {
            let resubmitted_tx = self.handle_resubmission(tx).await?;
            return Ok(resubmitted_tx);
        }

        self.update_transaction_status_if_needed(tx, TransactionStatus::Submitted)
            .await
    }

    /// Processes transaction resubmission logic
    async fn handle_resubmission(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        debug!("scheduling resubmit job for transaction");

        // Check if transaction gas limit exceeds block gas limit before resubmitting
        let (should_noop, reason) = self.should_noop(&tx).await?;
        let tx_to_process = if should_noop {
            self.process_noop_transaction(&tx, reason).await?
        } else {
            tx
        };

        self.send_transaction_resubmit_job(&tx_to_process).await?;
        Ok(tx_to_process)
    }

    /// Handles NOOP transaction processing before resubmission
    async fn process_noop_transaction(
        &self,
        tx: &TransactionRepoModel,
        reason: Option<String>,
    ) -> Result<TransactionRepoModel, TransactionError> {
        debug!("preparing transaction NOOP before resubmission");
        let update = self.prepare_noop_update_request(tx, false, reason).await?;
        let updated_tx = self
            .transaction_repository()
            .partial_update(tx.id.clone(), update)
            .await?;

        let res = self.send_transaction_update_notification(&updated_tx).await;
        if let Err(e) = res {
            error!(
                tx_id = %updated_tx.id,
                status = ?updated_tx.status,
                "sending transaction update notification failed for NOOP transaction: {:?}",
                e
            );
        }
        Ok(updated_tx)
    }

    /// Handles transactions in the Pending state.
    async fn handle_pending_state(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        let (should_noop, reason) = self.should_noop(&tx).await?;
        if should_noop {
            // For Pending state transactions, nonces are not yet assigned, so we mark as Failed
            // instead of NOOP. This matches prepare_transaction behavior.
            debug!(
                tx_id = %tx.id,
                reason = %reason.as_ref().unwrap_or(&"unknown".to_string()),
                "marking pending transaction as Failed (nonce not assigned, no NOOP needed)"
            );
            let update = TransactionUpdateRequest {
                status: Some(TransactionStatus::Failed),
                status_reason: reason,
                ..Default::default()
            };
            let updated_tx = self
                .transaction_repository()
                .partial_update(tx.id.clone(), update)
                .await?;

            let res = self.send_transaction_update_notification(&updated_tx).await;
            if let Err(e) = res {
                error!(
                    tx_id = %updated_tx.id,
                    status = ?updated_tx.status,
                    "sending transaction update notification failed: {:?}",
                    e
                );
            }
            return Ok(updated_tx);
        }

        // Check if transaction is stuck in Pending (prepare job may have failed)
        let age = get_age_since_created(&tx)?;
        if age > get_evm_pending_recovery_trigger_timeout() {
            warn!(
                tx_id = %tx.id,
                age_seconds = age.num_seconds(),
                "transaction stuck in Pending, queuing prepare job"
            );

            // Re-queue prepare job
            self.send_transaction_request_job(&tx).await?;
        }

        Ok(tx)
    }

    /// Handles transactions in the Mined state.
    async fn handle_mined_state(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        self.update_transaction_status_if_needed(tx, TransactionStatus::Mined)
            .await
    }

    /// Handles transactions in final states (Confirmed, Failed, Expired).
    async fn handle_final_state(
        &self,
        tx: TransactionRepoModel,
        status: TransactionStatus,
    ) -> Result<TransactionRepoModel, TransactionError> {
        self.update_transaction_status_if_needed(tx, status).await
    }

    /// Inherent status-handling method.
    ///
    /// This method encapsulates the full logic for handling transaction status,
    /// including resubmission, NOOP replacement, timeout detection, and updating status.
    pub async fn handle_status_impl(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        debug!("checking transaction status {}", tx.id);

        // 1. Early return if final state
        if is_final_state(&tx.status) {
            debug!(status = ?tx.status, "transaction already in final state");
            return Ok(tx);
        }

        // 2. Check transaction status first
        // This allows fast transactions to update their status immediately,
        // even if they're young (<20s). For Pending/Sent states, this returns
        // early without querying the blockchain.
        let status = self.check_transaction_status(&tx).await?;

        debug!(
            tx_id = %tx.id,
            previous_status = ?tx.status,
            new_status = ?status,
            "transaction status check completed"
        );

        // 2.1. Reload transaction from DB if status changed
        // This ensures we have fresh data if check_transaction_status triggered a recovery
        // or any other update that modified the transaction in the database.
        let tx = if status != tx.status {
            debug!(
                tx_id = %tx.id,
                old_status = ?tx.status,
                new_status = ?status,
                "status changed during check, reloading transaction from DB to ensure fresh data"
            );
            self.transaction_repository()
                .get_by_id(tx.id.clone())
                .await?
        } else {
            tx
        };

        // 3. Check if too early for resubmission on in-progress transactions
        // For Pending/Sent/Submitted states, defer resubmission logic and timeout checks
        // if the transaction is too young. Just update status and return.
        // For other states (Mined/Confirmed/Failed/etc), process immediately regardless of age.
        if is_too_early_to_resubmit(&tx)? && is_pending_transaction(&status) {
            // Update status if it changed, then return
            return self.update_transaction_status_if_needed(tx, status).await;
        }

        // 4. Handle based on status (including complex operations like resubmission)
        match status {
            TransactionStatus::Pending => self.handle_pending_state(tx).await,
            TransactionStatus::Sent => self.handle_sent_state(tx).await,
            TransactionStatus::Submitted => self.handle_submitted_state(tx).await,
            TransactionStatus::Mined => self.handle_mined_state(tx).await,
            TransactionStatus::Confirmed
            | TransactionStatus::Failed
            | TransactionStatus::Expired
            | TransactionStatus::Canceled => self.handle_final_state(tx, status).await,
        }
    }

    /// Handle transactions stuck in Sent (prepared but not submitted)
    async fn handle_sent_state(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        debug!(tx_id = %tx.id, "handling Sent state");

        // Check if transaction should be replaced with NOOP (expired, too many attempts on rollup, etc.)
        let (should_noop, reason) = self.should_noop(&tx).await?;
        if should_noop {
            debug!("preparing NOOP for sent transaction {}", tx.id);
            let update = self.prepare_noop_update_request(&tx, false, reason).await?;
            let updated_tx = self
                .transaction_repository()
                .partial_update(tx.id.clone(), update)
                .await?;

            self.send_transaction_submit_job(&updated_tx).await?;
            let res = self.send_transaction_update_notification(&updated_tx).await;
            if let Err(e) = res {
                error!(
                    tx_id = %updated_tx.id,
                    status = ?updated_tx.status,
                    "sending transaction update notification failed for Sent state NOOP: {:?}",
                    e
                );
            }
            return Ok(updated_tx);
        }

        // Transaction was prepared but submission job may have failed
        // Re-queue a resend job if it's been stuck for a while
        let age_since_sent = get_age_since_status_change(&tx)?;

        if age_since_sent > get_evm_resend_timeout() {
            warn!(
                tx_id = %tx.id,
                age_seconds = age_since_sent.num_seconds(),
                "transaction stuck in Sent, queuing resubmit job with repricing"
            );

            // Queue resubmit job to reprice the transaction for better acceptance
            self.send_transaction_resubmit_job(&tx).await?;
        }

        self.update_transaction_status_if_needed(tx, TransactionStatus::Sent)
            .await
    }

    /// Determines if we should attempt hash recovery for a stuck transaction.
    ///
    /// This is an expensive operation, so we only do it when:
    /// - Transaction has been in Submitted status for a while (> 2 minutes)
    /// - Transaction has had at least 2 resubmission attempts (hashes.len() > 1)
    /// - Haven't tried recovery too recently (to avoid repeated attempts)
    fn should_try_hash_recovery(
        &self,
        tx: &TransactionRepoModel,
    ) -> Result<bool, TransactionError> {
        // Only try recovery for transactions stuck in Submitted
        if tx.status != TransactionStatus::Submitted {
            return Ok(false);
        }

        // Must have multiple hashes (indicating resubmissions happened)
        if tx.hashes.len() <= 1 {
            return Ok(false);
        }

        // Only try if transaction has been stuck for a while
        let age = get_age_of_sent_at(tx)?;
        let min_age_for_recovery = get_evm_min_age_for_hash_recovery();

        if age < min_age_for_recovery {
            return Ok(false);
        }

        // Check if we've had enough resubmission attempts (more attempts = more likely to have wrong hash)
        // Only try recovery if we have at least 3 hashes (2 resubmissions)
        if tx.hashes.len() < EVM_MIN_HASHES_FOR_RECOVERY {
            return Ok(false);
        }

        Ok(true)
    }

    /// Attempts to recover transaction status by checking all historical hashes.
    ///
    /// When a transaction is resubmitted multiple times due to timeouts, the database
    /// may contain multiple hashes. The "current" hash (network_data.hash) might not
    /// be the one that actually got mined. This method checks all historical hashes
    /// to find if any were mined, and updates the database with the correct one.
    ///
    /// Returns the updated transaction model if recovery was successful, None otherwise.
    async fn try_recover_with_historical_hashes(
        &self,
        tx: &TransactionRepoModel,
        evm_data: &crate::models::EvmTransactionData,
    ) -> Result<Option<TransactionRepoModel>, TransactionError> {
        warn!(
            tx_id = %tx.id,
            current_hash = ?evm_data.hash,
            total_hashes = %tx.hashes.len(),
            "attempting hash recovery - checking historical hashes"
        );

        // Check each historical hash (most recent first, since it's more likely)
        for (idx, historical_hash) in tx.hashes.iter().rev().enumerate() {
            // Skip if this is the current hash (already checked)
            if Some(historical_hash) == evm_data.hash.as_ref() {
                continue;
            }

            debug!(
                tx_id = %tx.id,
                hash = %historical_hash,
                index = %idx,
                "checking historical hash"
            );

            // Try to get receipt for this hash
            match self
                .provider()
                .get_transaction_receipt(historical_hash)
                .await
            {
                Ok(Some(receipt)) => {
                    warn!(
                        tx_id = %tx.id,
                        mined_hash = %historical_hash,
                        wrong_hash = ?evm_data.hash,
                        block_number = ?receipt.block_number,
                        "RECOVERED: found mined transaction with historical hash - correcting database"
                    );

                    // Update with correct hash and Mined status
                    // Let the normal status check flow handle confirmation checking
                    let updated_tx = self
                        .update_transaction_with_corrected_hash(
                            tx,
                            evm_data,
                            historical_hash,
                            TransactionStatus::Mined,
                        )
                        .await?;

                    return Ok(Some(updated_tx));
                }
                Ok(None) => {
                    // This hash not found either, continue to next
                    continue;
                }
                Err(e) => {
                    // Network error, log but continue checking other hashes
                    warn!(
                        tx_id = %tx.id,
                        hash = %historical_hash,
                        error = %e,
                        "error checking historical hash, continuing to next"
                    );
                    continue;
                }
            }
        }

        // None of the historical hashes found on-chain
        debug!(
            tx_id = %tx.id,
            "hash recovery completed - no historical hashes found on-chain"
        );
        Ok(None)
    }

    /// Updates transaction with the corrected hash and status
    ///
    /// Returns the updated transaction model and sends a notification about the status change.
    async fn update_transaction_with_corrected_hash(
        &self,
        tx: &TransactionRepoModel,
        evm_data: &crate::models::EvmTransactionData,
        correct_hash: &str,
        status: TransactionStatus,
    ) -> Result<TransactionRepoModel, TransactionError> {
        let mut corrected_data = evm_data.clone();
        corrected_data.hash = Some(correct_hash.to_string());

        let updated_tx = self
            .transaction_repository()
            .partial_update(
                tx.id.clone(),
                TransactionUpdateRequest {
                    network_data: Some(NetworkTransactionData::Evm(corrected_data)),
                    status: Some(status),
                    ..Default::default()
                },
            )
            .await?;

        // Send notification about the recovered transaction
        if let Err(e) = self.send_transaction_update_notification(&updated_tx).await {
            error!(
                tx_id = %updated_tx.id,
                error = %e,
                "failed to send notification after hash recovery"
            );
        }

        Ok(updated_tx)
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        config::{EvmNetworkConfig, NetworkConfigCommon},
        domain::transaction::evm::{EvmRelayerTransaction, MockPriceCalculatorTrait},
        jobs::MockJobProducerTrait,
        models::{
            evm::Speed, EvmTransactionData, NetworkConfigData, NetworkRepoModel,
            NetworkTransactionData, NetworkType, RelayerEvmPolicy, RelayerNetworkPolicy,
            RelayerRepoModel, TransactionReceipt, TransactionRepoModel, TransactionStatus, U256,
        },
        repositories::{
            MockNetworkRepository, MockRelayerRepository, MockTransactionCounterTrait,
            MockTransactionRepository,
        },
        services::{provider::MockEvmProviderTrait, signer::MockSigner},
    };
    use alloy::{
        consensus::{Eip658Value, Receipt, ReceiptWithBloom},
        network::AnyReceiptEnvelope,
        primitives::{b256, Address, BlockHash, Bloom, TxHash},
    };
    use chrono::{Duration, Utc};
    use std::sync::Arc;

    /// Helper struct holding all the mocks we often need
    pub struct TestMocks {
        pub provider: MockEvmProviderTrait,
        pub relayer_repo: MockRelayerRepository,
        pub network_repo: MockNetworkRepository,
        pub tx_repo: MockTransactionRepository,
        pub job_producer: MockJobProducerTrait,
        pub signer: MockSigner,
        pub counter: MockTransactionCounterTrait,
        pub price_calc: MockPriceCalculatorTrait,
    }

    /// Returns a default `TestMocks` with zero-configuration stubs.
    /// You can override expectations in each test as needed.
    pub fn default_test_mocks() -> TestMocks {
        TestMocks {
            provider: MockEvmProviderTrait::new(),
            relayer_repo: MockRelayerRepository::new(),
            network_repo: MockNetworkRepository::new(),
            tx_repo: MockTransactionRepository::new(),
            job_producer: MockJobProducerTrait::new(),
            signer: MockSigner::new(),
            counter: MockTransactionCounterTrait::new(),
            price_calc: MockPriceCalculatorTrait::new(),
        }
    }

    /// Returns a `TestMocks` with network repository configured for prepare_noop_update_request tests.
    pub fn default_test_mocks_with_network() -> TestMocks {
        let mut mocks = default_test_mocks();
        // Set up default expectation for get_by_chain_id that prepare_noop_update_request tests need
        mocks
            .network_repo
            .expect_get_by_chain_id()
            .returning(|network_type, chain_id| {
                if network_type == NetworkType::Evm && chain_id == 1 {
                    Ok(Some(create_test_network_model()))
                } else {
                    Ok(None)
                }
            });
        mocks
    }

    /// Creates a test NetworkRepoModel for chain_id 1 (mainnet)
    pub fn create_test_network_model() -> NetworkRepoModel {
        let evm_config = EvmNetworkConfig {
            common: NetworkConfigCommon {
                network: "mainnet".to_string(),
                from: None,
                rpc_urls: Some(vec!["https://rpc.example.com".to_string()]),
                explorer_urls: Some(vec!["https://explorer.example.com".to_string()]),
                average_blocktime_ms: Some(12000),
                is_testnet: Some(false),
                tags: Some(vec!["mainnet".to_string()]),
            },
            chain_id: Some(1),
            required_confirmations: Some(12),
            features: Some(vec!["eip1559".to_string()]),
            symbol: Some("ETH".to_string()),
            gas_price_cache: None,
        };
        NetworkRepoModel {
            id: "evm:mainnet".to_string(),
            name: "mainnet".to_string(),
            network_type: NetworkType::Evm,
            config: NetworkConfigData::Evm(evm_config),
        }
    }

    /// Creates a test NetworkRepoModel for chain_id 42161 (Arbitrum-like) with no-mempool tag
    pub fn create_test_no_mempool_network_model() -> NetworkRepoModel {
        let evm_config = EvmNetworkConfig {
            common: NetworkConfigCommon {
                network: "arbitrum".to_string(),
                from: None,
                rpc_urls: Some(vec!["https://arb-rpc.example.com".to_string()]),
                explorer_urls: Some(vec!["https://arb-explorer.example.com".to_string()]),
                average_blocktime_ms: Some(1000),
                is_testnet: Some(false),
                tags: Some(vec![
                    "arbitrum".to_string(),
                    "rollup".to_string(),
                    "no-mempool".to_string(),
                ]),
            },
            chain_id: Some(42161),
            required_confirmations: Some(12),
            features: Some(vec!["eip1559".to_string()]),
            symbol: Some("ETH".to_string()),
            gas_price_cache: None,
        };
        NetworkRepoModel {
            id: "evm:arbitrum".to_string(),
            name: "arbitrum".to_string(),
            network_type: NetworkType::Evm,
            config: NetworkConfigData::Evm(evm_config),
        }
    }

    /// Minimal "builder" for TransactionRepoModel.
    /// Allows quick creation of a test transaction with default fields,
    /// then updates them based on the provided status or overrides.
    pub fn make_test_transaction(status: TransactionStatus) -> TransactionRepoModel {
        TransactionRepoModel {
            id: "test-tx-id".to_string(),
            relayer_id: "test-relayer-id".to_string(),
            status,
            status_reason: None,
            created_at: Utc::now().to_rfc3339(),
            sent_at: None,
            confirmed_at: None,
            valid_until: None,
            delete_at: None,
            network_type: NetworkType::Evm,
            network_data: NetworkTransactionData::Evm(EvmTransactionData {
                chain_id: 1,
                from: "0xSender".to_string(),
                to: Some("0xRecipient".to_string()),
                value: U256::from(0),
                data: Some("0xData".to_string()),
                gas_limit: Some(21000),
                gas_price: Some(20000000000),
                max_fee_per_gas: None,
                max_priority_fee_per_gas: None,
                nonce: None,
                signature: None,
                hash: None,
                speed: Some(Speed::Fast),
                raw: None,
            }),
            priced_at: None,
            hashes: Vec::new(),
            noop_count: None,
            is_canceled: Some(false),
        }
    }

    /// Minimal "builder" for EvmRelayerTransaction.
    /// Takes mock dependencies as arguments.
    pub fn make_test_evm_relayer_transaction(
        relayer: RelayerRepoModel,
        mocks: TestMocks,
    ) -> EvmRelayerTransaction<
        MockEvmProviderTrait,
        MockRelayerRepository,
        MockNetworkRepository,
        MockTransactionRepository,
        MockJobProducerTrait,
        MockSigner,
        MockTransactionCounterTrait,
        MockPriceCalculatorTrait,
    > {
        EvmRelayerTransaction::new(
            relayer,
            mocks.provider,
            Arc::new(mocks.relayer_repo),
            Arc::new(mocks.network_repo),
            Arc::new(mocks.tx_repo),
            Arc::new(mocks.counter),
            Arc::new(mocks.job_producer),
            mocks.price_calc,
            mocks.signer,
        )
        .unwrap()
    }

    fn create_test_relayer() -> RelayerRepoModel {
        RelayerRepoModel {
            id: "test-relayer-id".to_string(),
            name: "Test Relayer".to_string(),
            paused: false,
            system_disabled: false,
            network: "test_network".to_string(),
            network_type: NetworkType::Evm,
            policies: RelayerNetworkPolicy::Evm(RelayerEvmPolicy::default()),
            signer_id: "test_signer".to_string(),
            address: "0x".to_string(),
            notification_id: None,
            custom_rpc_urls: None,
            ..Default::default()
        }
    }

    fn make_mock_receipt(status: bool, block_number: Option<u64>) -> TransactionReceipt {
        // Use some placeholder values for minimal completeness
        let tx_hash = TxHash::from(b256!(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        ));
        let block_hash = BlockHash::from(b256!(
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        ));
        let from_address = Address::from([0x11; 20]);

        TransactionReceipt {
            inner: alloy::rpc::types::TransactionReceipt {
                inner: AnyReceiptEnvelope {
                    inner: ReceiptWithBloom {
                        receipt: Receipt {
                            status: Eip658Value::Eip658(status), // determines success/fail
                            cumulative_gas_used: 0,
                            logs: vec![],
                        },
                        logs_bloom: Bloom::ZERO,
                    },
                    r#type: 0, // Legacy transaction type
                },
                transaction_hash: tx_hash,
                transaction_index: Some(0),
                block_hash: block_number.map(|_| block_hash), // only set if mined
                block_number,
                gas_used: 21000,
                effective_gas_price: 1000,
                blob_gas_used: None,
                blob_gas_price: None,
                from: from_address,
                to: None,
                contract_address: None,
            },
            other: Default::default(),
        }
    }

    // Tests for `check_transaction_status`
    mod check_transaction_status_tests {
        use super::*;

        #[tokio::test]
        async fn test_not_mined() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let mut tx = make_test_transaction(TransactionStatus::Submitted);

            // Provide a hash so we can check for receipt
            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.hash = Some("0xFakeHash".to_string());
            }

            // Mock that get_transaction_receipt returns None (not mined)
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|_| Box::pin(async { Ok(None) }));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);

            let status = evm_transaction.check_transaction_status(&tx).await.unwrap();
            assert_eq!(status, TransactionStatus::Submitted);
        }

        #[tokio::test]
        async fn test_mined_but_not_confirmed() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let mut tx = make_test_transaction(TransactionStatus::Submitted);

            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.hash = Some("0xFakeHash".to_string());
            }

            // Mock a mined receipt with block_number = 100
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|_| Box::pin(async { Ok(Some(make_mock_receipt(true, Some(100)))) }));

            // Mock block_number that hasn't reached the confirmation threshold
            mocks
                .provider
                .expect_get_block_number()
                .return_once(|| Box::pin(async { Ok(100) }));

            // Mock network repository to return a test network model
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);

            let status = evm_transaction.check_transaction_status(&tx).await.unwrap();
            assert_eq!(status, TransactionStatus::Mined);
        }

        #[tokio::test]
        async fn test_confirmed() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let mut tx = make_test_transaction(TransactionStatus::Submitted);

            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.hash = Some("0xFakeHash".to_string());
            }

            // Mock a mined receipt with block_number = 100
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|_| Box::pin(async { Ok(Some(make_mock_receipt(true, Some(100)))) }));

            // Mock block_number that meets the confirmation threshold
            mocks
                .provider
                .expect_get_block_number()
                .return_once(|| Box::pin(async { Ok(113) }));

            // Mock network repository to return a test network model
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);

            let status = evm_transaction.check_transaction_status(&tx).await.unwrap();
            assert_eq!(status, TransactionStatus::Confirmed);
        }

        #[tokio::test]
        async fn test_failed() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let mut tx = make_test_transaction(TransactionStatus::Submitted);

            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.hash = Some("0xFakeHash".to_string());
            }

            // Mock a mined receipt with failure
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|_| Box::pin(async { Ok(Some(make_mock_receipt(false, Some(100)))) }));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);

            let status = evm_transaction.check_transaction_status(&tx).await.unwrap();
            assert_eq!(status, TransactionStatus::Failed);
        }
    }

    // Tests for `should_resubmit`
    mod should_resubmit_tests {
        use super::*;
        use crate::models::TransactionError;

        #[tokio::test]
        async fn test_should_resubmit_true() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            // Set sent_at to 600 seconds ago to force resubmission
            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.sent_at = Some((Utc::now() - Duration::seconds(600)).to_rfc3339());

            // Mock network repository to return a regular network model
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let res = evm_transaction.should_resubmit(&tx).await.unwrap();
            assert!(res, "Transaction should be resubmitted after timeout.");
        }

        #[tokio::test]
        async fn test_should_resubmit_false() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            // Make a transaction with status Submitted but recently sent
            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.sent_at = Some(Utc::now().to_rfc3339());

            // Mock network repository to return a regular network model
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let res = evm_transaction.should_resubmit(&tx).await.unwrap();
            assert!(!res, "Transaction should not be resubmitted immediately.");
        }

        #[tokio::test]
        async fn test_should_resubmit_true_for_no_mempool_network() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            // Set up a transaction that would normally be resubmitted (sent_at long ago)
            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.sent_at = Some((Utc::now() - Duration::seconds(600)).to_rfc3339());

            // Set chain_id to match the no-mempool network
            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.chain_id = 42161; // Arbitrum chain ID
            }

            // Mock network repository to return a no-mempool network model
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_no_mempool_network_model())));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let res = evm_transaction.should_resubmit(&tx).await.unwrap();
            assert!(
                res,
                "Transaction should be resubmitted for no-mempool networks."
            );
        }

        #[tokio::test]
        async fn test_should_resubmit_network_not_found() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.sent_at = Some((Utc::now() - Duration::seconds(600)).to_rfc3339());

            // Mock network repository to return None (network not found)
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(None));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.should_resubmit(&tx).await;

            assert!(
                result.is_err(),
                "should_resubmit should return error when network not found"
            );
            let error = result.unwrap_err();
            match error {
                TransactionError::UnexpectedError(msg) => {
                    assert!(msg.contains("Network with chain id 1 not found"));
                }
                _ => panic!("Expected UnexpectedError for network not found"),
            }
        }

        #[tokio::test]
        async fn test_should_resubmit_network_conversion_error() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.sent_at = Some((Utc::now() - Duration::seconds(600)).to_rfc3339());

            // Create a network model with invalid EVM config (missing chain_id)
            let invalid_evm_config = EvmNetworkConfig {
                common: NetworkConfigCommon {
                    network: "invalid-network".to_string(),
                    from: None,
                    rpc_urls: Some(vec!["https://rpc.example.com".to_string()]),
                    explorer_urls: Some(vec!["https://explorer.example.com".to_string()]),
                    average_blocktime_ms: Some(12000),
                    is_testnet: Some(false),
                    tags: Some(vec!["testnet".to_string()]),
                },
                chain_id: None, // This will cause the conversion to fail
                required_confirmations: Some(12),
                features: Some(vec!["eip1559".to_string()]),
                symbol: Some("ETH".to_string()),
                gas_price_cache: None,
            };
            let invalid_network = NetworkRepoModel {
                id: "evm:invalid".to_string(),
                name: "invalid-network".to_string(),
                network_type: NetworkType::Evm,
                config: NetworkConfigData::Evm(invalid_evm_config),
            };

            // Mock network repository to return the invalid network model
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(move |_, _| Ok(Some(invalid_network.clone())));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.should_resubmit(&tx).await;

            assert!(
                result.is_err(),
                "should_resubmit should return error when network conversion fails"
            );
            let error = result.unwrap_err();
            match error {
                TransactionError::UnexpectedError(msg) => {
                    assert!(msg.contains("Error converting network model to EvmNetwork"));
                }
                _ => panic!("Expected UnexpectedError for network conversion failure"),
            }
        }
    }

    // Tests for `should_noop`
    mod should_noop_tests {
        use super::*;

        #[tokio::test]
        async fn test_expired_transaction_triggers_noop() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            // Force the transaction to be "expired" by setting valid_until in the past
            tx.valid_until = Some((Utc::now() - Duration::seconds(10)).to_rfc3339());

            // Mock network repository to return a test network model
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let (res, reason) = evm_transaction.should_noop(&tx).await.unwrap();
            assert!(res, "Expired transaction should be replaced with a NOOP.");
            assert!(
                reason.is_some(),
                "Reason should be provided for expired transaction"
            );
            assert!(
                reason.unwrap().contains("expired"),
                "Reason should mention expiration"
            );
        }

        #[tokio::test]
        async fn test_too_many_noop_attempts_returns_false() {
            let mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.noop_count = Some(51); // Max is 50, so this should return false

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let (res, reason) = evm_transaction.should_noop(&tx).await.unwrap();
            assert!(
                !res,
                "Transaction with too many NOOP attempts should not be replaced."
            );
            assert!(
                reason.is_none(),
                "Reason should not be provided when should_noop is false"
            );
        }

        #[tokio::test]
        async fn test_already_noop_returns_false() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            // Make it a NOOP by setting to=None and value=0
            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.to = None;
                evm_data.value = U256::from(0);
            }

            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            // Mock get_block_by_number for gas limit validation (won't be called since is_noop returns early, but needed for compilation)
            mocks.provider.expect_get_block_by_number().returning(|| {
                Box::pin(async {
                    use alloy::{network::AnyRpcBlock, rpc::types::Block};
                    let mut block: Block = Block::default();
                    block.header.gas_limit = 30_000_000u64;
                    Ok(AnyRpcBlock::from(block))
                })
            });

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let (res, reason) = evm_transaction.should_noop(&tx).await.unwrap();
            assert!(
                !res,
                "Transaction that is already a NOOP should not be replaced."
            );
            assert!(
                reason.is_none(),
                "Reason should not be provided when should_noop is false"
            );
        }

        #[tokio::test]
        async fn test_rollup_with_too_many_attempts_triggers_noop() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            // Set chain_id to Arbitrum (rollup network)
            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.chain_id = 42161; // Arbitrum
            }
            // Set enough hashes to trigger too_many_attempts (> 50)
            tx.hashes = vec!["0xHash1".to_string(); 51];

            // Mock network repository to return Arbitrum network
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_no_mempool_network_model())));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let (res, reason) = evm_transaction.should_noop(&tx).await.unwrap();
            assert!(
                res,
                "Rollup transaction with too many attempts should be replaced with NOOP."
            );
            assert!(
                reason.is_some(),
                "Reason should be provided for rollup transaction"
            );
            assert!(
                reason.unwrap().contains("too many attempts"),
                "Reason should mention too many attempts"
            );
        }

        #[tokio::test]
        async fn test_pending_state_timeout_triggers_noop() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Pending);
            // Set created_at to 3 minutes ago (> 2 minute timeout)
            tx.created_at = (Utc::now() - Duration::minutes(3)).to_rfc3339();

            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let (res, reason) = evm_transaction.should_noop(&tx).await.unwrap();
            assert!(
                res,
                "Pending transaction stuck for >2 minutes should be replaced with NOOP."
            );
            assert!(
                reason.is_some(),
                "Reason should be provided for pending timeout"
            );
            assert!(
                reason.unwrap().contains("Pending state"),
                "Reason should mention Pending state"
            );
        }

        #[tokio::test]
        async fn test_valid_transaction_returns_false() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let tx = make_test_transaction(TransactionStatus::Submitted);
            // Transaction is recent, not expired, not on rollup, no issues

            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            // Mock get_block_by_number for gas limit validation
            mocks.provider.expect_get_block_by_number().returning(|| {
                Box::pin(async {
                    use alloy::{network::AnyRpcBlock, rpc::types::Block};
                    let mut block: Block = Block::default();
                    block.header.gas_limit = 30_000_000u64;
                    Ok(AnyRpcBlock::from(block))
                })
            });

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let (res, reason) = evm_transaction.should_noop(&tx).await.unwrap();
            assert!(!res, "Valid transaction should not be replaced with NOOP.");
            assert!(
                reason.is_none(),
                "Reason should not be provided when should_noop is false"
            );
        }
    }

    // Tests for `update_transaction_status_if_needed`
    mod update_transaction_status_tests {
        use super::*;

        #[tokio::test]
        async fn test_no_update_when_status_is_same() {
            // Create mocks, relayer, and a transaction with status Submitted.
            let mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let tx = make_test_transaction(TransactionStatus::Submitted);
            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);

            // When new status is the same as current, update_transaction_status_if_needed
            // should simply return the original transaction.
            let updated_tx = evm_transaction
                .update_transaction_status_if_needed(tx.clone(), TransactionStatus::Submitted)
                .await
                .unwrap();
            assert_eq!(updated_tx.status, TransactionStatus::Submitted);
            assert_eq!(updated_tx.id, tx.id);
        }

        #[tokio::test]
        async fn test_updates_when_status_differs() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let tx = make_test_transaction(TransactionStatus::Submitted);

            // Mock partial_update to return a transaction with new status
            mocks
                .tx_repo
                .expect_partial_update()
                .returning(|_, update| {
                    let mut updated_tx = make_test_transaction(TransactionStatus::Submitted);
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    Ok(updated_tx)
                });

            // Mock notification job
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let updated_tx = evm_transaction
                .update_transaction_status_if_needed(tx.clone(), TransactionStatus::Mined)
                .await
                .unwrap();

            assert_eq!(updated_tx.status, TransactionStatus::Mined);
        }
    }

    // Tests for `handle_sent_state`
    mod handle_sent_state_tests {
        use super::*;

        #[tokio::test]
        async fn test_sent_state_recent_no_resend() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Sent);
            // Set sent_at to recent (e.g., 10 seconds ago)
            tx.sent_at = Some((Utc::now() - Duration::seconds(10)).to_rfc3339());

            // Mock network repository to return a test network model for should_noop check
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            // Mock get_block_by_number for gas limit validation in handle_sent_state
            mocks.provider.expect_get_block_by_number().returning(|| {
                Box::pin(async {
                    use alloy::{network::AnyRpcBlock, rpc::types::Block};
                    let mut block: Block = Block::default();
                    block.header.gas_limit = 30_000_000u64;
                    Ok(AnyRpcBlock::from(block))
                })
            });

            // Mock status check job scheduling
            mocks
                .job_producer
                .expect_produce_check_transaction_status_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.handle_sent_state(tx.clone()).await.unwrap();

            assert_eq!(result.status, TransactionStatus::Sent);
        }

        #[tokio::test]
        async fn test_sent_state_stuck_schedules_resubmit() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Sent);
            // Set sent_at to long ago (> 30 seconds for resend timeout)
            tx.sent_at = Some((Utc::now() - Duration::seconds(60)).to_rfc3339());

            // Mock network repository to return a test network model for should_noop check
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            // Mock get_block_by_number for gas limit validation in handle_sent_state
            mocks.provider.expect_get_block_by_number().returning(|| {
                Box::pin(async {
                    use alloy::{network::AnyRpcBlock, rpc::types::Block};
                    let mut block: Block = Block::default();
                    block.header.gas_limit = 30_000_000u64;
                    Ok(AnyRpcBlock::from(block))
                })
            });

            // Mock resubmit job scheduling
            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            // Mock status check job scheduling
            mocks
                .job_producer
                .expect_produce_check_transaction_status_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.handle_sent_state(tx.clone()).await.unwrap();

            assert_eq!(result.status, TransactionStatus::Sent);
        }
    }

    // Tests for `prepare_noop_update_request`
    mod prepare_noop_update_request_tests {
        use super::*;

        #[tokio::test]
        async fn test_noop_request_without_cancellation() {
            // Create a transaction with an initial noop_count of 2 and is_canceled set to false.
            let mocks = default_test_mocks_with_network();
            let relayer = create_test_relayer();
            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.noop_count = Some(2);
            tx.is_canceled = Some(false);

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let update_req = evm_transaction
                .prepare_noop_update_request(&tx, false, None)
                .await
                .unwrap();

            // NOOP count should be incremented: 2 becomes 3.
            assert_eq!(update_req.noop_count, Some(3));
            // When not cancelling, the is_canceled flag should remain as in the original transaction.
            assert_eq!(update_req.is_canceled, Some(false));
        }

        #[tokio::test]
        async fn test_noop_request_with_cancellation() {
            // Create a transaction with no initial noop_count (None) and is_canceled false.
            let mocks = default_test_mocks_with_network();
            let relayer = create_test_relayer();
            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.noop_count = None;
            tx.is_canceled = Some(false);

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let update_req = evm_transaction
                .prepare_noop_update_request(&tx, true, None)
                .await
                .unwrap();

            // NOOP count should default to 1.
            assert_eq!(update_req.noop_count, Some(1));
            // When cancelling, the is_canceled flag should be forced to true.
            assert_eq!(update_req.is_canceled, Some(true));
        }
    }

    // Tests for `handle_submitted_state`
    mod handle_submitted_state_tests {
        use super::*;

        #[tokio::test]
        async fn test_schedules_resubmit_job() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            // Set sent_at far in the past to force resubmission
            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.sent_at = Some((Utc::now() - Duration::seconds(600)).to_rfc3339());

            // Mock network repository to return a test network model for should_noop check
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            // Mock get_block_by_number for gas limit validation
            mocks.provider.expect_get_block_by_number().returning(|| {
                Box::pin(async {
                    use alloy::{network::AnyRpcBlock, rpc::types::Block};
                    let mut block: Block = Block::default();
                    block.header.gas_limit = 30_000_000u64;
                    Ok(AnyRpcBlock::from(block))
                })
            });

            // Expect the resubmit job to be produced
            mocks
                .job_producer
                .expect_produce_submit_transaction_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            // Expect status check to be scheduled
            mocks
                .job_producer
                .expect_produce_check_transaction_status_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let updated_tx = evm_transaction.handle_submitted_state(tx).await.unwrap();

            // We remain in "Submitted" after scheduling the resubmit
            assert_eq!(updated_tx.status, TransactionStatus::Submitted);
        }
    }

    // Tests for `handle_pending_state`
    mod handle_pending_state_tests {
        use super::*;

        #[tokio::test]
        async fn test_pending_state_no_noop() {
            // Create a pending transaction that is fresh (created now).
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let mut tx = make_test_transaction(TransactionStatus::Pending);
            tx.created_at = Utc::now().to_rfc3339(); // less than one minute old

            // Mock network repository to return a test network model
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            // Mock get_block_by_number for gas limit validation
            mocks.provider.expect_get_block_by_number().returning(|| {
                Box::pin(async {
                    use alloy::{network::AnyRpcBlock, rpc::types::Block};
                    let mut block: Block = Block::default();
                    block.header.gas_limit = 30_000_000u64;
                    Ok(AnyRpcBlock::from(block))
                })
            });

            // Expect status check to be scheduled when not doing NOOP
            mocks
                .job_producer
                .expect_produce_check_transaction_status_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction
                .handle_pending_state(tx.clone())
                .await
                .unwrap();

            // When should_noop returns false the original transaction is returned unchanged.
            assert_eq!(result.id, tx.id);
            assert_eq!(result.status, tx.status);
            assert_eq!(result.noop_count, tx.noop_count);
        }

        #[tokio::test]
        async fn test_pending_state_with_noop() {
            // Create a pending transaction that is old (created 2 minutes ago)
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let mut tx = make_test_transaction(TransactionStatus::Pending);
            tx.created_at = (Utc::now() - Duration::minutes(2)).to_rfc3339();

            // Mock network repository to return a test network model
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));

            // Mock get_block_by_number for gas limit validation
            mocks.provider.expect_get_block_by_number().returning(|| {
                Box::pin(async {
                    use alloy::{network::AnyRpcBlock, rpc::types::Block};
                    let mut block: Block = Block::default();
                    block.header.gas_limit = 30_000_000u64;
                    Ok(AnyRpcBlock::from(block))
                })
            });

            // Expect partial_update to be called and simulate a Failed update
            // (Pending state transactions are marked as Failed, not NOOP, since nonces aren't assigned)
            let tx_clone = tx.clone();
            mocks
                .tx_repo
                .expect_partial_update()
                .withf(move |id, update| {
                    id == "test-tx-id"
                        && update.status == Some(TransactionStatus::Failed)
                        && update.status_reason.is_some()
                })
                .returning(move |_, update| {
                    let mut updated_tx = tx_clone.clone();
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    updated_tx.status_reason = update.status_reason.clone();
                    Ok(updated_tx)
                });
            // Expect that a notification is produced (no submit job needed for Failed status)
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction
                .handle_pending_state(tx.clone())
                .await
                .unwrap();

            // Since should_noop returns true for pending timeout, transaction should be marked as Failed
            assert_eq!(result.status, TransactionStatus::Failed);
            assert!(result.status_reason.is_some());
            assert!(result.status_reason.unwrap().contains("Pending state"));
        }
    }

    // Tests for `handle_mined_state`
    mod handle_mined_state_tests {
        use super::*;

        #[tokio::test]
        async fn test_updates_status_and_schedules_check() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            // Create a transaction in Submitted state (the mined branch is reached via status check).
            let tx = make_test_transaction(TransactionStatus::Submitted);

            // Expect schedule_status_check to be called with delay 5.
            mocks
                .job_producer
                .expect_produce_check_transaction_status_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));
            // Expect partial_update to update the transaction status to Mined.
            mocks
                .tx_repo
                .expect_partial_update()
                .returning(|_, update| {
                    let mut updated_tx = make_test_transaction(TransactionStatus::Submitted);
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    Ok(updated_tx)
                });

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction
                .handle_mined_state(tx.clone())
                .await
                .unwrap();
            assert_eq!(result.status, TransactionStatus::Mined);
        }
    }

    // Tests for `handle_final_state`
    mod handle_final_state_tests {
        use super::*;

        #[tokio::test]
        async fn test_final_state_confirmed() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let tx = make_test_transaction(TransactionStatus::Submitted);

            // Expect partial_update to update status to Confirmed.
            mocks
                .tx_repo
                .expect_partial_update()
                .returning(|_, update| {
                    let mut updated_tx = make_test_transaction(TransactionStatus::Submitted);
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    Ok(updated_tx)
                });

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction
                .handle_final_state(tx.clone(), TransactionStatus::Confirmed)
                .await
                .unwrap();
            assert_eq!(result.status, TransactionStatus::Confirmed);
        }

        #[tokio::test]
        async fn test_final_state_failed() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let tx = make_test_transaction(TransactionStatus::Submitted);

            // Expect partial_update to update status to Failed.
            mocks
                .tx_repo
                .expect_partial_update()
                .returning(|_, update| {
                    let mut updated_tx = make_test_transaction(TransactionStatus::Submitted);
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    Ok(updated_tx)
                });

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction
                .handle_final_state(tx.clone(), TransactionStatus::Failed)
                .await
                .unwrap();
            assert_eq!(result.status, TransactionStatus::Failed);
        }

        #[tokio::test]
        async fn test_final_state_expired() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let tx = make_test_transaction(TransactionStatus::Submitted);

            // Expect partial_update to update status to Expired.
            mocks
                .tx_repo
                .expect_partial_update()
                .returning(|_, update| {
                    let mut updated_tx = make_test_transaction(TransactionStatus::Submitted);
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    Ok(updated_tx)
                });

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction
                .handle_final_state(tx.clone(), TransactionStatus::Expired)
                .await
                .unwrap();
            assert_eq!(result.status, TransactionStatus::Expired);
        }
    }

    // Integration tests for `handle_status_impl`
    mod handle_status_impl_tests {
        use super::*;

        #[tokio::test]
        async fn test_impl_submitted_branch() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.sent_at = Some((Utc::now() - Duration::seconds(120)).to_rfc3339());
            // Set a dummy hash so check_transaction_status can proceed.
            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.hash = Some("0xFakeHash".to_string());
            }
            // Simulate no receipt found.
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|_| Box::pin(async { Ok(None) }));
            // Mock network repository for should_resubmit check
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));
            // Expect that a status check job is scheduled.
            mocks
                .job_producer
                .expect_produce_check_transaction_status_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));
            // Expect update_transaction_status_if_needed to update status to Submitted.
            mocks
                .tx_repo
                .expect_partial_update()
                .returning(|_, update| {
                    let mut updated_tx = make_test_transaction(TransactionStatus::Submitted);
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    Ok(updated_tx)
                });

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.handle_status_impl(tx).await.unwrap();
            assert_eq!(result.status, TransactionStatus::Submitted);
        }

        #[tokio::test]
        async fn test_impl_mined_branch() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            // Set created_at to be old enough to pass is_too_early_to_resubmit
            tx.created_at = (Utc::now() - Duration::minutes(1)).to_rfc3339();
            // Set a dummy hash.
            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.hash = Some("0xFakeHash".to_string());
            }
            // Simulate a receipt with a block number of 100 and a successful receipt.
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|_| Box::pin(async { Ok(Some(make_mock_receipt(true, Some(100)))) }));
            // Simulate that the current block number is 100 (so confirmations are insufficient).
            mocks
                .provider
                .expect_get_block_number()
                .return_once(|| Box::pin(async { Ok(100) }));
            // Mock network repository to return a test network model
            mocks
                .network_repo
                .expect_get_by_chain_id()
                .returning(|_, _| Ok(Some(create_test_network_model())));
            // Mock the notification job that gets sent after status update
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));
            // Expect get_by_id to reload the transaction after status change
            mocks.tx_repo.expect_get_by_id().returning(|_| {
                let updated_tx = make_test_transaction(TransactionStatus::Mined);
                Ok(updated_tx)
            });
            // Expect update_transaction_status_if_needed to update status to Mined.
            mocks
                .tx_repo
                .expect_partial_update()
                .returning(|_, update| {
                    let mut updated_tx = make_test_transaction(TransactionStatus::Submitted);
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    Ok(updated_tx)
                });

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.handle_status_impl(tx).await.unwrap();
            assert_eq!(result.status, TransactionStatus::Mined);
        }

        #[tokio::test]
        async fn test_impl_final_confirmed_branch() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            // Create a transaction with status Confirmed.
            let tx = make_test_transaction(TransactionStatus::Confirmed);

            // In this branch, check_transaction_status returns the final status immediately,
            // so we expect partial_update to update the transaction status to Confirmed.
            mocks
                .tx_repo
                .expect_partial_update()
                .returning(|_, update| {
                    let mut updated_tx = make_test_transaction(TransactionStatus::Submitted);
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    Ok(updated_tx)
                });

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.handle_status_impl(tx).await.unwrap();
            assert_eq!(result.status, TransactionStatus::Confirmed);
        }

        #[tokio::test]
        async fn test_impl_final_failed_branch() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            // Create a transaction with status Failed.
            let tx = make_test_transaction(TransactionStatus::Failed);

            mocks
                .tx_repo
                .expect_partial_update()
                .returning(|_, update| {
                    let mut updated_tx = make_test_transaction(TransactionStatus::Submitted);
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    Ok(updated_tx)
                });

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.handle_status_impl(tx).await.unwrap();
            assert_eq!(result.status, TransactionStatus::Failed);
        }

        #[tokio::test]
        async fn test_impl_final_expired_branch() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();
            // Create a transaction with status Expired.
            let tx = make_test_transaction(TransactionStatus::Expired);

            mocks
                .tx_repo
                .expect_partial_update()
                .returning(|_, update| {
                    let mut updated_tx = make_test_transaction(TransactionStatus::Submitted);
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    Ok(updated_tx)
                });

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.handle_status_impl(tx).await.unwrap();
            assert_eq!(result.status, TransactionStatus::Expired);
        }
    }

    // Tests for hash recovery functions
    mod hash_recovery_tests {
        use super::*;

        #[tokio::test]
        async fn test_should_try_hash_recovery_not_submitted() {
            let mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Sent);
            tx.hashes = vec![
                "0xHash1".to_string(),
                "0xHash2".to_string(),
                "0xHash3".to_string(),
            ];

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.should_try_hash_recovery(&tx).unwrap();

            assert!(
                !result,
                "Should not attempt recovery for non-Submitted transactions"
            );
        }

        #[tokio::test]
        async fn test_should_try_hash_recovery_not_enough_hashes() {
            let mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.hashes = vec!["0xHash1".to_string()]; // Only 1 hash
            tx.sent_at = Some((Utc::now() - Duration::minutes(3)).to_rfc3339());

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.should_try_hash_recovery(&tx).unwrap();

            assert!(
                !result,
                "Should not attempt recovery with insufficient hashes"
            );
        }

        #[tokio::test]
        async fn test_should_try_hash_recovery_too_recent() {
            let mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.hashes = vec![
                "0xHash1".to_string(),
                "0xHash2".to_string(),
                "0xHash3".to_string(),
            ];
            tx.sent_at = Some(Utc::now().to_rfc3339()); // Recent

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.should_try_hash_recovery(&tx).unwrap();

            assert!(
                !result,
                "Should not attempt recovery for recently sent transactions"
            );
        }

        #[tokio::test]
        async fn test_should_try_hash_recovery_success() {
            let mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.hashes = vec![
                "0xHash1".to_string(),
                "0xHash2".to_string(),
                "0xHash3".to_string(),
            ];
            tx.sent_at = Some((Utc::now() - Duration::minutes(3)).to_rfc3339());

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.should_try_hash_recovery(&tx).unwrap();

            assert!(
                result,
                "Should attempt recovery for stuck transactions with multiple hashes"
            );
        }

        #[tokio::test]
        async fn test_try_recover_no_historical_hash_found() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.hashes = vec![
                "0xHash1".to_string(),
                "0xHash2".to_string(),
                "0xHash3".to_string(),
            ];

            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.hash = Some("0xHash3".to_string());
            }

            // Mock provider to return None for all hash lookups
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|_| Box::pin(async { Ok(None) }));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let evm_data = tx.network_data.get_evm_transaction_data().unwrap();
            let result = evm_transaction
                .try_recover_with_historical_hashes(&tx, &evm_data)
                .await
                .unwrap();

            assert!(
                result.is_none(),
                "Should return None when no historical hash is found"
            );
        }

        #[tokio::test]
        async fn test_try_recover_finds_mined_historical_hash() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.hashes = vec![
                "0xHash1".to_string(),
                "0xHash2".to_string(), // This one is mined
                "0xHash3".to_string(),
            ];

            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.hash = Some("0xHash3".to_string()); // Current hash (wrong one)
            }

            // Mock provider to return None for Hash1 and Hash3, but receipt for Hash2
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|hash| {
                    if hash == "0xHash2" {
                        Box::pin(async { Ok(Some(make_mock_receipt(true, Some(100)))) })
                    } else {
                        Box::pin(async { Ok(None) })
                    }
                });

            // Mock partial_update for correcting the hash
            let tx_clone = tx.clone();
            mocks
                .tx_repo
                .expect_partial_update()
                .returning(move |_, update| {
                    let mut updated_tx = tx_clone.clone();
                    if let Some(status) = update.status {
                        updated_tx.status = status;
                    }
                    if let Some(NetworkTransactionData::Evm(ref evm_data)) = update.network_data {
                        if let NetworkTransactionData::Evm(ref mut updated_evm) =
                            updated_tx.network_data
                        {
                            updated_evm.hash = evm_data.hash.clone();
                        }
                    }
                    Ok(updated_tx)
                });

            // Mock notification job
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let evm_data = tx.network_data.get_evm_transaction_data().unwrap();
            let result = evm_transaction
                .try_recover_with_historical_hashes(&tx, &evm_data)
                .await
                .unwrap();

            assert!(result.is_some(), "Should recover the transaction");
            let recovered_tx = result.unwrap();
            assert_eq!(recovered_tx.status, TransactionStatus::Mined);
        }

        #[tokio::test]
        async fn test_try_recover_network_error_continues() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            tx.hashes = vec![
                "0xHash1".to_string(),
                "0xHash2".to_string(), // Network error
                "0xHash3".to_string(), // This one is mined
            ];

            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.hash = Some("0xHash1".to_string());
            }

            // Mock provider to return error for Hash2, receipt for Hash3
            mocks
                .provider
                .expect_get_transaction_receipt()
                .returning(|hash| {
                    if hash == "0xHash2" {
                        Box::pin(async { Err(crate::services::provider::ProviderError::Timeout) })
                    } else if hash == "0xHash3" {
                        Box::pin(async { Ok(Some(make_mock_receipt(true, Some(100)))) })
                    } else {
                        Box::pin(async { Ok(None) })
                    }
                });

            // Mock partial_update for correcting the hash
            let tx_clone = tx.clone();
            mocks
                .tx_repo
                .expect_partial_update()
                .returning(move |_, update| {
                    let mut updated_tx = tx_clone.clone();
                    if let Some(status) = update.status {
                        updated_tx.status = status;
                    }
                    Ok(updated_tx)
                });

            // Mock notification job
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let evm_data = tx.network_data.get_evm_transaction_data().unwrap();
            let result = evm_transaction
                .try_recover_with_historical_hashes(&tx, &evm_data)
                .await
                .unwrap();

            assert!(
                result.is_some(),
                "Should continue checking after network error and find mined hash"
            );
        }

        #[tokio::test]
        async fn test_update_transaction_with_corrected_hash() {
            let mut mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let mut tx = make_test_transaction(TransactionStatus::Submitted);
            if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
                evm_data.hash = Some("0xWrongHash".to_string());
            }

            // Mock partial_update
            mocks
                .tx_repo
                .expect_partial_update()
                .returning(move |_, update| {
                    let mut updated_tx = make_test_transaction(TransactionStatus::Submitted);
                    if let Some(status) = update.status {
                        updated_tx.status = status;
                    }
                    if let Some(NetworkTransactionData::Evm(ref evm_data)) = update.network_data {
                        if let NetworkTransactionData::Evm(ref mut updated_evm) =
                            updated_tx.network_data
                        {
                            updated_evm.hash = evm_data.hash.clone();
                        }
                    }
                    Ok(updated_tx)
                });

            // Mock notification job
            mocks
                .job_producer
                .expect_produce_send_notification_job()
                .returning(|_, _| Box::pin(async { Ok(()) }));

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let evm_data = tx.network_data.get_evm_transaction_data().unwrap();
            let result = evm_transaction
                .update_transaction_with_corrected_hash(
                    &tx,
                    &evm_data,
                    "0xCorrectHash",
                    TransactionStatus::Mined,
                )
                .await
                .unwrap();

            assert_eq!(result.status, TransactionStatus::Mined);
            if let NetworkTransactionData::Evm(ref updated_evm) = result.network_data {
                assert_eq!(updated_evm.hash.as_ref().unwrap(), "0xCorrectHash");
            }
        }
    }

    // Tests for check_transaction_status edge cases
    mod check_transaction_status_edge_cases {
        use super::*;

        #[tokio::test]
        async fn test_missing_hash_returns_error() {
            let mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let tx = make_test_transaction(TransactionStatus::Submitted);
            // Hash is None by default

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let result = evm_transaction.check_transaction_status(&tx).await;

            assert!(result.is_err(), "Should return error when hash is missing");
        }

        #[tokio::test]
        async fn test_pending_status_early_return() {
            let mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let tx = make_test_transaction(TransactionStatus::Pending);

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let status = evm_transaction.check_transaction_status(&tx).await.unwrap();

            assert_eq!(
                status,
                TransactionStatus::Pending,
                "Should return Pending without querying blockchain"
            );
        }

        #[tokio::test]
        async fn test_sent_status_early_return() {
            let mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let tx = make_test_transaction(TransactionStatus::Sent);

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let status = evm_transaction.check_transaction_status(&tx).await.unwrap();

            assert_eq!(
                status,
                TransactionStatus::Sent,
                "Should return Sent without querying blockchain"
            );
        }

        #[tokio::test]
        async fn test_final_state_early_return() {
            let mocks = default_test_mocks();
            let relayer = create_test_relayer();

            let tx = make_test_transaction(TransactionStatus::Confirmed);

            let evm_transaction = make_test_evm_relayer_transaction(relayer, mocks);
            let status = evm_transaction.check_transaction_status(&tx).await.unwrap();

            assert_eq!(
                status,
                TransactionStatus::Confirmed,
                "Should return final state without querying blockchain"
            );
        }
    }
}
