//! This module defines the `EvmRelayerTransaction` struct and its associated
//! functionality for handling Ethereum Virtual Machine (EVM) transactions.
//! It includes methods for preparing, submitting, handling status, and
//! managing notifications for transactions. The module leverages various
//! services and repositories to perform these operations asynchronously.

use async_trait::async_trait;
use chrono::Utc;
use eyre::Result;
use log::{debug, info, warn};
use std::sync::Arc;

use super::PriceParams;
use crate::{
    domain::{
        is_pending_transaction,
        transaction::{
            evm::price_calculator::{PriceCalculator, PriceCalculatorTrait},
            Transaction,
        },
        EvmTransactionValidator,
    },
    jobs::{JobProducer, JobProducerTrait, TransactionSend, TransactionStatusCheck},
    models::{
        produce_transaction_update_notification_payload, NetworkTransactionData, RelayerRepoModel,
        TransactionError, TransactionRepoModel, TransactionStatus, TransactionUpdateRequest,
    },
    repositories::{
        InMemoryNetworkRepository, InMemoryRelayerRepository, InMemoryTransactionCounter,
        NetworkRepository, RelayerRepositoryStorage, Repository, TransactionCounterTrait,
        TransactionRepository,
    },
    services::{EvmGasPriceService, EvmProvider, EvmProviderTrait, EvmSigner, Signer},
};

#[allow(dead_code)]
pub struct EvmRelayerTransaction<P, R, N, T, J, S, C, PC>
where
    P: EvmProviderTrait,
    R: Repository<RelayerRepoModel, String>,
    N: NetworkRepository,
    T: TransactionRepository,
    J: JobProducerTrait,
    S: Signer,
    C: TransactionCounterTrait,
    PC: PriceCalculatorTrait,
{
    provider: P,
    relayer_repository: Arc<R>,
    network_repository: Arc<N>,
    transaction_repository: Arc<T>,
    job_producer: Arc<J>,
    signer: S,
    relayer: RelayerRepoModel,
    transaction_counter_service: Arc<C>,
    price_calculator: PC,
}

#[allow(dead_code, clippy::too_many_arguments)]
impl<P, R, N, T, J, S, C, PC> EvmRelayerTransaction<P, R, N, T, J, S, C, PC>
where
    P: EvmProviderTrait,
    R: Repository<RelayerRepoModel, String>,
    N: NetworkRepository,
    T: TransactionRepository,
    J: JobProducerTrait,
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
    /// * `price_calculator` - Price calculator for gas price management.
    /// * `signer` - The EVM signer.
    ///
    /// # Returns
    ///
    /// A result containing the new `EvmRelayerTransaction` or a `TransactionError`.
    pub fn new(
        relayer: RelayerRepoModel,
        provider: P,
        relayer_repository: Arc<R>,
        network_repository: Arc<N>,
        transaction_repository: Arc<T>,
        transaction_counter_service: Arc<C>,
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
    pub fn network_repository(&self) -> &N {
        &self.network_repository
    }

    /// Returns a reference to the job producer.
    pub fn job_producer(&self) -> &J {
        &self.job_producer
    }

    pub fn transaction_repository(&self) -> &T {
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
}

#[async_trait]
impl<P, R, N, T, J, S, C, PC> Transaction for EvmRelayerTransaction<P, R, N, T, J, S, C, PC>
where
    P: EvmProviderTrait + Send + Sync,
    R: Repository<RelayerRepoModel, String> + Send + Sync,
    N: NetworkRepository + Send + Sync,
    T: TransactionRepository + Send + Sync,
    J: JobProducerTrait + Send + Sync,
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
        let balance_validation = EvmTransactionValidator::validate_sufficient_relayer_balance(
            price_params.total_cost,
            &self.relayer().address,
            &self.relayer().policies.get_evm_policy(),
            &self.provider,
        )
        .await;

        if let Err(validation_error) = balance_validation {
            info!(
                "Insufficient balance for transaction {}: {}",
                tx.id, validation_error
            );

            let update = TransactionUpdateRequest {
                status: Some(TransactionStatus::Failed),
                status_reason: Some(validation_error.to_string()),
                ..Default::default()
            };

            let updated_tx = self
                .transaction_repository
                .partial_update(tx.id.clone(), update)
                .await?;

            let _ = self.send_transaction_update_notification(&updated_tx).await;

            return Err(TransactionError::InsufficientBalance(
                validation_error.to_string(),
            ));
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
            .calculate_bumped_gas_price(&tx, self.relayer())
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
        let balance_validation = EvmTransactionValidator::validate_sufficient_relayer_balance(
            bumped_price_params.total_cost,
            &self.relayer().address,
            &self.relayer().policies.get_evm_policy(),
            &self.provider,
        )
        .await;

        if let Err(validation_error) = balance_validation {
            warn!(
                "Insufficient balance for resubmitting transaction {}: {}",
                tx.id, validation_error
            );

            return Err(TransactionError::InsufficientBalance(
                validation_error.to_string(),
            ));
        }

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
    RelayerRepositoryStorage<InMemoryRelayerRepository>,
    InMemoryNetworkRepository,
    crate::repositories::transaction::InMemoryTransactionRepository,
    JobProducer,
    EvmSigner,
    InMemoryTransactionCounter,
    PriceCalculator<EvmGasPriceService<EvmProvider>>,
>;
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domain::price_calculator::PriceParams,
        jobs::MockJobProducerTrait,
        models::{evm::Speed, EvmTransactionData, NetworkType, RelayerNetworkPolicy, U256},
        repositories::{
            MockNetworkRepository, MockRepository, MockTransactionCounterTrait,
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
                tx: &TransactionRepoModel,
                relayer: &RelayerRepoModel,
            ) -> Result<PriceParams, TransactionError>;
        }
    }

    // Helper to create test transactions
    #[allow(dead_code)]
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
            hashes: Vec::new(),
            noop_count: None,
            is_canceled: Some(false),
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
            custom_rpc_urls: None,
        }
    }

    #[tokio::test]
    async fn test_prepare_transaction_with_sufficient_balance() {
        let mut mock_transaction = MockTransactionRepository::new();
        let mock_relayer = MockRepository::<RelayerRepoModel, String>::new();
        let mut mock_provider = MockEvmProviderTrait::new();
        let mut mock_signer = MockSigner::new();
        let mut mock_job_producer = MockJobProducerTrait::new();
        let mut mock_price_calculator = MockPriceCalculator::new();
        let mut counter_service = MockTransactionCounterTrait::new();

        let relayer = create_test_relayer();
        let test_tx = create_test_transaction();

        counter_service
            .expect_get_and_increment()
            .returning(|_, _| Ok(42));

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
        let mock_relayer = MockRepository::<RelayerRepoModel, String>::new();
        let mut mock_provider = MockEvmProviderTrait::new();
        let mut mock_signer = MockSigner::new();
        let mut mock_job_producer = MockJobProducerTrait::new();
        let mut mock_price_calculator = MockPriceCalculator::new();
        let mut counter_service = MockTransactionCounterTrait::new();

        let relayer = create_test_relayer();
        let test_tx = create_test_transaction();

        counter_service
            .expect_get_and_increment()
            .returning(|_, _| Ok(42));

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
            let mock_relayer = MockRepository::<RelayerRepoModel, String>::new();
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
            let mock_relayer = MockRepository::<RelayerRepoModel, String>::new();
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
            let mock_relayer = MockRepository::<RelayerRepoModel, String>::new();
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
}
