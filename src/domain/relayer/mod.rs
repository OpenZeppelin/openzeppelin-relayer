//! # Relayer Domain Module
//!
//! This module contains the core domain logic for the relayer service.
//! It handles transaction submission, validation, and monitoring across
//! different blockchain networks.
//! ## Architecture
//!
//! The relayer domain is organized into network-specific implementations
//! that share common interfaces for transaction handling and monitoring.

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use crate::{
    config::ServerConfig,
    jobs::JobProducer,
    models::{
        DecoratedSignature, EvmNetwork, EvmTransactionDataSignature, NetworkRpcRequest,
        NetworkRpcResult, NetworkTransactionRequest, NetworkType, RelayerError, RelayerRepoModel,
        SignerRepoModel, TransactionError, TransactionRepoModel,
    },
    repositories::{
        InMemoryRelayerRepository, InMemoryTransactionCounter, InMemoryTransactionRepository,
        RelayerRepositoryStorage,
    },
    services::{
        get_solana_network_provider, EvmSignerFactory, JupiterService, SolanaSignerFactory,
        TransactionCounterService,
    },
};

use crate::services::EvmProvider;
use async_trait::async_trait;
use eyre::Result;

mod evm;
mod solana;
mod stellar;
mod util;

pub use evm::*;
pub use solana::*;
pub use stellar::*;
pub use util::*;

/// The `Relayer` trait defines the core functionality required for a relayer
/// in the system. Implementors of this trait are responsible for handling
/// transaction requests, managing balances, and interacting with the network.
#[async_trait]
#[allow(dead_code)]
pub trait Relayer {
    /// Processes a transaction request and returns the result.
    ///
    /// # Arguments
    ///
    /// * `tx_request` - The transaction request to be processed.
    ///
    /// # Returns
    ///
    /// A `Result` containing a `TransactionRepoModel` on success, or a
    /// `RelayerError` on failure.
    async fn process_transaction_request(
        &self,
        tx_request: NetworkTransactionRequest,
    ) -> Result<TransactionRepoModel, RelayerError>;

    /// Retrieves the current balance of the relayer.
    ///
    /// # Returns
    ///
    /// A `Result` containing a `BalanceResponse` on success, or a
    /// `RelayerError` on failure.
    async fn get_balance(&self) -> Result<BalanceResponse, RelayerError>;

    /// Deletes all pending transactions.
    ///
    /// # Returns
    ///
    /// A `Result` containing `true` if transactions were successfully deleted,
    /// or a `RelayerError` on failure.
    async fn delete_pending_transactions(&self) -> Result<bool, RelayerError>;

    /// Signs data using the relayer's credentials.
    ///
    /// # Arguments
    ///
    /// * `request` - The data to be signed.
    ///
    /// # Returns
    ///
    /// A `Result` containing a `SignDataResponse` on success, or a
    /// `RelayerError` on failure.
    async fn sign_data(&self, request: SignDataRequest) -> Result<SignDataResponse, RelayerError>;

    /// Signs typed data using the relayer's credentials.
    ///
    /// # Arguments
    ///
    /// * `request` - The typed data to be signed.
    ///
    /// # Returns
    ///
    /// A `Result` containing a `SignDataResponse` on success, or a
    /// `RelayerError` on failure.
    async fn sign_typed_data(
        &self,
        request: SignTypedDataRequest,
    ) -> Result<SignDataResponse, RelayerError>;

    /// Executes a JSON-RPC request.
    ///
    /// # Arguments
    ///
    /// * `request` - The JSON-RPC request to be executed.
    ///
    /// # Returns
    ///
    /// A `Result` containing a `JsonRpcResponse` on success, or a
    /// `RelayerError` on failure.
    async fn rpc(
        &self,
        request: JsonRpcRequest<NetworkRpcRequest>,
    ) -> Result<JsonRpcResponse<NetworkRpcResult>, RelayerError>;

