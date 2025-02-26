//! This module defines the core transaction handling logic for different blockchain networks,
//! including Ethereum (EVM), Solana, and Stellar. It provides a unified interface for preparing,
//! submitting, handling, canceling, replacing, signing, and validating transactions across these
//! networks. The module also includes a factory for creating network-specific transaction handlers
//! based on relayer and repository information.
//!
//! The main components of this module are:
//! - `Transaction` trait: Defines the operations for handling transactions.
//! - `NetworkTransaction` enum: Represents a transaction for different network types.
//! - `RelayerTransactionFactory`: A factory for creating network transactions.
//!
//! The module leverages async traits to handle asynchronous operations and uses the `eyre` crate
//! for error handling.
use crate::{
    jobs::JobProducer,
    models::{
        EvmNetwork, NetworkType, RelayerRepoModel, SignerRepoModel, TransactionError,
        TransactionRepoModel,
    },
    repositories::{
        InMemoryTransactionCounter, InMemoryTransactionRepository, RelayerRepositoryStorage,
    },
    services::{
        get_solana_network_provider_from_str, EvmGasPriceService, EvmProvider, EvmSignerFactory,
        TransactionCounterService,
    },
};
use async_trait::async_trait;
use eyre::Result;
use std::sync::Arc;

mod evm;
mod solana;
mod stellar;
mod util;

pub use evm::*;
pub use solana::*;
pub use stellar::*;
pub use util::*;

/// A trait that defines the operations for handling transactions across different networks.
#[async_trait]
#[allow(dead_code)]
pub trait Transaction {
    /// Prepares a transaction for submission.
    ///
    /// # Arguments
    ///
    /// * `tx` - A `TransactionRepoModel` representing the transaction to be prepared.
    ///
    /// # Returns
    ///
    /// A `Result` containing the prepared `TransactionRepoModel` or a `TransactionError`.

    async fn prepare_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError>;

    /// Submits a transaction to the network.
    ///
    /// # Arguments
    ///
    /// * `tx` - A `TransactionRepoModel` representing the transaction to be submitted.
    ///
    /// # Returns
    ///
    /// A `Result` containing the submitted `TransactionRepoModel` or a `TransactionError`.
    async fn submit_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError>;

    /// Handles the status of a transaction.
    ///
    /// # Arguments
    ///
    /// * `tx` - A `TransactionRepoModel` representing the transaction whose status is to be
    ///   handled.
    ///
    /// # Returns
    ///
    /// A `Result` containing the updated `TransactionRepoModel` or a `TransactionError`.
    async fn handle_transaction_status(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError>;

    /// Cancels a transaction.
    ///
    /// # Arguments
    ///
    /// * `tx` - A `TransactionRepoModel` representing the transaction to be canceled.
    ///
    /// # Returns
    ///
    /// A `Result` containing the canceled `TransactionRepoModel` or a `TransactionError`.
    async fn cancel_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError>;

    /// Replaces a transaction with a new one.
    ///
    /// # Arguments
    ///
    /// * `tx` - A `TransactionRepoModel` representing the transaction to be replaced.
    ///
    /// # Returns
    ///
    /// A `Result` containing the new `TransactionRepoModel` or a `TransactionError`.
    async fn replace_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError>;

    /// Signs a transaction.
    ///
    /// # Arguments
    ///
    /// * `tx` - A `TransactionRepoModel` representing the transaction to be signed.
    ///
    /// # Returns
    ///
    /// A `Result` containing the signed `TransactionRepoModel` or a `TransactionError`.
    async fn sign_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError>;

    /// Validates a transaction.
    ///
    /// # Arguments
    ///
    /// * `tx` - A `TransactionRepoModel` representing the transaction to be validated.
    ///
    /// # Returns
    ///
    /// A `Result` containing a boolean indicating the validity of the transaction or a
    /// `TransactionError`.
    async fn validate_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<bool, TransactionError>;
}

/// An enum representing a transaction for different network types.
pub enum NetworkTransaction {
    Evm(EvmRelayerTransaction),
    Solana(SolanaRelayerTransaction),
    Stellar(StellarRelayerTransaction),
}

#[async_trait]
impl Transaction for NetworkTransaction {
    /// Prepares a transaction for submission based on the network type.
    ///
    /// # Arguments
    ///
    /// * `tx` - A `TransactionRepoModel` representing the transaction to be prepared.
    ///
    /// # Returns
    ///
    /// A `Result` containing the prepared `TransactionRepoModel` or a `TransactionError`.
    async fn prepare_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        match self {
            NetworkTransaction::Evm(relayer) => relayer.prepare_transaction(tx).await,
            NetworkTransaction::Solana(relayer) => relayer.prepare_transaction(tx).await,
            NetworkTransaction::Stellar(relayer) => relayer.prepare_transaction(tx).await,
        }
    }

