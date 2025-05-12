/// This module defines the `StellarRelayer` struct and its associated functionality for
/// interacting with Stellar networks. The `StellarRelayer` is responsible for managing
/// transactions, synchronizing sequence numbers, and ensuring the relayer's state is
/// consistent with the Stellar blockchain.
///
/// # Components
///
/// - `StellarRelayer`: The main struct that encapsulates the relayer's state and operations for Stellar.
/// - `RelayerRepoModel`: Represents the relayer's data model.
/// - `StellarProvider`: Provides blockchain interaction capabilities, such as fetching account details.
/// - `TransactionCounterService`: Manages the sequence number for transactions to ensure correct ordering.
/// - `JobProducer`: Produces jobs for processing transactions and sending notifications.
///
/// # Error Handling
///
/// The module uses the `RelayerError` enum to handle various errors that can occur during
/// operations, such as provider errors, sequence synchronization failures, and transaction failures.
///
/// # Usage
///
/// To use the `StellarRelayer`, create an instance using the `new` method, providing the necessary
/// components. Then, call the appropriate methods to process transactions and manage the relayer's state.
use crate::{
    domain::{
        next_sequence_u64, BalanceResponse, JsonRpcRequest, JsonRpcResponse, SignDataRequest,
        SignDataResponse, SignTypedDataRequest,
    },
    jobs::{JobProducer, JobProducerTrait, TransactionRequest},
    models::{
        produce_relayer_disabled_payload, NetworkRpcRequest, NetworkRpcResult,
        NetworkTransactionRequest, RelayerRepoModel, RepositoryError, StellarNetwork,
        StellarRpcResult, TransactionRepoModel,
    },
    repositories::{
        InMemoryRelayerRepository, InMemoryTransactionCounter, InMemoryTransactionRepository,
        RelayerRepository, RelayerRepositoryStorage, Repository,
    },
    services::{
        StellarProvider, StellarProviderTrait, TransactionCounterService,
        TransactionCounterServiceTrait,
    },
};
use async_trait::async_trait;
use eyre::Result;
use log::{info, warn};
use std::sync::Arc;

use crate::domain::relayer::{Relayer, RelayerError};

#[allow(dead_code)]
pub struct StellarRelayer<P, R, T, J, C>
where
    P: StellarProviderTrait + Send + Sync,
    R: Repository<RelayerRepoModel, String> + RelayerRepository + Send + Sync,
    T: Repository<TransactionRepoModel, String> + Send + Sync,
    J: JobProducerTrait + Send + Sync,
    C: TransactionCounterServiceTrait + Send + Sync,
{
    relayer: RelayerRepoModel,
    network: StellarNetwork,
    provider: P,
    relayer_repository: Arc<R>,
    transaction_repository: Arc<T>,
    transaction_counter_service: Arc<C>,
    job_producer: Arc<J>,
}

pub type DefaultStellarRelayer = StellarRelayer<
    StellarProvider,
    RelayerRepositoryStorage<InMemoryRelayerRepository>,
    InMemoryTransactionRepository,
    JobProducer,
    TransactionCounterService<InMemoryTransactionCounter>,
>;

impl<P, R, T, J, C> StellarRelayer<P, R, T, J, C>
where
    P: StellarProviderTrait + Send + Sync,
    R: Repository<RelayerRepoModel, String> + RelayerRepository + Send + Sync,
    T: Repository<TransactionRepoModel, String> + Send + Sync,
    J: JobProducerTrait + Send + Sync,
    C: TransactionCounterServiceTrait + Send + Sync,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        relayer: RelayerRepoModel,
        provider: P,
        relayer_repository: Arc<R>,
        transaction_repository: Arc<T>,
        transaction_counter_service: Arc<C>,
        job_producer: Arc<J>,
    ) -> Result<Self, RelayerError> {
        let network = match StellarNetwork::from_network_str(&relayer.network) {
            Ok(network) => network,
            Err(e) => return Err(RelayerError::NetworkConfiguration(e.to_string())),
        };

        Ok(Self {
            relayer,
            network,
            provider,
            relayer_repository,
            transaction_repository,
            transaction_counter_service,
            job_producer,
        })
    }

    async fn sync_sequence(&self) -> Result<(), RelayerError> {
        info!(
            "Fetching sequence for relayer: {} ({})",
            self.relayer.id, self.relayer.address
        );

        let account_entry = self
            .provider
            .get_account(&self.relayer.address)
            .await
            .map_err(|e| RelayerError::ProviderError(format!("Failed to fetch account: {}", e)))?;

        let next = next_sequence_u64(account_entry.seq_num.0)?;

        info!(
            "Setting next sequence {} for relayer {}",
            next, self.relayer.id
        );
        self.transaction_counter_service
            .set(next)
            .await
            .map_err(RelayerError::from)?;
        Ok(())
    }

    async fn disable_relayer(&self, reasons: &[String]) -> Result<(), RelayerError> {
        let reason = reasons.join(", ");
        warn!("Disabling relayer {} due to: {}", self.relayer.id, reason);

        let updated = self
            .relayer_repository
            .disable_relayer(self.relayer.id.clone())
            .await?;

        if let Some(nid) = &self.relayer.notification_id {
            self.job_producer
                .produce_send_notification_job(
                    produce_relayer_disabled_payload(nid, &updated, &reason),
                    None,
                )
                .await?;
        }
        Ok(())
    }
}

#[async_trait]
impl<P, R, T, J, C> Relayer for StellarRelayer<P, R, T, J, C>
where
    P: StellarProviderTrait + Send + Sync,
    R: Repository<RelayerRepoModel, String> + RelayerRepository + Send + Sync,
    T: Repository<TransactionRepoModel, String> + Send + Sync,
    J: JobProducerTrait + Send + Sync,
    C: TransactionCounterServiceTrait + Send + Sync,
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
        info!("Initializing Stellar relayer: {}", self.relayer.id);

        let seq_res = self.sync_sequence().await.err();

        let mut failures: Vec<String> = Vec::new();
        if let Some(e) = seq_res {
            failures.push(format!("Sequence sync failed: {}", e));
        }

        if !failures.is_empty() {
            self.disable_relayer(&failures).await?;
            return Ok(()); // same semantics as EVM
        }

        info!(
            "Stellar relayer initialized successfully: {}",
            self.relayer.id
        );
        Ok(())
    }
}
