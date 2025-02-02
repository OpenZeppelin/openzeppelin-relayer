use std::sync::Arc;

use crate::{
    domain::{
        relayer::{Relayer, RelayerError},
        BalanceResponse, JsonRpcRequest, JsonRpcResponse, SignDataRequest, SignDataResponse,
        SignDataResponseSolana, SignTypedDataRequest,
    },
    jobs::JobProducer,
    models::{NetworkTransactionRequest, RelayerRepoModel, SolanaNetwork, TransactionRepoModel},
    repositories::{InMemoryRelayerRepository, InMemoryTransactionRepository},
};
use async_trait::async_trait;
use eyre::Result;
use log::info;

#[allow(dead_code)]
pub struct SolanaRelayer {
    relayer: RelayerRepoModel,
    network: SolanaNetwork,
    relayer_repository: Arc<InMemoryRelayerRepository>,
    transaction_repository: Arc<InMemoryTransactionRepository>,
    job_producer: Arc<JobProducer>,
}

impl SolanaRelayer {
    pub fn new(
        relayer: RelayerRepoModel,
        relayer_repository: Arc<InMemoryRelayerRepository>,
        transaction_repository: Arc<InMemoryTransactionRepository>,
        job_producer: Arc<JobProducer>,
    ) -> Result<Self, RelayerError> {
        let network = match SolanaNetwork::from_network_str(&relayer.network) {
            Ok(network) => network,
            Err(e) => return Err(RelayerError::NetworkConfiguration(e.to_string())),
        };

        Ok(Self {
            relayer,
            network,
            relayer_repository,
            transaction_repository,
            job_producer,
        })
    }
}

#[async_trait]
impl Relayer for SolanaRelayer {
    async fn process_transaction_request(
        &self,
        network_transaction: NetworkTransactionRequest,
    ) -> Result<TransactionRepoModel, RelayerError> {
        let transaction = TransactionRepoModel::try_from((&network_transaction, &self.relayer))?;

        info!("Solana Sending transaction...");
        Ok(transaction)
    }

    async fn get_balance(&self) -> Result<BalanceResponse, RelayerError> {
        println!("Solana get_balance...");
        Ok(BalanceResponse {
            balance: 0,
            unit: "".to_string(),
        })
    }

    async fn get_status(&self) -> Result<bool, RelayerError> {
        println!("Solana get_status...");
        Ok(true)
    }

    async fn delete_pending_transactions(&self) -> Result<bool, RelayerError> {
        println!("Solana delete_pending_transactions...");
        Ok(true)
    }

    async fn sign_data(&self, _request: SignDataRequest) -> Result<SignDataResponse, RelayerError> {
        println!("Solana sign_data...");

        let signature = SignDataResponseSolana {
            signature: "".to_string(),
            public_key: "".to_string(),
        };

        Ok(SignDataResponse::Solana(signature))
    }

    async fn sign_typed_data(
        &self,
        _request: SignTypedDataRequest,
    ) -> Result<SignDataResponse, RelayerError> {
        Err(RelayerError::NotSupported(
            "Signing typed data not supported for Solana".to_string(),
        ))
    }

    async fn rpc(&self, _request: JsonRpcRequest) -> Result<JsonRpcResponse, RelayerError> {
        println!("Solana rpc...");
        Ok(JsonRpcResponse {
            id: 1,
            jsonrpc: "2.0".to_string(),
            result: Some(serde_json::Value::Null),
            error: None,
        })
    }

    async fn initialize_relayer(&self) -> Result<(), RelayerError> {
        println!("Stellar sync relayer...");
        Ok(())
    }
}

#[cfg(test)]
mod tests {}
