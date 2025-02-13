use std::sync::Arc;

use crate::{
    constants::EVM_SMALLEST_UNIT_NAME,
    domain::{
        relayer::{Relayer, RelayerError},
        BalanceResponse, JsonRpcRequest, JsonRpcResponse, SignDataRequest, SignDataResponse,
        SignTypedDataRequest,
    },
    jobs::{JobProducer, TransactionRequest},
    models::{
        produce_relayer_disabled_payload, EvmNetwork, NetworkTransactionRequest, RelayerRepoModel,
        RepositoryError, TransactionRepoModel,
    },
    repositories::{
        InMemoryTransactionRepository, RelayerRepository, RelayerRepositoryStorage, Repository,
    },
    services::{DataSignerTrait, EvmProvider, EvmSigner, TransactionCounterService},
};
use async_trait::async_trait;
use eyre::Result;
use log::{info, warn};

#[allow(dead_code)]
pub struct EvmRelayer {
    relayer: RelayerRepoModel,
    signer: EvmSigner,
    network: EvmNetwork,
    provider: EvmProvider,
    relayer_repository: Arc<RelayerRepositoryStorage>,
    transaction_repository: Arc<InMemoryTransactionRepository>,
    transaction_counter_service: TransactionCounterService,
    job_producer: Arc<JobProducer>,
}

#[allow(clippy::too_many_arguments)]
impl EvmRelayer {
    pub fn new(
        relayer: RelayerRepoModel,
        signer: EvmSigner,
        provider: EvmProvider,
        network: EvmNetwork,
        relayer_repository: Arc<RelayerRepositoryStorage>,
        transaction_repository: Arc<InMemoryTransactionRepository>,
        transaction_counter_service: TransactionCounterService,
        job_producer: Arc<JobProducer>,
    ) -> Result<Self, RelayerError> {
        Ok(Self {
            relayer,
            signer,
            network,
            provider,
            relayer_repository,
            transaction_repository,
            transaction_counter_service,
            job_producer,
        })
    }

    async fn sync_nonce(&self) -> Result<(), RelayerError> {
        let on_chain_nonce = self
            .provider
            .get_transaction_count(&self.relayer.address)
            .await
            .map_err(|e| RelayerError::ProviderError(e.to_string()))?;

        info!(
            "Setting nonce: {} for relayer: {}",
            on_chain_nonce, self.relayer.id
        );

        self.transaction_counter_service.set(on_chain_nonce)?;

        Ok(())
    }

    async fn validate_rpc(&self) -> Result<(), RelayerError> {
        self.provider
            .health_check()
            .await
            .map_err(|e| RelayerError::ProviderError(e.to_string()))?;

        Ok(())
    }
}

#[async_trait]
impl Relayer for EvmRelayer {
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
        let balance: u128 = self
            .provider
            .get_balance(&self.relayer.address)
            .await
            .map_err(|e| RelayerError::ProviderError(e.to_string()))?
            .try_into()
            .map_err(|_| {
                RelayerError::ProviderError("Failed to convert balance to u128".to_string())
            })?;

        Ok(BalanceResponse {
            balance,
            unit: EVM_SMALLEST_UNIT_NAME.to_string(),
        })
    }

    async fn get_status(&self) -> Result<bool, RelayerError> {
        println!("EVM get_status...");
        Ok(true)
    }

    async fn delete_pending_transactions(&self) -> Result<bool, RelayerError> {
        println!("EVM delete_pending_transactions...");
        Ok(true)
    }

    async fn sign_data(&self, request: SignDataRequest) -> Result<SignDataResponse, RelayerError> {
        let result = self.signer.sign_data(request).await?;

        Ok(result)
    }

    async fn sign_typed_data(
        &self,
        request: SignTypedDataRequest,
    ) -> Result<SignDataResponse, RelayerError> {
        let result = self.signer.sign_typed_data(request).await?;

        Ok(result)
    }

    async fn rpc(&self, _request: JsonRpcRequest) -> Result<JsonRpcResponse, RelayerError> {
        println!("EVM rpc...");
        Ok(JsonRpcResponse {
            id: 1,
            jsonrpc: "2.0".to_string(),
            result: Some(serde_json::Value::Null),
            error: None,
        })
    }

    async fn validate_min_balance(&self) -> Result<(), RelayerError> {
        let balance: u128 = self
            .provider
            .get_balance(&self.relayer.address)
            .await
            .map_err(|e| RelayerError::ProviderError(e.to_string()))?
            .try_into()
            .map_err(|_| {
                RelayerError::ProviderError("Failed to convert balance to u128".to_string())
            })?;

        info!("Balance : {} for relayer: {}", balance, self.relayer.id);

        let policy = self.relayer.policies.get_evm_policy();

        if balance < policy.min_balance {
            return Err(RelayerError::InsufficientBalanceError(
                "Insufficient balance".to_string(),
            ));
        }

        Ok(())
    }

    async fn initialize_relayer(&self) -> Result<(), RelayerError> {
        info!("Initializing relayer: {}", self.relayer.id);
        let nonce_sync_result = self.sync_nonce().await;
        let validate_rpc_result = self.validate_rpc().await;
        let validate_min_balance_result = self.validate_min_balance().await;

        // disable relayer if any check fails
        if nonce_sync_result.is_err()
            || validate_rpc_result.is_err()
            || validate_min_balance_result.is_err()
        {
            let reason = vec![
                nonce_sync_result
                    .err()
                    .map(|e| format!("Nonce sync failed: {}", e)),
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
