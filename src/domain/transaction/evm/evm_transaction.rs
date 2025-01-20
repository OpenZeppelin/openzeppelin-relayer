use async_trait::async_trait;
use eyre::Result;
use log::info;
use std::sync::Arc;

use crate::{
    domain::transaction::Transaction,
    jobs::{JobProducer, NotificationSend, TransactionStatusCheck, TransactionSubmit},
    models::{RelayerRepoModel, TransactionError, TransactionRepoModel},
    repositories::{InMemoryRelayerRepository, InMemoryTransactionRepository},
    services::EvmProvider,
};

#[allow(dead_code)]
pub struct EvmRelayerTransaction {
    relayer: RelayerRepoModel,
    provider: EvmProvider,
    relayer_repository: Arc<InMemoryRelayerRepository>,
    transaction_repository: Arc<InMemoryTransactionRepository>,
    job_producer: Arc<JobProducer>,
}

#[allow(dead_code)]
impl EvmRelayerTransaction {
    pub fn new(
        relayer: RelayerRepoModel,
        provider: EvmProvider,
        relayer_repository: Arc<InMemoryRelayerRepository>,
        transaction_repository: Arc<InMemoryTransactionRepository>,
        job_producer: Arc<JobProducer>,
    ) -> Result<Self, TransactionError> {
        Ok(Self {
            relayer,
            provider,
            relayer_repository,
            transaction_repository,
            job_producer,
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

        // after preparing the transaction, we need to submit it to the job queue
        self.job_producer
            .produce_submit_transaction_job(
                TransactionSubmit::new(tx.id.clone(), tx.relayer_id.clone()),
                None,
            )
            .await?;
        Ok(tx)
    }

    async fn submit_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        info!("submitting transaction");

        // after submitting the transaction, we need to handle the transaction status
        self.job_producer
            .produce_check_transaction_status_job(
                TransactionStatusCheck::new(tx.id.clone(), tx.relayer_id.clone()),
                None,
            )
            .await?;

        Ok(tx)
    }

    async fn handle_transaction_status(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        self.job_producer
            .produce_send_notification_job(
                NotificationSend::new(tx.id.clone(), tx.relayer_id.clone()),
                None,
            )
            .await?;
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