    /// Retrieves the current status of the relayer.
    ///
    /// # Returns
    ///
    /// A `Result` containing `true` if the relayer is active, or a
    /// `RelayerError` on failure.
    async fn get_status(&self) -> Result<bool, RelayerError>;

    /// Initializes the relayer.
    ///
    /// # Returns
    ///
    /// A `Result` indicating success, or a `RelayerError` on failure.
    async fn initialize_relayer(&self) -> Result<(), RelayerError>;

    /// Validates that the relayer's balance meets the minimum required.
    ///
    /// # Returns
    ///
    /// A `Result` indicating success, or a `RelayerError` on failure.
    async fn validate_min_balance(&self) -> Result<(), RelayerError>;
}

/// Solana Relayer Trait
/// Subset of methods for Solana relayer
#[async_trait]
#[allow(dead_code)]
pub trait SolanaRelayerTrait {
    /// Retrieves the current balance of the relayer.
    ///
    /// # Returns
    ///
    /// A `Result` containing a `BalanceResponse` on success, or a
    /// `RelayerError` on failure.
    async fn get_balance(&self) -> Result<BalanceResponse, RelayerError>;

    /// Executes a JSON-RPC request.
    ///
    /// # Arguments
    ///
    /// * `request` - The JSON-RPC request to be executed.
    ///
    /// # Returns
    ///
    /// A `Result` containing a `JsonRpcResponse` on success, or a
    /// `RelayerError` on failure.
    async fn rpc(
        &self,
        request: JsonRpcRequest<NetworkRpcRequest>,
    ) -> Result<JsonRpcResponse<NetworkRpcResult>, RelayerError>;

    /// Initializes the relayer.
    ///
    /// # Returns
    ///
    /// A `Result` indicating success, or a `RelayerError` on failure.
    async fn initialize_relayer(&self) -> Result<(), RelayerError>;

    /// Validates that the relayer's balance meets the minimum required.
    ///
    /// # Returns
    ///
    /// A `Result` indicating success, or a `RelayerError` on failure.
    async fn validate_min_balance(&self) -> Result<(), RelayerError>;
}

pub enum NetworkRelayer {
    Evm(DefaultEvmRelayer),
    Solana(SolanaRelayer),
    Stellar(DefaultStellarRelayer),
}

#[async_trait]
impl Relayer for NetworkRelayer {
    async fn process_transaction_request(
        &self,
        tx_request: NetworkTransactionRequest,
    ) -> Result<TransactionRepoModel, RelayerError> {
        match self {
            NetworkRelayer::Evm(relayer) => relayer.process_transaction_request(tx_request).await,
            NetworkRelayer::Solana(_) => solana_not_supported_relayer(),
            NetworkRelayer::Stellar(relayer) => {
                relayer.process_transaction_request(tx_request).await
            }
        }
    }

    async fn get_balance(&self) -> Result<BalanceResponse, RelayerError> {
        match self {
            NetworkRelayer::Evm(relayer) => relayer.get_balance().await,
            NetworkRelayer::Solana(relayer) => relayer.get_balance().await,
            NetworkRelayer::Stellar(relayer) => relayer.get_balance().await,
        }
    }

    async fn delete_pending_transactions(&self) -> Result<bool, RelayerError> {
        match self {
            NetworkRelayer::Evm(relayer) => relayer.delete_pending_transactions().await,
            NetworkRelayer::Solana(_) => solana_not_supported_relayer(),
            NetworkRelayer::Stellar(relayer) => relayer.delete_pending_transactions().await,
        }
    }

    async fn sign_data(&self, request: SignDataRequest) -> Result<SignDataResponse, RelayerError> {
        match self {
            NetworkRelayer::Evm(relayer) => relayer.sign_data(request).await,
            NetworkRelayer::Solana(_) => solana_not_supported_relayer(),
            NetworkRelayer::Stellar(relayer) => relayer.sign_data(request).await,
        }
    }