    /// Submits a transaction to the network based on the network type.
    ///
    /// # Arguments
    ///
    /// * `tx` - A `TransactionRepoModel` representing the transaction to be submitted.
    ///
    /// # Returns
    ///
    /// A `Result` containing the submitted `TransactionRepoModel` or a `TransactionError`.
    async fn submit_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        match self {
            NetworkTransaction::Evm(relayer) => relayer.submit_transaction(tx).await,
            NetworkTransaction::Solana(relayer) => relayer.submit_transaction(tx).await,
            NetworkTransaction::Stellar(relayer) => relayer.submit_transaction(tx).await,
        }
    }

    /// Handles the status of a transaction based on the network type.
    ///
    /// # Arguments
    ///
    /// * `tx` - A `TransactionRepoModel` representing the transaction whose status is to be
    ///   handled.
    ///
    /// # Returns
    ///
    /// A `Result` containing the updated `TransactionRepoModel` or a `TransactionError`.
    async fn handle_transaction_status(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        match self {
            NetworkTransaction::Evm(relayer) => relayer.handle_transaction_status(tx).await,
            NetworkTransaction::Solana(relayer) => relayer.handle_transaction_status(tx).await,
            NetworkTransaction::Stellar(relayer) => relayer.handle_transaction_status(tx).await,
        }
    }

    /// Cancels a transaction based on the network type.
    ///
    /// # Arguments
    ///
    /// * `tx` - A `TransactionRepoModel` representing the transaction to be canceled.
    ///
    /// # Returns
    ///
    /// A `Result` containing the canceled `TransactionRepoModel` or a `TransactionError`.
    async fn cancel_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        match self {
            NetworkTransaction::Evm(relayer) => relayer.cancel_transaction(tx).await,
            NetworkTransaction::Solana(_) => solana_not_supported(),
            NetworkTransaction::Stellar(relayer) => relayer.cancel_transaction(tx).await,
        }
    }

    /// Replaces a transaction with a new one based on the network type.
    ///
    /// # Arguments
    ///
    /// * `tx` - A `TransactionRepoModel` representing the transaction to be replaced.
    ///
    /// # Returns
    ///
    /// A `Result` containing the new `TransactionRepoModel` or a `TransactionError`.
    async fn replace_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        match self {
            NetworkTransaction::Evm(relayer) => relayer.replace_transaction(tx).await,
            NetworkTransaction::Solana(_) => solana_not_supported(),
            NetworkTransaction::Stellar(relayer) => relayer.replace_transaction(tx).await,
        }
    }

    /// Signs a transaction based on the network type.
    ///
    /// # Arguments
    ///
    /// * `tx` - A `TransactionRepoModel` representing the transaction to be signed.
    ///
    /// # Returns
    ///
    /// A `Result` containing the signed `TransactionRepoModel` or a `TransactionError`.
    async fn sign_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        match self {
            NetworkTransaction::Evm(relayer) => relayer.sign_transaction(tx).await,
            NetworkTransaction::Solana(relayer) => relayer.sign_transaction(tx).await,
            NetworkTransaction::Stellar(relayer) => relayer.sign_transaction(tx).await,
        }
    }

    /// Validates a transaction based on the network type.
    ///
    /// # Arguments
    ///
    /// * `tx` - A `TransactionRepoModel` representing the transaction to be validated.
    ///
    /// # Returns
    ///
    /// A `Result` containing a boolean indicating the validity of the transaction or a
    /// `TransactionError`.
    async fn validate_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<bool, TransactionError> {
        match self {
            NetworkTransaction::Evm(relayer) => relayer.validate_transaction(tx).await,
            NetworkTransaction::Solana(relayer) => relayer.validate_transaction(tx).await,
            NetworkTransaction::Stellar(relayer) => relayer.validate_transaction(tx).await,
        }
    }
}

