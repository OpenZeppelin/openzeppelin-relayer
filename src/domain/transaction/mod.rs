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
        InMemoryRelayerRepository, InMemoryTransactionCounter, InMemoryTransactionRepository,
        RelayerRepositoryStorage,
    },
    services::{
        get_solana_network_provider_from_str, EvmGasPriceService, EvmProvider, EvmSignerFactory,
    },
};
use async_trait::async_trait;
use eyre::Result;
#[cfg(test)]
use mockall::automock;
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
    Evm(DefaultEvmTransaction),
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
            NetworkTransaction::Solana(_) => solana_not_supported_transaction(),
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
        relayer_repository: Arc<RelayerRepositoryStorage<InMemoryRelayerRepository>>,
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
        relayer_repository: Arc<RelayerRepositoryStorage<InMemoryRelayerRepository>>,
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

                let signer_service = EvmSignerFactory::create_evm_signer(&signer)?;
                let price_calculator =
                    PriceCalculator::new(EvmGasPriceService::new(evm_provider.clone(), network));

                Ok(NetworkTransaction::Evm(DefaultEvmTransaction::new(
                    relayer,
                    evm_provider,
                    relayer_repository,
                    transaction_repository,
                    transaction_counter_store,
                    job_producer,
                    price_calculator,
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

#[cfg(test)]
mod tests {
    use super::*;
    use mockall::predicate::{self};
    // Import the actual types from your models module.
    use crate::models::{EvmTransactionData, NetworkTransactionData, TransactionStatus};
    use chrono::{Duration, Utc};

    fn create_test_transaction(status: TransactionStatus) -> TransactionRepoModel {
        TransactionRepoModel {
            id: "1".to_string(),
            relayer_id: "1".to_string(),
            status,
            created_at: Utc::now().to_rfc3339(),
            sent_at: Some(Utc::now().to_rfc3339()),
            confirmed_at: None,
            valid_until: Some((Utc::now() + Duration::days(1)).to_rfc3339()),
            network_type: NetworkType::Evm,
            network_data: NetworkTransactionData::Evm(EvmTransactionData::default()),
            priced_at: Some(Utc::now().to_rfc3339()),
            hashes: vec!["0xprevioushash".to_string()],
            noop_count: Some(0),
            is_canceled: Some(false),
        }
    }

    #[tokio::test]
    async fn test_automock_prepare_transaction() {
        // Create an instance of the generated mock.
        let mut mock = MockTransaction::new();

        // Create an instance of your actual TransactionRepoModel.
        let input_tx = create_test_transaction(TransactionStatus::Pending);

        // Clone the values for later assertions.
        let input_tx_clone = input_tx.clone();
        let id = input_tx.id.clone();
        let relayer_id = input_tx.relayer_id.clone();
        let status = input_tx.status.clone();
        let network_type = input_tx.network_type;

        mock.expect_prepare_transaction()
            .with(predicate::function(move |tx: &TransactionRepoModel| {
                tx.id == input_tx.id
                    && tx.relayer_id == input_tx.relayer_id
                    && tx.status == input_tx.status
                    && tx.network_type == input_tx.network_type
            }))
            .times(1)
            .returning(|tx| Ok(tx.clone()));

        let result = mock.prepare_transaction(input_tx_clone).await;
        let result_tx = result.unwrap();

        assert_eq!(result_tx.id, id);
        assert_eq!(result_tx.relayer_id, relayer_id);
        assert_eq!(result_tx.status, status);
        assert_eq!(result_tx.network_type, network_type);
    }

    #[tokio::test]
    async fn test_automock_submit_transaction() {
        // Create an instance of the generated mock
        let mut mock = MockTransaction::new();

        // Create a test transaction
        let input_tx = create_test_transaction(TransactionStatus::Pending);

        // Clone values for assertions
        let input_tx_clone = input_tx.clone();
        let id = input_tx.id.clone();
        let relayer_id = input_tx.relayer_id.clone();
        let network_type = input_tx.network_type;

        // Set up mock expectations
        mock.expect_submit_transaction()
            .with(predicate::function(move |tx: &TransactionRepoModel| {
                tx.id == input_tx.id
                    && tx.relayer_id == input_tx.relayer_id
                    && tx.status == input_tx.status
                    && tx.network_type == input_tx.network_type
            }))
            .times(1)
            .returning(|mut tx| {
                // Simulate successful submission by updating status and sent_at
                tx.status = TransactionStatus::Submitted;
                tx.sent_at = Some(Utc::now().to_rfc3339());
                Ok(tx)
            });

        let result = mock.submit_transaction(input_tx_clone).await;
        let result_tx = result.unwrap();

        // Verify the results
        assert_eq!(result_tx.id, id);
        assert_eq!(result_tx.relayer_id, relayer_id);
        assert_eq!(result_tx.status, TransactionStatus::Submitted);
        assert_eq!(result_tx.network_type, network_type);
        assert!(result_tx.sent_at.is_some());
    }

    #[tokio::test]
    async fn test_automock_resubmit_transaction() {
        // Create an instance of the generated mock
        let mut mock = MockTransaction::new();

        // Create a test transaction
        let input_tx = create_test_transaction(TransactionStatus::Submitted);

        // Clone values for assertions
        let input_tx_clone = input_tx.clone();
        let id = input_tx.id.clone();
        let relayer_id = input_tx.relayer_id.clone();
        let network_type = input_tx.network_type;
        let original_hash = input_tx.hashes[0].clone();

        // Set up mock expectations
        mock.expect_resubmit_transaction()
            .with(predicate::function(move |tx: &TransactionRepoModel| {
                tx.id == input_tx.id
                    && tx.relayer_id == input_tx.relayer_id
                    && tx.status == input_tx.status
                    && tx.network_type == input_tx.network_type
            }))
            .times(1)
            .returning(|mut tx| {
                // Simulate successful resubmission by:
                // 1. Updating status back to Pending
                // 2. Adding a new hash to the hashes vector
                // 3. Incrementing noop_count
                tx.status = TransactionStatus::Pending;
                tx.hashes.push("0xnewhash".to_string());
                tx.noop_count = Some(tx.noop_count.unwrap_or(0) + 1);
                tx.sent_at = None;
                Ok(tx)
            });

        let result = mock.resubmit_transaction(input_tx_clone).await;
        let result_tx = result.unwrap();

        // Verify the results
        assert_eq!(result_tx.id, id);
        assert_eq!(result_tx.relayer_id, relayer_id);
        assert_eq!(result_tx.status, TransactionStatus::Pending);
        assert_eq!(result_tx.network_type, network_type);
        assert_eq!(result_tx.noop_count, Some(1));
        assert!(result_tx.sent_at.is_none());
        assert_eq!(result_tx.hashes.len(), 2);
        assert_eq!(result_tx.hashes[0], original_hash);
        assert_eq!(result_tx.hashes[1], "0xnewhash");
    }
}
