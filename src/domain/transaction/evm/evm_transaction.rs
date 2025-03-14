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

use crate::{
    constants::DEFAULT_TX_VALID_TIMESPAN,
    domain::{make_noop, transaction::Transaction, PriceCalculator},
    jobs::{JobProducer, JobProducerTrait, TransactionSend, TransactionStatusCheck},
    models::{
        evm::Speed, produce_transaction_update_notification_payload, EvmNetwork,
        EvmTransactionData, NetworkTransactionData, RelayerRepoModel, TransactionError,
        TransactionRepoModel, TransactionStatus, TransactionUpdateRequest, U256,
    },
    repositories::{InMemoryTransactionRepository, RelayerRepositoryStorage, Repository},
    services::{
        EvmGasPriceService, EvmProvider, EvmProviderTrait, EvmSigner, Signer,
        TransactionCounterService,
    },
};

/// Parameters for determining the price of a transaction.
#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
pub struct TransactionPriceParams {
    /// The gas price for the transaction.
    pub gas_price: Option<u128>,
    /// The maximum priority fee per gas.
    pub max_priority_fee_per_gas: Option<u128>,
    /// The maximum fee per gas.
    pub max_fee_per_gas: Option<u128>,
    /// The balance available for the transaction.
    pub balance: Option<U256>,
}

#[allow(dead_code)]
pub struct EvmRelayerTransaction {
    relayer: RelayerRepoModel,
    provider: EvmProvider,
    relayer_repository: Arc<RelayerRepositoryStorage>,
    transaction_repository: Arc<InMemoryTransactionRepository>,
    transaction_counter_service: TransactionCounterService,
    job_producer: Arc<JobProducer>,
    gas_price_service: Arc<EvmGasPriceService<EvmProvider>>,
    signer: EvmSigner,
}

