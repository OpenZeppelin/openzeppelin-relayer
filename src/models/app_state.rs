//! This module defines the `AppState` struct, which encapsulates the application's state.
//! It includes various repositories and services necessary for the application's operation.
//! The `AppState` provides methods to access these components in a thread-safe manner.
use std::sync::Arc;

use crate::{
    jobs::JobProducer,
    repositories::{
        InMemoryNotificationRepository, InMemoryRelayerRepository, InMemorySignerRepository,
        InMemoryTransactionCounter, InMemoryTransactionRepository, RelayerRepositoryStorage,
    },
};

/// Represents the application state, holding various repositories and services
/// required for the application's operation.
#[derive(Clone, Debug)]
pub struct AppState {
    /// Repository for managing relayer data.
    pub relayer_repository: Arc<RelayerRepositoryStorage<InMemoryRelayerRepository>>,
    /// Repository for managing transaction data.
    pub transaction_repository: Arc<InMemoryTransactionRepository>,
    /// Repository for managing signer data.
    pub signer_repository: Arc<InMemorySignerRepository>,
    /// Repository for managing notification data.
    pub notification_repository: Arc<InMemoryNotificationRepository>,
    /// Store for managing transaction counters.
    pub transaction_counter_store: Arc<InMemoryTransactionCounter>,
    /// Producer for managing job creation and execution.
    pub job_producer: Arc<JobProducer>,
}

impl AppState {
    /// Returns a clone of the relayer repository.
    ///
    /// # Returns
    ///
    /// An `Arc` pointing to the `RelayerRepositoryStorage`.
    pub fn relayer_repository(&self) -> Arc<RelayerRepositoryStorage<InMemoryRelayerRepository>> {
        Arc::clone(&self.relayer_repository)
    }

    /// Returns a clone of the transaction repository.
    ///
    /// # Returns
    ///
    /// An `Arc` pointing to the `InMemoryTransactionRepository`.
    pub fn transaction_repository(&self) -> Arc<InMemoryTransactionRepository> {
        Arc::clone(&self.transaction_repository)
    }

    /// Returns a clone of the signer repository.
    ///
    /// # Returns
    ///
    /// An `Arc` pointing to the `InMemorySignerRepository`.
    pub fn signer_repository(&self) -> Arc<InMemorySignerRepository> {
        Arc::clone(&self.signer_repository)
    }

    /// Returns a clone of the notification repository.
    ///
    /// # Returns
    ///
    /// An `Arc` pointing to the `InMemoryNotificationRepository`.
    pub fn notification_repository(&self) -> Arc<InMemoryNotificationRepository> {
        Arc::clone(&self.notification_repository)
    }

    /// Returns a clone of the transaction counter store.
    ///
    /// # Returns
    ///
    /// An `Arc` pointing to the `InMemoryTransactionCounter`.
    pub fn transaction_counter_store(&self) -> Arc<InMemoryTransactionCounter> {
        Arc::clone(&self.transaction_counter_store)
    }

    /// Returns a clone of the job producer.
    ///
    /// # Returns
    ///
    /// An `Arc` pointing to the `JobProducer`.
    pub fn job_producer(&self) -> Arc<JobProducer> {
        Arc::clone(&self.job_producer)
    }
}
