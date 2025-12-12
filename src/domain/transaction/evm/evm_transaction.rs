//! This module defines the `EvmRelayerTransaction` struct and its associated
//! functionality for handling Ethereum Virtual Machine (EVM) transactions.
//! It includes methods for preparing, submitting, handling status, and
//! managing notifications for transactions. The module leverages various
//! services and repositories to perform these operations asynchronously.

use async_trait::async_trait;
use chrono::Utc;
use eyre::Result;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

use crate::{
    constants::{DEFAULT_EVM_GAS_LIMIT_ESTIMATION, GAS_LIMIT_BUFFER_MULTIPLIER},
    domain::{
        evm::is_noop,
        transaction::{
            evm::{ensure_status, ensure_status_one_of, PriceCalculator, PriceCalculatorTrait},
            Transaction,
        },
        EvmTransactionValidationError, EvmTransactionValidator,
    },
    jobs::{JobProducer, JobProducerTrait, TransactionSend, TransactionStatusCheck},
    models::{
        produce_transaction_update_notification_payload, EvmNetwork, EvmTransactionData,
        NetworkRepoModel, NetworkTransactionData, NetworkTransactionRequest, NetworkType,
        RelayerEvmPolicy, RelayerRepoModel, TransactionError, TransactionRepoModel,
        TransactionStatus, TransactionUpdateRequest,
    },
    repositories::{
        NetworkRepository, NetworkRepositoryStorage, RelayerRepository, RelayerRepositoryStorage,
        Repository, TransactionCounterRepositoryStorage, TransactionCounterTrait,
        TransactionRepository, TransactionRepositoryStorage,
    },
    services::{
        gas::evm_gas_price::EvmGasPriceService,
        provider::{EvmProvider, EvmProviderTrait},
        signer::{EvmSigner, Signer},
    },
    utils::{calculate_scheduled_timestamp, get_evm_default_gas_limit_for_tx},
};

use super::PriceParams;

#[allow(dead_code)]
pub struct EvmRelayerTransaction<P, RR, NR, TR, J, S, TCR, PC>
where
    P: EvmProviderTrait,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    J: JobProducerTrait + Send + Sync + 'static,
    S: Signer + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PC: PriceCalculatorTrait,
{
    provider: P,
    relayer_repository: Arc<RR>,
    network_repository: Arc<NR>,
    transaction_repository: Arc<TR>,
    job_producer: Arc<J>,
    signer: S,
    relayer: RelayerRepoModel,
    transaction_counter_service: Arc<TCR>,
    price_calculator: PC,
}

#[allow(dead_code, clippy::too_many_arguments)]
impl<P, RR, NR, TR, J, S, TCR, PC> EvmRelayerTransaction<P, RR, NR, TR, J, S, TCR, PC>
where
    P: EvmProviderTrait,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    J: JobProducerTrait + Send + Sync + 'static,
    S: Signer + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PC: PriceCalculatorTrait,
{
    /// Creates a new `EvmRelayerTransaction`.
    ///
    /// # Arguments
    ///
    /// * `relayer` - The relayer model.
    /// * `provider` - The EVM provider.
    /// * `relayer_repository` - Storage for relayer repository.
    /// * `transaction_repository` - Storage for transaction repository.
    /// * `transaction_counter_service` - Service for managing transaction counters.
    /// * `job_producer` - Producer for job queue.
    /// * `price_calculator` - Price calculator for gas price management.
    /// * `signer` - The EVM signer.
    ///
    /// # Returns
    ///
    /// A result containing the new `EvmRelayerTransaction` or a `TransactionError`.
    pub fn new(
        relayer: RelayerRepoModel,
        provider: P,
        relayer_repository: Arc<RR>,
        network_repository: Arc<NR>,
        transaction_repository: Arc<TR>,
        transaction_counter_service: Arc<TCR>,
        job_producer: Arc<J>,
        price_calculator: PC,
        signer: S,
    ) -> Result<Self, TransactionError> {
        Ok(Self {
            relayer,
            provider,
            relayer_repository,
            network_repository,
            transaction_repository,
            transaction_counter_service,
            job_producer,
            price_calculator,
            signer,
        })
    }

    /// Returns a reference to the provider.
    pub fn provider(&self) -> &P {
        &self.provider
    }

    /// Returns a reference to the relayer model.
    pub fn relayer(&self) -> &RelayerRepoModel {
        &self.relayer
    }

    /// Returns a reference to the network repository.
    pub fn network_repository(&self) -> &NR {
        &self.network_repository
    }

    /// Returns a reference to the job producer.
    pub fn job_producer(&self) -> &J {
        &self.job_producer
    }

    pub fn transaction_repository(&self) -> &TR {
        &self.transaction_repository
    }

    /// Checks if a provider error indicates the transaction was already submitted to the blockchain.
    /// This handles cases where the transaction was submitted by another instance or in a previous retry.
    fn is_already_submitted_error(error: &impl std::fmt::Display) -> bool {
        let error_msg = error.to_string().to_lowercase();
        error_msg.contains("already known")
            || error_msg.contains("nonce too low")
            || error_msg.contains("replacement transaction underpriced")
    }

    /// Helper method to schedule a transaction status check job.
    pub(super) async fn schedule_status_check(
        &self,
        tx: &TransactionRepoModel,
        delay_seconds: Option<i64>,
    ) -> Result<(), TransactionError> {
        let delay = delay_seconds.map(calculate_scheduled_timestamp);
        self.job_producer()
            .produce_check_transaction_status_job(
                TransactionStatusCheck::new(
                    tx.id.clone(),
                    tx.relayer_id.clone(),
                    crate::models::NetworkType::Evm,
                ),
                delay,
            )
            .await
            .map_err(|e| {
                TransactionError::UnexpectedError(format!("Failed to schedule status check: {e}"))
            })
    }

    /// Helper method to produce a submit transaction job.
    pub(super) async fn send_transaction_submit_job(
        &self,
        tx: &TransactionRepoModel,
    ) -> Result<(), TransactionError> {
        let job = TransactionSend::submit(tx.id.clone(), tx.relayer_id.clone());

        self.job_producer()
            .produce_submit_transaction_job(job, None)
            .await
            .map_err(|e| {
                TransactionError::UnexpectedError(format!("Failed to produce submit job: {e}"))
            })
    }

    /// Helper method to produce a resubmit transaction job.
    pub(super) async fn send_transaction_resubmit_job(
        &self,
        tx: &TransactionRepoModel,
    ) -> Result<(), TransactionError> {
        let job = TransactionSend::resubmit(tx.id.clone(), tx.relayer_id.clone());

        self.job_producer()
            .produce_submit_transaction_job(job, None)
            .await
            .map_err(|e| {
                TransactionError::UnexpectedError(format!("Failed to produce resubmit job: {e}"))
            })
    }

    /// Helper method to produce a resend transaction job.
    pub(super) async fn send_transaction_resend_job(
        &self,
        tx: &TransactionRepoModel,
    ) -> Result<(), TransactionError> {
        let job = TransactionSend::resend(tx.id.clone(), tx.relayer_id.clone());

        self.job_producer()
            .produce_submit_transaction_job(job, None)
            .await
            .map_err(|e| {
                TransactionError::UnexpectedError(format!("Failed to produce resend job: {e}"))
            })
    }

    /// Helper method to produce a transaction request (prepare) job.
    pub(super) async fn send_transaction_request_job(
        &self,
        tx: &TransactionRepoModel,
    ) -> Result<(), TransactionError> {
        use crate::jobs::TransactionRequest;

        let job = TransactionRequest::new(tx.id.clone(), tx.relayer_id.clone());

        self.job_producer()
            .produce_transaction_request_job(job, None)
            .await
            .map_err(|e| {
                TransactionError::UnexpectedError(format!("Failed to produce request job: {e}"))
            })
    }

    /// Updates a transaction's status.
    pub(super) async fn update_transaction_status(
        &self,
        tx: TransactionRepoModel,
        new_status: TransactionStatus,
    ) -> Result<TransactionRepoModel, TransactionError> {
        let confirmed_at = if new_status == TransactionStatus::Confirmed {
            Some(Utc::now().to_rfc3339())
        } else {
            None
        };

        let update_request = TransactionUpdateRequest {
            status: Some(new_status),
            confirmed_at,
            ..Default::default()
        };

        let updated_tx = self
            .transaction_repository()
            .partial_update(tx.id.clone(), update_request)
            .await?;

        if let Err(e) = self.send_transaction_update_notification(&updated_tx).await {
            error!(
                tx_id = %updated_tx.id,
                status = ?updated_tx.status,
                "sending transaction update notification failed: {:?}",
                e
            );
        }
        Ok(updated_tx)
    }

    /// Sends a transaction update notification if a notification ID is configured.
    ///
    /// This is a best-effort operation that logs errors but does not propagate them,
    /// as notification failures should not affect the transaction lifecycle.
    pub(super) async fn send_transaction_update_notification(
        &self,
        tx: &TransactionRepoModel,
    ) -> Result<(), eyre::Report> {
        if let Some(notification_id) = &self.relayer().notification_id {
            self.job_producer()
                .produce_send_notification_job(
                    produce_transaction_update_notification_payload(notification_id, tx),
                    None,
                )
                .await?;
        }
        Ok(())
    }

    /// Marks a transaction as failed with a reason, updates it, sends notification, and returns the updated transaction.
    ///
    /// This is a common pattern used when a transaction should be marked as failed.
    ///
    /// # Arguments
    ///
    /// * `tx` - The transaction to mark as failed
    /// * `reason` - The reason for the failure
    /// * `error_context` - Context string for error logging (e.g., "gas limit exceeds block gas limit")
    ///
    /// # Returns
    ///
    /// The updated transaction with Failed status
    async fn mark_transaction_as_failed(
        &self,
        tx: &TransactionRepoModel,
        reason: String,
        error_context: &str,
    ) -> Result<TransactionRepoModel, TransactionError> {
        let update = TransactionUpdateRequest {
            status: Some(TransactionStatus::Failed),
            status_reason: Some(reason.clone()),
            ..Default::default()
        };

        let updated_tx = self
            .transaction_repository
            .partial_update(tx.id.clone(), update)
            .await?;

        if let Err(e) = self.send_transaction_update_notification(&updated_tx).await {
            error!(
                tx_id = %updated_tx.id,
                status = ?TransactionStatus::Failed,
                "sending transaction update notification failed for {}: {:?}",
                error_context,
                e
            );
        }

        Ok(updated_tx)
    }

    /// Validates that the relayer has sufficient balance for the transaction.
    ///
    /// # Arguments
    ///
    /// * `total_cost` - The total cost of the transaction (gas + value)
    ///
    /// # Returns
    ///
    /// A `Result` indicating success or a `TransactionError`.
    /// - Returns `InsufficientBalance` only when balance is truly insufficient (permanent failure)
    /// - Returns `UnexpectedError` for RPC/network issues (retryable)
    async fn ensure_sufficient_balance(
        &self,
        total_cost: crate::models::U256,
    ) -> Result<(), TransactionError> {
        EvmTransactionValidator::validate_sufficient_relayer_balance(
            total_cost,
            &self.relayer().address,
            &self.relayer().policies.get_evm_policy(),
            &self.provider,
        )
        .await
        .map_err(|validation_error| match validation_error {
            // Only convert actual insufficient balance to permanent failure
            EvmTransactionValidationError::InsufficientBalance(msg) => {
                TransactionError::InsufficientBalance(msg)
            }
            // Provider errors are retryable (RPC down, timeout, etc.)
            EvmTransactionValidationError::ProviderError(msg) => {
                TransactionError::UnexpectedError(format!("Failed to check balance: {msg}"))
            }
            // Validation errors are also retryable
            EvmTransactionValidationError::ValidationError(msg) => {
                TransactionError::UnexpectedError(format!("Balance validation error: {msg}"))
            }
        })
    }

    /// Estimates the gas limit for a transaction.
    ///
    /// # Arguments
    ///
    /// * `evm_data` - The EVM transaction data.
    /// * `relayer_policy` - The relayer policy.
    ///
    async fn estimate_tx_gas_limit(
        &self,
        evm_data: &EvmTransactionData,
        relayer_policy: &RelayerEvmPolicy,
    ) -> Result<u64, TransactionError> {
        if !relayer_policy
            .gas_limit_estimation
            .unwrap_or(DEFAULT_EVM_GAS_LIMIT_ESTIMATION)
        {
            warn!("gas limit estimation is disabled for relayer");
            return Err(TransactionError::UnexpectedError(
                "Gas limit estimation is disabled".to_string(),
            ));
        }

        let estimated_gas = self.provider.estimate_gas(evm_data).await.map_err(|e| {
            warn!(error = ?e, tx_data = ?evm_data, "failed to estimate gas");
            TransactionError::UnexpectedError(format!("Failed to estimate gas: {e}"))
        })?;

        Ok(estimated_gas * GAS_LIMIT_BUFFER_MULTIPLIER / 100)
    }
}

#[async_trait]
impl<P, RR, NR, TR, J, S, TCR, PC> Transaction
    for EvmRelayerTransaction<P, RR, NR, TR, J, S, TCR, PC>
