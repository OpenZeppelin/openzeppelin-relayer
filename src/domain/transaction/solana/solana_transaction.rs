//! Solana transaction implementation
//!
//! This module provides the main SolanaRelayerTransaction struct and
//! implements the Transaction trait for Solana transactions.

use async_trait::async_trait;
use eyre::Result;
use log::info;
use std::sync::Arc;

use crate::{
    domain::transaction::Transaction,
    jobs::{JobProducer, JobProducerTrait},
    models::{NetworkTransactionRequest, RelayerRepoModel, TransactionError, TransactionRepoModel},
    repositories::{
        RelayerRepository, RelayerRepositoryStorage, Repository, TransactionRepository,
        TransactionRepositoryStorage,
    },
    services::{SolanaProvider, SolanaProviderTrait},
};

#[allow(dead_code)]
pub struct SolanaRelayerTransaction<P, RR, TR, J>
where
    P: SolanaProviderTrait,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    J: JobProducerTrait + Send + Sync + 'static,
{
    relayer: RelayerRepoModel,
    relayer_repository: Arc<RR>,
    provider: Arc<P>,
    job_producer: Arc<J>,
    transaction_repository: Arc<TR>,
}

pub type DefaultSolanaTransaction = SolanaRelayerTransaction<
    SolanaProvider,
    RelayerRepositoryStorage,
    TransactionRepositoryStorage,
    JobProducer,
>;

#[allow(dead_code)]
impl<P, RR, TR, J> SolanaRelayerTransaction<P, RR, TR, J>
where
    P: SolanaProviderTrait,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    J: JobProducerTrait + Send + Sync + 'static,
{
    pub fn new(
        relayer: RelayerRepoModel,
        relayer_repository: Arc<RR>,
        provider: Arc<P>,
        transaction_repository: Arc<TR>,
        job_producer: Arc<J>,
    ) -> Result<Self, TransactionError> {
        Ok(Self {
            relayer,
            relayer_repository,
            provider,
            transaction_repository,
            job_producer,
        })
    }

    // Getter methods for status module access
    pub(super) fn provider(&self) -> &P {
        &self.provider
    }

    pub(super) fn transaction_repository(&self) -> &TR {
        &self.transaction_repository
    }

    pub(super) fn relayer(&self) -> &RelayerRepoModel {
        &self.relayer
    }

    pub(super) fn job_producer(&self) -> &J {
        &self.job_producer
    }
}

#[async_trait]
impl<P, RR, TR, J> Transaction for SolanaRelayerTransaction<P, RR, TR, J>
where
    P: SolanaProviderTrait,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    J: JobProducerTrait + Send + Sync + 'static,
{
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
        Ok(tx)
    }

    /// Main entry point for transaction status handling
    async fn handle_transaction_status(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        self.handle_transaction_status_impl(tx).await
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        jobs::MockJobProducerTrait,
        repositories::{MockRelayerRepository, MockTransactionRepository},
        services::MockSolanaProviderTrait,
        utils::mocks::mockutils::{create_mock_solana_relayer, create_mock_solana_transaction},
    };

    #[tokio::test]
    async fn test_solana_transaction_creation() {
        let relayer = create_mock_solana_relayer("test-solana-relayer".to_string(), false);
        let relayer_repository = Arc::new(MockRelayerRepository::new());
        let provider = Arc::new(MockSolanaProviderTrait::new());
        let transaction_repository = Arc::new(MockTransactionRepository::new());
        let job_producer = Arc::new(MockJobProducerTrait::new());

        let transaction = SolanaRelayerTransaction::new(
            relayer,
            relayer_repository,
            provider,
            transaction_repository,
            job_producer,
        );

        assert!(transaction.is_ok());
    }

    #[tokio::test]
    async fn test_handle_transaction_status_calls_impl() {
        // Create test data
        let relayer = create_mock_solana_relayer("test-solana-relayer".to_string(), false);
        let relayer_repository = Arc::new(MockRelayerRepository::new());
        let provider = Arc::new(MockSolanaProviderTrait::new());
        let transaction_repository = Arc::new(MockTransactionRepository::new());
        let job_producer = Arc::new(MockJobProducerTrait::new());

        // Create test transaction
        let test_tx = create_mock_solana_transaction();

        // Create transaction handler
        let transaction_handler = SolanaRelayerTransaction::new(
            relayer,
            relayer_repository,
            provider,
            transaction_repository,
            job_producer,
        )
        .unwrap();

        // Mock handle_transaction_status_impl to return Ok(test_tx.clone())
        let result = transaction_handler
            .handle_transaction_status(test_tx.clone())
            .await;

        // Verify the result matches what we expect from handle_transaction_status_impl
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(
            error.to_string(),
            "Transaction validation error: Transaction signature is missing".to_string()
        );
    }
}
