//! This module contains the status-related functionality for EVM transactions.
//! It includes methods for checking transaction status, determining when to resubmit
//! or replace transactions with NOOPs, and updating transaction status in the repository.

use chrono::{DateTime, Duration, Utc};
use eyre::Result;
use log::{info, warn};

use super::EvmRelayerTransaction;
use super::{is_noop, make_noop, too_many_attempts, too_many_noop_attempts};
use crate::{
    constants::DEFAULT_TX_VALID_TIMESPAN,
    domain::transaction::evm::price_calculator::PriceCalculatorTrait,
    jobs::{JobProducerTrait, TransactionSend, TransactionStatusCheck},
    models::{
        produce_transaction_update_notification_payload, EvmNetwork, NetworkTransactionData,
        RelayerRepoModel, TransactionError, TransactionRepoModel, TransactionStatus,
        TransactionUpdateRequest,
    },
    repositories::{Repository, TransactionCounterTrait, TransactionRepository},
    services::{EvmProviderTrait, Signer},
    utils::{get_resubmit_timeout_for_speed, get_resubmit_timeout_with_backoff},
};

impl<P, R, T, J, S, C, PC> EvmRelayerTransaction<P, R, T, J, S, C, PC>
where
    P: EvmProviderTrait + Send + Sync,
    R: Repository<RelayerRepoModel, String> + Send + Sync,
    T: TransactionRepository + Send + Sync,
    J: JobProducerTrait + Send + Sync,
    S: Signer + Send + Sync,
    C: TransactionCounterTrait + Send + Sync,
    PC: PriceCalculatorTrait + Send + Sync,
{
    /// Helper function to check if a transaction has enough confirmations.
    pub(super) fn has_enough_confirmations(
        tx_block_number: u64,
        current_block_number: u64,
        chain_id: u64,
    ) -> bool {
        let network = EvmNetwork::from_id(chain_id);
        let required_confirmations = network.required_confirmations();
        current_block_number >= tx_block_number + required_confirmations
    }

    /// Checks if a transaction is still valid based on its valid_until timestamp.
    pub(super) fn is_transaction_valid(created_at: &str, valid_until: &Option<String>) -> bool {
        if let Some(valid_until_str) = valid_until {
            match DateTime::parse_from_rfc3339(valid_until_str) {
                Ok(valid_until_time) => return Utc::now() < valid_until_time,
                Err(e) => {
                    warn!("Failed to parse valid_until timestamp: {}", e);
                    return false;
                }
            }
        }
        match DateTime::parse_from_rfc3339(created_at) {
            Ok(created_time) => {
                let default_valid_until =
                    created_time + Duration::milliseconds(DEFAULT_TX_VALID_TIMESPAN);
                Utc::now() < default_valid_until
            }
            Err(e) => {
                warn!("Failed to parse created_at timestamp: {}", e);
                false
            }
        }
    }

    /// Checks transaction confirmation status.
    pub(super) async fn check_transaction_status(
        &self,
        tx: &TransactionRepoModel,
    ) -> Result<TransactionStatus, TransactionError> {
        if tx.status == TransactionStatus::Expired
            || tx.status == TransactionStatus::Failed
            || tx.status == TransactionStatus::Confirmed
        {
            return Ok(tx.status.clone());
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
            if !receipt.status() {
                return Ok(TransactionStatus::Failed);
            }
            let last_block_number = self.provider().get_block_number().await?;
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
            Ok(TransactionStatus::Confirmed)
        } else {
            info!("Transaction not yet mined: {}", tx_hash);
            Ok(TransactionStatus::Submitted)
        }
    }

    /// Sends a transaction update notification if a notification ID is configured.
    pub(super) async fn send_transaction_update_notification(
        &self,
        tx: &TransactionRepoModel,
    ) -> Result<(), TransactionError> {
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

    /// Updates a transaction's status.
    pub(super) async fn update_transaction_status(
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
            .transaction_repository()
            .partial_update(tx.id.clone(), update_request)
            .await?;

        self.send_transaction_update_notification(&updated_tx)
            .await?;
        Ok(updated_tx)
    }

    /// Gets the age of a transaction since it was sent.
    pub(super) fn get_age_of_sent_at(
        &self,
        tx: &TransactionRepoModel,
    ) -> Result<Duration, TransactionError> {
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

    /// Determines if a transaction should be resubmitted.
    pub(super) async fn should_resubmit(
        &self,
        tx: &TransactionRepoModel,
    ) -> Result<bool, TransactionError> {
        if tx.status != TransactionStatus::Submitted {
            return Err(TransactionError::UnexpectedError(format!(
                "Transaction must be in Submitted status to resubmit, found: {:?}",
                tx.status
            )));
        }

        let age = self.get_age_of_sent_at(tx)?;
        let timeout = match tx.network_data.get_evm_transaction_data() {
            Ok(data) => get_resubmit_timeout_for_speed(&data.speed),
            Err(e) => return Err(e),
        };

        let timeout_with_backoff = get_resubmit_timeout_with_backoff(timeout, tx.hashes.len());
        if age > Duration::milliseconds(timeout_with_backoff) {
            info!("Transaction has been pending for too long, resubmitting");
            return Ok(true);
        }
        Ok(false)
    }

    /// Determines if a transaction should be replaced with a NOOP transaction.
    pub(super) async fn should_noop(
        &self,
        tx: &TransactionRepoModel,
    ) -> Result<bool, TransactionError> {
        if too_many_noop_attempts(tx) {
            info!("Transaction has too many NOOP attempts already");
            return Ok(false);
        }

        let evm_data = tx.network_data.get_evm_transaction_data()?;
        if is_noop(&evm_data) {
            return Ok(false);
        }

        let network = EvmNetwork::from_id(evm_data.chain_id);
        if network.is_rollup() && too_many_attempts(tx) {
            info!("Rollup transaction has too many attempts, will replace with NOOP");
            return Ok(true);
        }

        if !Self::is_transaction_valid(&tx.created_at, &tx.valid_until) {
            info!("Transaction is expired, will replace with NOOP");
            return Ok(true);
        }

        if tx.status == TransactionStatus::Pending {
            let created_at = &tx.created_at;
            let created_time = DateTime::parse_from_rfc3339(created_at)
                .map_err(|_| {
                    TransactionError::UnexpectedError("Error parsing created_at time".to_string())
                })?
                .with_timezone(&Utc);
            let age = Utc::now().signed_duration_since(created_time);
            if age > Duration::minutes(1) {
                info!("Transaction in Pending state for over 1 minute, will replace with NOOP");
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Prepares a NOOP transaction update request.
    pub(super) async fn prepare_noop_update_request(
        &self,
        tx: &TransactionRepoModel,
        is_cancellation: bool,
    ) -> Result<TransactionUpdateRequest, TransactionError> {
        let mut evm_data = tx.network_data.get_evm_transaction_data()?;
        make_noop(&mut evm_data).await?;

        let noop_count = tx.noop_count.unwrap_or(0) + 1;
        let update_request = TransactionUpdateRequest {
            network_data: Some(NetworkTransactionData::Evm(evm_data)),
            noop_count: Some(noop_count),
            is_canceled: if is_cancellation {
                Some(true)
            } else {
                tx.is_canceled
            },
            ..Default::default()
        };
        Ok(update_request)
    }

    /// Inherent status-handling method.
    ///
    /// This method encapsulates the full logic for handling transaction status,
    /// including resubmission, NOOP replacement, and updating status.
    pub async fn handle_status_impl(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        info!("Checking transaction status for tx: {:?}", tx.id);

        let status = self.check_transaction_status(&tx).await?;
        info!("Transaction status: {:?}", status);

        match status {
            TransactionStatus::Submitted => {
                if self.should_resubmit(&tx).await? {
                    info!("Scheduling resubmit job for transaction: {}", tx.id);
                    if self.should_noop(&tx).await? {
                        info!("Preparing transaction NOOP before resubmission: {}", tx.id);
                        let update = self.prepare_noop_update_request(&tx, false).await?;
                        let updated_tx = self
                            .transaction_repository()
                            .partial_update(tx.id.clone(), update)
                            .await?;
                        self.job_producer()
                            .produce_submit_transaction_job(
                                TransactionSend::resubmit(
                                    updated_tx.id.clone(),
                                    updated_tx.relayer_id.clone(),
                                ),
                                None,
                            )
                            .await?;
                        self.send_transaction_update_notification(&updated_tx)
                            .await?;
                        return Ok(updated_tx);
                    }
                    self.job_producer()
                        .produce_submit_transaction_job(
                            TransactionSend::resubmit(tx.id.clone(), tx.relayer_id.clone()),
                            None,
                        )
                        .await?;
                    return Ok(tx);
                }
                self.job_producer()
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
            TransactionStatus::Pending => {
                if self.should_noop(&tx).await? {
                    info!("Preparing NOOP for pending transaction: {}", tx.id);
                    let update = self.prepare_noop_update_request(&tx, false).await?;
                    let updated_tx = self
                        .transaction_repository()
                        .partial_update(tx.id.clone(), update)
                        .await?;
                    self.job_producer()
                        .produce_submit_transaction_job(
                            TransactionSend::submit(
                                updated_tx.id.clone(),
                                updated_tx.relayer_id.clone(),
                            ),
                            None,
                        )
                        .await?;
                    self.send_transaction_update_notification(&updated_tx)
                        .await?;
                    return Ok(updated_tx);
                }
                Ok(tx)
            }
            TransactionStatus::Mined => {
                self.job_producer()
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