where
    P: EvmProviderTrait + Send + Sync + 'static,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    J: JobProducerTrait + Send + Sync + 'static,
    S: Signer + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PC: PriceCalculatorTrait + Send + Sync + 'static,
{
    /// Prepares a transaction for submission.
    ///
    /// # Arguments
    ///
    /// * `tx` - The transaction model to prepare.
    ///
    /// # Returns
    ///
    /// A result containing the updated transaction model or a `TransactionError`.
    async fn prepare_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        debug!("preparing transaction {}", tx.id);

        // If transaction is not in Pending status, return Ok to avoid wasteful retries
        // (e.g., if it's already Sent, Failed, or in another state)
        if let Err(e) = ensure_status(&tx, TransactionStatus::Pending, Some("prepare_transaction"))
        {
            warn!(
                tx_id = %tx.id,
                status = ?tx.status,
                error = %e,
                "transaction not in Pending status, skipping preparation"
            );
            return Ok(tx);
        }

        let mut evm_data = tx.network_data.get_evm_transaction_data()?;
        let relayer = self.relayer();

        if evm_data.gas_limit.is_none() {
            match self
                .estimate_tx_gas_limit(&evm_data, &relayer.policies.get_evm_policy())
                .await
            {
                Ok(estimated_gas_limit) => {
                    evm_data.gas_limit = Some(estimated_gas_limit);
                }
                Err(estimation_error) => {
                    error!(error = ?estimation_error, "failed to estimate gas limit");

                    let default_gas_limit = get_evm_default_gas_limit_for_tx(&evm_data);
                    debug!(gas_limit = %default_gas_limit, "fallback to default gas limit");
                    evm_data.gas_limit = Some(default_gas_limit);
                }
            }
        } else {
            // do user gas limit validation against block gas limit
            let block = self.provider.get_block_by_number().await;
            if let Ok(block) = block {
                let block_gas_limit = block.header.gas_limit;
                if let Some(gas_limit) = evm_data.gas_limit {
                    if gas_limit > block_gas_limit {
                        let reason = format!(
                            "Transaction gas limit ({gas_limit}) exceeds block gas limit ({block_gas_limit})",
                        );
                        warn!(
                            tx_id = %tx.id,
                            tx_gas_limit = %gas_limit,
                            block_gas_limit = %block_gas_limit,
                            "transaction gas limit exceeds block gas limit"
                        );

                        let updated_tx = self
                            .mark_transaction_as_failed(
                                &tx,
                                reason,
                                "gas limit exceeds block gas limit",
                            )
                            .await?;
                        return Ok(updated_tx);
                    }
                }
            }
        }

        // set the gas price
        let price_params: PriceParams = self
            .price_calculator
            .get_transaction_price_params(&evm_data, relayer)
            .await?;

        debug!(gas_price = ?price_params.gas_price, "gas price");

        // Validate the relayer has sufficient balance before consuming nonce and signing
        if let Err(balance_error) = self
            .ensure_sufficient_balance(price_params.total_cost)
            .await
        {
            // Only mark as Failed for actual insufficient balance, not RPC errors
            match &balance_error {
                TransactionError::InsufficientBalance(_) => {
                    warn!(error = %balance_error, "insufficient balance for transaction");

                    let updated_tx = self
                        .mark_transaction_as_failed(
                            &tx,
                            balance_error.to_string(),
                            "insufficient balance",
                        )
                        .await?;

                    // Return Ok since transaction is in final Failed state - no retry needed
                    return Ok(updated_tx);
                }
                // For RPC/provider errors, propagate without marking as Failed
                // This allows the handler to retry
                _ => {
                    debug!(error = %balance_error, "failed to check balance, will retry");
                    return Err(balance_error);
                }
            }
        }

        // Check if transaction already has a nonce (recovery from failed signing attempt)
        let tx_with_nonce = if let Some(existing_nonce) = evm_data.nonce {
            debug!(
                nonce = existing_nonce,
                "transaction already has nonce assigned, reusing for retry"
            );
            // Retry flow: When reusing an existing nonce from a failed attempt, we intentionally
            // do NOT persist the fresh price_params (computed earlier) to the DB here. The DB may
            // temporarily hold stale price_params from the failed attempt. However, fresh price_params
            // are applied just before signing, ensuring the transaction uses
            // current gas prices.
            tx
        } else {
            // Balance validation passed, proceed to increment nonce
            let new_nonce = self
                .transaction_counter_service
                .get_and_increment(&self.relayer.id, &self.relayer.address)
                .await
                .map_err(|e| TransactionError::UnexpectedError(e.to_string()))?;

            debug!(nonce = new_nonce, "assigned new nonce to transaction");

            let updated_evm_data = evm_data
                .with_price_params(price_params.clone())
                .with_nonce(new_nonce);

            // Save transaction with nonce BEFORE signing
            // This ensures we can recover if signing fails (timeout, KMS error, etc.)
            let presign_update = TransactionUpdateRequest {
                network_data: Some(NetworkTransactionData::Evm(updated_evm_data.clone())),
                priced_at: Some(Utc::now().to_rfc3339()),
                ..Default::default()
            };

            self.transaction_repository
                .partial_update(tx.id.clone(), presign_update)
                .await?
        };

        // Apply price params for signing (recalculated on every attempt)
        let updated_evm_data = tx_with_nonce
            .network_data
            .get_evm_transaction_data()?
            .with_price_params(price_params.clone());

        // Now sign the transaction - if this fails, we still have the tx with nonce saved
        let sig_result = self
            .signer
            .sign_transaction(NetworkTransactionData::Evm(updated_evm_data.clone()))
            .await?;

        let updated_evm_data =
            updated_evm_data.with_signed_transaction_data(sig_result.into_evm()?);

        // Track the transaction hash
        let mut hashes = tx_with_nonce.hashes.clone();
        if let Some(hash) = updated_evm_data.hash.clone() {
            hashes.push(hash);
        }

        // Update with signed data and mark as Sent
        let postsign_update = TransactionUpdateRequest {
            status: Some(TransactionStatus::Sent),
            network_data: Some(NetworkTransactionData::Evm(updated_evm_data)),
            hashes: Some(hashes),
            ..Default::default()
        };

        let updated_tx = self
            .transaction_repository
            .partial_update(tx_with_nonce.id.clone(), postsign_update)
            .await?;

        // after preparing the transaction, we need to submit it to the job queue
        self.job_producer
            .produce_submit_transaction_job(
                TransactionSend::submit(updated_tx.id.clone(), updated_tx.relayer_id.clone()),
                None,
            )
            .await?;

        if let Err(e) = self.send_transaction_update_notification(&updated_tx).await {
            error!(
                tx_id = %updated_tx.id,
                status = ?TransactionStatus::Sent,
                "sending transaction update notification failed after prepare: {:?}",
                e
            );
        }

        Ok(updated_tx)
    }

    /// Submits a transaction for processing.
    ///
    /// # Arguments
    ///
    /// * `tx` - The transaction model to submit.
    ///
    /// # Returns
    ///
    /// A result containing the updated transaction model or a `TransactionError`.
    async fn submit_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        debug!("submitting transaction {}", tx.id);

        // If transaction is not in correct status, return Ok to avoid wasteful retries
        // (e.g., if it's already in a final state like Failed, Confirmed, etc.)
        if let Err(e) = ensure_status_one_of(
            &tx,
            &[TransactionStatus::Sent, TransactionStatus::Submitted],
            Some("submit_transaction"),
        ) {
            warn!(
                tx_id = %tx.id,
                status = ?tx.status,
                error = %e,
                "transaction not in expected status for submission, skipping"
            );
            return Ok(tx);
        }

        let evm_tx_data = tx.network_data.get_evm_transaction_data()?;
        let raw_tx = evm_tx_data.raw.as_ref().ok_or_else(|| {
            TransactionError::InvalidType("Raw transaction data is missing".to_string())
        })?;

        // Send transaction to blockchain - this is the critical operation
        // If this fails, retry is safe due to nonce idempotency
        match self.provider.send_raw_transaction(raw_tx).await {
            Ok(_) => {
                // Transaction submitted successfully
            }
            Err(e) => {
                // SAFETY CHECK: If transaction is in Sent status and we get "already known" or
                // "nonce too low" errors, it means the transaction was already submitted
                // (possibly by another instance or in a previous retry)
                if tx.status == TransactionStatus::Sent && Self::is_already_submitted_error(&e) {
                    warn!(
                        tx_id = %tx.id,
                        error = %e,
                        "transaction appears to be already submitted based on RPC error - treating as success"
                    );
                    // Continue to update status to Submitted
                } else {
                    // Real error - propagate it
                    return Err(e.into());
                }
            }
        }

        // Transaction is now on-chain - update database
        // If this fails, transaction is still valid, just not tracked correctly
        let update = TransactionUpdateRequest {
            status: Some(TransactionStatus::Submitted),
            sent_at: Some(Utc::now().to_rfc3339()),
            ..Default::default()
        };

        let updated_tx = match self
            .transaction_repository
            .partial_update(tx.id.clone(), update)
            .await
        {
            Ok(tx) => tx,
            Err(e) => {
                error!(
                    error = %e,
                    tx_id = %tx.id,
                    "CRITICAL: transaction sent to blockchain but failed to update database - transaction may not be tracked correctly"
                );
                // Transaction is on-chain - don't propagate error to avoid wasteful retries
                // Return the original transaction data
                tx
            }
        };

        if let Err(e) = self.send_transaction_update_notification(&updated_tx).await {
            error!(
                tx_id = %updated_tx.id,
                status = ?TransactionStatus::Submitted,
                "sending transaction update notification failed after submit: {:?}",
                e
            );
        }

        Ok(updated_tx)
    }

    /// Handles the status of a transaction.
    ///
    /// # Arguments
    ///
    /// * `tx` - The transaction model to handle.
    ///
    /// # Returns
    ///
    /// A result containing the updated transaction model or a `TransactionError`.
    async fn handle_transaction_status(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        self.handle_status_impl(tx).await
    }
    /// Resubmits a transaction with updated parameters.
    ///
    /// # Arguments
    ///
    /// * `tx` - The transaction model to resubmit.
    ///
    /// # Returns
    ///
    /// A result containing the resubmitted transaction model or a `TransactionError`.
    async fn resubmit_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        debug!("resubmitting transaction {}", tx.id);

        // If transaction is not in correct status, return Ok to avoid wasteful retries
        if let Err(e) = ensure_status_one_of(
            &tx,
            &[TransactionStatus::Sent, TransactionStatus::Submitted],
            Some("resubmit_transaction"),
        ) {
            warn!(
                tx_id = %tx.id,
                status = ?tx.status,
                error = %e,
                "transaction not in expected status for resubmission, skipping"
            );
            return Ok(tx);
        }

        let evm_data = tx.network_data.get_evm_transaction_data()?;

        // Calculate bumped gas price
        // For noop transactions, force_bump=true to skip gas price cap and ensure bump succeeds
        let bumped_price_params = self
            .price_calculator
            .calculate_bumped_gas_price(&evm_data, self.relayer(), is_noop(&evm_data))
            .await?;

        if !bumped_price_params.is_min_bumped.is_some_and(|b| b) {
            warn!(price_params = ?bumped_price_params, "bumped gas price does not meet minimum requirement, skipping resubmission");
            return Ok(tx);
        }

        // Validate the relayer has sufficient balance
        self.ensure_sufficient_balance(bumped_price_params.total_cost)
            .await?;

        // Create new transaction data with bumped gas price
        let updated_evm_data = evm_data.with_price_params(bumped_price_params.clone());

        // Sign the transaction
        let sig_result = self
            .signer
            .sign_transaction(NetworkTransactionData::Evm(updated_evm_data.clone()))
            .await?;

        let final_evm_data = updated_evm_data.with_signed_transaction_data(sig_result.into_evm()?);

        let raw_tx = final_evm_data.raw.as_ref().ok_or_else(|| {
            TransactionError::InvalidType("Raw transaction data is missing".to_string())
        })?;

        // Send resubmitted transaction to blockchain - this is the critical operation
        let was_already_submitted = match self.provider.send_raw_transaction(raw_tx).await {
            Ok(_) => {
                // Transaction resubmitted successfully with new pricing
                false
            }
            Err(e) => {
                // SAFETY CHECK: If we get "already known" or "nonce too low" errors,
                // it means a transaction with this nonce was already submitted
                let is_already_submitted = Self::is_already_submitted_error(&e);

                if is_already_submitted {
                    warn!(
                        tx_id = %tx.id,
                        error = %e,
                        "resubmission indicates transaction already in mempool/mined - keeping original hash"
                    );
                    // Don't update with new hash - the original transaction is what's on-chain
                    true
                } else {
                    // Real error - propagate it
                    return Err(e.into());
                }
            }
        };

        // If transaction was already submitted, just update status without changing hash
        let update = if was_already_submitted {
            // Keep original hash and data - just ensure status is Submitted
            TransactionUpdateRequest {
                status: Some(TransactionStatus::Submitted),
                ..Default::default()
            }
        } else {
            // Transaction resubmitted successfully - update with new hash and pricing
            let mut hashes = tx.hashes.clone();
            if let Some(hash) = final_evm_data.hash.clone() {
                hashes.push(hash);
            }

            TransactionUpdateRequest {
                network_data: Some(NetworkTransactionData::Evm(final_evm_data)),
                hashes: Some(hashes),
                status: Some(TransactionStatus::Submitted),
                priced_at: Some(Utc::now().to_rfc3339()),
                sent_at: Some(Utc::now().to_rfc3339()),
                ..Default::default()
            }
        };

        let updated_tx = match self
            .transaction_repository
            .partial_update(tx.id.clone(), update)
            .await
        {
            Ok(tx) => tx,
            Err(e) => {
                error!(
                    error = %e,
                    tx_id = %tx.id,
                    "CRITICAL: resubmitted transaction sent to blockchain but failed to update database"
                );
                // Transaction is on-chain - return original tx data to avoid wasteful retries
                tx
            }
        };

        Ok(updated_tx)
    }

    /// Cancels a transaction.
    ///
    /// # Arguments
    ///
    /// * `tx` - The transaction model to cancel.
    ///
    /// # Returns
    ///
    /// A result containing the transaction model or a `TransactionError`.
    async fn cancel_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        info!("cancelling transaction {}", tx.id);
        debug!(status = ?tx.status, "transaction status");

        // Validate state: can only cancel transactions that are still pending
        ensure_status_one_of(
            &tx,
            &[
                TransactionStatus::Pending,
                TransactionStatus::Sent,
                TransactionStatus::Submitted,
            ],
            Some("cancel_transaction"),
        )?;

        // If the transaction is in Pending state, we can just update its status
        if tx.status == TransactionStatus::Pending {
            debug!("transaction is in pending state, updating status to canceled");
            return self
                .update_transaction_status(tx, TransactionStatus::Canceled)
                .await;
        }

        let update = self.prepare_noop_update_request(&tx, true, None).await?;
        let updated_tx = self
            .transaction_repository()
            .partial_update(tx.id.clone(), update)
            .await?;

        // Submit the updated transaction to the network using the resubmit job
        self.send_transaction_resubmit_job(&updated_tx).await?;

        // Send notification for the updated transaction
        if let Err(e) = self.send_transaction_update_notification(&updated_tx).await {
            error!(
                tx_id = %updated_tx.id,
                status = ?updated_tx.status,
                "sending transaction update notification failed after cancel: {:?}",
                e
            );
        }

        debug!("original transaction updated with cancellation data");
        Ok(updated_tx)
    }

    /// Replaces a transaction with a new one.
    ///
    /// # Arguments
    ///
    /// * `old_tx` - The transaction model to replace.
    /// * `new_tx_request` - The new transaction request data.
    ///
    /// # Returns
    ///
    /// A result containing the updated transaction model or a `TransactionError`.
    async fn replace_transaction(
        &self,
        old_tx: TransactionRepoModel,
        new_tx_request: NetworkTransactionRequest,
    ) -> Result<TransactionRepoModel, TransactionError> {
        debug!("replacing transaction");

        // Validate state: can only replace transactions that are still pending
        ensure_status_one_of(
            &old_tx,
            &[
                TransactionStatus::Pending,
                TransactionStatus::Sent,
                TransactionStatus::Submitted,
            ],
            Some("replace_transaction"),
        )?;

        // Extract EVM data from both old transaction and new request
        let old_evm_data = old_tx.network_data.get_evm_transaction_data()?;
        let new_evm_request = match new_tx_request {
            NetworkTransactionRequest::Evm(evm_req) => evm_req,
            _ => {
                return Err(TransactionError::InvalidType(
                    "New transaction request must be EVM type".to_string(),
                ))
            }
        };

        let network_repo_model = self
            .network_repository()
            .get_by_chain_id(NetworkType::Evm, old_evm_data.chain_id)
            .await
            .map_err(|e| {
                TransactionError::NetworkConfiguration(format!(
                    "Failed to get network by chain_id {}: {}",
                    old_evm_data.chain_id, e
                ))
            })?
            .ok_or_else(|| {
                TransactionError::NetworkConfiguration(format!(
                    "Network with chain_id {} not found",
                    old_evm_data.chain_id
                ))
            })?;

        let network = EvmNetwork::try_from(network_repo_model).map_err(|e| {
            TransactionError::NetworkConfiguration(format!("Failed to convert network model: {e}"))
        })?;

        // First, create updated EVM data without price parameters
        let updated_evm_data = EvmTransactionData::for_replacement(&old_evm_data, &new_evm_request);

        // Then determine pricing strategy and calculate price parameters using the updated data
        let price_params = super::replacement::determine_replacement_pricing(
            &old_evm_data,
            &updated_evm_data,
            self.relayer(),
            &self.price_calculator,
            network.lacks_mempool(),
        )
        .await?;

        debug!(price_params = ?price_params, "replacement price params");

        // Apply the calculated price parameters to the updated EVM data
        let evm_data_with_price_params = updated_evm_data.with_price_params(price_params.clone());

        // Validate the relayer has sufficient balance
        self.ensure_sufficient_balance(price_params.total_cost)
            .await?;

        let sig_result = self
            .signer
            .sign_transaction(NetworkTransactionData::Evm(
                evm_data_with_price_params.clone(),
            ))
            .await?;

        let final_evm_data =
            evm_data_with_price_params.with_signed_transaction_data(sig_result.into_evm()?);

        // Update the transaction in the repository
        let updated_tx = self
            .transaction_repository
            .update_network_data(
                old_tx.id.clone(),
                NetworkTransactionData::Evm(final_evm_data),
            )
            .await?;

        self.send_transaction_resubmit_job(&updated_tx).await?;

        // Send notification
        if let Err(e) = self.send_transaction_update_notification(&updated_tx).await {
            error!(
                tx_id = %updated_tx.id,
                status = ?updated_tx.status,
                "sending transaction update notification failed after replace: {:?}",
                e
            );
        }

        Ok(updated_tx)
    }

    /// Signs a transaction.
    ///
    /// # Arguments
    ///
    /// * `tx` - The transaction model to sign.
    ///
    /// # Returns
    ///
    /// A result containing the transaction model or a `TransactionError`.
    async fn sign_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        Ok(tx)
    }

    /// Validates a transaction.
    ///
    /// # Arguments
    ///
    /// * `_tx` - The transaction model to validate.
    ///
    /// # Returns
    ///
    /// A result containing a boolean indicating validity or a `TransactionError`.
    async fn validate_transaction(
        &self,
        _tx: TransactionRepoModel,
    ) -> Result<bool, TransactionError> {
        Ok(true)
    }
}
// P: EvmProviderTrait,
// R: Repository<RelayerRepoModel, String>,
// T: TransactionRepository,
// J: JobProducerTrait,
// S: Signer,
// C: TransactionCounterTrait,
// PC: PriceCalculatorTrait,
// we define concrete type for the evm transaction
pub type DefaultEvmTransaction = EvmRelayerTransaction<
    EvmProvider,
    RelayerRepositoryStorage,
    NetworkRepositoryStorage,
    TransactionRepositoryStorage,
    JobProducer,
    EvmSigner,
    TransactionCounterRepositoryStorage,
    PriceCalculator<EvmGasPriceService<EvmProvider>>,
