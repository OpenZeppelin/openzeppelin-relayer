use async_trait::async_trait;
use eyre::Result;
use log::info;
use std::sync::Arc;

use crate::{
    domain::transaction::Transaction,
    jobs::JobProducer,
    models::{RelayerRepoModel, TransactionError, TransactionRepoModel},
    repositories::{InMemoryTransactionRepository, RelayerRepositoryStorage},
    services::SolanaProvider,
};

#[allow(dead_code)]
pub struct SolanaRelayerTransaction {
    relayer: RelayerRepoModel,
    relayer_repository: Arc<RelayerRepositoryStorage>,
    provider: Arc<SolanaProvider>,
    transaction_repository: Arc<InMemoryTransactionRepository>,
    job_producer: Arc<JobProducer>,
}

#[allow(dead_code)]
impl SolanaRelayerTransaction {
    pub fn new(
        relayer: RelayerRepoModel,
        relayer_repository: Arc<RelayerRepositoryStorage>,
        provider: Arc<SolanaProvider>,
        transaction_repository: Arc<InMemoryTransactionRepository>,
        job_producer: Arc<JobProducer>,
    ) -> Result<Self, TransactionError> {
        Ok(Self {
            relayer_repository,
            provider,
            transaction_repository,
            relayer,
            job_producer,
        })
    }
}

#[async_trait]
impl Transaction for SolanaRelayerTransaction {
    async fn prepare_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        info!("preparing transaction");
        Ok(tx)
    }

    async fn submit_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        info!("submitting transaction");
        Ok(tx)
    }

    async fn handle_transaction_status(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
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
