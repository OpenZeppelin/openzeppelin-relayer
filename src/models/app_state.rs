//! This module defines the `AppState` struct, which encapsulates the application's state.
//! It includes various repositories and services necessary for the application's operation.
//! The `AppState` provides methods to access these components in a thread-safe manner.
use std::sync::Arc;

use actix_web::web::ThinData;

use crate::{
    jobs::{JobProducer, JobProducerTrait},
    models::{
        NetworkRepoModel, NotificationRepoModel, RelayerRepoModel, SignerRepoModel,
        TransactionRepoModel,
    },
    repositories::{
        NetworkRepository, NetworkRepositoryStorage, NotificationRepositoryStorage,
        PluginRepositoryStorage, PluginRepositoryTrait, RelayerRepository,
        RelayerRepositoryStorage, Repository, SignerRepositoryStorage,
        TransactionCounterRepositoryStorage, TransactionCounterTrait, TransactionRepository,
        TransactionRepositoryStorage,
    },
};

/// Represents the application state, holding various repositories and services
/// required for the application's operation.
#[derive(Clone, Debug)]
pub struct AppState<
    J: JobProducerTrait + Send + Sync + 'static,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
    SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PR: PluginRepositoryTrait + Send + Sync + 'static,
> {
    /// Repository for managing relayer data.
    pub relayer_repository: Arc<RR>,
    /// Repository for managing transaction data.
    pub transaction_repository: Arc<TR>,
    /// Repository for managing signer data.
    pub signer_repository: Arc<SR>,
    /// Repository for managing notification data.
    pub notification_repository: Arc<NFR>,
    /// Repository for managing network data.
    pub network_repository: Arc<NR>,
    /// Store for managing transaction counters.
    pub transaction_counter_store: Arc<TCR>,
    /// Producer for managing job creation and execution.
    pub job_producer: Arc<J>,
    /// Repository for managing plugins.
    pub plugin_repository: Arc<PR>,
}

/// type alias for the app state wrapped in a ThinData to avoid clippy warnings
pub type ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR> =
    ThinData<AppState<J, RR, TR, NR, NFR, SR, TCR, PR>>;

pub type DefaultAppState = AppState<
    JobProducer,
    RelayerRepositoryStorage,
    TransactionRepositoryStorage,
    NetworkRepositoryStorage,
    NotificationRepositoryStorage,
    SignerRepositoryStorage,
    TransactionCounterRepositoryStorage,
    PluginRepositoryStorage,
>;

impl<
        J: JobProducerTrait,
        RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
        TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
        NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
        NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
        SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
        TCR: TransactionCounterTrait + Send + Sync + 'static,
        PR: PluginRepositoryTrait + Send + Sync + 'static,
    > AppState<J, RR, TR, NR, NFR, SR, TCR, PR>
{
    /// Returns a clone of the relayer repository.
    ///
    /// # Returns
    ///
    /// An `Arc` pointing to the `RelayerRepositoryStorage`.
    pub fn relayer_repository(&self) -> Arc<RR> {
        self.relayer_repository.clone()
    }

    /// Returns a clone of the transaction repository.
    ///
    /// # Returns
    ///
    /// An `Arc` pointing to the `Arc<TransactionRepositoryStorage>`.
    pub fn transaction_repository(&self) -> Arc<TR> {
        Arc::clone(&self.transaction_repository)
    }

    /// Returns a clone of the signer repository.
    ///
    /// # Returns
    ///
    /// An `Arc` pointing to the `InMemorySignerRepository`.
    pub fn signer_repository(&self) -> Arc<SR> {
        Arc::clone(&self.signer_repository)
    }

    /// Returns a clone of the notification repository.
    ///
    /// # Returns
    ///
    /// An `Arc` pointing to the `InMemoryNotificationRepository`.
    pub fn notification_repository(&self) -> Arc<NFR> {
        Arc::clone(&self.notification_repository)
    }

    /// Returns a clone of the network repository.
    ///
    /// # Returns
    ///
    /// An `Arc` pointing to the `InMemoryNetworkRepository`.
    pub fn network_repository(&self) -> Arc<NR> {
        Arc::clone(&self.network_repository)
    }

    /// Returns a clone of the transaction counter store.
    ///
    /// # Returns
    ///
    /// An `Arc` pointing to the `InMemoryTransactionCounter`.
    pub fn transaction_counter_store(&self) -> Arc<TCR> {
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
    pub fn plugin_repository(&self) -> Arc<PR> {
        Arc::clone(&self.plugin_repository)
    }
}

#[cfg(test)]
mod tests {
    use crate::{jobs::MockJobProducerTrait, repositories::TransactionRepositoryStorage};

    use super::*;
    use std::sync::Arc;

    fn create_test_app_state() -> AppState<
        MockJobProducerTrait,
        RelayerRepositoryStorage,
        TransactionRepositoryStorage,
        NetworkRepositoryStorage,
        NotificationRepositoryStorage,
        SignerRepositoryStorage,
        TransactionCounterRepositoryStorage,
        PluginRepositoryStorage,
    > {
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
            relayer_repository: Arc::new(RelayerRepositoryStorage::new_in_memory()),
            transaction_repository: Arc::new(TransactionRepositoryStorage::new_in_memory()),
            signer_repository: Arc::new(SignerRepositoryStorage::new_in_memory()),
            notification_repository: Arc::new(NotificationRepositoryStorage::new_in_memory()),
            network_repository: Arc::new(NetworkRepositoryStorage::new_in_memory()),
            transaction_counter_store: Arc::new(
                TransactionCounterRepositoryStorage::new_in_memory(),
            ),
            job_producer: Arc::new(mock_job_producer),
            plugin_repository: Arc::new(PluginRepositoryStorage::new_in_memory()),
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
