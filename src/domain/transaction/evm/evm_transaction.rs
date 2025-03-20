//! This module defines the `EvmRelayerTransaction` struct and its associated
//! functionality for handling Ethereum Virtual Machine (EVM) transactions.
//! It includes methods for preparing, submitting, handling status, and
//! managing notifications for transactions. The module leverages various
//! services and repositories to perform these operations asynchronously.
use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use eyre::Result;
use log::{debug, info, warn};
use std::sync::Arc;

use super::PriceParams;
use crate::{
    constants::DEFAULT_TX_VALID_TIMESPAN,
    domain::{is_transaction_not_yet_mined, make_noop, transaction::Transaction, PriceCalculator},
    jobs::{JobProducer, JobProducerTrait, TransactionSend, TransactionStatusCheck},
    models::{
        evm::Speed, produce_transaction_update_notification_payload, EvmNetwork,
        EvmTransactionData, NetworkTransactionData, RelayerRepoModel, TransactionError,
        TransactionRepoModel, TransactionStatus, TransactionUpdateRequest,
    },
    repositories::{
        InMemoryRelayerRepository, InMemoryTransactionCounter, RelayerRepositoryStorage,
        Repository, TransactionCounterTrait, TransactionRepository,
    },
    services::{
        EvmGasPriceService, EvmGasPriceServiceTrait, EvmProvider, EvmProviderTrait, EvmSigner,
        Signer,
    },
};

use crate::domain::transaction::evm::price_calculator::PriceCalculatorTrait;

#[allow(dead_code)]
pub struct EvmRelayerTransaction<P, R, T, J, G, S, C, PC>
where
    P: EvmProviderTrait,
    R: Repository<RelayerRepoModel, String>,
    T: TransactionRepository,
    J: JobProducerTrait,
    G: EvmGasPriceServiceTrait,
    S: Signer,
    C: TransactionCounterTrait,
    PC: PriceCalculatorTrait,
{
    relayer: RelayerRepoModel,
    provider: P,
    relayer_repository: Arc<R>,
    transaction_repository: Arc<T>,
    transaction_counter_service: Arc<C>,
    job_producer: Arc<J>,
    gas_price_service: Arc<G>,
    price_calculator: Arc<PC>,
    signer: S,
}

