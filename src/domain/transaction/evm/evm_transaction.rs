//! This module defines the `EvmRelayerTransaction` struct and its associated
//! functionality for handling Ethereum Virtual Machine (EVM) transactions.
//! It includes methods for preparing, submitting, handling status, and
//! managing notifications for transactions. The module leverages various
//! services and repositories to perform these operations asynchronously.

use async_trait::async_trait;
use chrono::Utc;
use eyre::Result;
use log::{debug, error, info, warn};
use std::sync::Arc;

use crate::{
    constants::{DEFAULT_EVM_GAS_LIMIT_ESTIMATION, GAS_LIMIT_BUFFER_MULTIPLIER},
    domain::{
        transaction::{
            evm::{is_pending_transaction, PriceCalculator, PriceCalculatorTrait},
            Transaction,
        },
        EvmTransactionValidator,
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
    services::{EvmGasPriceService, EvmProvider, EvmProviderTrait, EvmSigner, Signer},
    utils::get_evm_default_gas_limit_for_tx,
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

    /// Helper method to schedule a transaction status check job.
    pub(super) async fn schedule_status_check(
        &self,
        tx: &TransactionRepoModel,
        delay_seconds: Option<i64>,
    ) -> Result<(), TransactionError> {
        let delay = delay_seconds.map(|seconds| Utc::now().timestamp() + seconds);
        self.job_producer()
            .produce_check_transaction_status_job(
                TransactionStatusCheck::new(tx.id.clone(), tx.relayer_id.clone()),
                delay,
            )
            .await
            .map_err(|e| {
                TransactionError::UnexpectedError(format!("Failed to schedule status check: {}", e))
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
                TransactionError::UnexpectedError(format!("Failed to produce submit job: {}", e))
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
                TransactionError::UnexpectedError(format!("Failed to produce resubmit job: {}", e))
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

        self.send_transaction_update_notification(&updated_tx)
            .await?;
        Ok(updated_tx)
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
                .await
                .map_err(|e| {
                    TransactionError::UnexpectedError(format!("Failed to send notification: {}", e))
                })?;
        }
        Ok(())
    }

    /// Validates that the relayer has sufficient balance for the transaction.
    ///
    /// # Arguments
    ///
    /// * `total_cost` - The total cost of the transaction (gas + value)
    ///
    /// # Returns
    ///
    /// A `Result` indicating success or a `TransactionError` if insufficient balance.
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
        .map_err(|validation_error| {
            TransactionError::InsufficientBalance(validation_error.to_string())
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
            warn!(
                "Gas limit estimation is disabled for relayer: {:?}",
                self.relayer().id
            );
            return Err(TransactionError::UnexpectedError(
                "Gas limit estimation is disabled".to_string(),
            ));
        }

        let estimated_gas = self.provider.estimate_gas(evm_data).await.map_err(|e| {
            warn!("Failed to estimate gas: {:?} for tx: {:?}", e, evm_data);
            TransactionError::UnexpectedError(format!("Failed to estimate gas: {}", e))
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
        info!("Preparing transaction: {:?}", tx.id);

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
                    error!(
                        "Failed to estimate gas limit for tx: {} : {:?}",
                        tx.id, estimation_error
                    );

                    let default_gas_limit = get_evm_default_gas_limit_for_tx(&evm_data);
                    info!(
                        "Fallback to default gas limit: {} for tx: {}",
                        default_gas_limit, tx.id
                    );
                    evm_data.gas_limit = Some(default_gas_limit);
                }
            }
        }

        // set the gas price
        let price_params: PriceParams = self
            .price_calculator
            .get_transaction_price_params(&evm_data, relayer)
            .await?;

        debug!("Gas price: {:?}", price_params.gas_price);
        // increment the nonce
        let nonce = self
            .transaction_counter_service
            .get_and_increment(&self.relayer.id, &self.relayer.address)
            .await
            .map_err(|e| TransactionError::UnexpectedError(e.to_string()))?;

        let updated_evm_data = evm_data
            .with_price_params(price_params.clone())
            .with_nonce(nonce);

        // sign the transaction
        let sig_result = self
            .signer
            .sign_transaction(NetworkTransactionData::Evm(updated_evm_data.clone()))
            .await?;

        let updated_evm_data =
            updated_evm_data.with_signed_transaction_data(sig_result.into_evm()?);

        // Validate the relayer has sufficient balance
        match self
            .ensure_sufficient_balance(price_params.total_cost)
            .await
        {
            Ok(()) => {}
            Err(balance_error) => {
                info!(
                    "Insufficient balance for transaction {}: {}",
                    tx.id, balance_error
                );

                let update = TransactionUpdateRequest {
                    status: Some(TransactionStatus::Failed),
                    status_reason: Some(balance_error.to_string()),
                    ..Default::default()
                };

                let updated_tx = self
                    .transaction_repository
                    .partial_update(tx.id.clone(), update)
                    .await?;

                let _ = self.send_transaction_update_notification(&updated_tx).await;
                return Err(balance_error);
            }
        }

        // Balance validation passed, continue with normal flow
        // Track the transaction hash
        let mut hashes = tx.hashes.clone();
        if let Some(hash) = updated_evm_data.hash.clone() {
            hashes.push(hash);
        }

        let update = TransactionUpdateRequest {
            status: Some(TransactionStatus::Sent),
            network_data: Some(NetworkTransactionData::Evm(updated_evm_data)),
            priced_at: Some(Utc::now().to_rfc3339()),
            hashes: Some(hashes),
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

        // Schedule status check
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
        info!("Resubmitting transaction: {:?}", tx.id);

        // Calculate bumped gas price
        let bumped_price_params = self
            .price_calculator
            .calculate_bumped_gas_price(
                &tx.network_data.get_evm_transaction_data()?,
                self.relayer(),
            )
            .await?;

        if !bumped_price_params.is_min_bumped.is_some_and(|b| b) {
            warn!(
                "Bumped gas price does not meet minimum requirement, skipping resubmission: {:?}",
                bumped_price_params
            );
            return Ok(tx);
        }

        // Get transaction data
        let evm_data = tx.network_data.get_evm_transaction_data()?;

        // Create new transaction data with bumped gas price
        let updated_evm_data = evm_data.with_price_params(bumped_price_params.clone());

        // Sign the transaction
        let sig_result = self
            .signer
            .sign_transaction(NetworkTransactionData::Evm(updated_evm_data.clone()))
            .await?;

        let final_evm_data = updated_evm_data.with_signed_transaction_data(sig_result.into_evm()?);

        // Validate the relayer has sufficient balance
        self.ensure_sufficient_balance(bumped_price_params.total_cost)
            .await?;

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
        if !is_pending_transaction(&tx.status) {
            return Err(TransactionError::ValidationError(format!(
                "Cannot cancel transaction with status: {:?}",
                tx.status
            )));
        }

        // If the transaction is in Pending state, we can just update its status
        if tx.status == TransactionStatus::Pending {
            info!("Transaction is in Pending state, updating status to Canceled");
            return self
                .update_transaction_status(tx, TransactionStatus::Canceled)
                .await;
        }

        let update = self.prepare_noop_update_request(&tx, true).await?;
        let updated_tx = self
            .transaction_repository()
            .partial_update(tx.id.clone(), update)
            .await?;

        // Submit the updated transaction to the network using the resubmit job
        self.send_transaction_resubmit_job(&updated_tx).await?;

        // Send notification for the updated transaction
        self.send_transaction_update_notification(&updated_tx)
            .await?;

        info!(
            "Original transaction updated with cancellation data: {:?}",
            updated_tx.id
        );
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
        info!("Replacing transaction: {:?}", old_tx.id);

        // Check if the transaction can be replaced
        if !is_pending_transaction(&old_tx.status) {
            return Err(TransactionError::ValidationError(format!(
                "Cannot replace transaction with status: {:?}",
                old_tx.status
            )));
        }

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
            TransactionError::NetworkConfiguration(format!(
                "Failed to convert network model: {}",
                e
            ))
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

        info!("Replacement price params: {:?}", price_params);

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
        self.send_transaction_update_notification(&updated_tx)
            .await?;

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
        services::{MockEvmProviderTrait, MockSigner},
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

        let test_tx_clone = test_tx.clone();
        mock_transaction
            .expect_partial_update()
            .withf(move |id, update| {
                id == "test-tx-id" && update.status == Some(TransactionStatus::Failed)
            })
            .returning(move |_, update| {
                let mut updated_tx = test_tx_clone.clone();
                updated_tx.status = update.status.unwrap_or(updated_tx.status);
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
        assert!(
            matches!(result, Err(TransactionError::InsufficientBalance(_))),
            "Expected InsufficientBalance error, got: {:?}",
            result
        );
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
                        extra_fee: Some(0),
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
                assert!(msg.contains("Cannot cancel transaction with status"));
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
                        extra_fee: Some(0),
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
                assert!(msg.contains("Cannot replace transaction with status"));
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
                Err(crate::services::ProviderError::Other(
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
        let expected_gas_limit = EXPECTED_GAS_WITH_BUFFER;

        let test_tx_clone = test_tx.clone();
        mock_transaction
            .expect_partial_update()
            .times(1)
            .returning(move |_, _update| {
                let mut updated_tx = test_tx_clone.clone();
                updated_tx.network_data = NetworkTransactionData::Evm(EvmTransactionData {
                    gas_limit: Some(expected_gas_limit),
                    ..test_tx_clone
                        .network_data
                        .get_evm_transaction_data()
                        .unwrap()
                });
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
}
