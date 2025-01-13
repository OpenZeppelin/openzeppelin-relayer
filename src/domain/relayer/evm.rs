use std::sync::Arc;

use super::{Relayer, RelayerError};
use crate::models::{EvmNetwork, NetworkTransactionRequest, RepositoryError};
use crate::models::{RelayerRepoModel, TransactionRepoModel};
use crate::repositories::Repository;
use crate::services::EvmProvider;
use crate::AppState;
use async_trait::async_trait;
use eyre::Result;
use log::info;

#[allow(dead_code)]
pub struct EvmRelayer {
    relayer: RelayerRepoModel,
    network: EvmNetwork,
    app_state: Arc<AppState>,
    provider: EvmProvider,
}

impl EvmRelayer {
    pub fn new(
        relayer: RelayerRepoModel,
        app_state: Arc<AppState>,
        provider: EvmProvider,
        network: EvmNetwork,
    ) -> Result<Self, RelayerError> {
        Ok(Self {
            relayer,
            network,
            app_state,
            provider,
        })
    }
}

#[async_trait]
impl Relayer for EvmRelayer {
    async fn send_transaction(
        &self,
        network_transaction: NetworkTransactionRequest,
    ) -> Result<TransactionRepoModel, RelayerError> {
        // create
        let transaction = TransactionRepoModel::try_from((&network_transaction, &self.relayer))?;

        let test = self.provider.get_block_number().await.unwrap();

        info!("EVM test: {:?}", test);

        // send TODO
        info!("EVM Sending transaction...");
        self.app_state
            .transaction_repository
            .create(transaction.clone())
            .await
            .map_err(|e| RepositoryError::TransactionFailure(e.to_string()))?;

        Ok(transaction)
    }

    async fn get_balance(&self) -> Result<bool, RelayerError> {
        println!("EVM get_balance...");
        Ok(true)
    }

    async fn get_nonce(&self) -> Result<bool, RelayerError> {
        println!("EVM get_nonce...");
        Ok(true)
    }

    async fn delete_pending_transactions(&self) -> Result<bool, RelayerError> {
        println!("EVM delete_pending_transactions...");
        Ok(true)
    }

    async fn cancel_transaction(&self) -> Result<bool, RelayerError> {
        println!("EVM cancel_transaction...");
        Ok(true)
    }

    async fn replace_transaction(&self) -> Result<bool, RelayerError> {
        println!("EVM replace_transaction...");
        Ok(true)
    }

    async fn sign_data(&self) -> Result<bool, RelayerError> {
        println!("EVM sign_data...");
        Ok(true)
    }

    async fn sign_typed_data(&self) -> Result<bool, RelayerError> {
        println!("EVM sign_typed_data...");
        Ok(true)
    }

    async fn rpc(&self) -> Result<bool, RelayerError> {
        println!("EVM rpc...");
        Ok(true)
    }
}

#[cfg(test)]
mod tests {}
