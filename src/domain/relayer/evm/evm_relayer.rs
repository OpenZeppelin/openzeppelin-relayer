use std::sync::Arc;

use crate::{
    domain::{
        relayer::{Relayer, RelayerError},
        JsonRpcRequest, JsonRpcResponse, SignDataRequest, SignDataResponse,
    },
    jobs::{JobProducer, TransactionRequest},
    models::{
        EvmNetwork, NetworkTransactionRequest, RelayerRepoModel, RepositoryError,
        TransactionRepoModel,
    },
    repositories::{InMemoryRelayerRepository, InMemoryTransactionRepository, Repository},
    services::EvmProvider,
};
use async_trait::async_trait;
use eyre::Result;

#[allow(dead_code)]
pub struct EvmRelayer {
    relayer: RelayerRepoModel,
    network: EvmNetwork,
    provider: EvmProvider,
    relayer_repository: Arc<InMemoryRelayerRepository>,
    transaction_repository: Arc<InMemoryTransactionRepository>,
    job_producer: Arc<JobProducer>,
}

impl EvmRelayer {
    pub fn new(
        relayer: RelayerRepoModel,
        provider: EvmProvider,
        network: EvmNetwork,
        relayer_repository: Arc<InMemoryRelayerRepository>,
        transaction_repository: Arc<InMemoryTransactionRepository>,
        job_producer: Arc<JobProducer>,
    ) -> Result<Self, RelayerError> {
        Ok(Self {
            relayer,
            network,
            provider,
            relayer_repository,
            transaction_repository,
            job_producer,
        })
    }
}

#[async_trait]
impl Relayer for EvmRelayer {
    async fn process_transaction_request(
        &self,
        network_transaction: NetworkTransactionRequest,
    ) -> Result<TransactionRepoModel, RelayerError> {
        let transaction = TransactionRepoModel::try_from((&network_transaction, &self.relayer))?;

        self.transaction_repository
            .create(transaction.clone())
            .await
            .map_err(|e| RepositoryError::TransactionFailure(e.to_string()))?;

        self.job_producer
            .produce_transaction_request_job(
                TransactionRequest::new(transaction.id.clone(), transaction.relayer_id.clone()),
                None,
            )
            .await?;

        Ok(transaction)
    }

    async fn get_balance(&self) -> Result<u128, RelayerError> {
        println!("EVM get_balance...");
        Ok(0)
    }

    async fn get_status(&self) -> Result<bool, RelayerError> {
        println!("EVM get_status...");
        Ok(true)
    }

    async fn delete_pending_transactions(&self) -> Result<bool, RelayerError> {
        println!("EVM delete_pending_transactions...");
        Ok(true)
    }

    async fn sign_data(&self, _request: SignDataRequest) -> Result<SignDataResponse, RelayerError> {
        println!("EVM sign_data...");
        Ok(SignDataResponse {
            sig: "".to_string(),
            r: "".to_string(),
            s: "".to_string(),
            v: 0,
        })
    }

    async fn sign_typed_data(
        &self,
        _request: SignDataRequest,
    ) -> Result<SignDataResponse, RelayerError> {
        println!("EVM sign_typed_data...");
        Ok(SignDataResponse {
            sig: "".to_string(),
            r: "".to_string(),
            s: "".to_string(),
            v: 0,
        })
    }

    async fn rpc(&self, _request: JsonRpcRequest) -> Result<JsonRpcResponse, RelayerError> {
        println!("EVM rpc...");
        Ok(JsonRpcResponse {
            id: 1,
            jsonrpc: "2.0".to_string(),
            result: Some(serde_json::Value::Null),
            error: None,
        })
    }

    async fn validate_relayer(&self) -> Result<bool, RelayerError> {
        println!("EVM validate relayer...");
        Ok(true)
    }

    async fn sync_relayer(&self) -> Result<bool, RelayerError> {
        println!("EVM sync relayer...");
        Ok(true)
    }
}

#[cfg(test)]
mod tests {}
