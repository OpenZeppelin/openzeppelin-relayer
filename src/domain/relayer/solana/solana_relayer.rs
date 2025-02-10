//! # Solana Relayer Module
//!
//! This module implements a relayer for the Solana network. It defines a trait
//! `SolanaRelayerTrait` for common operations such as sending JSON RPC requests,
//! fetching balance information, signing transactions, etc. The module uses a
//! [`SolanaProvider`](crate::services::SolanaProvider) for making RPC calls.
//!
//! It integrates with other parts of the system including the job queue ([`JobProducer`]),
//! in-memory repositories, and the application's domain models.
use std::sync::Arc;

use crate::{
    constants::SOLANA_SMALLEST_UNIT_NAME,
    domain::{
        relayer::RelayerError, BalanceResponse, JsonRpcRequest, JsonRpcResponse, SolanaRelayerTrait,
    },
    jobs::JobProducer,
    models::{RelayerRepoModel, SolanaNetwork},
    repositories::{InMemoryRelayerRepository, InMemoryTransactionRepository},
    services::{SolanaProvider, SolanaProviderTrait},
};
use async_trait::async_trait;
use eyre::Result;

#[allow(dead_code)]
pub struct SolanaRelayer {
    relayer: RelayerRepoModel,
    network: SolanaNetwork,
    provider: Arc<SolanaProvider>,
    relayer_repository: Arc<InMemoryRelayerRepository>,
    transaction_repository: Arc<InMemoryTransactionRepository>,
    job_producer: Arc<JobProducer>,
}

impl SolanaRelayer {
    pub fn new(
        relayer: RelayerRepoModel,
        provider: Arc<SolanaProvider>,
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
            provider,
            relayer_repository,
            transaction_repository,
            job_producer,
        })
    }
}

#[async_trait]
impl SolanaRelayerTrait for SolanaRelayer {
    async fn get_balance(&self) -> Result<BalanceResponse, RelayerError> {
        let address = &self.relayer.address;
        let balance = self.provider.get_balance(address).await?;

        Ok(BalanceResponse {
            balance: balance as u128,
            unit: SOLANA_SMALLEST_UNIT_NAME.to_string(),
        })
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
        Ok(())
    }
}

#[cfg(test)]
mod tests {}
