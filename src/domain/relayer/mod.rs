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

use crate::models::{
    EvmNetwork, NetworkTransactionRequest, NetworkType, RelayerError, RelayerRepoModel,
    TransactionRepoModel,
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

pub use evm::*;
pub use solana::*;
pub use stellar::*;

#[async_trait]
#[allow(dead_code)]
pub trait Relayer {
    async fn send_transaction(
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

pub trait RelayerFactoryTrait {
    fn create_relayer(
        model: RelayerRepoModel,
        relayer_repository: Arc<InMemoryRelayerRepository>,
        transaction_repository: Arc<InMemoryTransactionRepository>,
    ) -> Result<Box<dyn Relayer>, RelayerError>;
}
pub struct RelayerFactory;

impl RelayerFactoryTrait for RelayerFactory {
    fn create_relayer(
        relayer: RelayerRepoModel,
        relayer_repository: Arc<InMemoryRelayerRepository>,
        transaction_repository: Arc<InMemoryTransactionRepository>,
    ) -> Result<Box<dyn Relayer>, RelayerError> {
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
                let evm_provider: EvmProvider = EvmProvider::new(rpc_url).unwrap();
                let relayer = EvmRelayer::new(
                    relayer,
                    evm_provider,
                    network,
                    relayer_repository,
                    transaction_repository,
                )?;

                Ok(Box::new(relayer) as Box<dyn Relayer>)
            }
            NetworkType::Solana => {
                let relayer =
                    SolanaRelayer::new(relayer, relayer_repository, transaction_repository)?;
                Ok(Box::new(relayer))
            }
            NetworkType::Stellar => {
                let relayer =
                    StellarRelayer::new(relayer, relayer_repository, transaction_repository)?;
                Ok(Box::new(relayer))
            }
        }
    }
}
