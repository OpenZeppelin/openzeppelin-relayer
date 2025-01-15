use crate::{
    domain::{JsonRpcRequest, JsonRpcResponse, SignDataRequest, SignDataResponse},
    models::{NetworkTransactionRequest, RelayerRepoModel, StellarNetwork, TransactionRepoModel},
    repositories::{InMemoryRelayerRepository, InMemoryTransactionRepository},
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
    relayer_repository: Arc<InMemoryRelayerRepository>,
    transaction_repository: Arc<InMemoryTransactionRepository>,
}

impl StellarRelayer {
    pub fn new(
        relayer: RelayerRepoModel,
        relayer_repository: Arc<InMemoryRelayerRepository>,
        transaction_repository: Arc<InMemoryTransactionRepository>,
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
        })
    }
}

#[async_trait]
impl Relayer for StellarRelayer {
    async fn send_transaction(
        &self,
        network_transaction: NetworkTransactionRequest,
    ) -> Result<TransactionRepoModel, RelayerError> {
        let transaction = TransactionRepoModel::try_from((&network_transaction, &self.relayer))?;

        info!("Stellar Sending transaction...");
        Ok(transaction)
    }

    async fn get_balance(&self) -> Result<u128, RelayerError> {
        println!("Stellar get_balance...");
        Ok(0)
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
        info!("Stellar sign_data...");
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
        info!("Stellar sign_typed_data...");
        Ok(SignDataResponse {
            sig: "".to_string(),
            r: "".to_string(),
            s: "".to_string(),
            v: 0,
        })
    }

    async fn rpc(&self, _request: JsonRpcRequest) -> Result<JsonRpcResponse, RelayerError> {
        println!("Stellar rpc...");
        Ok(JsonRpcResponse {
            id: 1,
            jsonrpc: "2.0".to_string(),
            result: Some(serde_json::Value::Null),
            error: None,
        })
    }

    async fn validate_relayer(&self) -> Result<bool, RelayerError> {
        println!("Stellar validate relayer...");
        Ok(true)
    }

    async fn sync_relayer(&self) -> Result<bool, RelayerError> {
        println!("Stellar sync relayer...");
        Ok(true)
    }
}
