use crate::{
    models::{EvmNetwork, NetworkType, RelayerRepoModel, TransactionError, TransactionRepoModel},
    repositories::{InMemoryRelayerRepository, InMemoryTransactionRepository},
    services::EvmProvider,
};
use async_trait::async_trait;
use eyre::Result;
use std::sync::Arc;

mod evm;
mod solana;
mod stellar;

pub use evm::*;
pub use solana::*;
pub use stellar::*;

#[async_trait]
#[allow(dead_code)]
pub trait Transaction {
    async fn submit_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError>;

    async fn handle_transaction_status(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError>;

    async fn cancel_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError>;

    async fn replace_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError>;

    async fn sign_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError>;

    async fn validate_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<bool, TransactionError>;
}

#[allow(dead_code)]
pub trait RelayerTransactionFactoryTrait {
    fn create_transaction(
        relayer: RelayerRepoModel,
        relayer_repository: Arc<InMemoryRelayerRepository>,
        transaction_repository: Arc<InMemoryTransactionRepository>,
    ) -> Result<Box<dyn Transaction>, TransactionError>;
}
pub struct RelayerTransactionFactory;

#[allow(dead_code)]
impl RelayerTransactionFactory {
    pub fn create_transaction(
        relayer: RelayerRepoModel,
        relayer_repository: Arc<InMemoryRelayerRepository>,
        transaction_repository: Arc<InMemoryTransactionRepository>,
    ) -> Result<Box<dyn Transaction>, TransactionError> {
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
                let evm_provider: EvmProvider = EvmProvider::new(rpc_url).unwrap();

                Ok(Box::new(EvmRelayerTransaction::new(
                    relayer,
                    evm_provider,
                    relayer_repository,
                    transaction_repository,
                )?))
            }
            NetworkType::Solana => Ok(Box::new(SolanaRelayerTransaction::new(
                relayer,
                relayer_repository,
                transaction_repository,
            )?)),
            NetworkType::Stellar => Ok(Box::new(StellarRelayerTransaction::new(
                relayer,
                relayer_repository,
                transaction_repository,
            )?)),
        }
    }
}