    async fn sign_typed_data(
        &self,
        request: SignTypedDataRequest,
    ) -> Result<SignDataResponse, RelayerError> {
        match self {
            NetworkRelayer::Evm(relayer) => relayer.sign_typed_data(request).await,
            NetworkRelayer::Solana(_) => solana_not_supported_relayer(),
            NetworkRelayer::Stellar(relayer) => relayer.sign_typed_data(request).await,
        }
    }

    async fn rpc(
        &self,
        request: JsonRpcRequest<NetworkRpcRequest>,
    ) -> Result<JsonRpcResponse<NetworkRpcResult>, RelayerError> {
        match self {
            NetworkRelayer::Evm(relayer) => relayer.rpc(request).await,
            NetworkRelayer::Solana(relayer) => relayer.rpc(request).await,
            NetworkRelayer::Stellar(relayer) => relayer.rpc(request).await,
        }
    }

    async fn get_status(&self) -> Result<bool, RelayerError> {
        match self {
            NetworkRelayer::Evm(relayer) => relayer.get_status().await,
            NetworkRelayer::Solana(_) => solana_not_supported_relayer(),
            NetworkRelayer::Stellar(relayer) => relayer.get_status().await,
        }
    }

    async fn validate_min_balance(&self) -> Result<(), RelayerError> {
        match self {
            NetworkRelayer::Evm(relayer) => relayer.validate_min_balance().await,
            NetworkRelayer::Solana(relayer) => relayer.validate_min_balance().await,
            NetworkRelayer::Stellar(relayer) => relayer.validate_min_balance().await,
        }
    }

    async fn initialize_relayer(&self) -> Result<(), RelayerError> {
        match self {
            NetworkRelayer::Evm(relayer) => relayer.initialize_relayer().await,
            NetworkRelayer::Solana(relayer) => relayer.initialize_relayer().await,
            NetworkRelayer::Stellar(relayer) => relayer.initialize_relayer().await,
        }
    }
}

pub trait RelayerFactoryTrait {
    fn create_relayer(
        relayer: RelayerRepoModel,
        signer: SignerRepoModel,
        relayer_repository: Arc<RelayerRepositoryStorage<InMemoryRelayerRepository>>,
        transaction_repository: Arc<InMemoryTransactionRepository>,
        transaction_counter_store: Arc<InMemoryTransactionCounter>,
        job_producer: Arc<JobProducer>,
    ) -> Result<NetworkRelayer, RelayerError>;
}
pub struct RelayerFactory;

