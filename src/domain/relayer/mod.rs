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

use crate::{
    jobs::JobProducer,
    models::{
        EvmNetwork, NetworkTransactionRequest, NetworkType, RelayerError, RelayerRepoModel,
        SignerRepoModel, TransactionRepoModel,
    },
    repositories::RelayerRepositoryStorage,
    services::{get_solana_network_provider_from_str, EvmSignerFactory, TransactionCounterService},
};

use crate::{
    repositories::{InMemoryTransactionCounter, InMemoryTransactionRepository},
    services::EvmProvider,
};
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

#[async_trait]
#[allow(dead_code)]
pub trait Relayer {
    async fn process_transaction_request(
        &self,
        tx_request: NetworkTransactionRequest,
    ) -> Result<TransactionRepoModel, RelayerError>;
    async fn get_balance(&self) -> Result<BalanceResponse, RelayerError>;
    async fn delete_pending_transactions(&self) -> Result<bool, RelayerError>;
    async fn sign_data(&self, request: SignDataRequest) -> Result<SignDataResponse, RelayerError>;
    async fn sign_typed_data(
        &self,
        request: SignTypedDataRequest,
    ) -> Result<SignDataResponse, RelayerError>;
    async fn rpc(&self, request: JsonRpcRequest) -> Result<JsonRpcResponse, RelayerError>;
    async fn get_status(&self) -> Result<bool, RelayerError>;
    async fn initialize_relayer(&self) -> Result<(), RelayerError>;
    async fn validate_min_balance(&self) -> Result<(), RelayerError>;
}

// Solana Relayer Trait
// Subset of methods for Solana relayer
#[async_trait]
#[allow(dead_code)]
pub trait SolanaRelayerTrait {
    async fn get_balance(&self) -> Result<BalanceResponse, RelayerError>;
    async fn rpc(&self, request: JsonRpcRequest) -> Result<JsonRpcResponse, RelayerError>;
    async fn initialize_relayer(&self) -> Result<(), RelayerError>;
    async fn validate_min_balance(&self) -> Result<(), RelayerError>;
}

pub enum NetworkRelayer {
    Evm(EvmRelayer),
    Solana(SolanaRelayer),
    Stellar(StellarRelayer),
}

#[async_trait]
impl Relayer for NetworkRelayer {
    async fn process_transaction_request(
        &self,
        tx_request: NetworkTransactionRequest,
    ) -> Result<TransactionRepoModel, RelayerError> {
        match self {
            NetworkRelayer::Evm(relayer) => relayer.process_transaction_request(tx_request).await,
            NetworkRelayer::Solana(_) => solana_not_supported(),
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
            NetworkRelayer::Solana(_) => solana_not_supported(),
            NetworkRelayer::Stellar(relayer) => relayer.delete_pending_transactions().await,
        }
    }

    async fn sign_data(&self, request: SignDataRequest) -> Result<SignDataResponse, RelayerError> {
        match self {
            NetworkRelayer::Evm(relayer) => relayer.sign_data(request).await,
            NetworkRelayer::Solana(_) => solana_not_supported(),
            NetworkRelayer::Stellar(relayer) => relayer.sign_data(request).await,
        }
    }

    async fn sign_typed_data(
        &self,
        request: SignTypedDataRequest,
    ) -> Result<SignDataResponse, RelayerError> {
        match self {
            NetworkRelayer::Evm(relayer) => relayer.sign_typed_data(request).await,
            NetworkRelayer::Solana(_) => solana_not_supported(),
            NetworkRelayer::Stellar(relayer) => relayer.sign_typed_data(request).await,
        }
    }

    async fn rpc(&self, request: JsonRpcRequest) -> Result<JsonRpcResponse, RelayerError> {
        match self {
            NetworkRelayer::Evm(relayer) => relayer.rpc(request).await,
            NetworkRelayer::Solana(relayer) => relayer.rpc(request).await,
            NetworkRelayer::Stellar(relayer) => relayer.rpc(request).await,
        }
    }

    async fn get_status(&self) -> Result<bool, RelayerError> {
        match self {
            NetworkRelayer::Evm(relayer) => relayer.get_status().await,
            NetworkRelayer::Solana(_) => solana_not_supported(),
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
        relayer_repository: Arc<RelayerRepositoryStorage>,
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
        relayer_repository: Arc<RelayerRepositoryStorage>,
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
                let rpc_url = network
                    .public_rpc_urls()
                    .and_then(|urls| urls.first().cloned())
                    .ok_or_else(|| {
                        RelayerError::NetworkConfiguration("No RPC URLs configured".to_string())
                    })?;
                let evm_provider: EvmProvider = EvmProvider::new(rpc_url)
                    .map_err(|e| RelayerError::NetworkConfiguration(e.to_string()))?;
                let signer_service = EvmSignerFactory::create_evm_signer(&signer)?;
                let transaction_counter_service = TransactionCounterService::new(
                    relayer.id.clone(),
                    relayer.address.clone(),
                    transaction_counter_store,
                );
                let relayer = EvmRelayer::new(
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
                let solana_provider =
                    Arc::new(get_solana_network_provider_from_str(&relayer.network)?);

                let relayer = SolanaRelayer::new(
                    relayer,
                    relayer_repository,
                    solana_provider,
                    transaction_repository,
                    job_producer,
                )?;
                Ok(NetworkRelayer::Solana(relayer))
            }
            NetworkType::Stellar => {
                let relayer = StellarRelayer::new(
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

#[derive(Serialize, Deserialize)]
pub struct SignDataRequest {
    pub message: String,
}

#[derive(Serialize, Deserialize)]
pub struct SignDataResponseEvm {
    pub r: String,
    pub s: String,
    pub v: u8,
    pub sig: String,
}

#[derive(Serialize, Deserialize)]
pub struct SignDataResponseSolana {
    pub signature: String,
    pub public_key: String,
}

#[derive(Serialize, Deserialize)]
pub enum SignDataResponse {
    Evm(SignDataResponseEvm),
    Solana(SignDataResponseSolana),
}

#[derive(Serialize, Deserialize)]
pub struct SignTypedDataRequest {
    pub domain_separator: String,
    pub hash_struct_message: String,
}

// JSON-RPC Request struct
#[derive(Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub method: String,
    pub params: serde_json::Value,
    pub id: u64,
}

// JSON-RPC Response struct
#[derive(Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub result: Option<serde_json::Value>,
    pub error: Option<JsonRpcError>,
    pub id: u64,
}

// JSON-RPC Error struct
#[derive(Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct BalanceResponse {
    pub balance: u128,
    pub unit: String,
}

#[derive(Serialize, Deserialize)]
pub struct RelayerUpdateRequest {
    pub paused: Option<bool>,
}
