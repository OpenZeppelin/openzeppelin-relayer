use std::sync::Arc;

use crate::repositories::{InMemoryRelayerRepository, InMemoryTransactionRepository};

#[derive(Clone)]
pub struct AppState {
    pub relayer_repository: Arc<InMemoryRelayerRepository>,
    pub transaction_repository: Arc<InMemoryTransactionRepository>,
}

impl AppState {
    pub fn relayer_repository(&self) -> Arc<InMemoryRelayerRepository> {
        self.relayer_repository.clone()
    }

    pub fn transaction_repository(&self) -> Arc<InMemoryTransactionRepository> {
        self.transaction_repository.clone()
    }
}