impl RelayerFactoryTrait for RelayerFactory {
    fn create_relayer(
        relayer: RelayerRepoModel,
        signer: SignerRepoModel,
        relayer_repository: Arc<RelayerRepositoryStorage<InMemoryRelayerRepository>>,
        transaction_repository: Arc<InMemoryTransactionRepository>,
        transaction_counter_store: Arc<InMemoryTransactionCounter>,
        job_producer: Arc<JobProducer>,
    ) -> Result<NetworkRelayer, RelayerError> {
        match relayer.network_type {
            NetworkType::Evm => {
                let network = match EvmNetwork::from_network_str(&relayer.network) {
                    Ok(network) => network,
                    Err(e) => return Err(RelayerError::NetworkConfiguration(e.to_string())),
                };

                // Try custom RPC URL first, then fall back to public RPC URLs
                let rpc_url = network
                    .get_rpc_url(relayer.custom_rpc_urls.clone())
                    .ok_or_else(|| {
                        RelayerError::NetworkConfiguration("No RPC URLs configured".to_string())
                    })?;

                let rpc_timeout_ms = ServerConfig::from_env().rpc_timeout_ms;
                let evm_provider = EvmProvider::new_with_timeout(&rpc_url, rpc_timeout_ms)
                    .map_err(|e| RelayerError::NetworkConfiguration(e.to_string()))?;

                let signer_service = EvmSignerFactory::create_evm_signer(&signer)?;
                let transaction_counter_service = Arc::new(TransactionCounterService::new(
                    relayer.id.clone(),
                    relayer.address.clone(),
                    transaction_counter_store,
                ));
                let relayer = DefaultEvmRelayer::new(
                    relayer,
                    signer_service,
                    evm_provider,
                    network,
                    relayer_repository,
                    transaction_repository,
                    transaction_counter_service,
                    job_producer,
                )?;

                Ok(NetworkRelayer::Evm(relayer))
            }
            NetworkType::Solana => {
                let provider = Arc::new(get_solana_network_provider(
                    &relayer.network,
                    relayer.custom_rpc_urls.clone(),
                )?);
                let signer_service = Arc::new(SolanaSignerFactory::create_solana_signer(&signer)?);
                let jupiter_service = JupiterService::new_from_network(relayer.network.as_str());
                let rpc_methods = SolanaRpcMethodsImpl::new(
                    relayer.clone(),
                    provider.clone(),
                    signer_service.clone(),
                    Arc::new(jupiter_service),
                    job_producer.clone(),
                );
                let rpc_handler = Arc::new(SolanaRpcHandler::new(rpc_methods));
                let relayer = SolanaRelayer::new(
                    relayer,
                    signer_service,
                    relayer_repository,
                    provider,
                    rpc_handler,
                    transaction_repository,
                    job_producer,
                )?;
                Ok(NetworkRelayer::Solana(relayer))
            }
            NetworkType::Stellar => {
                let relayer = DefaultStellarRelayer::new(
                    relayer,
                    relayer_repository,
                    transaction_repository,
                    job_producer,
                )?;
                Ok(NetworkRelayer::Stellar(relayer))
            }
        }
    }
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct SignDataRequest {
    pub message: String,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct SignDataResponseEvm {
    pub r: String,
    pub s: String,
    pub v: u8,
    pub sig: String,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct SignDataResponseSolana {
    pub signature: String,
    pub public_key: String,
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(untagged)]
pub enum SignDataResponse {
    Evm(SignDataResponseEvm),
    Solana(SignDataResponseSolana),
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct SignTypedDataRequest {
    pub domain_separator: String,
    pub hash_struct_message: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SignTransactionResponseEvm {
    pub hash: String,
    pub signature: EvmTransactionDataSignature,
    pub raw: Vec<u8>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SignTransactionResponseStellar {
    pub signature: DecoratedSignature,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum SignTransactionResponse {
    Evm(SignTransactionResponseEvm),
    Solana(Vec<u8>),
    Stellar(SignTransactionResponseStellar),
}

impl SignTransactionResponse {
    pub fn into_evm(self) -> Result<SignTransactionResponseEvm, TransactionError> {
        match self {
            SignTransactionResponse::Evm(e) => Ok(e),
            _ => Err(TransactionError::InvalidType(
                "Expected EVM signature".to_string(),
            )),
        }
    }
}

// JSON-RPC Request struct
#[derive(Serialize, Deserialize, ToSchema)]
pub struct JsonRpcRequest<T> {
    pub jsonrpc: String,
    #[serde(flatten)]
    pub params: T,
    pub id: u64,
}

// JSON-RPC Response struct
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct JsonRpcResponse<T> {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub result: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub error: Option<JsonRpcError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub id: Option<u64>,
}

impl<T> JsonRpcResponse<T> {
    /// Creates a new successful JSON-RPC response with the given result and id.
    ///
    /// # Arguments
    /// * `id` - The request identifier
    /// * `result` - The result value to include in the response
    ///
    /// # Returns
    /// A new JsonRpcResponse with the specified result
    pub fn result(id: u64, result: T) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            result: Some(result),
            error: None,
            id: Some(id),
        }
    }

    pub fn error(code: i32, message: &str, description: &str) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.to_string(),
                description: description.to_string(),
            }),
            id: None,
        }
    }
}

// JSON-RPC Error struct
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    pub description: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct BalanceResponse {
    pub balance: u128,
    #[schema(example = "wei")]
    pub unit: String,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct RelayerUpdateRequest {
    #[schema(nullable = false)]
    pub paused: Option<bool>,
}
