use crate::{
    domain::{
        BalanceResponse, JsonRpcRequest, JsonRpcResponse, SignDataRequest, SignDataResponse,
        SignTypedDataRequest,
    },
    jobs::JobProducer,
    models::{NetworkTransactionRequest, RelayerRepoModel, StellarNetwork, TransactionRepoModel},
    repositories::{InMemoryTransactionRepository, RelayerRepositoryStorage},
};
use async_trait::async_trait;
use eyre::Result;
use log::info;
use std::sync::Arc;

use crate::domain::relayer::{Relayer, RelayerError};

#[allow(dead_code)]
pub struct StellarRelayer {
    relayer: RelayerRepoModel,
    network: StellarNetwork,
    relayer_repository: Arc<RelayerRepositoryStorage>,
    transaction_repository: Arc<InMemoryTransactionRepository>,
    job_producer: Arc<JobProducer>,
}

impl StellarRelayer {
    pub fn new(
        relayer: RelayerRepoModel,
        relayer_repository: Arc<RelayerRepositoryStorage>,
        transaction_repository: Arc<InMemoryTransactionRepository>,
        job_producer: Arc<JobProducer>,
    ) -> Result<Self, RelayerError> {
        let network = match StellarNetwork::from_network_str(&relayer.network) {
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
impl Relayer for StellarRelayer {
    async fn process_transaction_request(
        &self,
        network_transaction: NetworkTransactionRequest,
    ) -> Result<TransactionRepoModel, RelayerError> {
        let transaction = TransactionRepoModel::try_from((&network_transaction, &self.relayer))?;

        info!("Stellar Sending transaction...");
        Ok(transaction)
    }

    async fn get_balance(&self) -> Result<BalanceResponse, RelayerError> {
        println!("Stellar get_balance...");
        Ok(BalanceResponse {
            balance: 0,
            unit: "".to_string(),
        })
    }

    async fn get_status(&self) -> Result<bool, RelayerError> {
        println!("Stellar get_status...");
        Ok(true)
    }

    async fn delete_pending_transactions(&self) -> Result<bool, RelayerError> {
        println!("Stellar delete_pending_transactions...");
        Ok(true)
    }

    async fn sign_data(&self, _request: SignDataRequest) -> Result<SignDataResponse, RelayerError> {
        Err(RelayerError::NotSupported(
            "Signing data not supported for Stellar".to_string(),
        ))
    }

    async fn sign_typed_data(
        &self,
        _request: SignTypedDataRequest,
    ) -> Result<SignDataResponse, RelayerError> {
        Err(RelayerError::NotSupported(
            "Signing typed data not supported for Stellar".to_string(),
        ))
    }

    async fn rpc(&self, _request: JsonRpcRequest) -> Result<JsonRpcResponse, RelayerError> {
        println!("Stellar rpc...");
        Ok(JsonRpcResponse {
            id: Some(1),
            jsonrpc: "2.0".to_string(),
            result: Some(serde_json::Value::Null),
            error: None,
        })
    }

    async fn validate_min_balance(&self) -> Result<(), RelayerError> {
        Ok(())
    }

    async fn initialize_relayer(&self) -> Result<(), RelayerError> {
        println!("Stellar sync relayer...");
        Ok(())
    }
}
