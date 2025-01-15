use async_trait::async_trait;
use eyre::Result;
use std::sync::Arc;

use crate::{
    domain::transaction::Transaction,
    models::{RelayerRepoModel, TransactionError, TransactionRepoModel},
    repositories::{InMemoryRelayerRepository, InMemoryTransactionRepository},
};

#[allow(dead_code)]
pub struct StellarRelayerTransaction {
    relayer: RelayerRepoModel,
    relayer_repository: Arc<InMemoryRelayerRepository>,
    transaction_repository: Arc<InMemoryTransactionRepository>,
}

#[allow(dead_code)]
impl StellarRelayerTransaction {
    pub fn new(
        relayer: RelayerRepoModel,
        relayer_repository: Arc<InMemoryRelayerRepository>,
        transaction_repository: Arc<InMemoryTransactionRepository>,
    ) -> Result<Self, TransactionError> {
        Ok(Self {
            relayer_repository,
            transaction_repository,
            relayer,
        })
    }
}

#[async_trait]
impl Transaction for StellarRelayerTransaction {
    async fn submit_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
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
