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
        EvmNetwork, NetworkTransactionRequest, NetworkType, RelayerRepoModel, SignerRepoModel,
        SolanaNetwork, StellarNetwork, TransactionError, TransactionRepoModel,
    },
    repositories::{
        NetworkRepository, NetworkRepositoryStorage, RelayerRepositoryStorage,
        TransactionCounterRepositoryStorage, TransactionRepositoryStorage,
    },
    services::{
        get_network_extra_fee_calculator_service, get_network_provider, EvmGasPriceService,
        EvmSignerFactory, StellarSignerFactory,
    },
};
use async_trait::async_trait;
use eyre::Result;
#[cfg(test)]
use mockall::automock;
use std::sync::Arc;

pub mod evm;
pub mod solana;
pub mod stellar;

mod util;
pub use util::*;

// Explicit re-exports to avoid ambiguous glob re-exports
pub use evm::{DefaultEvmTransaction, EvmRelayerTransaction};
pub use solana::SolanaRelayerTransaction;
pub use stellar::{DefaultStellarTransaction, StellarRelayerTransaction};

/// A trait that defines the operations for handling transactions across different networks.
#[cfg_attr(test, automock)]
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

    /// Resubmits a transaction with updated parameters.
    ///
    /// # Arguments
    ///
    /// * `tx` - A `TransactionRepoModel` representing the transaction to be resubmitted.
    ///
    /// # Returns
    ///
    /// A `Result` containing the resubmitted `TransactionRepoModel` or a `TransactionError`.
    async fn resubmit_transaction(
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
    /// * `old_tx` - A `TransactionRepoModel` representing the transaction to be replaced.
    /// * `new_tx_request` - A `NetworkTransactionRequest` representing the new transaction data.
    ///
    /// # Returns
    ///
    /// A `Result` containing the new `TransactionRepoModel` or a `TransactionError`.
    async fn replace_transaction(
        &self,
        old_tx: TransactionRepoModel,
        new_tx_request: NetworkTransactionRequest,
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
    Evm(Box<DefaultEvmTransaction>),
    Solana(SolanaRelayerTransaction),
    Stellar(DefaultStellarTransaction),
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
    /// Resubmits a transaction with updated parameters based on the network type.
    ///
    /// # Arguments
    ///
    /// * `tx` - A `TransactionRepoModel` representing the transaction to be resubmitted.
    ///
    /// # Returns
    ///
    /// A `Result` containing the resubmitted `TransactionRepoModel` or a `TransactionError`.
    async fn resubmit_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        match self {
            NetworkTransaction::Evm(relayer) => relayer.resubmit_transaction(tx).await,
            NetworkTransaction::Solana(relayer) => relayer.resubmit_transaction(tx).await,
            NetworkTransaction::Stellar(relayer) => relayer.resubmit_transaction(tx).await,
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
            NetworkTransaction::Solana(_) => solana_not_supported_transaction(),
            NetworkTransaction::Stellar(relayer) => relayer.cancel_transaction(tx).await,
        }
    }

    /// Replaces a transaction with a new one based on the network type.
    ///
    /// # Arguments
    ///
    /// * `old_tx` - A `TransactionRepoModel` representing the transaction to be replaced.
    /// * `new_tx_request` - A `NetworkTransactionRequest` representing the new transaction data.
    ///
    /// # Returns
    ///
    /// A `Result` containing the new `TransactionRepoModel` or a `TransactionError`.
    async fn replace_transaction(
        &self,
        old_tx: TransactionRepoModel,
        new_tx_request: NetworkTransactionRequest,
    ) -> Result<TransactionRepoModel, TransactionError> {
        match self {
            NetworkTransaction::Evm(relayer) => {
                relayer.replace_transaction(old_tx, new_tx_request).await
            }
            NetworkTransaction::Solana(_) => solana_not_supported_transaction(),
            NetworkTransaction::Stellar(relayer) => {
                relayer.replace_transaction(old_tx, new_tx_request).await
            }
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
    /// * `transaction_repository` - An `Arc` to the `TransactionRepositoryStorage`.
    /// * `job_producer` - An `Arc` to the `JobProducer`.
    ///
    /// # Returns
    ///
    /// A `Result` containing the created `NetworkTransaction` or a `TransactionError`.
    fn create_transaction(
        relayer: RelayerRepoModel,
        relayer_repository: Arc<RelayerRepositoryStorage>,
        transaction_repository: Arc<TransactionRepositoryStorage>,
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
    pub async fn create_transaction(
        relayer: RelayerRepoModel,
        signer: SignerRepoModel,
        relayer_repository: Arc<RelayerRepositoryStorage>,
        network_repository: Arc<NetworkRepositoryStorage>,
        transaction_repository: Arc<TransactionRepositoryStorage>,
        transaction_counter_store: Arc<TransactionCounterRepositoryStorage>,
        job_producer: Arc<JobProducer>,
    ) -> Result<NetworkTransaction, TransactionError> {
        match relayer.network_type {
            NetworkType::Evm => {
                let network_repo = network_repository
                    .get_by_name(NetworkType::Evm, &relayer.network)
                    .await
                    .ok()
                    .flatten()
                    .ok_or_else(|| {
                        TransactionError::NetworkConfiguration(format!(
                            "Network {} not found",
                            relayer.network
                        ))
                    })?;

                let network = EvmNetwork::try_from(network_repo)
                    .map_err(|e| TransactionError::NetworkConfiguration(e.to_string()))?;

                let evm_provider = get_network_provider(&network, relayer.custom_rpc_urls.clone())?;
                let signer_service = EvmSignerFactory::create_evm_signer(signer.into()).await?;
                let network_extra_fee_calculator =
                    get_network_extra_fee_calculator_service(network.clone(), evm_provider.clone());
                let price_calculator = evm::PriceCalculator::new(
                    EvmGasPriceService::new(evm_provider.clone(), network),
                    network_extra_fee_calculator,
                );

                Ok(NetworkTransaction::Evm(Box::new(
                    DefaultEvmTransaction::new(
                        relayer,
                        evm_provider,
                        relayer_repository,
                        network_repository,
                        transaction_repository,
                        transaction_counter_store,
                        job_producer,
                        price_calculator,
                        signer_service,
                    )?,
                )))
            }
            NetworkType::Solana => {
                let network_repo = network_repository
                    .get_by_name(NetworkType::Solana, &relayer.network)
                    .await
                    .ok()
                    .flatten()
                    .ok_or_else(|| {
                        TransactionError::NetworkConfiguration(format!(
                            "Network {} not found",
                            relayer.network
                        ))
                    })?;

                let network = SolanaNetwork::try_from(network_repo)
                    .map_err(|e| TransactionError::NetworkConfiguration(e.to_string()))?;

                let solana_provider = Arc::new(get_network_provider(
                    &network,
                    relayer.custom_rpc_urls.clone(),
                )?);

                Ok(NetworkTransaction::Solana(SolanaRelayerTransaction::new(
                    relayer,
                    relayer_repository,
                    solana_provider,
                    transaction_repository,
                    job_producer,
                )?))
            }
            NetworkType::Stellar => {
                let signer_service =
                    Arc::new(StellarSignerFactory::create_stellar_signer(&signer.into())?);

                let network_repo = network_repository
                    .get_by_name(NetworkType::Stellar, &relayer.network)
                    .await
                    .ok()
                    .flatten()
                    .ok_or_else(|| {
                        TransactionError::NetworkConfiguration(format!(
                            "Network {} not found",
                            relayer.network
                        ))
                    })?;

                let network = StellarNetwork::try_from(network_repo)
                    .map_err(|e| TransactionError::NetworkConfiguration(e.to_string()))?;

                let stellar_provider =
                    get_network_provider(&network, relayer.custom_rpc_urls.clone())
                        .map_err(|e| TransactionError::NetworkConfiguration(e.to_string()))?;

                Ok(NetworkTransaction::Stellar(DefaultStellarTransaction::new(
                    relayer,
                    relayer_repository,
                    transaction_repository,
                    job_producer,
                    signer_service,
                    stellar_provider,
                    transaction_counter_store,
                )?))
            }
        }
    }
}