#[allow(dead_code, clippy::too_many_arguments)]
impl<P, R, T, J, G, S, C, PC> EvmRelayerTransaction<P, R, T, J, G, S, C, PC>
where
    P: EvmProviderTrait,
    R: Repository<RelayerRepoModel, String>,
    T: TransactionRepository,
    J: JobProducerTrait,
    G: EvmGasPriceServiceTrait,
    S: Signer,
    C: TransactionCounterTrait,
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
    /// * `gas_price_service` - Service for gas price management.
    /// * `signer` - The EVM signer.
    ///
    /// # Returns
    ///
    /// A result containing the new `EvmRelayerTransaction` or a `TransactionError`.
    pub fn new(
        relayer: RelayerRepoModel,
        provider: P,
        relayer_repository: Arc<R>,
        transaction_repository: Arc<T>,
        transaction_counter_service: Arc<C>,
        job_producer: Arc<J>,
        gas_price_service: Arc<G>,
        price_calculator: Arc<PC>,
        signer: S,
    ) -> Result<Self, TransactionError> {
        Ok(Self {
            relayer,
            provider,
            relayer_repository,
            transaction_repository,
            transaction_counter_service,
            job_producer,
            gas_price_service,
            price_calculator,
            signer,
        })
    }

    /// Prepares a cancellation transaction with higher gas price to replace the original transaction
    ///
    /// # Arguments
    ///
    /// * `evm_data` - The original transaction's EVM data
    /// * `relayer` - The relayer model
    ///
    /// # Returns
    ///
    /// A result containing the prepared cancellation transaction data or a `TransactionError`
    async fn prepare_cancel_transaction(
        &self,
        evm_data: &mut EvmTransactionData,
        relayer: &RelayerRepoModel,
    ) -> Result<EvmTransactionData, TransactionError> {
        // Get the nonce from the transaction
        let nonce = evm_data.nonce.ok_or_else(|| {
            TransactionError::UnexpectedError("Transaction nonce is missing".to_string())
        })?;

        // clean the price params and set speed to average for the cancellation
        evm_data.gas_price = None;
        evm_data.max_fee_per_gas = None;
        evm_data.max_priority_fee_per_gas = None;
        evm_data.speed = Some(Speed::Average);

        // Calculate gas price for cancellation (higher than original)
        let price_params: PriceParams = self
            .price_calculator
            .get_transaction_price_params(evm_data, relayer)
            .await?;

        // Create a "noop" transaction with higher gas price
        let mut cancel_tx_data = make_noop(
            relayer.address.clone(),
            price_params,
            EvmNetwork::from_id(evm_data.chain_id),
        )
        .await?;

        // Set the nonce to match the original transaction
        cancel_tx_data.nonce = Some(nonce);

        // Sign the cancellation transaction
        let sig_result = self
            .signer
            .sign_transaction(NetworkTransactionData::Evm(cancel_tx_data.clone()))
            .await?;

        Ok(cancel_tx_data.with_signed_transaction_data(sig_result.into_evm()?))
    }

    /// Helper function to check if a transaction has enough confirmations
    ///
    /// # Arguments
    ///
    /// * `tx_block_number` - Block number where the transaction was mined
    /// * `current_block_number` - Current block number
    /// * `chain_id` - The chain ID to determine confirmation requirements
    ///
    /// # Returns
    ///
    /// `true` if the transaction has enough confirmations for the given network
    fn has_enough_confirmations(
        tx_block_number: u64,
        current_block_number: u64,
        chain_id: u64,
    ) -> bool {
        let network = EvmNetwork::from_id(chain_id);
        let required_confirmations = network.required_confirmations();
        current_block_number >= tx_block_number + required_confirmations
    }

    /// Checks if a transaction is still valid based on its valid_until timestamp.
    /// If valid_until is not set, it uses the default timespan from constants.
    ///
    /// # Arguments
    ///
    /// * `created_at` - When the transaction was created
    /// * `valid_until` - Optional timestamp string when the transaction expires
    ///
    /// # Returns
    ///
    /// `true` if the transaction is still valid, `false` if it has expired
    fn is_transaction_valid(created_at: &str, valid_until: &Option<String>) -> bool {
        // If valid_until is provided, use it to determine validity
        if let Some(valid_until_str) = valid_until {
            match DateTime::parse_from_rfc3339(valid_until_str) {
                Ok(valid_until_time) => {
                    // Valid if current time is before valid_until time
                    return Utc::now() < valid_until_time;
                }
                Err(e) => {
                    warn!("Failed to parse valid_until timestamp: {}", e);
                    return false;
                }
            }
        }

        // If we get here valid_until wasn't provided
        match DateTime::parse_from_rfc3339(created_at) {
            Ok(created_time) => {
                // Calculate default expiration time
                let default_valid_until =
                    created_time + Duration::milliseconds(DEFAULT_TX_VALID_TIMESPAN);
                // Valid if current time is before default expiration
                Utc::now() < default_valid_until
            }
            Err(e) => {
                warn!("Failed to parse created_at timestamp: {}", e);
                false
            }
        }
    }

    /// Checks transaction confirmation status.
    ///
    /// # Arguments
    ///
    /// * `tx` - The transaction repository model containing metadata like valid_until.
    ///
    /// # Returns
    ///
    /// A result containing either:
    /// - `Ok(TransactionStatus::Confirmed)` if the transaction succeeded with enough confirmations
    /// - `Ok(TransactionStatus::Mined)` if the transaction is mined but doesn't have enough
    ///   confirmations
    /// - `Ok(TransactionStatus::Submitted)` if the transaction is not yet mined
    /// - `Ok(TransactionStatus::Failed)` if the transaction has failed
    /// - `Ok(TransactionStatus::Expired)` if the transaction has expired
    /// - `Err(TransactionError)` if an error occurred
    async fn check_transaction_status(
        &self,
        tx: &TransactionRepoModel,
    ) -> Result<TransactionStatus, TransactionError> {
        if tx.status == TransactionStatus::Expired
            || tx.status == TransactionStatus::Failed
            || tx.status == TransactionStatus::Confirmed
        {
            return Ok(tx.status.clone());
        }

        // Check if the transaction has expired
        if !Self::is_transaction_valid(&tx.created_at, &tx.valid_until) {
            info!("Transaction expired: {}", tx.id);
            return Ok(TransactionStatus::Expired);
        }

        let evm_data = tx.network_data.get_evm_transaction_data()?;
        let tx_hash = evm_data
            .hash
            .as_ref()
            .ok_or(TransactionError::UnexpectedError(
                "Transaction hash is missing".to_string(),
            ))?;

        // Check if transaction is mined
        let receipt_result = self.provider.get_transaction_receipt(tx_hash).await?;

        // Use if let Some to extract the receipt if it exists
        if let Some(receipt) = receipt_result {
            // If transaction failed, return Failed status
            if !receipt.status() {
                return Ok(TransactionStatus::Failed);
            }

            let last_block_number = self.provider.get_block_number().await?;
            let tx_block_number = receipt
                .block_number
                .ok_or(TransactionError::UnexpectedError(
                    "Transaction receipt missing block number".to_string(),
                ))?;
            if !Self::has_enough_confirmations(
                tx_block_number,
                last_block_number,
                evm_data.chain_id,
            ) {
                info!("Transaction mined but not confirmed: {}", tx_hash);
                return Ok(TransactionStatus::Mined);
            }

            // Transaction is confirmed
            Ok(TransactionStatus::Confirmed)
        } else {
            // If we get here, there's no receipt, so the transaction is not yet mined
            info!("Transaction not yet mined: {}", tx_hash);
            Ok(TransactionStatus::Submitted)
        }
    }

    /// Returns a reference to the gas price service.
    pub fn gas_price_service(&self) -> &Arc<G> {
        &self.gas_price_service
    }

    /// Returns a reference to the provider.
    pub fn provider(&self) -> &P {
        &self.provider
    }

    /// Returns a reference to the relayer model.
    pub fn relayer(&self) -> &RelayerRepoModel {
        &self.relayer
    }

    /// Helper method to send a transaction update notification if a notification ID is configured.
    ///
    /// # Arguments
    ///
    /// * `tx` - The transaction model to send a notification for.
    ///
    /// # Returns
    ///
    /// A result indicating success or a `TransactionError`.
    async fn send_transaction_update_notification(
        &self,
        tx: &TransactionRepoModel,
    ) -> Result<(), TransactionError> {
        if let Some(notification_id) = &self.relayer.notification_id {
            self.job_producer
                .produce_send_notification_job(
                    produce_transaction_update_notification_payload(notification_id, tx),
                    None,
                )
                .await?;
        }
        Ok(())
    }

    async fn update_transaction_status(
        &self,
        tx: TransactionRepoModel,
        new_status: TransactionStatus,
        confirmed_at: Option<String>,
    ) -> Result<TransactionRepoModel, TransactionError> {
        let update_request = TransactionUpdateRequest {
            status: Some(new_status),
            confirmed_at,
            ..Default::default()
        };

        let updated_tx = self
            .transaction_repository
            .partial_update(tx.id.clone(), update_request)
            .await?;

        self.send_transaction_update_notification(&updated_tx)
            .await?;
        Ok(updated_tx)
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

    /// Replaces a transaction.
    ///
    /// # Arguments
    ///
    /// * `tx` - The transaction model to replace.
    ///
    /// # Returns
    ///
    /// A result containing the transaction model or a `TransactionError`.
    async fn replace_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        Ok(tx)
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

    /// Gets the age of a transaction since it was sent
    fn get_age_of_sent_at(&self, tx: &TransactionRepoModel) -> Result<Duration, TransactionError> {
        let now = Utc::now();
        let sent_at_str = tx.sent_at.as_ref().ok_or_else(|| {
            TransactionError::UnexpectedError("Transaction sent_at time is missing".to_string())
        })?;
        let sent_time = DateTime::parse_from_rfc3339(sent_at_str)
            .map_err(|_| {
                TransactionError::UnexpectedError("Error parsing sent_at time".to_string())
            })?
            .with_timezone(&Utc);
        Ok(now.signed_duration_since(sent_time))
    }

    /// Determines if a transaction should be resubmitted
    /// Returns true if the transaction should be resubmitted
    async fn should_resubmit(&self, tx: &TransactionRepoModel) -> Result<bool, TransactionError> {
        if tx.status != TransactionStatus::Submitted {
            return Err(TransactionError::UnexpectedError(format!(
                "Transaction must be in Submitted status to resubmit, found: {:?}",
                tx.status
            )));
        }

        let age = self.get_age_of_sent_at(tx)?;

        // Check for speed and determine timeout
        let timeout =
            self.get_resubmit_timeout_for_speed(&tx.network_data.get_evm_transaction_data()?.speed);

        let timeout_with_backoff = self.get_resubmit_timeout_with_backoff(timeout, tx.hashes.len());

        // If the transaction has been waiting for more than our timeout, resubmit it
        if age > Duration::milliseconds(timeout_with_backoff) {
            info!("Transaction has been pending for too long, resubmitting");
            return Ok(true);
        }

        Ok(false)
    }

    /// Gets the appropriate resubmission timeout based on transaction speed
    fn get_resubmit_timeout_for_speed(&self, speed: &Option<Speed>) -> i64 {
        // Default timeout (30 seconds for regular transactions)
        const DEFAULT_TIMEOUT: i64 = 30_000;
        // Faster timeout (15 seconds for faster transactions)
        const FAST_TIMEOUT: i64 = 15_000;
        // Slow timeout (60 seconds for slower transactions)
        const SLOW_TIMEOUT: i64 = 60_000;

        match speed {
            Some(Speed::Fast) | Some(Speed::Fastest) => FAST_TIMEOUT,
            Some(Speed::Average) => DEFAULT_TIMEOUT,
            Some(Speed::SafeLow) => SLOW_TIMEOUT,
            None => DEFAULT_TIMEOUT,
        }
    }

    /// Applies exponential backoff to resubmission timeout based on number of attempts
    fn get_resubmit_timeout_with_backoff(&self, base_timeout: i64, attempts: usize) -> i64 {
        // Apply exponential backoff, but cap at a reasonable maximum (5 minutes)
        const MAX_TIMEOUT: i64 = 300_000;

        let multiplier = 2_i64.pow(attempts.min(10) as u32);
        (base_timeout * multiplier).min(MAX_TIMEOUT)
    }
}