>;
#[cfg(test)]
mod tests {

    use super::*;
    use crate::{
        domain::evm::price_calculator::PriceParams,
        jobs::MockJobProducerTrait,
        models::{
            evm::Speed, EvmTransactionData, EvmTransactionRequest, NetworkType,
            RelayerNetworkPolicy, U256,
        },
        repositories::{
            MockNetworkRepository, MockRelayerRepository, MockTransactionCounterTrait,
            MockTransactionRepository,
        },
        services::{provider::MockEvmProviderTrait, signer::MockSigner},
    };
    use chrono::Utc;
    use futures::future::ready;
    use mockall::{mock, predicate::*};

    // Create a mock for PriceCalculatorTrait
    mock! {
        pub PriceCalculator {}
        #[async_trait]
        impl PriceCalculatorTrait for PriceCalculator {
            async fn get_transaction_price_params(
                &self,
                tx_data: &EvmTransactionData,
                relayer: &RelayerRepoModel
            ) -> Result<PriceParams, TransactionError>;

            async fn calculate_bumped_gas_price(
                &self,
                tx: &EvmTransactionData,
                relayer: &RelayerRepoModel,
                force_bump: bool,
            ) -> Result<PriceParams, TransactionError>;
        }
    }

    // Helper to create a relayer model with specific configuration for these tests
    fn create_test_relayer() -> RelayerRepoModel {
        create_test_relayer_with_policy(crate::models::RelayerEvmPolicy {
            min_balance: Some(100000000000000000u128), // 0.1 ETH
            gas_limit_estimation: Some(true),
            gas_price_cap: Some(100000000000), // 100 Gwei
            whitelist_receivers: Some(vec!["0xRecipient".to_string()]),
            eip1559_pricing: Some(false),
            private_transactions: Some(false),
        })
    }

    fn create_test_relayer_with_policy(evm_policy: RelayerEvmPolicy) -> RelayerRepoModel {
        RelayerRepoModel {
            id: "test-relayer-id".to_string(),
            name: "Test Relayer".to_string(),
            network: "1".to_string(), // Ethereum Mainnet
            address: "0xSender".to_string(),
            paused: false,
            system_disabled: false,
            signer_id: "test-signer-id".to_string(),
            notification_id: Some("test-notification-id".to_string()),
            policies: RelayerNetworkPolicy::Evm(evm_policy),
            network_type: NetworkType::Evm,
            custom_rpc_urls: None,
            ..Default::default()
        }
    }

