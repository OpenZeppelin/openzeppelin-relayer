//! This module defines the `AppState` struct, which encapsulates the application's state.
//! It includes various repositories and services necessary for the application's operation.
//! The `AppState` provides methods to access these components in a thread-safe manner.
use std::sync::Arc;

use crate::{
    jobs::{JobProducer, JobProducerTrait},
    models::TransactionRepoModel,
    repositories::{
        InMemoryNetworkRepository, InMemoryNotificationRepository, InMemoryPluginRepository,
        InMemorySignerRepository, InMemoryTransactionCounter, RelayerRepositoryImpl, Repository,
        TransactionRepository, TransactionRepositoryImpl,
    },
};

/// Represents the application state, holding various repositories and services
/// required for the application's operation.
#[derive(Clone, Debug)]
pub struct AppState<
    J: JobProducerTrait,
    T: TransactionRepository + Repository<TransactionRepoModel, String>,
> {
    /// Repository for managing relayer data.
    pub relayer_repository: Arc<RelayerRepositoryImpl>,
    /// Repository for managing transaction data.
    pub transaction_repository: Arc<T>,
    /// Repository for managing signer data.
    pub signer_repository: Arc<InMemorySignerRepository>,
    /// Repository for managing notification data.
    pub notification_repository: Arc<InMemoryNotificationRepository>,
    /// Repository for managing network data.
    pub network_repository: Arc<InMemoryNetworkRepository>,
    /// Store for managing transaction counters.
    pub transaction_counter_store: Arc<InMemoryTransactionCounter>,
    /// Producer for managing job creation and execution.
    pub job_producer: Arc<J>,
    /// Repository for managing plugins.
    pub plugin_repository: Arc<InMemoryPluginRepository>,
}

pub type DefaultAppState = AppState<JobProducer, TransactionRepositoryImpl>;

impl<J: JobProducerTrait, T: TransactionRepository + Repository<TransactionRepoModel, String>>
    AppState<J, T>
{
    /// Returns a clone of the relayer repository.
    ///
    /// # Returns
    ///
    /// An `Arc` pointing to the `RelayerRepositoryImpl`.
    pub fn relayer_repository(&self) -> Arc<RelayerRepositoryImpl> {
        self.relayer_repository.clone()
    }

    /// Returns a clone of the transaction repository.
    ///
    /// # Returns
    ///
    /// An `Arc` pointing to the `Arc<TransactionRepositoryImpl>`.
    pub fn transaction_repository(&self) -> Arc<T> {
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

    /// Returns a clone of the network repository.
    ///
    /// # Returns
    ///
    /// An `Arc` pointing to the `InMemoryNetworkRepository`.
    pub fn network_repository(&self) -> Arc<InMemoryNetworkRepository> {
        Arc::clone(&self.network_repository)
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
    pub fn job_producer(&self) -> Arc<J> {
        Arc::clone(&self.job_producer)
    }

    /// Returns a clone of the plugin repository.
    ///
    /// # Returns
    ///
    /// An `Arc` pointing to the `InMemoryPluginRepository`.
    pub fn plugin_repository(&self) -> Arc<InMemoryPluginRepository> {
        Arc::clone(&self.plugin_repository)
    }
}

#[cfg(test)]
mod tests {
    use crate::{jobs::MockJobProducerTrait, repositories::InMemoryTransactionRepository};

    use super::*;
    use std::sync::Arc;

    fn create_test_app_state() -> AppState<MockJobProducerTrait, InMemoryTransactionRepository> {
        // Create a mock job producer
        let mut mock_job_producer = MockJobProducerTrait::new();

        // Set up expectations for the mock
        mock_job_producer
            .expect_produce_transaction_request_job()
            .returning(|_, _| Box::pin(async { Ok(()) }));

        mock_job_producer
            .expect_produce_submit_transaction_job()
            .returning(|_, _| Box::pin(async { Ok(()) }));

        mock_job_producer
            .expect_produce_check_transaction_status_job()
            .returning(|_, _| Box::pin(async { Ok(()) }));

        mock_job_producer
            .expect_produce_send_notification_job()
            .returning(|_, _| Box::pin(async { Ok(()) }));

        AppState {
            relayer_repository: Arc::new(RelayerRepositoryImpl::default()),
            transaction_repository: Arc::new(InMemoryTransactionRepository::new()),
            signer_repository: Arc::new(InMemorySignerRepository::default()),
            notification_repository: Arc::new(InMemoryNotificationRepository::default()),
            network_repository: Arc::new(InMemoryNetworkRepository::default()),
            transaction_counter_store: Arc::new(InMemoryTransactionCounter::default()),
            job_producer: Arc::new(mock_job_producer),
            plugin_repository: Arc::new(InMemoryPluginRepository::new()),
        }
    }

    #[test]
    fn test_relayer_repository_getter() {
        let app_state = create_test_app_state();
        let repo1 = app_state.relayer_repository();
        let repo2 = app_state.relayer_repository();

        // Verify that we get a new Arc pointing to the same underlying data
        assert!(Arc::ptr_eq(&repo1, &repo2));
        assert!(Arc::ptr_eq(&repo1, &app_state.relayer_repository));
    }

    #[test]
    fn test_transaction_repository_getter() {
        let app_state = create_test_app_state();
        let repo1 = app_state.transaction_repository();
        let repo2 = app_state.transaction_repository();

        assert!(Arc::ptr_eq(&repo1, &repo2));
        assert!(Arc::ptr_eq(&repo1, &app_state.transaction_repository));
    }

    #[test]
    fn test_signer_repository_getter() {
        let app_state = create_test_app_state();
        let repo1 = app_state.signer_repository();
        let repo2 = app_state.signer_repository();

        assert!(Arc::ptr_eq(&repo1, &repo2));
        assert!(Arc::ptr_eq(&repo1, &app_state.signer_repository));
    }

    #[test]
    fn test_notification_repository_getter() {
        let app_state = create_test_app_state();
        let repo1 = app_state.notification_repository();
        let repo2 = app_state.notification_repository();

        assert!(Arc::ptr_eq(&repo1, &repo2));
        assert!(Arc::ptr_eq(&repo1, &app_state.notification_repository));
    }

    #[test]
    fn test_transaction_counter_store_getter() {
        let app_state = create_test_app_state();
        let store1 = app_state.transaction_counter_store();
        let store2 = app_state.transaction_counter_store();

        assert!(Arc::ptr_eq(&store1, &store2));
        assert!(Arc::ptr_eq(&store1, &app_state.transaction_counter_store));
    }

    #[test]
    fn test_job_producer_getter() {
        let app_state = create_test_app_state();
        let producer1 = app_state.job_producer();
        let producer2 = app_state.job_producer();

        assert!(Arc::ptr_eq(&producer1, &producer2));
        assert!(Arc::ptr_eq(&producer1, &app_state.job_producer));
    }

    #[test]
    fn test_plugin_repository_getter() {
        let app_state = create_test_app_state();
        let store1 = app_state.plugin_repository();
        let store2 = app_state.plugin_repository();

        assert!(Arc::ptr_eq(&store1, &store2));
        assert!(Arc::ptr_eq(&store1, &app_state.plugin_repository));
    }
}