/// A trait for creating network transactions.
#[allow(dead_code)]
pub trait RelayerTransactionFactoryTrait {
    /// Creates a network transaction based on the relayer and repository information.
    ///
    /// # Arguments
    ///
    /// * `relayer` - A `RelayerRepoModel` representing the relayer.
    /// * `relayer_repository` - An `Arc` to the `RelayerRepositoryStorage`.
    /// * `transaction_repository` - An `Arc` to the `InMemoryTransactionRepository`.
    /// * `job_producer` - An `Arc` to the `JobProducer`.
    ///
    /// # Returns
    ///
    /// A `Result` containing the created `NetworkTransaction` or a `TransactionError`.
    fn create_transaction(
        relayer: RelayerRepoModel,
        relayer_repository: Arc<RelayerRepositoryStorage>,
        transaction_repository: Arc<InMemoryTransactionRepository>,
        job_producer: Arc<JobProducer>,
    ) -> Result<NetworkTransaction, TransactionError>;
}
/// A factory for creating relayer transactions.
pub struct RelayerTransactionFactory;

#[allow(dead_code)]
impl RelayerTransactionFactory {
    /// Creates a network transaction based on the relayer, signer, and repository information.
    ///
    /// # Arguments
    ///
    /// * `relayer` - A `RelayerRepoModel` representing the relayer.
    /// * `signer` - A `SignerRepoModel` representing the signer.
    /// * `relayer_repository` - An `Arc` to the `RelayerRepositoryStorage`.
    /// * `transaction_repository` - An `Arc` to the `InMemoryTransactionRepository`.
    /// * `transaction_counter_store` - An `Arc` to the `InMemoryTransactionCounter`.
    /// * `job_producer` - An `Arc` to the `JobProducer`.
    ///
    /// # Returns
    ///
    /// A `Result` containing the created `NetworkTransaction` or a `TransactionError`.
    pub fn create_transaction(
        relayer: RelayerRepoModel,
        signer: SignerRepoModel,
        relayer_repository: Arc<RelayerRepositoryStorage>,
        transaction_repository: Arc<InMemoryTransactionRepository>,
        transaction_counter_store: Arc<InMemoryTransactionCounter>,
        job_producer: Arc<JobProducer>,
    ) -> Result<NetworkTransaction, TransactionError> {
        match relayer.network_type {
            NetworkType::Evm => {
                let network = match EvmNetwork::from_network_str(&relayer.network) {
                    Ok(network) => network,
                    Err(e) => return Err(TransactionError::NetworkConfiguration(e.to_string())),
                };
                let rpc_url = network
                    .public_rpc_urls()
                    .and_then(|urls| urls.first().cloned())
                    .ok_or_else(|| {
                        TransactionError::NetworkConfiguration("No RPC URLs configured".to_string())
                    })?;
                let evm_provider: EvmProvider = EvmProvider::new(rpc_url)
                    .map_err(|e| TransactionError::NetworkConfiguration(e.to_string()))?;
                let transaction_counter_service = TransactionCounterService::new(
                    relayer.id.clone(),
                    relayer.address.clone(),
                    transaction_counter_store,
                );
                let gas_price_service = Arc::new(EvmGasPriceService::new(evm_provider.clone()));
                let signer_service = EvmSignerFactory::create_evm_signer(&signer)?;

                Ok(NetworkTransaction::Evm(EvmRelayerTransaction::new(
                    relayer,
                    evm_provider,
                    relayer_repository,
                    transaction_repository,
                    transaction_counter_service,
                    job_producer,
                    gas_price_service,
                    signer_service,
                )?))
            }
            NetworkType::Solana => {
                let solana_provider =
                    Arc::new(get_solana_network_provider_from_str(&relayer.network)?);

                Ok(NetworkTransaction::Solana(SolanaRelayerTransaction::new(
                    relayer,
                    relayer_repository,
                    solana_provider,
                    transaction_repository,
                    job_producer,
                )?))
            }
            NetworkType::Stellar => {
                Ok(NetworkTransaction::Stellar(StellarRelayerTransaction::new(
                    relayer,
                    relayer_repository,
                    transaction_repository,
                    job_producer,
                )?))
            }
        }
    }
}