    // Helper to create test transaction with specific configuration for these tests
    fn create_test_transaction() -> TransactionRepoModel {
        TransactionRepoModel {
            id: "test-tx-id".to_string(),
            relayer_id: "test-relayer-id".to_string(),
            status: TransactionStatus::Pending,
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
                value: U256::from(1000000000000000000u64), // 1 ETH
                data: Some("0xData".to_string()),
                gas_limit: Some(21000),
                gas_price: Some(20000000000), // 20 Gwei
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

    #[tokio::test]
    async fn test_prepare_transaction_with_sufficient_balance() {
        let mut mock_transaction = MockTransactionRepository::new();
        let mock_relayer = MockRelayerRepository::new();
        let mut mock_provider = MockEvmProviderTrait::new();
        let mut mock_signer = MockSigner::new();
        let mut mock_job_producer = MockJobProducerTrait::new();
        let mut mock_price_calculator = MockPriceCalculator::new();
        let mut counter_service = MockTransactionCounterTrait::new();

        let relayer = create_test_relayer();
        let test_tx = create_test_transaction();

        counter_service
            .expect_get_and_increment()
            .returning(|_, _| Box::pin(ready(Ok(42))));

        let price_params = PriceParams {
            gas_price: Some(30000000000),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            is_min_bumped: None,
            extra_fee: None,
            total_cost: U256::from(630000000000000u64),
        };
        mock_price_calculator
            .expect_get_transaction_price_params()
            .returning(move |_, _| Ok(price_params.clone()));

        mock_signer.expect_sign_transaction().returning(|_| {
            Box::pin(ready(Ok(
                crate::domain::relayer::SignTransactionResponse::Evm(
                    crate::domain::relayer::SignTransactionResponseEvm {
                        hash: "0xtx_hash".to_string(),
                        signature: crate::models::EvmTransactionDataSignature {
                            r: "r".to_string(),
                            s: "s".to_string(),
                            v: 1,
                            sig: "0xsignature".to_string(),
                        },
                        raw: vec![1, 2, 3],
                    },
                ),
            )))
        });

        mock_provider
            .expect_get_balance()
            .with(eq("0xSender"))
            .returning(|_| Box::pin(ready(Ok(U256::from(1000000000000000000u64)))));

        // Mock get_block_by_number for gas limit validation (tx has gas_limit: Some(21000))
        mock_provider
            .expect_get_block_by_number()
            .times(1)
            .returning(|| {
                Box::pin(async {
                    use alloy::{network::AnyRpcBlock, rpc::types::Block};
                    let mut block: Block = Block::default();
                    // Set block gas limit to 30M (higher than tx gas limit of 21_000)
                    block.header.gas_limit = 30_000_000u64;
                    Ok(AnyRpcBlock::from(block))
                })
            });

        let test_tx_clone = test_tx.clone();
        mock_transaction
            .expect_partial_update()
            .returning(move |_, update| {
                let mut updated_tx = test_tx_clone.clone();
                if let Some(status) = &update.status {
                    updated_tx.status = status.clone();
                }
                if let Some(network_data) = &update.network_data {
                    updated_tx.network_data = network_data.clone();
                }
                if let Some(hashes) = &update.hashes {
                    updated_tx.hashes = hashes.clone();
                }
                Ok(updated_tx)
            });

        mock_job_producer
            .expect_produce_submit_transaction_job()
            .returning(|_, _| Box::pin(ready(Ok(()))));
        mock_job_producer
            .expect_produce_send_notification_job()
            .returning(|_, _| Box::pin(ready(Ok(()))));

        let mock_network = MockNetworkRepository::new();

        let evm_transaction = EvmRelayerTransaction {
            relayer: relayer.clone(),
            provider: mock_provider,
            relayer_repository: Arc::new(mock_relayer),
            network_repository: Arc::new(mock_network),
            transaction_repository: Arc::new(mock_transaction),
            transaction_counter_service: Arc::new(counter_service),
            job_producer: Arc::new(mock_job_producer),
            price_calculator: mock_price_calculator,
            signer: mock_signer,
        };

        let result = evm_transaction.prepare_transaction(test_tx.clone()).await;
        assert!(result.is_ok());
        let prepared_tx = result.unwrap();
        assert_eq!(prepared_tx.status, TransactionStatus::Sent);
        assert!(!prepared_tx.hashes.is_empty());
    }

    #[tokio::test]
    async fn test_prepare_transaction_with_insufficient_balance() {
        let mut mock_transaction = MockTransactionRepository::new();
        let mock_relayer = MockRelayerRepository::new();
        let mut mock_provider = MockEvmProviderTrait::new();
        let mut mock_signer = MockSigner::new();
        let mut mock_job_producer = MockJobProducerTrait::new();
        let mut mock_price_calculator = MockPriceCalculator::new();
        let mut counter_service = MockTransactionCounterTrait::new();

        let relayer = create_test_relayer_with_policy(RelayerEvmPolicy {
            gas_limit_estimation: Some(false),
            min_balance: Some(100000000000000000u128),
            ..Default::default()
        });
        let test_tx = create_test_transaction();

        counter_service
            .expect_get_and_increment()
            .returning(|_, _| Box::pin(ready(Ok(42))));

        let price_params = PriceParams {
            gas_price: Some(30000000000),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            is_min_bumped: None,
            extra_fee: None,
            total_cost: U256::from(630000000000000u64),
        };
        mock_price_calculator
            .expect_get_transaction_price_params()
            .returning(move |_, _| Ok(price_params.clone()));

        mock_signer.expect_sign_transaction().returning(|_| {
            Box::pin(ready(Ok(
                crate::domain::relayer::SignTransactionResponse::Evm(
                    crate::domain::relayer::SignTransactionResponseEvm {
                        hash: "0xtx_hash".to_string(),
                        signature: crate::models::EvmTransactionDataSignature {
                            r: "r".to_string(),
                            s: "s".to_string(),
                            v: 1,
                            sig: "0xsignature".to_string(),
                        },
                        raw: vec![1, 2, 3],
                    },
                ),
            )))
        });

        mock_provider
            .expect_get_balance()
            .with(eq("0xSender"))
            .returning(|_| Box::pin(ready(Ok(U256::from(90000000000000000u64)))));

        // Mock get_block_by_number for gas limit validation (tx has gas_limit: Some(21000))
        mock_provider
            .expect_get_block_by_number()
            .times(1)
            .returning(|| {
                Box::pin(async {
                    use alloy::{network::AnyRpcBlock, rpc::types::Block};
                    let mut block: Block = Block::default();
                    // Set block gas limit to 30M (higher than tx gas limit of 21_000)
                    block.header.gas_limit = 30_000_000u64;
                    Ok(AnyRpcBlock::from(block))
                })
            });

        let test_tx_clone = test_tx.clone();
        mock_transaction
            .expect_partial_update()
            .withf(move |id, update| {
                id == "test-tx-id" && update.status == Some(TransactionStatus::Failed)
            })
            .returning(move |_, update| {
                let mut updated_tx = test_tx_clone.clone();
                updated_tx.status = update.status.unwrap_or(updated_tx.status);
                updated_tx.status_reason = update.status_reason.clone();
                Ok(updated_tx)
            });

        mock_job_producer
            .expect_produce_send_notification_job()
            .returning(|_, _| Box::pin(ready(Ok(()))));

        let mock_network = MockNetworkRepository::new();

        let evm_transaction = EvmRelayerTransaction {
            relayer: relayer.clone(),
            provider: mock_provider,
            relayer_repository: Arc::new(mock_relayer),
            network_repository: Arc::new(mock_network),
            transaction_repository: Arc::new(mock_transaction),
            transaction_counter_service: Arc::new(counter_service),
            job_producer: Arc::new(mock_job_producer),
            price_calculator: mock_price_calculator,
            signer: mock_signer,
        };

        let result = evm_transaction.prepare_transaction(test_tx.clone()).await;
        assert!(result.is_ok(), "Expected Ok, got: {:?}", result);

        let updated_tx = result.unwrap();
        assert_eq!(
            updated_tx.status,
            TransactionStatus::Failed,
            "Transaction should be marked as Failed"
        );
        assert!(
            updated_tx.status_reason.is_some(),
            "Status reason should be set"
        );
        assert!(
            updated_tx
                .status_reason
                .as_ref()
                .unwrap()
                .to_lowercase()
                .contains("insufficient balance"),
            "Status reason should contain insufficient balance error, got: {:?}",
            updated_tx.status_reason
        );
    }

    #[tokio::test]
    async fn test_prepare_transaction_with_gas_limit_exceeding_block_limit() {
        let mut mock_transaction = MockTransactionRepository::new();
        let mock_relayer = MockRelayerRepository::new();
        let mut mock_provider = MockEvmProviderTrait::new();
        let mock_signer = MockSigner::new();
        let mut mock_job_producer = MockJobProducerTrait::new();
        let mock_price_calculator = MockPriceCalculator::new();
        let mut counter_service = MockTransactionCounterTrait::new();

        let relayer = create_test_relayer_with_policy(RelayerEvmPolicy {
            gas_limit_estimation: Some(false), // User provides gas limit
            min_balance: Some(100000000000000000u128),
            ..Default::default()
        });

        // Create a transaction with a gas limit that exceeds block gas limit
        let mut test_tx = create_test_transaction();
        if let NetworkTransactionData::Evm(ref mut evm_data) = test_tx.network_data {
            evm_data.gas_limit = Some(30_000_001); // Exceeds typical block gas limit of 30M
        }

        counter_service
            .expect_get_and_increment()
            .returning(|_, _| Box::pin(ready(Ok(42))));

        // Mock get_block_by_number to return a block with gas_limit lower than tx gas_limit
        mock_provider
            .expect_get_block_by_number()
            .times(1)
            .returning(|| {
                Box::pin(async {
                    use alloy::{network::AnyRpcBlock, rpc::types::Block};
                    let mut block: Block = Block::default();
                    // Set block gas limit to 30M (lower than tx gas limit of 30_000_001)
                    block.header.gas_limit = 30_000_000u64;
                    Ok(AnyRpcBlock::from(block))
                })
            });

        // Mock partial_update to be called when marking transaction as failed
        let test_tx_clone = test_tx.clone();
        mock_transaction
            .expect_partial_update()
            .withf(move |id, update| {
                id == "test-tx-id"
                    && update.status == Some(TransactionStatus::Failed)
                    && update.status_reason.is_some()
                    && update
                        .status_reason
                        .as_ref()
                        .unwrap()
                        .contains("exceeds block gas limit")
            })
            .returning(move |_, update| {
                let mut updated_tx = test_tx_clone.clone();
                updated_tx.status = update.status.unwrap_or(updated_tx.status);
                updated_tx.status_reason = update.status_reason.clone();
                Ok(updated_tx)
            });

        mock_job_producer
            .expect_produce_send_notification_job()
            .returning(|_, _| Box::pin(ready(Ok(()))));

        let mock_network = MockNetworkRepository::new();

        let evm_transaction = EvmRelayerTransaction {
            relayer: relayer.clone(),
            provider: mock_provider,
            relayer_repository: Arc::new(mock_relayer),
            network_repository: Arc::new(mock_network),
            transaction_repository: Arc::new(mock_transaction),
            transaction_counter_service: Arc::new(counter_service),
            job_producer: Arc::new(mock_job_producer),
            price_calculator: mock_price_calculator,
            signer: mock_signer,
        };

        let result = evm_transaction.prepare_transaction(test_tx.clone()).await;
        assert!(result.is_ok(), "Expected Ok, got: {:?}", result);

        let updated_tx = result.unwrap();
        assert_eq!(
            updated_tx.status,
            TransactionStatus::Failed,
            "Transaction should be marked as Failed"
        );
        assert!(
            updated_tx.status_reason.is_some(),
            "Status reason should be set"
        );
        assert!(
            updated_tx
                .status_reason
                .as_ref()
                .unwrap()
                .contains("exceeds block gas limit"),
            "Status reason should mention gas limit exceeds block gas limit, got: {:?}",
            updated_tx.status_reason
        );
        assert!(
            updated_tx
                .status_reason
                .as_ref()
                .unwrap()
                .contains("30000001"),
            "Status reason should contain transaction gas limit, got: {:?}",
            updated_tx.status_reason
        );
        assert!(
            updated_tx
                .status_reason
                .as_ref()
                .unwrap()
                .contains("30000000"),
            "Status reason should contain block gas limit, got: {:?}",
            updated_tx.status_reason
        );
    }

    #[tokio::test]
    async fn test_prepare_transaction_with_gas_limit_within_block_limit() {
        let mut mock_transaction = MockTransactionRepository::new();
        let mock_relayer = MockRelayerRepository::new();
        let mut mock_provider = MockEvmProviderTrait::new();
        let mut mock_signer = MockSigner::new();
        let mut mock_job_producer = MockJobProducerTrait::new();
        let mut mock_price_calculator = MockPriceCalculator::new();
        let mut counter_service = MockTransactionCounterTrait::new();

        let relayer = create_test_relayer_with_policy(RelayerEvmPolicy {
            gas_limit_estimation: Some(false), // User provides gas limit
            min_balance: Some(100000000000000000u128),
            ..Default::default()
        });

        // Create a transaction with a gas limit within block gas limit
        let mut test_tx = create_test_transaction();
        if let NetworkTransactionData::Evm(ref mut evm_data) = test_tx.network_data {
            evm_data.gas_limit = Some(21_000); // Within typical block gas limit of 30M
        }

        counter_service
            .expect_get_and_increment()
            .returning(|_, _| Box::pin(ready(Ok(42))));

        let price_params = PriceParams {
            gas_price: Some(30000000000),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            is_min_bumped: None,
            extra_fee: None,
            total_cost: U256::from(630000000000000u64),
        };
        mock_price_calculator
            .expect_get_transaction_price_params()
            .returning(move |_, _| Ok(price_params.clone()));

        mock_signer.expect_sign_transaction().returning(|_| {
            Box::pin(ready(Ok(
                crate::domain::relayer::SignTransactionResponse::Evm(
                    crate::domain::relayer::SignTransactionResponseEvm {
                        hash: "0xtx_hash".to_string(),
                        signature: crate::models::EvmTransactionDataSignature {
                            r: "r".to_string(),
                            s: "s".to_string(),
                            v: 1,
                            sig: "0xsignature".to_string(),
                        },
                        raw: vec![1, 2, 3],
                    },
                ),
            )))
        });

        mock_provider
            .expect_get_balance()
            .with(eq("0xSender"))
            .returning(|_| Box::pin(ready(Ok(U256::from(1000000000000000000u64)))));

        // Mock get_block_by_number to return a block with gas_limit higher than tx gas_limit
        mock_provider
            .expect_get_block_by_number()
            .times(1)
            .returning(|| {
                Box::pin(async {
                    use alloy::{network::AnyRpcBlock, rpc::types::Block};
                    let mut block: Block = Block::default();
                    // Set block gas limit to 30M (higher than tx gas limit of 21_000)
                    block.header.gas_limit = 30_000_000u64;
                    Ok(AnyRpcBlock::from(block))
                })
            });

        let test_tx_clone = test_tx.clone();
        mock_transaction
            .expect_partial_update()
            .returning(move |_, update| {
                let mut updated_tx = test_tx_clone.clone();
                if let Some(status) = &update.status {
                    updated_tx.status = status.clone();
                }
                if let Some(network_data) = &update.network_data {
                    updated_tx.network_data = network_data.clone();
                }
                if let Some(hashes) = &update.hashes {
                    updated_tx.hashes = hashes.clone();
                }
                Ok(updated_tx)
            });

        mock_job_producer
            .expect_produce_submit_transaction_job()
            .returning(|_, _| Box::pin(ready(Ok(()))));
        mock_job_producer
            .expect_produce_send_notification_job()
            .returning(|_, _| Box::pin(ready(Ok(()))));

        let mock_network = MockNetworkRepository::new();

        let evm_transaction = EvmRelayerTransaction {
            relayer: relayer.clone(),
            provider: mock_provider,
            relayer_repository: Arc::new(mock_relayer),
            network_repository: Arc::new(mock_network),
            transaction_repository: Arc::new(mock_transaction),
            transaction_counter_service: Arc::new(counter_service),
            job_producer: Arc::new(mock_job_producer),
            price_calculator: mock_price_calculator,
            signer: mock_signer,
        };

        let result = evm_transaction.prepare_transaction(test_tx.clone()).await;
        assert!(result.is_ok(), "Expected Ok, got: {:?}", result);

        let prepared_tx = result.unwrap();
        // Transaction should proceed normally (not be marked as Failed)
        assert_eq!(prepared_tx.status, TransactionStatus::Sent);
        assert!(!prepared_tx.hashes.is_empty());
    }

    #[tokio::test]
    async fn test_cancel_transaction() {
        // Test Case 1: Canceling a pending transaction
        {
            // Create mocks for all dependencies
            let mut mock_transaction = MockTransactionRepository::new();
            let mock_relayer = MockRelayerRepository::new();
            let mock_provider = MockEvmProviderTrait::new();
            let mock_signer = MockSigner::new();
            let mut mock_job_producer = MockJobProducerTrait::new();
            let mock_price_calculator = MockPriceCalculator::new();
            let counter_service = MockTransactionCounterTrait::new();

            // Create test relayer and pending transaction
            let relayer = create_test_relayer();
            let mut test_tx = create_test_transaction();
            test_tx.status = TransactionStatus::Pending;

            // Transaction repository should update the transaction with Canceled status
            let test_tx_clone = test_tx.clone();
            mock_transaction
                .expect_partial_update()
                .withf(move |id, update| {
                    id == "test-tx-id" && update.status == Some(TransactionStatus::Canceled)
                })
                .returning(move |_, update| {
                    let mut updated_tx = test_tx_clone.clone();
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    Ok(updated_tx)
                });

            // Job producer should send notification
            mock_job_producer
                .expect_produce_send_notification_job()
                .returning(|_, _| Box::pin(ready(Ok(()))));

            let mock_network = MockNetworkRepository::new();

            // Set up EVM transaction with the mocks
            let evm_transaction = EvmRelayerTransaction {
                relayer: relayer.clone(),
                provider: mock_provider,
                relayer_repository: Arc::new(mock_relayer),
                network_repository: Arc::new(mock_network),
                transaction_repository: Arc::new(mock_transaction),
                transaction_counter_service: Arc::new(counter_service),
                job_producer: Arc::new(mock_job_producer),
                price_calculator: mock_price_calculator,
                signer: mock_signer,
            };

            // Call cancel_transaction and verify it succeeds
            let result = evm_transaction.cancel_transaction(test_tx.clone()).await;
            assert!(result.is_ok());
            let cancelled_tx = result.unwrap();
            assert_eq!(cancelled_tx.id, "test-tx-id");
            assert_eq!(cancelled_tx.status, TransactionStatus::Canceled);
        }

        // Test Case 2: Canceling a submitted transaction
        {
            // Create mocks for all dependencies
            let mut mock_transaction = MockTransactionRepository::new();
            let mock_relayer = MockRelayerRepository::new();
            let mock_provider = MockEvmProviderTrait::new();
            let mut mock_signer = MockSigner::new();
            let mut mock_job_producer = MockJobProducerTrait::new();
            let mut mock_price_calculator = MockPriceCalculator::new();
            let counter_service = MockTransactionCounterTrait::new();

            // Create test relayer and submitted transaction
            let relayer = create_test_relayer();
            let mut test_tx = create_test_transaction();
            test_tx.status = TransactionStatus::Submitted;
            test_tx.sent_at = Some(Utc::now().to_rfc3339());
            test_tx.network_data = NetworkTransactionData::Evm(EvmTransactionData {
                nonce: Some(42),
                hash: Some("0xoriginal_hash".to_string()),
                ..test_tx.network_data.get_evm_transaction_data().unwrap()
            });

            // Set up price calculator expectations for cancellation tx
            mock_price_calculator
                .expect_get_transaction_price_params()
                .return_once(move |_, _| {
                    Ok(PriceParams {
                        gas_price: Some(40000000000), // 40 Gwei (higher than original)
                        max_fee_per_gas: None,
                        max_priority_fee_per_gas: None,
                        is_min_bumped: Some(true),
                        extra_fee: Some(U256::ZERO),
                        total_cost: U256::ZERO,
                    })
                });

            // Signer should be called to sign the cancellation transaction
            mock_signer.expect_sign_transaction().returning(|_| {
                Box::pin(ready(Ok(
                    crate::domain::relayer::SignTransactionResponse::Evm(
                        crate::domain::relayer::SignTransactionResponseEvm {
                            hash: "0xcancellation_hash".to_string(),
                            signature: crate::models::EvmTransactionDataSignature {
                                r: "r".to_string(),
                                s: "s".to_string(),
                                v: 1,
                                sig: "0xsignature".to_string(),
                            },
                            raw: vec![1, 2, 3],
                        },
                    ),
                )))
            });

            // Transaction repository should update the transaction
            let test_tx_clone = test_tx.clone();
            mock_transaction
                .expect_partial_update()
                .returning(move |tx_id, update| {
                    let mut updated_tx = test_tx_clone.clone();
                    updated_tx.id = tx_id;
                    updated_tx.status = update.status.unwrap_or(updated_tx.status);
                    updated_tx.network_data =
                        update.network_data.unwrap_or(updated_tx.network_data);
                    if let Some(hashes) = update.hashes {
                        updated_tx.hashes = hashes;
                    }
                    Ok(updated_tx)
                });

            // Job producer expectations
            mock_job_producer
                .expect_produce_submit_transaction_job()
                .returning(|_, _| Box::pin(ready(Ok(()))));
            mock_job_producer
                .expect_produce_send_notification_job()
                .returning(|_, _| Box::pin(ready(Ok(()))));

            // Network repository expectations for cancellation NOOP transaction
            let mut mock_network = MockNetworkRepository::new();
            mock_network
                .expect_get_by_chain_id()
                .with(eq(NetworkType::Evm), eq(1))
                .returning(|_, _| {
                    use crate::config::{EvmNetworkConfig, NetworkConfigCommon};
                    use crate::models::{NetworkConfigData, NetworkRepoModel};

                    let config = EvmNetworkConfig {
                        common: NetworkConfigCommon {
                            network: "mainnet".to_string(),
                            from: None,
                            rpc_urls: Some(vec!["https://rpc.example.com".to_string()]),
                            explorer_urls: None,
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
                    Ok(Some(NetworkRepoModel {
                        id: "evm:mainnet".to_string(),
                        name: "mainnet".to_string(),
                        network_type: NetworkType::Evm,
                        config: NetworkConfigData::Evm(config),
                    }))
                });

            // Set up EVM transaction with the mocks
            let evm_transaction = EvmRelayerTransaction {
                relayer: relayer.clone(),
                provider: mock_provider,
                relayer_repository: Arc::new(mock_relayer),
                network_repository: Arc::new(mock_network),
                transaction_repository: Arc::new(mock_transaction),
                transaction_counter_service: Arc::new(counter_service),
                job_producer: Arc::new(mock_job_producer),
                price_calculator: mock_price_calculator,
                signer: mock_signer,
            };

            // Call cancel_transaction and verify it succeeds
            let result = evm_transaction.cancel_transaction(test_tx.clone()).await;
            assert!(result.is_ok());
            let cancelled_tx = result.unwrap();

            // Verify the cancellation transaction was properly created
            assert_eq!(cancelled_tx.id, "test-tx-id");
            assert_eq!(cancelled_tx.status, TransactionStatus::Submitted);

            // Verify the network data was properly updated
            if let NetworkTransactionData::Evm(evm_data) = &cancelled_tx.network_data {
                assert_eq!(evm_data.nonce, Some(42)); // Same nonce as original
            } else {
                panic!("Expected EVM transaction data");
            }
        }

        // Test Case 3: Attempting to cancel a confirmed transaction (should fail)
        {
            // Create minimal mocks for failure case
            let mock_transaction = MockTransactionRepository::new();
            let mock_relayer = MockRelayerRepository::new();
            let mock_provider = MockEvmProviderTrait::new();
            let mock_signer = MockSigner::new();
            let mock_job_producer = MockJobProducerTrait::new();
            let mock_price_calculator = MockPriceCalculator::new();
            let counter_service = MockTransactionCounterTrait::new();

            // Create test relayer and confirmed transaction
            let relayer = create_test_relayer();
            let mut test_tx = create_test_transaction();
            test_tx.status = TransactionStatus::Confirmed;

            let mock_network = MockNetworkRepository::new();

            // Set up EVM transaction with the mocks
            let evm_transaction = EvmRelayerTransaction {
                relayer: relayer.clone(),
                provider: mock_provider,
                relayer_repository: Arc::new(mock_relayer),
                network_repository: Arc::new(mock_network),
                transaction_repository: Arc::new(mock_transaction),
                transaction_counter_service: Arc::new(counter_service),
                job_producer: Arc::new(mock_job_producer),
                price_calculator: mock_price_calculator,
                signer: mock_signer,
            };

            // Call cancel_transaction and verify it fails
            let result = evm_transaction.cancel_transaction(test_tx.clone()).await;
            assert!(result.is_err());
            if let Err(TransactionError::ValidationError(msg)) = result {
                assert!(msg.contains("Invalid transaction state for cancel_transaction"));
            } else {
                panic!("Expected ValidationError");
            }
        }
    }

    #[tokio::test]
    async fn test_replace_transaction() {
        // Test Case: Replacing a submitted transaction with new gas price
        {
            // Create mocks for all dependencies
            let mut mock_transaction = MockTransactionRepository::new();
            let mock_relayer = MockRelayerRepository::new();
            let mut mock_provider = MockEvmProviderTrait::new();
            let mut mock_signer = MockSigner::new();
            let mut mock_job_producer = MockJobProducerTrait::new();
            let mut mock_price_calculator = MockPriceCalculator::new();
            let counter_service = MockTransactionCounterTrait::new();

            // Create test relayer and submitted transaction
            let relayer = create_test_relayer();
            let mut test_tx = create_test_transaction();
            test_tx.status = TransactionStatus::Submitted;
            test_tx.sent_at = Some(Utc::now().to_rfc3339());

            // Set up price calculator expectations for replacement
            mock_price_calculator
                .expect_get_transaction_price_params()
                .return_once(move |_, _| {
                    Ok(PriceParams {
                        gas_price: Some(40000000000), // 40 Gwei (higher than original)
                        max_fee_per_gas: None,
                        max_priority_fee_per_gas: None,
                        is_min_bumped: Some(true),
                        extra_fee: Some(U256::ZERO),
                        total_cost: U256::from(2001000000000000000u64), // 2 ETH + gas costs
                    })
                });

            // Signer should be called to sign the replacement transaction
            mock_signer.expect_sign_transaction().returning(|_| {
                Box::pin(ready(Ok(
                    crate::domain::relayer::SignTransactionResponse::Evm(
                        crate::domain::relayer::SignTransactionResponseEvm {
                            hash: "0xreplacement_hash".to_string(),
                            signature: crate::models::EvmTransactionDataSignature {
                                r: "r".to_string(),
                                s: "s".to_string(),
                                v: 1,
                                sig: "0xsignature".to_string(),
                            },
                            raw: vec![1, 2, 3],
                        },
                    ),
                )))
            });

            // Provider balance check should pass
            mock_provider
                .expect_get_balance()
                .with(eq("0xSender"))
                .returning(|_| Box::pin(ready(Ok(U256::from(3000000000000000000u64)))));

            // Transaction repository should update using update_network_data
            let test_tx_clone = test_tx.clone();
            mock_transaction
                .expect_update_network_data()
                .returning(move |tx_id, network_data| {
                    let mut updated_tx = test_tx_clone.clone();
                    updated_tx.id = tx_id;
                    updated_tx.network_data = network_data;
                    Ok(updated_tx)
                });

            // Job producer expectations
            mock_job_producer
                .expect_produce_submit_transaction_job()
                .returning(|_, _| Box::pin(ready(Ok(()))));
            mock_job_producer
                .expect_produce_send_notification_job()
                .returning(|_, _| Box::pin(ready(Ok(()))));

            // Network repository expectations for mempool check
            let mut mock_network = MockNetworkRepository::new();
            mock_network
                .expect_get_by_chain_id()
                .with(eq(NetworkType::Evm), eq(1))
                .returning(|_, _| {
                    use crate::config::{EvmNetworkConfig, NetworkConfigCommon};
                    use crate::models::{NetworkConfigData, NetworkRepoModel};

                    let config = EvmNetworkConfig {
                        common: NetworkConfigCommon {
                            network: "mainnet".to_string(),
                            from: None,
                            rpc_urls: Some(vec!["https://rpc.example.com".to_string()]),
                            explorer_urls: None,
                            average_blocktime_ms: Some(12000),
                            is_testnet: Some(false),
                            tags: Some(vec!["mainnet".to_string()]), // No "no-mempool" tag
                        },
                        chain_id: Some(1),
                        required_confirmations: Some(12),
                        features: Some(vec!["eip1559".to_string()]),
                        symbol: Some("ETH".to_string()),
                        gas_price_cache: None,
                    };
                    Ok(Some(NetworkRepoModel {
                        id: "evm:mainnet".to_string(),
                        name: "mainnet".to_string(),
                        network_type: NetworkType::Evm,
                        config: NetworkConfigData::Evm(config),
                    }))
                });

            // Set up EVM transaction with the mocks
            let evm_transaction = EvmRelayerTransaction {
                relayer: relayer.clone(),
                provider: mock_provider,
                relayer_repository: Arc::new(mock_relayer),
                network_repository: Arc::new(mock_network),
                transaction_repository: Arc::new(mock_transaction),
                transaction_counter_service: Arc::new(counter_service),
                job_producer: Arc::new(mock_job_producer),
                price_calculator: mock_price_calculator,
                signer: mock_signer,
            };

            // Create replacement request with speed-based pricing
            let replacement_request = NetworkTransactionRequest::Evm(EvmTransactionRequest {
                to: Some("0xNewRecipient".to_string()),
                value: U256::from(2000000000000000000u64), // 2 ETH
                data: Some("0xNewData".to_string()),
                gas_limit: Some(25000),
                gas_price: None, // Use speed-based pricing
                max_fee_per_gas: None,
                max_priority_fee_per_gas: None,
                speed: Some(Speed::Fast),
                valid_until: None,
            });

            // Call replace_transaction and verify it succeeds
            let result = evm_transaction
                .replace_transaction(test_tx.clone(), replacement_request)
                .await;
            if let Err(ref e) = result {
                eprintln!("Replace transaction failed with error: {:?}", e);
            }
            assert!(result.is_ok());
            let replaced_tx = result.unwrap();

            // Verify the replacement was properly processed
            assert_eq!(replaced_tx.id, "test-tx-id");

            // Verify the network data was properly updated
            if let NetworkTransactionData::Evm(evm_data) = &replaced_tx.network_data {
                assert_eq!(evm_data.to, Some("0xNewRecipient".to_string()));
                assert_eq!(evm_data.value, U256::from(2000000000000000000u64));
                assert_eq!(evm_data.gas_price, Some(40000000000));
                assert_eq!(evm_data.gas_limit, Some(25000));
                assert!(evm_data.hash.is_some());
                assert!(evm_data.raw.is_some());
            } else {
                panic!("Expected EVM transaction data");
            }
        }

        // Test Case: Attempting to replace a confirmed transaction (should fail)
        {
            // Create minimal mocks for failure case
            let mock_transaction = MockTransactionRepository::new();
            let mock_relayer = MockRelayerRepository::new();
            let mock_provider = MockEvmProviderTrait::new();
            let mock_signer = MockSigner::new();
            let mock_job_producer = MockJobProducerTrait::new();
            let mock_price_calculator = MockPriceCalculator::new();
            let counter_service = MockTransactionCounterTrait::new();

            // Create test relayer and confirmed transaction
            let relayer = create_test_relayer();
            let mut test_tx = create_test_transaction();
            test_tx.status = TransactionStatus::Confirmed;

            let mock_network = MockNetworkRepository::new();

            // Set up EVM transaction with the mocks
            let evm_transaction = EvmRelayerTransaction {
                relayer: relayer.clone(),
                provider: mock_provider,
                relayer_repository: Arc::new(mock_relayer),
                network_repository: Arc::new(mock_network),
                transaction_repository: Arc::new(mock_transaction),
                transaction_counter_service: Arc::new(counter_service),
                job_producer: Arc::new(mock_job_producer),
                price_calculator: mock_price_calculator,
                signer: mock_signer,
            };

            // Create dummy replacement request
            let replacement_request = NetworkTransactionRequest::Evm(EvmTransactionRequest {
                to: Some("0xNewRecipient".to_string()),
                value: U256::from(1000000000000000000u64),
                data: Some("0xData".to_string()),
                gas_limit: Some(21000),
                gas_price: Some(30000000000),
                max_fee_per_gas: None,
                max_priority_fee_per_gas: None,
                speed: Some(Speed::Fast),
                valid_until: None,
            });

            // Call replace_transaction and verify it fails
            let result = evm_transaction
                .replace_transaction(test_tx.clone(), replacement_request)
                .await;
            assert!(result.is_err());
            if let Err(TransactionError::ValidationError(msg)) = result {
                assert!(msg.contains("Invalid transaction state for replace_transaction"));
            } else {
                panic!("Expected ValidationError");
            }
        }
    }

    #[tokio::test]
    async fn test_estimate_tx_gas_limit_success() {
        let mock_transaction = MockTransactionRepository::new();
        let mock_relayer = MockRelayerRepository::new();
        let mut mock_provider = MockEvmProviderTrait::new();
        let mock_signer = MockSigner::new();
        let mock_job_producer = MockJobProducerTrait::new();
        let mock_price_calculator = MockPriceCalculator::new();
        let counter_service = MockTransactionCounterTrait::new();
        let mock_network = MockNetworkRepository::new();

        // Create test relayer and pending transaction
        let relayer = create_test_relayer_with_policy(RelayerEvmPolicy {
            gas_limit_estimation: Some(true),
            ..Default::default()
        });
        let evm_data = EvmTransactionData {
            from: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            to: Some("0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed".to_string()),
            value: U256::from(1000000000000000000u128),
            data: Some("0x".to_string()),
            gas_limit: None,
            gas_price: Some(20_000_000_000),
            nonce: Some(1),
            chain_id: 1,
            hash: None,
            signature: None,
            speed: Some(Speed::Average),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            raw: None,
        };

        // Mock provider to return 21000 as estimated gas
        mock_provider
            .expect_estimate_gas()
            .times(1)
            .returning(|_| Box::pin(async { Ok(21000) }));

        let transaction = EvmRelayerTransaction::new(
            relayer.clone(),
            mock_provider,
            Arc::new(mock_relayer),
            Arc::new(mock_network),
            Arc::new(mock_transaction),
            Arc::new(counter_service),
            Arc::new(mock_job_producer),
            mock_price_calculator,
            mock_signer,
        )
        .unwrap();

        let result = transaction
            .estimate_tx_gas_limit(&evm_data, &relayer.policies.get_evm_policy())
            .await;

        assert!(result.is_ok());
        // Expected: 21000 * 110 / 100 = 23100
        assert_eq!(result.unwrap(), 23100);
    }

    #[tokio::test]
    async fn test_estimate_tx_gas_limit_disabled() {
        let mock_transaction = MockTransactionRepository::new();
        let mock_relayer = MockRelayerRepository::new();
        let mut mock_provider = MockEvmProviderTrait::new();
        let mock_signer = MockSigner::new();
        let mock_job_producer = MockJobProducerTrait::new();
        let mock_price_calculator = MockPriceCalculator::new();
        let counter_service = MockTransactionCounterTrait::new();
        let mock_network = MockNetworkRepository::new();

        // Create test relayer and pending transaction
        let relayer = create_test_relayer_with_policy(RelayerEvmPolicy {
            gas_limit_estimation: Some(false),
            ..Default::default()
        });

        let evm_data = EvmTransactionData {
            from: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            to: Some("0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed".to_string()),
            value: U256::from(1000000000000000000u128),
            data: Some("0x".to_string()),
            gas_limit: None,
            gas_price: Some(20_000_000_000),
            nonce: Some(1),
            chain_id: 1,
            hash: None,
            signature: None,
            speed: Some(Speed::Average),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            raw: None,
        };

        // Provider should not be called when estimation is disabled
        mock_provider.expect_estimate_gas().times(0);

        let transaction = EvmRelayerTransaction::new(
            relayer.clone(),
            mock_provider,
            Arc::new(mock_relayer),
            Arc::new(mock_network),
            Arc::new(mock_transaction),
            Arc::new(counter_service),
            Arc::new(mock_job_producer),
            mock_price_calculator,
            mock_signer,
        )
        .unwrap();

        let result = transaction
            .estimate_tx_gas_limit(&evm_data, &relayer.policies.get_evm_policy())
            .await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TransactionError::UnexpectedError(_)
        ));
    }

    #[tokio::test]
    async fn test_estimate_tx_gas_limit_default_enabled() {
        let mock_transaction = MockTransactionRepository::new();
        let mock_relayer = MockRelayerRepository::new();
        let mut mock_provider = MockEvmProviderTrait::new();
        let mock_signer = MockSigner::new();
        let mock_job_producer = MockJobProducerTrait::new();
        let mock_price_calculator = MockPriceCalculator::new();
        let counter_service = MockTransactionCounterTrait::new();
        let mock_network = MockNetworkRepository::new();

        let relayer = create_test_relayer_with_policy(RelayerEvmPolicy {
            gas_limit_estimation: None, // Should default to true
            ..Default::default()
        });

        let evm_data = EvmTransactionData {
            from: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            to: Some("0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed".to_string()),
            value: U256::from(1000000000000000000u128),
            data: Some("0x".to_string()),
            gas_limit: None,
            gas_price: Some(20_000_000_000),
            nonce: Some(1),
            chain_id: 1,
            hash: None,
            signature: None,
            speed: Some(Speed::Average),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            raw: None,
        };

        // Mock provider to return 50000 as estimated gas
        mock_provider
            .expect_estimate_gas()
            .times(1)
            .returning(|_| Box::pin(async { Ok(50000) }));

        let transaction = EvmRelayerTransaction::new(
            relayer.clone(),
            mock_provider,
            Arc::new(mock_relayer),
            Arc::new(mock_network),
            Arc::new(mock_transaction),
            Arc::new(counter_service),
            Arc::new(mock_job_producer),
            mock_price_calculator,
            mock_signer,
        )
        .unwrap();

        let result = transaction
            .estimate_tx_gas_limit(&evm_data, &relayer.policies.get_evm_policy())
            .await;

        assert!(result.is_ok());
        // Expected: 50000 * 110 / 100 = 55000
        assert_eq!(result.unwrap(), 55000);
    }

    #[tokio::test]
    async fn test_estimate_tx_gas_limit_provider_error() {
        let mock_transaction = MockTransactionRepository::new();
        let mock_relayer = MockRelayerRepository::new();
        let mut mock_provider = MockEvmProviderTrait::new();
        let mock_signer = MockSigner::new();
        let mock_job_producer = MockJobProducerTrait::new();
        let mock_price_calculator = MockPriceCalculator::new();
        let counter_service = MockTransactionCounterTrait::new();
        let mock_network = MockNetworkRepository::new();

        let relayer = create_test_relayer_with_policy(RelayerEvmPolicy {
            gas_limit_estimation: Some(true),
            ..Default::default()
        });

        let evm_data = EvmTransactionData {
            from: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            to: Some("0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed".to_string()),
            value: U256::from(1000000000000000000u128),
            data: Some("0x".to_string()),
            gas_limit: None,
            gas_price: Some(20_000_000_000),
            nonce: Some(1),
            chain_id: 1,
            hash: None,
            signature: None,
            speed: Some(Speed::Average),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            raw: None,
        };

        // Mock provider to return an error
        mock_provider.expect_estimate_gas().times(1).returning(|_| {
            Box::pin(async {
                Err(crate::services::provider::ProviderError::Other(
                    "RPC error".to_string(),
                ))
            })
        });

        let transaction = EvmRelayerTransaction::new(
            relayer.clone(),
            mock_provider,
            Arc::new(mock_relayer),
            Arc::new(mock_network),
            Arc::new(mock_transaction),
            Arc::new(counter_service),
            Arc::new(mock_job_producer),
            mock_price_calculator,
            mock_signer,
        )
        .unwrap();

        let result = transaction
            .estimate_tx_gas_limit(&evm_data, &relayer.policies.get_evm_policy())
            .await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TransactionError::UnexpectedError(_)
        ));
    }

