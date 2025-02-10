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
    models::{produce_relayer_disabled_payload, RelayerRepoModel, SolanaNetwork},
    repositories::{InMemoryRelayerRepository, InMemoryTransactionRepository},
    services::{SolanaProvider, SolanaProviderTrait},
};
use async_trait::async_trait;
use eyre::Result;
use log::{info, warn};

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

    async fn validate_rpc(&self) -> Result<(), RelayerError> {
        self.provider
            .get_latest_blockhash()
            .await
            .map_err(|e| RelayerError::ProviderError(e.to_string()))?;

        Ok(())
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

    async fn validate_min_balance(&self) -> Result<(), RelayerError> {
        let balance = self
            .provider
            .get_balance(&self.relayer.address)
            .await
            .map_err(|e| RelayerError::ProviderError(e.to_string()))?;

        info!("Balance : {} for relayer: {}", balance, self.relayer.id);

        let policy = self.relayer.policies.get_solana_policy();

        if balance < policy.min_balance {
            return Err(RelayerError::InsufficientBalanceError(
                "Insufficient balance".to_string(),
            ));
        }

        Ok(())
    }

    async fn initialize_relayer(&self) -> Result<(), RelayerError> {
        info!("Initializing relayer: {}", self.relayer.id);
        let validate_rpc_result = self.validate_rpc().await;
        let validate_min_balance_result = self.validate_min_balance().await;

        // disable relayer if any check fails
        if validate_rpc_result.is_err() || validate_min_balance_result.is_err() {
            let reason = vec![
                validate_rpc_result
                    .err()
                    .map(|e| format!("RPC validation failed: {}", e)),
                validate_min_balance_result
                    .err()
                    .map(|e| format!("Balance check failed: {}", e)),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<String>>()
            .join(", ");

            warn!("Disabling relayer: {} due to: {}", self.relayer.id, reason);
            let updated_relayer = self
                .relayer_repository
                .disable_relayer(self.relayer.id.clone())
                .await?;
            if let Some(notification_id) = &self.relayer.notification_id {
                self.job_producer
                    .produce_send_notification_job(
                        produce_relayer_disabled_payload(
                            notification_id,
                            &updated_relayer,
                            &reason,
                        ),
                        None,
                    )
                    .await?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {}
