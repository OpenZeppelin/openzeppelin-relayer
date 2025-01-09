use crate::repositories::{InMemoryRelayerRepository, InMemoryTransactionRepository};

pub struct AppState {
    pub relayer_repository: InMemoryRelayerRepository,
    pub transaction_repository: InMemoryTransactionRepository,
}
