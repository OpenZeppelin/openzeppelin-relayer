use crate::{
    domain::{
        BalanceResponse, JsonRpcRequest, JsonRpcResponse, SignDataRequest, SignDataResponse,
        SignTypedDataRequest,
    },
    jobs::{JobProducer, JobProducerTrait, TransactionRequest},
    models::{
        NetworkRpcRequest, NetworkRpcResult, NetworkTransactionRequest, RelayerRepoModel,
        RepositoryError, StellarNetwork, StellarRpcResult, TransactionRepoModel,
    },
    repositories::{
        InMemoryRelayerRepository, InMemoryTransactionRepository, RelayerRepository,
        RelayerRepositoryStorage, Repository,
    },
};
use async_trait::async_trait;
use eyre::Result;
use std::sync::Arc;

use crate::domain::relayer::{Relayer, RelayerError};

#[allow(dead_code)]
pub struct StellarRelayer<R, T, J>
where
    R: Repository<RelayerRepoModel, String> + RelayerRepository + Send + Sync,
    T: Repository<TransactionRepoModel, String> + Send + Sync,
    J: JobProducerTrait + Send + Sync,
{
    relayer: RelayerRepoModel,
    network: StellarNetwork,
    relayer_repository: Arc<R>,
    transaction_repository: Arc<T>,
    job_producer: Arc<J>,
}

pub type DefaultStellarRelayer = StellarRelayer<
    RelayerRepositoryStorage<InMemoryRelayerRepository>,
    InMemoryTransactionRepository,
    JobProducer,
>;

impl<R, T, J> StellarRelayer<R, T, J>
where
    R: Repository<RelayerRepoModel, String> + RelayerRepository + Send + Sync,
    T: Repository<TransactionRepoModel, String> + Send + Sync,
    J: JobProducerTrait + Send + Sync,
{
    pub fn new(
        relayer: RelayerRepoModel,
        relayer_repository: Arc<R>,
        transaction_repository: Arc<T>,
        job_producer: Arc<J>,
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
impl<R, T, J> Relayer for StellarRelayer<R, T, J>
where
    R: Repository<RelayerRepoModel, String> + RelayerRepository + Send + Sync,
    T: Repository<TransactionRepoModel, String> + Send + Sync,
    J: JobProducerTrait + Send + Sync,
{
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

    async fn rpc(
        &self,
        _request: JsonRpcRequest<NetworkRpcRequest>,
    ) -> Result<JsonRpcResponse<NetworkRpcResult>, RelayerError> {
        println!("Stellar rpc...");
        Ok(JsonRpcResponse {
            id: Some(1),
            jsonrpc: "2.0".to_string(),
            result: Some(NetworkRpcResult::Stellar(
                StellarRpcResult::GenericRpcResult("".to_string()),
            )),
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