#[allow(dead_code, clippy::too_many_arguments)]
impl EvmRelayerTransaction {
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
        provider: EvmProvider,
        relayer_repository: Arc<RelayerRepositoryStorage>,
        transaction_repository: Arc<InMemoryTransactionRepository>,
        transaction_counter_service: TransactionCounterService,
        job_producer: Arc<JobProducer>,
        gas_price_service: Arc<EvmGasPriceService<EvmProvider>>,
        signer: EvmSigner,
    ) -> Result<Self, TransactionError> {
        Ok(Self {
            relayer,
            provider,
            relayer_repository,
            transaction_repository,
            transaction_counter_service,
            job_producer,
            gas_price_service,
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
        let price_params: TransactionPriceParams = PriceCalculator::get_transaction_price_params(
            evm_data,
            relayer,
            &self.gas_price_service,
            &self.provider,
        )
        .await?;

        // Create a "noop" transaction with higher gas price
        let mut cancel_tx_data = make_noop(
            &self.provider,
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
    pub fn gas_price_service(&self) -> &Arc<EvmGasPriceService<EvmProvider>> {
        &self.gas_price_service
    }

    /// Returns a reference to the provider.
    pub fn provider(&self) -> &EvmProvider {
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

    /// Updates the status of the original transaction when its noop transaction is confirmed
    ///
    /// # Arguments
    ///
    /// * `noop_tx` - The noop transaction that was confirmed
    ///
    /// # Returns
    ///
    /// A result containing the updated original transaction or a `TransactionError`
    async fn handle_noop_confirmation(
        &self,
        noop_tx: &TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        if let Some(original_tx_id) = &noop_tx.original_tx_id {
            let update = TransactionUpdateRequest {
                status: Some(TransactionStatus::Failed),
                confirmed_at: Some(Utc::now().to_rfc3339()),
                ..Default::default()
            };

            let updated_tx = self
                .transaction_repository
                .partial_update(original_tx_id.clone(), update)
                .await?;

            self.send_transaction_update_notification(&updated_tx)
                .await?;
            Ok(updated_tx)
        } else {
            Err(TransactionError::UnexpectedError(
                "Noop transaction has no original transaction ID".to_string(),
            ))
        }
    }
}

#[async_trait]
impl Transaction for EvmRelayerTransaction {
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
        let price_params: TransactionPriceParams = PriceCalculator::get_transaction_price_params(
            &evm_data,
            relayer,
            &self.gas_price_service,
            &self.provider,
        )
        .await?;
        debug!("Gas price: {:?}", price_params.gas_price);
        // increment the nonce
        let nonce = self
            .transaction_counter_service
            .get_and_increment()
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

        Ok(tx)
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

        // Handle noop transactions separately
        if tx.original_tx_id.is_some() {
            match status {
                TransactionStatus::Submitted => {
                    self.job_producer
                        .produce_check_transaction_status_job(
                            TransactionStatusCheck::new(tx.id.clone(), tx.relayer_id.clone()),
                            Some(Utc::now().timestamp() + 5),
                        )
                        .await?;
                    Ok(tx)
                }
                TransactionStatus::Mined | TransactionStatus::Confirmed => {
                    self.handle_noop_confirmation(&tx).await
                }
                TransactionStatus::Failed | TransactionStatus::Expired => {
                    self.update_transaction_status(tx, status, None).await
                }
                _ => Err(TransactionError::UnexpectedError(format!(
                    "Unexpected transaction status: {:?}",
                    status
                ))),
            }
        } else {
            // Original logic for regular transactions
            match status {
                TransactionStatus::Submitted | TransactionStatus::Mined => {
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
        if tx.status != TransactionStatus::Pending
            && tx.status != TransactionStatus::Sent
            && tx.status != TransactionStatus::Submitted
        {
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
        let relayer = self.relayer();

        // Prepare the cancellation transaction
        let signed_cancel_tx_data = self
            .prepare_cancel_transaction(&mut evm_data, relayer)
            .await?;

        // Create a new transaction for the noop operation
        let noop_tx = TransactionRepoModel {
            id: format!("{}-noop-{}", tx.id, tx.noop_count.unwrap_or(0) + 1),
            relayer_id: tx.relayer_id.clone(),
            status: TransactionStatus::Sent,
            network_data: NetworkTransactionData::Evm(signed_cancel_tx_data),
            created_at: Utc::now().to_rfc3339(),
            sent_at: None,
            confirmed_at: None,
            valid_until: tx.valid_until.clone(),
            network_type: tx.network_type,
            noop_count: Some(0),
            original_tx_id: Some(tx.id.clone()),
        };

        // Store the noop transaction
        let stored_noop_tx = self.transaction_repository.create(noop_tx).await?;

        // Update the original transaction to reflect the cancellation attempt
        let update = TransactionUpdateRequest {
            noop_count: Some(tx.noop_count.unwrap_or(0) + 1),
            ..Default::default()
        };

        let updated_original_tx = self
            .transaction_repository
            .partial_update(tx.id.clone(), update)
            .await?;

        // Submit the noop transaction to the network
        self.job_producer
            .produce_submit_transaction_job(
                TransactionSend::submit(
                    stored_noop_tx.id.clone(),
                    stored_noop_tx.relayer_id.clone(),
                ),
                None,
            )
            .await?;

        // Send notification for both transactions
        self.send_transaction_update_notification(&stored_noop_tx)
            .await?;
        self.send_transaction_update_notification(&updated_original_tx)
            .await?;

        info!(
            "Cancellation transaction created and submitted: {:?}",
            stored_noop_tx.id
        );
        Ok(updated_original_tx)
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};

    #[test]
    fn test_has_enough_confirmations() {
        // Test Ethereum Mainnet (requires 12 confirmations)
        let chain_id = 1; // Ethereum Mainnet

        // Not enough confirmations
        let tx_block_number = 100;
        let current_block_number = 110; // Only 10 confirmations
        assert!(!EvmRelayerTransaction::has_enough_confirmations(
            tx_block_number,
            current_block_number,
            chain_id
        ));

        // Exactly enough confirmations
        let current_block_number = 112; // Exactly 12 confirmations
        assert!(EvmRelayerTransaction::has_enough_confirmations(
            tx_block_number,
            current_block_number,
            chain_id
        ));

        // More than enough confirmations
        let current_block_number = 120; // 20 confirmations
        assert!(EvmRelayerTransaction::has_enough_confirmations(
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

        assert!(EvmRelayerTransaction::is_transaction_valid(
            &created_at,
            &valid_until
        ));

        // Test with valid_until in the past
        let valid_until = Some((Utc::now() - Duration::hours(1)).to_rfc3339());

        assert!(!EvmRelayerTransaction::is_transaction_valid(
            &created_at,
            &valid_until
        ));

        // Test with valid_until exactly at current time (should be invalid)
        let valid_until = Some(Utc::now().to_rfc3339());
        assert!(!EvmRelayerTransaction::is_transaction_valid(
            &created_at,
            &valid_until
        ));

        // Test with valid_until very far in the future
        let valid_until = Some((Utc::now() + Duration::days(365)).to_rfc3339());
        assert!(EvmRelayerTransaction::is_transaction_valid(
            &created_at,
            &valid_until
        ));

        // Test with invalid valid_until format
        let valid_until = Some("invalid-date-format".to_string());

        // Should return false when parsing fails
        assert!(!EvmRelayerTransaction::is_transaction_valid(
            &created_at,
            &valid_until
        ));

        // Test with empty valid_until string
        let valid_until = Some("".to_string());
        assert!(!EvmRelayerTransaction::is_transaction_valid(
            &created_at,
            &valid_until
        ));
    }

    #[test]
    fn test_is_transaction_valid_without_valid_until() {
        // Test with created_at within the default timespan
        let created_at = Utc::now().to_rfc3339();
        let valid_until = None;

        assert!(EvmRelayerTransaction::is_transaction_valid(
            &created_at,
            &valid_until
        ));

        // Test with created_at older than the default timespan (8 hours)
        let old_created_at =
            (Utc::now() - Duration::milliseconds(DEFAULT_TX_VALID_TIMESPAN + 1000)).to_rfc3339();

        assert!(!EvmRelayerTransaction::is_transaction_valid(
            &old_created_at,
            &valid_until
        ));

        // Test with created_at exactly at the boundary of default timespan
        let boundary_created_at =
            (Utc::now() - Duration::milliseconds(DEFAULT_TX_VALID_TIMESPAN)).to_rfc3339();
        assert!(!EvmRelayerTransaction::is_transaction_valid(
            &boundary_created_at,
            &valid_until
        ));

        // Test with created_at just within the default timespan
        let within_boundary_created_at =
            (Utc::now() - Duration::milliseconds(DEFAULT_TX_VALID_TIMESPAN - 1000)).to_rfc3339();
        assert!(EvmRelayerTransaction::is_transaction_valid(
            &within_boundary_created_at,
            &valid_until
        ));

        // Test with invalid created_at format
        let invalid_created_at = "invalid-date-format";

        // Should return false when parsing fails
        assert!(!EvmRelayerTransaction::is_transaction_valid(
            invalid_created_at,
            &valid_until
        ));

        // Test with empty created_at string
        assert!(!EvmRelayerTransaction::is_transaction_valid(
            "",
            &valid_until
        ));
    }
}
