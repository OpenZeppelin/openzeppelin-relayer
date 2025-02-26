//! This module defines the `EvmRelayerTransaction` struct and its associated
//! functionality for handling Ethereum Virtual Machine (EVM) transactions.
//! It includes methods for preparing, submitting, handling status, and
//! managing notifications for transactions. The module leverages various
//! services and repositories to perform these operations asynchronously.
use async_trait::async_trait;
use eyre::Result;
use log::{debug, info};
use std::sync::Arc;

use crate::{
    domain::{get_transaction_price_params, transaction::Transaction},
    jobs::{JobProducer, JobProducerTrait, TransactionSend, TransactionStatusCheck},
    models::{
        produce_transaction_update_notification_payload, NetworkTransactionData, RelayerRepoModel,
        TransactionError, TransactionRepoModel, TransactionStatus, U256,
    },
    repositories::{InMemoryTransactionRepository, RelayerRepositoryStorage},
    services::{EvmGasPriceService, EvmProvider, EvmSigner, Signer, TransactionCounterService},
};
/// Parameters for determining the price of a transaction.
#[allow(dead_code)]
pub struct TransactionPriceParams {
    /// The gas price for the transaction.
    pub gas_price: Option<U256>,
    /// The maximum priority fee per gas.
    pub max_priority_fee_per_gas: Option<U256>,
    /// The maximum fee per gas.
    pub max_fee_per_gas: Option<U256>,
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
    gas_price_service: Arc<EvmGasPriceService>,
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
        gas_price_service: Arc<EvmGasPriceService>,
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

    /// Returns a reference to the gas price service.
    pub fn gas_price_service(&self) -> &Arc<EvmGasPriceService> {
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
        info!("Preparing transaction");
        // set the gas price
        let price_params: TransactionPriceParams = get_transaction_price_params(self, &tx).await?;
        debug!("Gas price: {:?}", price_params.gas_price);
        // sign the transaction
        let sig_result = self
            .signer
            .sign_transaction(tx.network_data.clone())
            .await?;

        // increment the nonce
        let nonce = self
            .transaction_counter_service
            .get_and_increment()
            .map_err(|e| TransactionError::UnexpectedError(e.to_string()))?;

        let updated_evm_data = tx
            .network_data
            .get_evm_transaction_data()?
            .with_price_params(price_params)
            .with_nonce(Some(nonce))
            .with_signed_transaction_data(sig_result.into_evm()?);
        let updated_tx = self
            .transaction_repository
            .update_network_data(tx.id.clone(), NetworkTransactionData::Evm(updated_evm_data))
            .await?;

        // after preparing the transaction, we need to submit it to the job queue
        self.job_producer
            .produce_submit_transaction_job(
                TransactionSend::submit(updated_tx.id.clone(), updated_tx.relayer_id.clone()),
                None,
            )
            .await?;
        let updated_tx = self
            .transaction_repository
            .update_status(updated_tx.id.clone(), TransactionStatus::Sent)
            .await?;

        if let Some(notification_id) = &self.relayer.notification_id {
            self.job_producer
                .produce_send_notification_job(
                    produce_transaction_update_notification_payload(notification_id, &updated_tx),
                    None,
                )
                .await?;
        }
        info!("updated_tx: {:?}", updated_tx);
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
        info!("submitting transaction");

        let updated = self
            .transaction_repository
            .update_status(tx.id.clone(), TransactionStatus::Submitted)
            .await?;

        // after submitting the transaction, we need to handle the transaction status
        self.job_producer
            .produce_check_transaction_status_job(
                TransactionStatusCheck::new(tx.id.clone(), tx.relayer_id.clone()),
                None,
            )
            .await?;

        if let Some(notification_id) = &self.relayer.notification_id {
            self.job_producer
                .produce_send_notification_job(
                    produce_transaction_update_notification_payload(notification_id, &updated),
                    None,
                )
                .await?;
        }
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
        let updated: TransactionRepoModel = self
            .transaction_repository
            .update_status(tx.id.clone(), TransactionStatus::Confirmed)
            .await?;

        if let Some(notification_id) = &self.relayer.notification_id {
            self.job_producer
                .produce_send_notification_job(
                    produce_transaction_update_notification_payload(notification_id, &updated),
                    None,
                )
                .await?;
        }
        Ok(tx)
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
        Ok(tx)
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