#[async_trait]
impl<P, R, T, J, G, S, C, PC> Transaction for EvmRelayerTransaction<P, R, T, J, G, S, C, PC>
where
    P: EvmProviderTrait + Send + Sync,
    R: Repository<RelayerRepoModel, String> + Send + Sync,
    T: TransactionRepository + Send + Sync,
    J: JobProducerTrait + Send + Sync,
    G: EvmGasPriceServiceTrait + Send + Sync,
    S: Signer + Send + Sync,
    C: TransactionCounterTrait + Send + Sync,
    PC: PriceCalculatorTrait + Send + Sync,
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
        info!("Preparing transaction: {:?}", tx.id);

        let evm_data = tx.network_data.get_evm_transaction_data()?;
        // set the gas price
        let relayer = self.relayer();
        let price_params: PriceParams = self
            .price_calculator
            .get_transaction_price_params(&evm_data, relayer)
            .await?;
        debug!("Gas price: {:?}", price_params.gas_price);
        // increment the nonce
        let nonce = self
            .transaction_counter_service
            .get_and_increment(&self.relayer.id, &self.relayer.address)
            .map_err(|e| TransactionError::UnexpectedError(e.to_string()))?;

        let updated_evm_data = tx
            .network_data
            .get_evm_transaction_data()?
            .with_price_params(price_params)
            .with_nonce(nonce);

        // sign the transaction
        let sig_result = self
            .signer
            .sign_transaction(NetworkTransactionData::Evm(updated_evm_data.clone()))
            .await?;

        let updated_evm_data =
            updated_evm_data.with_signed_transaction_data(sig_result.into_evm()?);

        let update = TransactionUpdateRequest {
            status: Some(TransactionStatus::Sent),
            network_data: Some(NetworkTransactionData::Evm(updated_evm_data)),
            ..Default::default()
        };

        let updated_tx = self
            .transaction_repository
            .partial_update(tx.id.clone(), update)
            .await?;

        // after preparing the transaction, we need to submit it to the job queue
        self.job_producer
            .produce_submit_transaction_job(
                TransactionSend::submit(updated_tx.id.clone(), updated_tx.relayer_id.clone()),
                None,
            )
            .await?;

        self.send_transaction_update_notification(&updated_tx)
            .await?;

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
        info!("submitting transaction for tx: {:?}", tx.id);

        let evm_tx_data = tx.network_data.get_evm_transaction_data()?;
        let raw_tx = evm_tx_data.raw.as_ref().ok_or_else(|| {
            TransactionError::InvalidType("Raw transaction data is missing".to_string())
        })?;

        self.provider.send_raw_transaction(raw_tx).await?;

        let update = TransactionUpdateRequest {
            status: Some(TransactionStatus::Submitted),
            sent_at: Some(Utc::now().to_rfc3339()),
            ..Default::default()
        };

        let updated_tx = self
            .transaction_repository
            .partial_update(tx.id.clone(), update)
            .await?;

        // after submitting the transaction, we need to handle the transaction status
        self.job_producer
            .produce_check_transaction_status_job(
                TransactionStatusCheck::new(updated_tx.id.clone(), updated_tx.relayer_id.clone()),
                None,
            )
            .await?;

        self.send_transaction_update_notification(&updated_tx)
            .await?;

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
        info!("Checking transaction status for tx: {:?}", tx.id);

        let status = self.check_transaction_status(&tx).await?;

        match status {
            TransactionStatus::Submitted => {
                // Check if transaction needs resubmission
                if self.should_resubmit(&tx).await? {
                    info!(
                        "Transaction pending for too long, resubmitting: {:?}",
                        tx.id
                    );
                    self.job_producer
                        .produce_submit_transaction_job(
                            TransactionSend::resubmit(tx.id.clone(), tx.relayer_id.clone()),
                            None,
                        )
                        .await?;
                }

                // Otherwise, continue with normal status check
                self.job_producer
                    .produce_check_transaction_status_job(
                        TransactionStatusCheck::new(tx.id.clone(), tx.relayer_id.clone()),
                        Some(Utc::now().timestamp() + 5),
                    )
                    .await?;

                if tx.status != status {
                    return self.update_transaction_status(tx, status, None).await;
                }

                Ok(tx)
            }
            TransactionStatus::Mined => {
                // For mined transactions, just schedule the next check
                self.job_producer
                    .produce_check_transaction_status_job(
                        TransactionStatusCheck::new(tx.id.clone(), tx.relayer_id.clone()),
                        Some(Utc::now().timestamp() + 5),
                    )
                    .await?;

                if tx.status != status {
                    return self.update_transaction_status(tx, status, None).await;
                }

                Ok(tx)
            }
            TransactionStatus::Confirmed
            | TransactionStatus::Failed
            | TransactionStatus::Expired => {
                let confirmed_at = if status == TransactionStatus::Confirmed {
                    Some(Utc::now().to_rfc3339())
                } else {
                    None
                };

                self.update_transaction_status(tx, status, confirmed_at)
                    .await
            }
            _ => Err(TransactionError::UnexpectedError(format!(
                "Unexpected transaction status: {:?}",
                status
            ))),
        }
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
        info!("Resubmitting transaction: {:?}", tx.id);

        // Calculate bumped gas price
        let bumped_price_params = self
            .price_calculator
            .calculate_bumped_gas_price(&tx, self.relayer())
            .await?;

        // Get transaction data
        let evm_data = tx.network_data.get_evm_transaction_data()?;

        // Create new transaction data with bumped gas price
        let updated_evm_data = evm_data.with_price_params(bumped_price_params);

        // Sign the transaction
        let sig_result = self
            .signer
            .sign_transaction(NetworkTransactionData::Evm(updated_evm_data.clone()))
            .await?;

        let final_evm_data = updated_evm_data.with_signed_transaction_data(sig_result.into_evm()?);

        let raw_tx = final_evm_data.raw.as_ref().ok_or_else(|| {
            TransactionError::InvalidType("Raw transaction data is missing".to_string())
        })?;

        self.provider.send_raw_transaction(raw_tx).await?;

        // Track attempt count and hash history
        let mut hashes = tx.hashes.clone();
        if let Some(hash) = final_evm_data.hash.clone() {
            hashes.push(hash);
        }

        // Update the transaction in the repository
        let update = TransactionUpdateRequest {
            network_data: Some(NetworkTransactionData::Evm(final_evm_data)),
            hashes: Some(hashes),
            priced_at: Some(Utc::now().to_rfc3339()),
            ..Default::default()
        };

        let updated_tx = self
            .transaction_repository
            .partial_update(tx.id.clone(), update)
            .await?;

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
        info!("Cancelling transaction: {:?}", tx.id);
        info!("Transaction status: {:?}", tx.status);
        // Check if the transaction can be cancelled
        if !is_transaction_not_yet_mined(tx.status.clone()) {
            return Err(TransactionError::ValidationError(format!(
                "Cannot cancel transaction with status: {:?}",
                tx.status
            )));
        }

        // If the transaction is in Pending state, we can just delete it from the database
        // since it was never sent to the network
        if tx.status == TransactionStatus::Pending {
            info!("Transaction is in Pending state, deleting it from the database");

            // Store a copy of the transaction for notification purposes
            let tx_copy = tx.clone();

            // Delete the transaction from the database
            Repository::delete_by_id(&*self.transaction_repository, tx.id.clone()).await?;

            // Send notification if configured
            self.send_transaction_update_notification(&tx_copy).await?;

            info!("Transaction deleted successfully: {:?}", tx.id);
            return Ok(tx_copy);
        }

        // For transactions in Sent/Submitted state, we need to send a cancellation transaction
        let mut evm_data = tx.network_data.get_evm_transaction_data()?;
        let hash_previous = evm_data.hash.clone();
        let relayer = self.relayer();

        // Prepare the cancellation transaction
        let signed_cancel_tx_data = self
            .prepare_cancel_transaction(&mut evm_data, relayer)
            .await?;

        // Track the new transaction hash in the hashes history
        let mut hashes = tx.hashes.clone();
        if let Some(hash) = hash_previous {
            hashes.push(hash);
        }

        // Update original transaction with the cancel/noop transaction data
        let update = TransactionUpdateRequest {
            network_data: Some(NetworkTransactionData::Evm(signed_cancel_tx_data)),
            status: Some(TransactionStatus::Sent), // Reset status to Canceled
            sent_at: None, // Clear sent_at since it will be updated when the transaction is submitted
            hashes: Some(hashes),
            priced_at: Some(Utc::now().to_rfc3339()),
            ..Default::default()
        };

        let updated_tx = self
            .transaction_repository
            .partial_update(tx.id.clone(), update)
            .await?;

        // Submit the updated transaction to the network using the resubmit job
        self.job_producer
            .produce_submit_transaction_job(
                TransactionSend::resubmit(updated_tx.id.clone(), updated_tx.relayer_id.clone()),
                None,
            )
            .await?;

        // Send notification for the updated transaction
        self.send_transaction_update_notification(&updated_tx)
            .await?;

        info!(
            "Original transaction updated with cancellation data: {:?}",
            updated_tx.id
        );
        Ok(updated_tx)
    }

    /// Replaces a transaction.
    ///
    /// # Arguments
    ///
    /// * `tx` - The transaction model to replace.
    ///
    /// # Returns
    ///
    /// A result containing the transaction model or a `TransactionError`.
    async fn replace_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        Ok(tx)
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
}

