use async_trait::async_trait;
use eyre::Result;
use log::info;
use std::sync::Arc;

use crate::{
    domain::transaction::Transaction,
    jobs::{JobProducer, TransactionSend, TransactionStatusCheck},
    models::{
        produce_transaction_update_notification_payload, RelayerRepoModel, TransactionError,
        TransactionRepoModel, TransactionStatus,
    },
    repositories::{InMemoryTransactionRepository, RelayerRepositoryStorage, Repository},
    services::{EvmProvider, EvmSigner, Signer, TransactionCounterService},
};

#[allow(dead_code)]
pub struct EvmRelayerTransaction {
    relayer: RelayerRepoModel,
    provider: EvmProvider,
    relayer_repository: Arc<RelayerRepositoryStorage>,
    transaction_repository: Arc<InMemoryTransactionRepository>,
    transaction_counter_service: TransactionCounterService,
    job_producer: Arc<JobProducer>,
    signer: EvmSigner,
}

#[allow(dead_code)]
impl EvmRelayerTransaction {
    pub fn new(
        relayer: RelayerRepoModel,
        provider: EvmProvider,
        relayer_repository: Arc<RelayerRepositoryStorage>,
        transaction_repository: Arc<InMemoryTransactionRepository>,
        transaction_counter_service: TransactionCounterService,
        job_producer: Arc<JobProducer>,
        signer: EvmSigner,
    ) -> Result<Self, TransactionError> {
        Ok(Self {
            relayer,
            provider,
            relayer_repository,
            transaction_repository,
            transaction_counter_service,
            job_producer,
            signer,
        })
    }
}

#[async_trait]
impl Transaction for EvmRelayerTransaction {
    async fn prepare_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        info!("Preparing transaction");
        // validate the transaction

        // gas estimation
        let gas_estimation = self
            .provider
            .estimate_gas(&tx.network_data.get_evm_transaction_data()?)
            .await?;
        info!("Gas estimation: {:?}", gas_estimation);

        // After preparing the transaction, submit it to the job queue
        // sign the transaction
        let result = self
            .signer
            .sign_transaction(tx.network_data.clone())
            .await?;

        let updated_tx = tx.with_signed_transaction_data(result)?;
        let updated_tx = self
            .transaction_repository
            .update(updated_tx.id.clone(), updated_tx)
            .await?;

        // after preparing the transaction, we need to submit it to the job queue
        self.job_producer
            .produce_submit_transaction_job(
                TransactionSend::submit(updated_tx.id.clone(), updated_tx.relayer_id.clone()),
                None,
            )
            .await?;
        let updated = self
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

        Ok(updated)
    }

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

    async fn cancel_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        Ok(tx)
    }

    async fn replace_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        Ok(tx)
    }

    async fn sign_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        Ok(tx)
    }

    async fn validate_transaction(
        &self,
        _tx: TransactionRepoModel,
    ) -> Result<bool, TransactionError> {
        Ok(true)
    }
}
