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

pub enum NetworkTransaction {
    Evm(EvmRelayerTransaction),
    Solana(SolanaRelayerTransaction),
    Stellar(StellarRelayerTransaction),
}

#[async_trait]
impl Transaction for NetworkTransaction {
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

    async fn cancel_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        match self {
            NetworkTransaction::Evm(relayer) => relayer.cancel_transaction(tx).await,
            NetworkTransaction::Solana(relayer) => relayer.cancel_transaction(tx).await,
            NetworkTransaction::Stellar(relayer) => relayer.cancel_transaction(tx).await,
        }
    }

    async fn replace_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        match self {
            NetworkTransaction::Evm(relayer) => relayer.replace_transaction(tx).await,
            NetworkTransaction::Solana(relayer) => relayer.replace_transaction(tx).await,
            NetworkTransaction::Stellar(relayer) => relayer.replace_transaction(tx).await,
        }
    }

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

#[allow(dead_code)]
pub trait RelayerTransactionFactoryTrait {
    fn create_transaction(
        relayer: RelayerRepoModel,
        relayer_repository: Arc<InMemoryRelayerRepository>,
        transaction_repository: Arc<InMemoryTransactionRepository>,
    ) -> Result<NetworkTransaction, TransactionError>;
}
pub struct RelayerTransactionFactory;

#[allow(dead_code)]
impl RelayerTransactionFactory {
    pub fn create_transaction(
        relayer: RelayerRepoModel,
        relayer_repository: Arc<InMemoryRelayerRepository>,
        transaction_repository: Arc<InMemoryTransactionRepository>,
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

                Ok(NetworkTransaction::Evm(EvmRelayerTransaction::new(
                    relayer,
                    evm_provider,
                    relayer_repository,
                    transaction_repository,
                )?))
            }
            NetworkType::Solana => Ok(NetworkTransaction::Solana(SolanaRelayerTransaction::new(
                relayer,
                relayer_repository,
                transaction_repository,
            )?)),
            NetworkType::Stellar => {
                Ok(NetworkTransaction::Stellar(StellarRelayerTransaction::new(
                    relayer,
                    relayer_repository,
                    transaction_repository,
                )?))
            }
        }
    }
}