// we define concrete type for the evm transaction
pub type DefaultEvmTransaction = EvmRelayerTransaction<
    EvmProvider,
    RelayerRepositoryStorage<InMemoryRelayerRepository>,
    crate::repositories::transaction::InMemoryTransactionRepository,
    JobProducer,
    EvmGasPriceService<EvmProvider>,
    EvmSigner,
    InMemoryTransactionCounter,
    PriceCalculator<EvmGasPriceService<EvmProvider>>,
>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domain::MockPriceCalculatorTrait,
        jobs::MockJobProducerTrait,
        models::{
            evm::Speed, EvmNamedNetwork, EvmTransactionData, NetworkType, RelayerNetworkPolicy,
            U256,
        },
        repositories::{MockRepository, MockTransactionCounterTrait, MockTransactionRepository},
        services::{MockEvmGasPriceServiceTrait, MockEvmProviderTrait, MockSigner},
    };
    use chrono::{Duration, Utc};

    // Create a concrete type alias for testing
    type TestEvmTransaction = DefaultEvmTransaction;

    // Helper to create test transactions
    #[allow(dead_code)]
    fn create_test_transaction() -> TransactionRepoModel {
        TransactionRepoModel {
            id: "test-tx-id".to_string(),
            relayer_id: "test-relayer-id".to_string(),
            status: TransactionStatus::Pending,
            created_at: Utc::now().to_rfc3339(),
            sent_at: None,
            confirmed_at: None,
            valid_until: None,
            network_type: NetworkType::Evm,
            network_data: NetworkTransactionData::Evm(EvmTransactionData {
                chain_id: 1,
                from: "0xSender".to_string(),
                to: Some("0xRecipient".to_string()),
                value: U256::from(1000000000000000000u64), // 1 ETH
                data: Some("0xData".to_string()),
                gas_limit: 21000,
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
            hashes: vec![],
            noop_count: None,
        }
    }

    // Helper to create a relayer model
    fn create_test_relayer() -> RelayerRepoModel {
        RelayerRepoModel {
            id: "test-relayer-id".to_string(),
            name: "Test Relayer".to_string(),
            network: "1".to_string(), // Ethereum Mainnet
            address: "0xSender".to_string(),
            paused: false,
            system_disabled: false,
            signer_id: "test-signer-id".to_string(),
            notification_id: Some("test-notification-id".to_string()),
            policies: RelayerNetworkPolicy::Evm(crate::models::RelayerEvmPolicy {
                min_balance: 100000000000000000u128, // 0.1 ETH
                whitelist_receivers: Some(vec!["0xRecipient".to_string()]),
                gas_price_cap: Some(100000000000), // 100 Gwei
                eip1559_pricing: Some(false),
                private_transactions: false,
            }),
            network_type: NetworkType::Evm,
        }
    }

    // Test for the is_transaction_valid functionality
    #[test]
    fn test_is_transaction_valid_with_future_timestamp() {
        let now = Utc::now();
        let valid_until = Some((now + Duration::hours(1)).to_rfc3339());
        let created_at = now.to_rfc3339();

        assert!(TestEvmTransaction::is_transaction_valid(
            &created_at,
            &valid_until
        ));
    }

    #[test]
    fn test_is_transaction_valid_with_past_timestamp() {
        let now = Utc::now();
        let valid_until = Some((now - Duration::hours(1)).to_rfc3339());
        let created_at = now.to_rfc3339();

        assert!(!TestEvmTransaction::is_transaction_valid(
            &created_at,
            &valid_until
        ));
    }

    #[test]
    fn test_has_enough_confirmations() {
        // Test Ethereum Mainnet (requires 12 confirmations)
        let chain_id = 1; // Ethereum Mainnet

        // Not enough confirmations
        let tx_block_number = 100;
        let current_block_number = 110; // Only 10 confirmations
        assert!(!TestEvmTransaction::has_enough_confirmations(
            tx_block_number,
            current_block_number,
            chain_id
        ));

        // Exactly enough confirmations
        let current_block_number = 112; // Exactly 12 confirmations
        assert!(TestEvmTransaction::has_enough_confirmations(
            tx_block_number,
            current_block_number,
            chain_id
        ));

        // More than enough confirmations
        let current_block_number = 120; // 20 confirmations
        assert!(TestEvmTransaction::has_enough_confirmations(
            tx_block_number,
            current_block_number,
            chain_id
        ));
    }

    #[test]
    fn test_is_transaction_valid_with_valid_until() {
        // Test with valid_until in the future
        let created_at = Utc::now().to_rfc3339();
        let valid_until = Some((Utc::now() + Duration::hours(1)).to_rfc3339());

        assert!(TestEvmTransaction::is_transaction_valid(
            &created_at,
            &valid_until
        ));

        // Test with valid_until in the past
        let valid_until = Some((Utc::now() - Duration::hours(1)).to_rfc3339());

        assert!(!TestEvmTransaction::is_transaction_valid(
            &created_at,
            &valid_until
        ));

        // Test with valid_until exactly at current time (should be invalid)
        let valid_until = Some(Utc::now().to_rfc3339());
        assert!(!TestEvmTransaction::is_transaction_valid(
            &created_at,
            &valid_until
        ));

        // Test with valid_until very far in the future
        let valid_until = Some((Utc::now() + Duration::days(365)).to_rfc3339());
        assert!(TestEvmTransaction::is_transaction_valid(
            &created_at,
            &valid_until
        ));

        // Test with invalid valid_until format
        let valid_until = Some("invalid-date-format".to_string());

        // Should return false when parsing fails
        assert!(!TestEvmTransaction::is_transaction_valid(
            &created_at,
            &valid_until
        ));

        // Test with empty valid_until string
        let valid_until = Some("".to_string());
        assert!(!TestEvmTransaction::is_transaction_valid(
            &created_at,
            &valid_until
        ));
    }

    #[test]
    fn test_is_transaction_valid_without_valid_until() {
        // Test with created_at within the default timespan
        let created_at = Utc::now().to_rfc3339();
        let valid_until = None;

        assert!(TestEvmTransaction::is_transaction_valid(
            &created_at,
            &valid_until
        ));

        // Test with created_at older than the default timespan (8 hours)
        let old_created_at =
            (Utc::now() - Duration::milliseconds(DEFAULT_TX_VALID_TIMESPAN + 1000)).to_rfc3339();

        assert!(!TestEvmTransaction::is_transaction_valid(
            &old_created_at,
            &valid_until
        ));

        // Test with created_at exactly at the boundary of default timespan
        let boundary_created_at =
            (Utc::now() - Duration::milliseconds(DEFAULT_TX_VALID_TIMESPAN)).to_rfc3339();
        assert!(!TestEvmTransaction::is_transaction_valid(
            &boundary_created_at,
            &valid_until
        ));

        // Test with created_at just within the default timespan
        let within_boundary_created_at =
            (Utc::now() - Duration::milliseconds(DEFAULT_TX_VALID_TIMESPAN - 1000)).to_rfc3339();
        assert!(TestEvmTransaction::is_transaction_valid(
            &within_boundary_created_at,
            &valid_until
        ));

        // Test with invalid created_at format
        let invalid_created_at = "invalid-date-format";

        // Should return false when parsing fails
        assert!(!TestEvmTransaction::is_transaction_valid(
            invalid_created_at,
            &valid_until
        ));

        // Test with empty created_at string
        assert!(!TestEvmTransaction::is_transaction_valid("", &valid_until));
    }

    #[tokio::test]
    async fn test_prepare_transaction() {
        // Create mocks for all dependencies
        let mut mock_transaction = MockTransactionRepository::new();
        let mock_relayer = MockRepository::<RelayerRepoModel, String>::new();
        let mut mock_provider = MockEvmProviderTrait::new();
        let mut mock_signer = MockSigner::new();
        let mut mock_job_producer = MockJobProducerTrait::new();
        let mut mock_gas_price_service = MockEvmGasPriceServiceTrait::new();
        let mut counter_service = MockTransactionCounterTrait::new();

        // Create test relayer and transaction
        let relayer = create_test_relayer();
        let test_tx = create_test_transaction();

        // Set up expectations for the mocks

        // Gas price service should return gas price params
        mock_gas_price_service
            .expect_get_prices_from_json_rpc()
            .returning(|| {
                Box::pin(async {
                    Ok(crate::services::gas::evm_gas_price::GasPrices {
                        legacy_prices: crate::services::gas::evm_gas_price::SpeedPrices {
                            safe_low: 10000000000, // 10 Gwei
                            average: 20000000000,  // 20 Gwei
                            fast: 30000000000,     // 30 Gwei
                            fastest: 40000000000,  // 40 Gwei
                        },
                        max_priority_fee_per_gas:
                            crate::services::gas::evm_gas_price::SpeedPrices {
                                safe_low: 1000000000, // 1 Gwei
                                average: 2000000000,  // 2 Gwei
                                fast: 3000000000,     // 3 Gwei
                                fastest: 4000000000,  // 4 Gwei
                            },
                        base_fee_per_gas: 5000000000, // 5 Gwei
                    })
                })
            });

        mock_gas_price_service
            .expect_network()
            .return_const(EvmNetwork::from_named(EvmNamedNetwork::Mainnet));

        // Provider should be called for balance check
        mock_provider
            .expect_get_balance()
            .returning(|_| Box::pin(async { Ok(U256::from(1000000000000000000u64)) })); // 1 ETH

        // Transaction counter should increment and return a nonce
        counter_service
            .expect_get_and_increment()
            .returning(|_, _| Ok(42u64)); // Return nonce 42

        // Signer should be called to sign the transaction
        mock_signer.expect_sign_transaction().returning(|_| {
            Box::pin(async {
                Ok(crate::domain::relayer::SignTransactionResponse::Evm(
                    crate::domain::relayer::SignTransactionResponseEvm {
                        hash: "0xtxhash".to_string(),
                        signature: crate::models::EvmTransactionDataSignature {
                            r: "r".to_string(),
                            s: "s".to_string(),
                            v: 1,
                            sig: "0xsignature".to_string(),
                        },
                        raw: vec![1, 2, 3],
                    },
                ))
            })
        });

        // Transaction repository should update the transaction
        mock_transaction
            .expect_partial_update()
            .returning(|tx_id, update| {
                let mut updated_tx = create_test_transaction();
                updated_tx.id = tx_id;
                updated_tx.status = update.status.unwrap_or(TransactionStatus::Pending);
                updated_tx.network_data = update.network_data.unwrap_or(updated_tx.network_data);
                Ok(updated_tx)
            });

        // Job producer should create a submit transaction job
        mock_job_producer
            .expect_produce_submit_transaction_job()
            .returning(|_, _| Box::pin(async { Ok(()) }));
        mock_job_producer
            .expect_produce_send_notification_job()
            .returning(|_, _| Box::pin(async { Ok(()) }));
        // Set up EVM transaction with the mocks
        let evm_transaction = EvmRelayerTransaction {
            relayer: relayer.clone(),
            provider: mock_provider,
            relayer_repository: Arc::new(mock_relayer),
            transaction_repository: Arc::new(mock_transaction),
            transaction_counter_service: Arc::new(counter_service),
            job_producer: Arc::new(mock_job_producer),
            gas_price_service: Arc::new(mock_gas_price_service),
            price_calculator: Arc::new(MockPriceCalculatorTrait::new()),
            signer: mock_signer,
        };

        // Call prepare_transaction and verify it succeeds
        let result = evm_transaction.prepare_transaction(test_tx).await;

        // Verify the transaction was successfully prepared
        assert!(result.is_ok());

        // Verify the transaction has the expected values
        let prepared_tx = result.unwrap();
        assert_eq!(prepared_tx.status, TransactionStatus::Sent);

        // Verify the network data was properly updated
        if let NetworkTransactionData::Evm(evm_data) = &prepared_tx.network_data {
            assert_eq!(evm_data.nonce, Some(42));
            assert!(evm_data.raw.is_some());
            assert_eq!(evm_data.hash, Some("0xtxhash".to_string()));
            assert!(evm_data.signature.is_some());
        } else {
            panic!("Expected EVM transaction data");
        }
    }
}
