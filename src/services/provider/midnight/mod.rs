//! Midnight provider implementation
//!
//! This module provides network communication and prover server integration for Midnight.

mod client;
mod prover_client;

pub use client::MidnightRpcClient;
pub use prover_client::ProverClient;

use crate::models::MidnightNetwork;
use crate::services::provider::ProviderError;
use async_trait::async_trait;
use std::sync::Arc;

/// Main provider interface for Midnight network
#[async_trait]
pub trait MidnightProviderTrait: Send + Sync {
    /// Submit a transaction to the network
    async fn submit_transaction(&self, tx: Vec<u8>) -> Result<String, ProviderError>;

    /// Get transaction status
    async fn get_transaction_status(
        &self,
        tx_hash: &str,
    ) -> Result<TransactionStatus, ProviderError>;

    /// Query nullifier set to check if nullifier is spent
    async fn is_nullifier_spent(&self, nullifier: &[u8; 32]) -> Result<bool, ProviderError>;

    /// Get current Merkle tree root
    async fn get_merkle_root(&self) -> Result<[u8; 32], ProviderError>;

    /// Get commitment index in the Merkle tree
    async fn get_commitment_index(
        &self,
        commitment: &[u8; 32],
    ) -> Result<Option<u64>, ProviderError>;
}

/// Transaction status for Midnight
#[derive(Debug, Clone)]
pub enum TransactionStatus {
    Pending,
    SucceededEntirely,
    FailedEntirely,
    SucceededPartially {
        segment_success: std::collections::HashMap<u16, bool>,
    },
}

/// Default Midnight provider implementation
pub struct MidnightProvider {
    rpc_client: Arc<MidnightRpcClient>,
    prover_client: Arc<ProverClient>,
    _network: MidnightNetwork,
}

impl MidnightProvider {
    /// Creates a new Midnight provider
    pub fn new(
        network: MidnightNetwork,
        prover_url: Option<String>,
    ) -> Result<Self, ProviderError> {
        let rpc_client = Arc::new(MidnightRpcClient::new(network.rpc_urls.clone())?);

        let prover_client = Arc::new(ProverClient::new(
            prover_url.unwrap_or_else(|| "http://localhost:8080".to_string()),
        )?);

        Ok(Self {
            rpc_client,
            prover_client,
            _network: network,
        })
    }

    /// Get the prover client
    pub fn prover_client(&self) -> &ProverClient {
        &self.prover_client
    }
}

#[async_trait]
impl MidnightProviderTrait for MidnightProvider {
    async fn submit_transaction(&self, tx: Vec<u8>) -> Result<String, ProviderError> {
        self.rpc_client.submit_transaction(tx).await
    }

    async fn get_transaction_status(
        &self,
        tx_hash: &str,
    ) -> Result<TransactionStatus, ProviderError> {
        self.rpc_client.get_transaction_status(tx_hash).await
    }

    async fn is_nullifier_spent(&self, nullifier: &[u8; 32]) -> Result<bool, ProviderError> {
        self.rpc_client.is_nullifier_spent(nullifier).await
    }

    async fn get_merkle_root(&self) -> Result<[u8; 32], ProviderError> {
        self.rpc_client.get_merkle_root().await
    }

    async fn get_commitment_index(
        &self,
        commitment: &[u8; 32],
    ) -> Result<Option<u64>, ProviderError> {
        self.rpc_client.get_commitment_index(commitment).await
    }
}
