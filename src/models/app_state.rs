use std::sync::Arc;

use crate::{
    jobs::JobProducer,
    repositories::{
        InMemoryRelayerRepository, InMemorySignerRepository, InMemoryTransactionRepository,
    },
};

#[derive(Clone, Debug)]
pub struct AppState {
    pub relayer_repository: Arc<InMemoryRelayerRepository>,
    pub transaction_repository: Arc<InMemoryTransactionRepository>,
    pub signer_repository: Arc<InMemorySignerRepository>,
    pub job_producer: Arc<JobProducer>,
}

impl AppState {
    pub fn relayer_repository(&self) -> Arc<InMemoryRelayerRepository> {
        Arc::clone(&self.relayer_repository)
    }

    pub fn transaction_repository(&self) -> Arc<InMemoryTransactionRepository> {
        Arc::clone(&self.transaction_repository)
    }

    pub fn signer_repository(&self) -> Arc<InMemorySignerRepository> {
        Arc::clone(&self.signer_repository)
    }

    pub fn job_producer(&self) -> Arc<JobProducer> {
        Arc::clone(&self.job_producer)
    }
}
