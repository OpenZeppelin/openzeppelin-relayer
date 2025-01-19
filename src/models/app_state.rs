use std::sync::Arc;

use crate::{
    jobs::Queue,
    repositories::{InMemoryRelayerRepository, InMemoryTransactionRepository},
};

#[derive(Clone)]
pub struct AppState {
    pub relayer_repository: Arc<InMemoryRelayerRepository>,
    pub transaction_repository: Arc<InMemoryTransactionRepository>,
    pub queue: Option<Arc<Queue>>,
}

impl AppState {
    pub fn relayer_repository(&self) -> Arc<InMemoryRelayerRepository> {
        self.relayer_repository.clone()
    }

    pub fn transaction_repository(&self) -> Arc<InMemoryTransactionRepository> {
        self.transaction_repository.clone()
    }

    pub fn queue(&self) -> Arc<Queue> {
        self.queue.clone().expect("Queue not set")
    }

    pub fn with_queue(&mut self, queue: Queue) -> Self {
        self.queue = Some(Arc::new(queue));
        self.clone()
    }
}
