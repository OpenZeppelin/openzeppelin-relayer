use std::sync::Arc;

use crate::{
    domain::{
        relayer::{Relayer, RelayerError},
        BalanceResponse, JsonRpcRequest, JsonRpcResponse, SignDataRequest, SignDataResponse,
        SignTypedDataRequest,
    },
    jobs::{JobProducer, TransactionRequest},
    models::{
        EvmNetwork, NetworkTransactionRequest, RelayerRepoModel, RepositoryError,
        TransactionRepoModel,
    },
    repositories::{InMemoryRelayerRepository, InMemoryTransactionRepository, Repository},
    services::{DataSignerTrait, EvmProvider, EvmSigner},
};
use async_trait::async_trait;
use eyre::Result;

#[allow(dead_code)]
pub struct EvmRelayer {
    relayer: RelayerRepoModel,
    signer: EvmSigner,
    network: EvmNetwork,
    provider: EvmProvider,
    relayer_repository: Arc<InMemoryRelayerRepository>,
    transaction_repository: Arc<InMemoryTransactionRepository>,
    job_producer: Arc<JobProducer>,
}

impl EvmRelayer {
    pub fn new(
        relayer: RelayerRepoModel,
        signer: EvmSigner,
        provider: EvmProvider,
        network: EvmNetwork,
        relayer_repository: Arc<InMemoryRelayerRepository>,
        transaction_repository: Arc<InMemoryTransactionRepository>,
        job_producer: Arc<JobProducer>,
    ) -> Result<Self, RelayerError> {
        Ok(Self {
            relayer,
            signer,
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

    async fn get_balance(&self) -> Result<BalanceResponse, RelayerError> {
        let address = self
            .relayer
            .address
            .clone()
            .expect("Relayer address not found");
        let balance: u128 = self
            .provider
            .get_balance(&address)
            .await
            .map_err(|e| RelayerError::ProviderError(e.to_string()))?
            .try_into()
            .map_err(|_| {
                RelayerError::ProviderError("Failed to convert balance to u128".to_string())
            })?;

        Ok(BalanceResponse {
            balance,
            unit: "wei".to_string(),
        })
    }

    async fn get_status(&self) -> Result<bool, RelayerError> {
        println!("EVM get_status...");
        Ok(true)
    }

    async fn delete_pending_transactions(&self) -> Result<bool, RelayerError> {
        println!("EVM delete_pending_transactions...");
        Ok(true)
    }

    async fn sign_data(&self, request: SignDataRequest) -> Result<SignDataResponse, RelayerError> {
        let result = self.signer.sign_data(request).await?;

        Ok(result)
    }

    async fn sign_typed_data(
        &self,
        request: SignTypedDataRequest,
    ) -> Result<SignDataResponse, RelayerError> {
        let result = self.signer.sign_typed_data(request).await?;

        Ok(result)
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

    async fn sync_relayer(&self) -> Result<bool, RelayerError> {
        println!("EVM sync relayer...");
        Ok(true)
    }
}

#[cfg(test)]
mod tests {}
