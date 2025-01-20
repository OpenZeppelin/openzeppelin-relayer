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
        TransactionRepoModel,
    },
};

use crate::{
    repositories::{InMemoryRelayerRepository, InMemoryTransactionRepository},
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
    async fn get_balance(&self) -> Result<u128, RelayerError>;
    async fn delete_pending_transactions(&self) -> Result<bool, RelayerError>;
    async fn sign_data(&self, request: SignDataRequest) -> Result<SignDataResponse, RelayerError>;
    async fn sign_typed_data(
        &self,
        request: SignDataRequest,
    ) -> Result<SignDataResponse, RelayerError>;
    async fn rpc(&self, request: JsonRpcRequest) -> Result<JsonRpcResponse, RelayerError>;
    async fn get_status(&self) -> Result<bool, RelayerError>;
    async fn validate_relayer(&self) -> Result<bool, RelayerError>;
    async fn sync_relayer(&self) -> Result<bool, RelayerError>;
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
            NetworkRelayer::Solana(relayer) => {
                relayer.process_transaction_request(tx_request).await
            }
            NetworkRelayer::Stellar(relayer) => {
                relayer.process_transaction_request(tx_request).await
            }
        }
    }

    async fn get_balance(&self) -> Result<u128, RelayerError> {
        match self {
            NetworkRelayer::Evm(relayer) => relayer.get_balance().await,
            NetworkRelayer::Solana(relayer) => relayer.get_balance().await,
            NetworkRelayer::Stellar(relayer) => relayer.get_balance().await,
        }
    }

    async fn delete_pending_transactions(&self) -> Result<bool, RelayerError> {
        match self {
            NetworkRelayer::Evm(relayer) => relayer.delete_pending_transactions().await,
            NetworkRelayer::Solana(relayer) => relayer.delete_pending_transactions().await,
            NetworkRelayer::Stellar(relayer) => relayer.delete_pending_transactions().await,
        }
    }

    async fn sign_data(&self, request: SignDataRequest) -> Result<SignDataResponse, RelayerError> {
        match self {
            NetworkRelayer::Evm(relayer) => relayer.sign_data(request).await,
            NetworkRelayer::Solana(relayer) => relayer.sign_data(request).await,
            NetworkRelayer::Stellar(relayer) => relayer.sign_data(request).await,
        }
    }

    async fn sign_typed_data(
        &self,
        request: SignDataRequest,
    ) -> Result<SignDataResponse, RelayerError> {
        match self {
            NetworkRelayer::Evm(relayer) => relayer.sign_typed_data(request).await,
            NetworkRelayer::Solana(relayer) => relayer.sign_typed_data(request).await,
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
            NetworkRelayer::Solana(relayer) => relayer.get_status().await,
            NetworkRelayer::Stellar(relayer) => relayer.get_status().await,
        }
    }

    async fn validate_relayer(&self) -> Result<bool, RelayerError> {
        match self {
            NetworkRelayer::Evm(relayer) => relayer.validate_relayer().await,
            NetworkRelayer::Solana(relayer) => relayer.validate_relayer().await,
            NetworkRelayer::Stellar(relayer) => relayer.validate_relayer().await,
        }
    }

    async fn sync_relayer(&self) -> Result<bool, RelayerError> {
        match self {
            NetworkRelayer::Evm(relayer) => relayer.sync_relayer().await,
            NetworkRelayer::Solana(relayer) => relayer.sync_relayer().await,
            NetworkRelayer::Stellar(relayer) => relayer.sync_relayer().await,
        }
    }
}

pub trait RelayerFactoryTrait {
    fn create_relayer(
        model: RelayerRepoModel,
        relayer_repository: Arc<InMemoryRelayerRepository>,
        transaction_repository: Arc<InMemoryTransactionRepository>,
        job_producer: Arc<JobProducer>,
    ) -> Result<NetworkRelayer, RelayerError>;
}
pub struct RelayerFactory;

impl RelayerFactoryTrait for RelayerFactory {
    fn create_relayer(
        relayer: RelayerRepoModel,
        relayer_repository: Arc<InMemoryRelayerRepository>,
        transaction_repository: Arc<InMemoryTransactionRepository>,
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
                let relayer = EvmRelayer::new(
                    relayer,
                    evm_provider,
                    network,
                    relayer_repository,
                    transaction_repository,
                    job_producer,
                )?;

                Ok(NetworkRelayer::Evm(relayer))
            }
            NetworkType::Solana => {
                let relayer = SolanaRelayer::new(
                    relayer,
                    relayer_repository,
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
pub struct SignDataResponse {
    pub sig: String,
    pub r: String,
    pub s: String,
    pub v: u8,
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
