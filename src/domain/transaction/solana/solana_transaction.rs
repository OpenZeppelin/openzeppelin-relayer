use async_trait::async_trait;
use eyre::Result;
use log::info;
use std::sync::Arc;

use crate::{
    domain::transaction::Transaction,
    jobs::JobProducer,
    models::{NetworkTransactionRequest, RelayerRepoModel, TransactionError, TransactionRepoModel},
    repositories::{RelayerRepositoryStorage, TransactionRepositoryStorage},
    services::SolanaProvider,
};

#[allow(dead_code)]
pub struct SolanaRelayerTransaction {
    relayer_repository: Arc<RelayerRepositoryStorage>,
    provider: Arc<SolanaProvider>,
    transaction_repository: Arc<TransactionRepositoryStorage>,
    relayer: RelayerRepoModel,
    job_producer: Arc<JobProducer>,
}

#[allow(dead_code)]
impl SolanaRelayerTransaction {
    pub fn new(
        relayer: RelayerRepoModel,
        relayer_repository: Arc<RelayerRepositoryStorage>,
        provider: Arc<SolanaProvider>,
        transaction_repository: Arc<TransactionRepositoryStorage>,
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

    async fn resubmit_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        info!("resubmitting transaction");
        // For now, just call submit_transaction as Solana implementation is a stub
        self.submit_transaction(tx).await
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
        _old_tx: TransactionRepoModel,
        _new_tx_request: NetworkTransactionRequest,
    ) -> Result<TransactionRepoModel, TransactionError> {
        Ok(_old_tx)
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
