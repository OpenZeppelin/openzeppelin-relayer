use async_trait::async_trait;
use eyre::Result;
use std::sync::Arc;

use crate::{
    domain::transaction::Transaction,
    jobs::{JobProducer, JobProducerTrait},
    models::{NetworkTransactionData, RelayerRepoModel, TransactionError, TransactionRepoModel},
    repositories::{
        InMemoryRelayerRepository, InMemoryTransactionRepository, RelayerRepositoryStorage,
        Repository, TransactionRepository,
    },
    services::{Signer, StellarProvider, StellarProviderTrait, StellarSigner},
};

#[allow(dead_code)]
pub struct StellarRelayerTransaction<R, T, J, S, P>
where
    R: Repository<RelayerRepoModel, String>,
    T: TransactionRepository,
    J: JobProducerTrait,
    S: Signer,
    P: StellarProviderTrait,
{
    relayer: RelayerRepoModel,
    relayer_repository: Arc<R>,
    transaction_repository: Arc<T>,
    job_producer: Arc<J>,
    signer: Arc<S>,
    provider: P,
}

#[allow(dead_code)]
impl<R, T, J, S, P> StellarRelayerTransaction<R, T, J, S, P>
where
    R: Repository<RelayerRepoModel, String>,
    T: TransactionRepository,
    J: JobProducerTrait,
    S: Signer,
    P: StellarProviderTrait,
{
    pub fn new(
        relayer: RelayerRepoModel,
        relayer_repository: Arc<R>,
        transaction_repository: Arc<T>,
        job_producer: Arc<J>,
        signer: Arc<S>,
        provider: P,
    ) -> Result<Self, TransactionError> {
        Ok(Self {
            relayer,
            relayer_repository,
            transaction_repository,
            job_producer,
            signer,
            provider,
        })
    }

    pub fn provider(&self) -> &P {
        &self.provider
    }

    pub fn relayer(&self) -> &RelayerRepoModel {
        &self.relayer
    }

    pub fn job_producer(&self) -> &J {
        &self.job_producer
    }

    pub fn transaction_repository(&self) -> &T {
        &self.transaction_repository
    }
}

#[async_trait]
impl<R, T, J, S, P> Transaction for StellarRelayerTransaction<R, T, J, S, P>
where
    R: Repository<RelayerRepoModel, String> + Send + Sync,
    T: TransactionRepository + Send + Sync,
    J: JobProducerTrait + Send + Sync,
    S: Signer + Send + Sync,
    P: StellarProviderTrait + Send + Sync,
{
    async fn prepare_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        let _signature = self
            .signer
            .sign_transaction(NetworkTransactionData::Stellar(
                tx.network_data.get_stellar_transaction_data()?,
            ))
            .await?;

        Ok(tx)
    }

    async fn submit_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        Ok(tx)
    }

    async fn resubmit_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        // For now, just call submit_transaction as Stellar implementation is a stub
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

pub type DefaultStellarTransaction = StellarRelayerTransaction<
    RelayerRepositoryStorage<InMemoryRelayerRepository>,
    InMemoryTransactionRepository,
    JobProducer,
    StellarSigner,
    StellarProvider,
>;