    #[tokio::test]
    async fn test_prepare_transaction_uses_gas_estimation_and_stores_result() {
        let mut mock_transaction = MockTransactionRepository::new();
        let mock_relayer = MockRelayerRepository::new();
        let mut mock_provider = MockEvmProviderTrait::new();
        let mut mock_signer = MockSigner::new();
        let mut mock_job_producer = MockJobProducerTrait::new();
        let mut mock_price_calculator = MockPriceCalculator::new();
        let mut counter_service = MockTransactionCounterTrait::new();
        let mock_network = MockNetworkRepository::new();

        // Create test relayer with gas limit estimation enabled
        let relayer = create_test_relayer_with_policy(RelayerEvmPolicy {
            gas_limit_estimation: Some(true),
            min_balance: Some(100000000000000000u128),
            ..Default::default()
        });

        // Create test transaction WITHOUT gas_limit (so estimation will be triggered)
        let mut test_tx = create_test_transaction();
        if let NetworkTransactionData::Evm(ref mut evm_data) = test_tx.network_data {
            evm_data.gas_limit = None; // This should trigger gas estimation
            evm_data.nonce = None; // This will be set by the counter service
        }

        // Expected estimated gas from provider
        const PROVIDER_GAS_ESTIMATE: u64 = 45000;
        const EXPECTED_GAS_WITH_BUFFER: u64 = 49500; // 45000 * 110 / 100

        // Mock provider to return specific gas estimate
        mock_provider
            .expect_estimate_gas()
            .times(1)
            .returning(move |_| Box::pin(async move { Ok(PROVIDER_GAS_ESTIMATE) }));

        // Mock provider for balance check
        mock_provider
            .expect_get_balance()
            .times(1)
            .returning(|_| Box::pin(async { Ok(U256::from(2000000000000000000u128)) })); // 2 ETH

        let price_params = PriceParams {
            gas_price: Some(20_000_000_000), // 20 Gwei
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            is_min_bumped: None,
            extra_fee: None,
            total_cost: U256::from(1900000000000000000u128), // 1.9 ETH total cost
        };

        // Mock price calculator
        mock_price_calculator
            .expect_get_transaction_price_params()
            .returning(move |_, _| Ok(price_params.clone()));

        // Mock transaction counter to return a nonce
        counter_service
            .expect_get_and_increment()
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(42) }));

        // Mock signer to return a signed transaction
        mock_signer.expect_sign_transaction().returning(|_| {
            Box::pin(ready(Ok(
                crate::domain::relayer::SignTransactionResponse::Evm(
                    crate::domain::relayer::SignTransactionResponseEvm {
                        hash: "0xhash".to_string(),
                        signature: crate::models::EvmTransactionDataSignature {
                            r: "r".to_string(),
                            s: "s".to_string(),
                            v: 1,
                            sig: "0xsignature".to_string(),
                        },
                        raw: vec![1, 2, 3],
                    },
                ),
            )))
        });

        // Mock job producer to capture the submission job
        mock_job_producer
            .expect_produce_submit_transaction_job()
            .returning(|_, _| Box::pin(async { Ok(()) }));

        mock_job_producer
            .expect_produce_send_notification_job()
            .returning(|_, _| Box::pin(ready(Ok(()))));

        // Mock transaction repository partial_update calls
        // Note: prepare_transaction calls partial_update twice:
        // 1. Presign update (saves nonce before signing)
        // 2. Postsign update (saves signed data and marks as Sent)
        let expected_gas_limit = EXPECTED_GAS_WITH_BUFFER;

        let test_tx_clone = test_tx.clone();
        mock_transaction
            .expect_partial_update()
            .times(2)
            .returning(move |_, update| {
                let mut updated_tx = test_tx_clone.clone();

                // Apply the updates from the request
                if let Some(status) = &update.status {
                    updated_tx.status = status.clone();
                }
                if let Some(network_data) = &update.network_data {
                    updated_tx.network_data = network_data.clone();
                } else {
                    // If network_data is not being updated, ensure gas_limit is set
                    if let NetworkTransactionData::Evm(ref mut evm_data) = updated_tx.network_data {
                        if evm_data.gas_limit.is_none() {
                            evm_data.gas_limit = Some(expected_gas_limit);
                        }
                    }
                }
                if let Some(hashes) = &update.hashes {
                    updated_tx.hashes = hashes.clone();
                }

                Ok(updated_tx)
            });

        let transaction = EvmRelayerTransaction::new(
            relayer.clone(),
            mock_provider,
            Arc::new(mock_relayer),
            Arc::new(mock_network),
            Arc::new(mock_transaction),
            Arc::new(counter_service),
            Arc::new(mock_job_producer),
            mock_price_calculator,
            mock_signer,
        )
        .unwrap();

        // Call prepare_transaction
        let result = transaction.prepare_transaction(test_tx).await;

        // Verify the transaction was prepared successfully
        assert!(result.is_ok(), "prepare_transaction should succeed");
        let prepared_tx = result.unwrap();

        // Verify the final transaction has the estimated gas limit
        if let NetworkTransactionData::Evm(evm_data) = prepared_tx.network_data {
            assert_eq!(evm_data.gas_limit, Some(EXPECTED_GAS_WITH_BUFFER));
        } else {
            panic!("Expected EVM network data");
        }
    }

    #[test]
    fn test_is_already_submitted_error_detection() {
        // Test "already known" variants
        assert!(DefaultEvmTransaction::is_already_submitted_error(
            &"already known"
        ));
        assert!(DefaultEvmTransaction::is_already_submitted_error(
            &"Transaction already known"
        ));
        assert!(DefaultEvmTransaction::is_already_submitted_error(
            &"Error: already known"
        ));

        // Test "nonce too low" variants
        assert!(DefaultEvmTransaction::is_already_submitted_error(
            &"nonce too low"
        ));
        assert!(DefaultEvmTransaction::is_already_submitted_error(
            &"Nonce Too Low"
        ));
        assert!(DefaultEvmTransaction::is_already_submitted_error(
            &"Error: nonce too low"
        ));

        // Test "replacement transaction underpriced" variants
        assert!(DefaultEvmTransaction::is_already_submitted_error(
            &"replacement transaction underpriced"
        ));
        assert!(DefaultEvmTransaction::is_already_submitted_error(
            &"Replacement Transaction Underpriced"
        ));

        // Test non-matching errors
        assert!(!DefaultEvmTransaction::is_already_submitted_error(
            &"insufficient funds"
        ));
        assert!(!DefaultEvmTransaction::is_already_submitted_error(
            &"execution reverted"
        ));
        assert!(!DefaultEvmTransaction::is_already_submitted_error(
            &"gas too low"
        ));
        assert!(!DefaultEvmTransaction::is_already_submitted_error(
            &"timeout"
        ));
    }

    /// Test submit_transaction with "already known" error in Sent status
    /// This should treat the error as success and update to Submitted
    #[tokio::test]
    async fn test_submit_transaction_already_known_error_from_sent() {
        let mut mock_transaction = MockTransactionRepository::new();
        let mock_relayer = MockRelayerRepository::new();
        let mut mock_provider = MockEvmProviderTrait::new();
        let mock_signer = MockSigner::new();
        let mut mock_job_producer = MockJobProducerTrait::new();
        let mock_price_calculator = MockPriceCalculator::new();
        let counter_service = MockTransactionCounterTrait::new();
        let mock_network = MockNetworkRepository::new();

        let relayer = create_test_relayer();
        let mut test_tx = create_test_transaction();
        test_tx.status = TransactionStatus::Sent;
        test_tx.sent_at = Some(Utc::now().to_rfc3339());
        test_tx.network_data = NetworkTransactionData::Evm(EvmTransactionData {
            nonce: Some(42),
            hash: Some("0xhash".to_string()),
            raw: Some(vec![1, 2, 3]),
            ..test_tx.network_data.get_evm_transaction_data().unwrap()
        });

        // Provider returns "already known" error
        mock_provider
            .expect_send_raw_transaction()
            .times(1)
            .returning(|_| {
                Box::pin(async {
                    Err(crate::services::provider::ProviderError::Other(
                        "already known: transaction already in mempool".to_string(),
                    ))
                })
            });

        // Should still update to Submitted status
        let test_tx_clone = test_tx.clone();
        mock_transaction
            .expect_partial_update()
            .times(1)
            .withf(|_, update| update.status == Some(TransactionStatus::Submitted))
            .returning(move |_, update| {
                let mut updated_tx = test_tx_clone.clone();
                updated_tx.status = update.status.unwrap();
                updated_tx.sent_at = update.sent_at.clone();
                Ok(updated_tx)
            });

        mock_job_producer
            .expect_produce_send_notification_job()
            .times(1)
            .returning(|_, _| Box::pin(ready(Ok(()))));

        let evm_transaction = EvmRelayerTransaction {
            relayer: relayer.clone(),
            provider: mock_provider,
            relayer_repository: Arc::new(mock_relayer),
            network_repository: Arc::new(mock_network),
            transaction_repository: Arc::new(mock_transaction),
            transaction_counter_service: Arc::new(counter_service),
            job_producer: Arc::new(mock_job_producer),
            price_calculator: mock_price_calculator,
            signer: mock_signer,
        };

        let result = evm_transaction.submit_transaction(test_tx).await;
        assert!(result.is_ok());
        let updated_tx = result.unwrap();
        assert_eq!(updated_tx.status, TransactionStatus::Submitted);
    }

    /// Test submit_transaction with real error (not "already known") should fail
    #[tokio::test]
    async fn test_submit_transaction_real_error_fails() {
        let mock_transaction = MockTransactionRepository::new();
        let mock_relayer = MockRelayerRepository::new();
        let mut mock_provider = MockEvmProviderTrait::new();
        let mock_signer = MockSigner::new();
        let mock_job_producer = MockJobProducerTrait::new();
        let mock_price_calculator = MockPriceCalculator::new();
        let counter_service = MockTransactionCounterTrait::new();
        let mock_network = MockNetworkRepository::new();

        let relayer = create_test_relayer();
        let mut test_tx = create_test_transaction();
        test_tx.status = TransactionStatus::Sent;
        test_tx.network_data = NetworkTransactionData::Evm(EvmTransactionData {
            raw: Some(vec![1, 2, 3]),
            ..test_tx.network_data.get_evm_transaction_data().unwrap()
        });

        // Provider returns a real error
        mock_provider
            .expect_send_raw_transaction()
            .times(1)
            .returning(|_| {
                Box::pin(async {
                    Err(crate::services::provider::ProviderError::Other(
                        "insufficient funds for gas * price + value".to_string(),
                    ))
                })
            });

        let evm_transaction = EvmRelayerTransaction {
            relayer: relayer.clone(),
            provider: mock_provider,
            relayer_repository: Arc::new(mock_relayer),
            network_repository: Arc::new(mock_network),
            transaction_repository: Arc::new(mock_transaction),
            transaction_counter_service: Arc::new(counter_service),
            job_producer: Arc::new(mock_job_producer),
            price_calculator: mock_price_calculator,
            signer: mock_signer,
        };

        let result = evm_transaction.submit_transaction(test_tx).await;
        assert!(result.is_err());
    }

    /// Test resubmit_transaction when transaction is already submitted
    /// Should NOT update hash, only status
    #[tokio::test]
    async fn test_resubmit_transaction_already_submitted_preserves_hash() {
        let mut mock_transaction = MockTransactionRepository::new();
        let mock_relayer = MockRelayerRepository::new();
        let mut mock_provider = MockEvmProviderTrait::new();
        let mut mock_signer = MockSigner::new();
        let mock_job_producer = MockJobProducerTrait::new();
        let mut mock_price_calculator = MockPriceCalculator::new();
        let counter_service = MockTransactionCounterTrait::new();
        let mock_network = MockNetworkRepository::new();

        let relayer = create_test_relayer();
        let mut test_tx = create_test_transaction();
        test_tx.status = TransactionStatus::Submitted;
        test_tx.sent_at = Some(Utc::now().to_rfc3339());
        let original_hash = "0xoriginal_hash".to_string();
        test_tx.network_data = NetworkTransactionData::Evm(EvmTransactionData {
            nonce: Some(42),
            hash: Some(original_hash.clone()),
            raw: Some(vec![1, 2, 3]),
            ..test_tx.network_data.get_evm_transaction_data().unwrap()
        });
        test_tx.hashes = vec![original_hash.clone()];

        // Price calculator returns bumped price
        mock_price_calculator
            .expect_calculate_bumped_gas_price()
            .times(1)
            .returning(|_, _, _| {
                Ok(PriceParams {
                    gas_price: Some(25000000000), // 25% bump
                    max_fee_per_gas: None,
                    max_priority_fee_per_gas: None,
                    is_min_bumped: Some(true),
                    extra_fee: None,
                    total_cost: U256::from(525000000000000u64),
                })
            });

        // Balance check passes
        mock_provider
            .expect_get_balance()
            .times(1)
            .returning(|_| Box::pin(async { Ok(U256::from(1000000000000000000u64)) }));

        // Signer creates new transaction with new hash
        mock_signer
            .expect_sign_transaction()
            .times(1)
            .returning(|_| {
                Box::pin(ready(Ok(
                    crate::domain::relayer::SignTransactionResponse::Evm(
                        crate::domain::relayer::SignTransactionResponseEvm {
                            hash: "0xnew_hash_that_should_not_be_saved".to_string(),
                            signature: crate::models::EvmTransactionDataSignature {
                                r: "r".to_string(),
                                s: "s".to_string(),
                                v: 1,
                                sig: "0xsignature".to_string(),
                            },
                            raw: vec![4, 5, 6],
                        },
                    ),
                )))
            });

        // Provider returns "already known" - transaction is already in mempool
        mock_provider
            .expect_send_raw_transaction()
            .times(1)
            .returning(|_| {
                Box::pin(async {
                    Err(crate::services::provider::ProviderError::Other(
                        "already known: transaction with same nonce already in mempool".to_string(),
                    ))
                })
            });

        // Verify that partial_update is called with NO network_data (preserving original hash)
        let test_tx_clone = test_tx.clone();
        mock_transaction
            .expect_partial_update()
            .times(1)
            .withf(|_, update| {
                // Should only update status, NOT network_data or hashes
                update.status == Some(TransactionStatus::Submitted)
                    && update.network_data.is_none()
                    && update.hashes.is_none()
            })
            .returning(move |_, _| {
                let mut updated_tx = test_tx_clone.clone();
                updated_tx.status = TransactionStatus::Submitted;
                // Hash should remain unchanged!
                Ok(updated_tx)
            });

        let evm_transaction = EvmRelayerTransaction {
            relayer: relayer.clone(),
            provider: mock_provider,
            relayer_repository: Arc::new(mock_relayer),
            network_repository: Arc::new(mock_network),
            transaction_repository: Arc::new(mock_transaction),
            transaction_counter_service: Arc::new(counter_service),
            job_producer: Arc::new(mock_job_producer),
            price_calculator: mock_price_calculator,
            signer: mock_signer,
        };

        let result = evm_transaction.resubmit_transaction(test_tx.clone()).await;
        assert!(result.is_ok());
        let updated_tx = result.unwrap();

        // Verify hash was NOT changed
        if let NetworkTransactionData::Evm(evm_data) = &updated_tx.network_data {
            assert_eq!(evm_data.hash, Some(original_hash));
        } else {
            panic!("Expected EVM network data");
        }
    }

    /// Test submit_transaction with database update failure
    /// Transaction is on-chain, but DB update fails - should return Ok with original tx
    #[tokio::test]
    async fn test_submit_transaction_db_failure_after_blockchain_success() {
        let mut mock_transaction = MockTransactionRepository::new();
        let mock_relayer = MockRelayerRepository::new();
        let mut mock_provider = MockEvmProviderTrait::new();
        let mock_signer = MockSigner::new();
        let mut mock_job_producer = MockJobProducerTrait::new();
        let mock_price_calculator = MockPriceCalculator::new();
        let counter_service = MockTransactionCounterTrait::new();
        let mock_network = MockNetworkRepository::new();

        let relayer = create_test_relayer();
        let mut test_tx = create_test_transaction();
        test_tx.status = TransactionStatus::Sent;
        test_tx.network_data = NetworkTransactionData::Evm(EvmTransactionData {
            raw: Some(vec![1, 2, 3]),
            ..test_tx.network_data.get_evm_transaction_data().unwrap()
        });

        // Provider succeeds
        mock_provider
            .expect_send_raw_transaction()
            .times(1)
            .returning(|_| Box::pin(async { Ok("0xsubmitted_hash".to_string()) }));

        // But database update fails
        mock_transaction
            .expect_partial_update()
            .times(1)
            .returning(|_, _| {
                Err(crate::models::RepositoryError::UnexpectedError(
                    "Redis timeout".to_string(),
                ))
            });

        // Notification will still be sent (with original tx data)
        mock_job_producer
            .expect_produce_send_notification_job()
            .times(1)
            .returning(|_, _| Box::pin(ready(Ok(()))));

        let evm_transaction = EvmRelayerTransaction {
            relayer: relayer.clone(),
            provider: mock_provider,
            relayer_repository: Arc::new(mock_relayer),
            network_repository: Arc::new(mock_network),
            transaction_repository: Arc::new(mock_transaction),
            transaction_counter_service: Arc::new(counter_service),
            job_producer: Arc::new(mock_job_producer),
            price_calculator: mock_price_calculator,
            signer: mock_signer,
        };

        let result = evm_transaction.submit_transaction(test_tx.clone()).await;
        // Should return Ok (transaction is on-chain, don't retry)
        assert!(result.is_ok());
        let returned_tx = result.unwrap();
        // Should return original tx since DB update failed
        assert_eq!(returned_tx.id, test_tx.id);
        assert_eq!(returned_tx.status, TransactionStatus::Sent); // Original status
    }

    /// Test send_transaction_resend_job success
    #[tokio::test]
    async fn test_send_transaction_resend_job_success() {
        let mock_transaction = MockTransactionRepository::new();
        let mock_relayer = MockRelayerRepository::new();
        let mock_provider = MockEvmProviderTrait::new();
        let mock_signer = MockSigner::new();
        let mut mock_job_producer = MockJobProducerTrait::new();
        let mock_price_calculator = MockPriceCalculator::new();
        let counter_service = MockTransactionCounterTrait::new();
        let mock_network = MockNetworkRepository::new();

        let relayer = create_test_relayer();
        let test_tx = create_test_transaction();

        // Expect produce_submit_transaction_job to be called with resend job
        mock_job_producer
            .expect_produce_submit_transaction_job()
            .times(1)
            .withf(|job, delay| {
                // Verify it's a resend job with correct IDs
                job.transaction_id == "test-tx-id"
                    && job.relayer_id == "test-relayer-id"
                    && matches!(job.command, crate::jobs::TransactionCommand::Resend)
                    && delay.is_none()
            })
            .returning(|_, _| Box::pin(ready(Ok(()))));

        let evm_transaction = EvmRelayerTransaction {
            relayer: relayer.clone(),
            provider: mock_provider,
            relayer_repository: Arc::new(mock_relayer),
            network_repository: Arc::new(mock_network),
            transaction_repository: Arc::new(mock_transaction),
            transaction_counter_service: Arc::new(counter_service),
            job_producer: Arc::new(mock_job_producer),
            price_calculator: mock_price_calculator,
            signer: mock_signer,
        };

        let result = evm_transaction.send_transaction_resend_job(&test_tx).await;
        assert!(result.is_ok());
    }

    /// Test send_transaction_resend_job failure
    #[tokio::test]
    async fn test_send_transaction_resend_job_failure() {
        let mock_transaction = MockTransactionRepository::new();
        let mock_relayer = MockRelayerRepository::new();
        let mock_provider = MockEvmProviderTrait::new();
        let mock_signer = MockSigner::new();
        let mut mock_job_producer = MockJobProducerTrait::new();
        let mock_price_calculator = MockPriceCalculator::new();
        let counter_service = MockTransactionCounterTrait::new();
        let mock_network = MockNetworkRepository::new();

        let relayer = create_test_relayer();
        let test_tx = create_test_transaction();

        // Job producer returns an error
        mock_job_producer
            .expect_produce_submit_transaction_job()
            .times(1)
            .returning(|_, _| {
                Box::pin(ready(Err(crate::jobs::JobProducerError::QueueError(
                    "Job queue is full".to_string(),
                ))))
            });

        let evm_transaction = EvmRelayerTransaction {
            relayer: relayer.clone(),
            provider: mock_provider,
            relayer_repository: Arc::new(mock_relayer),
            network_repository: Arc::new(mock_network),
            transaction_repository: Arc::new(mock_transaction),
            transaction_counter_service: Arc::new(counter_service),
            job_producer: Arc::new(mock_job_producer),
            price_calculator: mock_price_calculator,
            signer: mock_signer,
        };

        let result = evm_transaction.send_transaction_resend_job(&test_tx).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            TransactionError::UnexpectedError(msg) => {
                assert!(msg.contains("Failed to produce resend job"));
            }
            _ => panic!("Expected UnexpectedError"),
        }
    }

    /// Test send_transaction_request_job success
    #[tokio::test]
    async fn test_send_transaction_request_job_success() {
        let mock_transaction = MockTransactionRepository::new();
        let mock_relayer = MockRelayerRepository::new();
        let mock_provider = MockEvmProviderTrait::new();
        let mock_signer = MockSigner::new();
        let mut mock_job_producer = MockJobProducerTrait::new();
        let mock_price_calculator = MockPriceCalculator::new();
        let counter_service = MockTransactionCounterTrait::new();
        let mock_network = MockNetworkRepository::new();

        let relayer = create_test_relayer();
        let test_tx = create_test_transaction();

        // Expect produce_transaction_request_job to be called
        mock_job_producer
            .expect_produce_transaction_request_job()
            .times(1)
            .withf(|job, delay| {
                // Verify correct transaction ID and relayer ID
                job.transaction_id == "test-tx-id"
                    && job.relayer_id == "test-relayer-id"
                    && delay.is_none()
            })
            .returning(|_, _| Box::pin(ready(Ok(()))));

        let evm_transaction = EvmRelayerTransaction {
            relayer: relayer.clone(),
            provider: mock_provider,
            relayer_repository: Arc::new(mock_relayer),
            network_repository: Arc::new(mock_network),
            transaction_repository: Arc::new(mock_transaction),
            transaction_counter_service: Arc::new(counter_service),
            job_producer: Arc::new(mock_job_producer),
            price_calculator: mock_price_calculator,
            signer: mock_signer,
        };

        let result = evm_transaction.send_transaction_request_job(&test_tx).await;
        assert!(result.is_ok());
    }

    /// Test send_transaction_request_job failure
    #[tokio::test]
    async fn test_send_transaction_request_job_failure() {
        let mock_transaction = MockTransactionRepository::new();
        let mock_relayer = MockRelayerRepository::new();
        let mock_provider = MockEvmProviderTrait::new();
        let mock_signer = MockSigner::new();
        let mut mock_job_producer = MockJobProducerTrait::new();
        let mock_price_calculator = MockPriceCalculator::new();
        let counter_service = MockTransactionCounterTrait::new();
        let mock_network = MockNetworkRepository::new();

        let relayer = create_test_relayer();
        let test_tx = create_test_transaction();

        // Job producer returns an error
        mock_job_producer
            .expect_produce_transaction_request_job()
            .times(1)
            .returning(|_, _| {
                Box::pin(ready(Err(crate::jobs::JobProducerError::QueueError(
                    "Redis connection failed".to_string(),
                ))))
            });

        let evm_transaction = EvmRelayerTransaction {
            relayer: relayer.clone(),
            provider: mock_provider,
            relayer_repository: Arc::new(mock_relayer),
            network_repository: Arc::new(mock_network),
            transaction_repository: Arc::new(mock_transaction),
            transaction_counter_service: Arc::new(counter_service),
            job_producer: Arc::new(mock_job_producer),
            price_calculator: mock_price_calculator,
            signer: mock_signer,
        };

        let result = evm_transaction.send_transaction_request_job(&test_tx).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            TransactionError::UnexpectedError(msg) => {
                assert!(msg.contains("Failed to produce request job"));
            }
            _ => panic!("Expected UnexpectedError"),
        }
    }

    /// Test resubmit_transaction successfully transitions from Sent to Submitted status
    #[tokio::test]
    async fn test_resubmit_transaction_sent_to_submitted() {
        let mut mock_transaction = MockTransactionRepository::new();
        let mock_relayer = MockRelayerRepository::new();
        let mut mock_provider = MockEvmProviderTrait::new();
        let mut mock_signer = MockSigner::new();
        let mock_job_producer = MockJobProducerTrait::new();
        let mut mock_price_calculator = MockPriceCalculator::new();
        let counter_service = MockTransactionCounterTrait::new();
        let mock_network = MockNetworkRepository::new();

        let relayer = create_test_relayer();
        let mut test_tx = create_test_transaction();
        test_tx.status = TransactionStatus::Sent;
        test_tx.sent_at = Some(Utc::now().to_rfc3339());
        let original_hash = "0xoriginal_hash".to_string();
        test_tx.network_data = NetworkTransactionData::Evm(EvmTransactionData {
            nonce: Some(42),
            hash: Some(original_hash.clone()),
            raw: Some(vec![1, 2, 3]),
            gas_price: Some(20000000000), // 20 Gwei
            ..test_tx.network_data.get_evm_transaction_data().unwrap()
        });
        test_tx.hashes = vec![original_hash.clone()];

        // Price calculator returns bumped price
        mock_price_calculator
            .expect_calculate_bumped_gas_price()
            .times(1)
            .returning(|_, _, _| {
                Ok(PriceParams {
                    gas_price: Some(25000000000), // 25 Gwei (25% bump)
                    max_fee_per_gas: None,
                    max_priority_fee_per_gas: None,
                    is_min_bumped: Some(true),
                    extra_fee: None,
                    total_cost: U256::from(525000000000000u64),
                })
            });

        // Mock balance check
        mock_provider
            .expect_get_balance()
            .returning(|_| Box::pin(ready(Ok(U256::from(1000000000000000000u64)))));

        // Mock signer to return new signed transaction
        mock_signer.expect_sign_transaction().returning(|_| {
            Box::pin(ready(Ok(
                crate::domain::relayer::SignTransactionResponse::Evm(
                    crate::domain::relayer::SignTransactionResponseEvm {
                        hash: "0xnew_hash".to_string(),
                        signature: crate::models::EvmTransactionDataSignature {
                            r: "r".to_string(),
                            s: "s".to_string(),
                            v: 1,
                            sig: "0xsignature".to_string(),
                        },
                        raw: vec![4, 5, 6],
                    },
                ),
            )))
        });

        // Provider successfully sends the resubmitted transaction
        mock_provider
            .expect_send_raw_transaction()
            .times(1)
            .returning(|_| Box::pin(async { Ok("0xnew_hash".to_string()) }));

        // Should update to Submitted status with new hash
        let test_tx_clone = test_tx.clone();
        mock_transaction
            .expect_partial_update()
            .times(1)
            .withf(|_, update| {
                update.status == Some(TransactionStatus::Submitted)
                    && update.sent_at.is_some()
                    && update.priced_at.is_some()
                    && update.hashes.is_some()
            })
            .returning(move |_, update| {
                let mut updated_tx = test_tx_clone.clone();
                updated_tx.status = update.status.unwrap();
                updated_tx.sent_at = update.sent_at.clone();
                updated_tx.priced_at = update.priced_at.clone();
                if let Some(hashes) = update.hashes.clone() {
                    updated_tx.hashes = hashes;
                }
                if let Some(network_data) = update.network_data.clone() {
                    updated_tx.network_data = network_data;
                }
                Ok(updated_tx)
            });

        let evm_transaction = EvmRelayerTransaction {
            relayer: relayer.clone(),
            provider: mock_provider,
            relayer_repository: Arc::new(mock_relayer),
            network_repository: Arc::new(mock_network),
            transaction_repository: Arc::new(mock_transaction),
            transaction_counter_service: Arc::new(counter_service),
            job_producer: Arc::new(mock_job_producer),
            price_calculator: mock_price_calculator,
            signer: mock_signer,
        };

        let result = evm_transaction.resubmit_transaction(test_tx.clone()).await;
        assert!(result.is_ok(), "Expected Ok, got: {:?}", result);
        let updated_tx = result.unwrap();
        assert_eq!(
            updated_tx.status,
            TransactionStatus::Submitted,
            "Transaction status should transition from Sent to Submitted"
        );
    }
}
