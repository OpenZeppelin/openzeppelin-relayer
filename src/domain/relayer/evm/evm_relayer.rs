/// This module defines the `EvmRelayer` struct and its associated functionality for
/// interacting with Ethereum Virtual Machine (EVM) networks. The `EvmRelayer` is responsible
/// for managing transactions, signing data, and ensuring the relayer's state is synchronized
/// with the blockchain.
///
/// # Components
///
/// - `EvmRelayer`: The main struct that encapsulates the relayer's state and operations.
/// - `RelayerRepoModel`: Represents the relayer's data model.
/// - `EvmSigner`: Handles signing of data and transactions.
/// - `EvmProvider`: Provides blockchain interaction capabilities, such as fetching balances
///   and transaction counts.
/// - `TransactionCounterService`: Manages the nonce for transactions to ensure they are
///   processed in the correct order.
/// - `JobProducer`: Produces jobs for processing transactions and sending notifications.
///
/// # Error Handling
///
/// The module uses the `RelayerError` enum to handle various errors that can occur during
/// operations, such as provider errors, insufficient balance, and transaction failures.
///
/// # Usage
///
/// To use the `EvmRelayer`, create an instance using the `new` method, providing the necessary
/// components. Then, call the appropriate methods to process transactions, sign data, and
/// manage the relayer's state.
use std::sync::Arc;

use crate::{
    constants::EVM_SMALLEST_UNIT_NAME,
    domain::{
        relayer::{Relayer, RelayerError},
        BalanceResponse, JsonRpcRequest, JsonRpcResponse, SignDataRequest, SignDataResponse,
        SignTypedDataRequest,
    },
    jobs::{JobProducer, JobProducerTrait, TransactionRequest},
    models::{
        produce_relayer_disabled_payload, EvmNetwork, NetworkTransactionRequest, RelayerRepoModel,
        RepositoryError, TransactionRepoModel,
    },
    repositories::{
        InMemoryTransactionRepository, RelayerRepository, RelayerRepositoryStorage, Repository,
    },
    services::{
        DataSignerTrait, EvmProvider, EvmProviderTrait, EvmSigner, TransactionCounterService,
    },
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
    /// Constructs a new `EvmRelayer` instance.
    ///
    /// # Arguments
    ///
    /// * `relayer` - The relayer's data model.
    /// * `signer` - The EVM signer for signing data and transactions.
    /// * `provider` - The EVM provider for blockchain interactions.
    /// * `network` - The EVM network configuration.
    /// * `relayer_repository` - The repository for relayer storage.
    /// * `transaction_repository` - The repository for transaction storage.
    /// * `transaction_counter_service` - The service for managing transaction nonces.
    /// * `job_producer` - The job producer for creating transaction jobs.
    ///
    /// # Returns
    ///
    /// A `Result` containing the new `EvmRelayer` instance or a `RelayerError`
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

    /// Synchronizes the nonce with the blockchain.
    ///
    /// # Returns
    ///
    /// A `Result` indicating success or a `RelayerError` if the operation fails.
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

    /// Validates the RPC connection to the blockchain provider.
    ///
    /// # Returns
    ///
    /// A `Result` indicating success or a `RelayerError` if the operation fails.
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
    /// Processes a transaction request and creates a job for it.
    ///
    /// # Arguments
    ///
    /// * `network_transaction` - The network transaction request to process.
    ///
    /// # Returns
    ///
    /// A `Result` containing the `TransactionRepoModel` or a `RelayerError`.
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

    /// Retrieves the balance of the relayer's address.
    ///
    /// # Returns
    ///
    /// A `Result` containing the `BalanceResponse` or a `RelayerError`.
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

    /// Gets the status of the relayer.
    ///
    /// # Returns
    ///
    /// A `Result` containing a boolean indicating the status or a `RelayerError`.
    async fn get_status(&self) -> Result<bool, RelayerError> {
        println!("EVM get_status...");
        Ok(true)
    }

    /// Deletes pending transactions.
    ///
    /// # Returns
    ///
    /// A `Result` containing a boolean indicating success or a `RelayerError`.
    async fn delete_pending_transactions(&self) -> Result<bool, RelayerError> {
        println!("EVM delete_pending_transactions...");
        Ok(true)
    }

    /// Signs data using the relayer's signer.
    ///
    /// # Arguments
    ///
    /// * `request` - The request containing the data to sign.
    ///
    /// # Returns
    ///
    /// A `Result` containing the `SignDataResponse` or a `RelayerError`.
    async fn sign_data(&self, request: SignDataRequest) -> Result<SignDataResponse, RelayerError> {
        let result = self.signer.sign_data(request).await?;

        Ok(result)
    }

    /// Signs typed data using the relayer's signer.
    ///
    /// # Arguments
    ///
    /// * `request` - The request containing the typed data to sign.
    ///
    /// # Returns
    ///
    /// A `Result` containing the `SignDataResponse` or a `RelayerError`.
    async fn sign_typed_data(
        &self,
        request: SignTypedDataRequest,
    ) -> Result<SignDataResponse, RelayerError> {
        let result = self.signer.sign_typed_data(request).await?;

        Ok(result)
    }

    /// Handles a JSON-RPC request.
    ///
    /// # Arguments
    ///
    /// * `_request` - The JSON-RPC request to handle.
    ///
    /// # Returns
    ///
    /// A `Result` containing the `JsonRpcResponse` or a `RelayerError`.
    async fn rpc(&self, _request: JsonRpcRequest) -> Result<JsonRpcResponse, RelayerError> {
        println!("EVM rpc...");
        Ok(JsonRpcResponse {
            id: Some(1),
            jsonrpc: "2.0".to_string(),
            result: Some(serde_json::Value::Null),
            error: None,
        })
    }

    /// Validates that the relayer's balance meets the minimum required balance.
    ///
    /// # Returns
    ///
    /// A `Result` indicating success or a `RelayerError` if the balance is insufficient.
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

    /// Initializes the relayer by performing necessary checks and synchronizations.
    ///
    /// # Returns
    ///
    /// A `Result` indicating success or a `RelayerError` if any initialization step fails.
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
