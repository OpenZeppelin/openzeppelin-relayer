use crate::{
    jobs::JobProducer,
    models::{
        EvmNetwork, NetworkType, RelayerRepoModel, SignerRepoModel, TransactionError,
        TransactionRepoModel,
    },
    repositories::{
        InMemoryTransactionCounter, InMemoryTransactionRepository, RelayerRepositoryStorage,
    },
    services::{
        get_solana_network_provider_from_str, EvmProvider, EvmSignerFactory,
        TransactionCounterService,
    },
};
use async_trait::async_trait;
use eyre::Result;
use std::sync::Arc;

mod evm;
mod solana;
mod stellar;
mod util;

pub use evm::*;
pub use solana::*;
pub use stellar::*;
pub use util::*;

#[async_trait]
#[allow(dead_code)]
pub trait Transaction {
    async fn prepare_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError>;

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
            NetworkTransaction::Solana(_) => solana_not_supported(),
            NetworkTransaction::Stellar(relayer) => relayer.cancel_transaction(tx).await,
        }
    }

    async fn replace_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        match self {
            NetworkTransaction::Evm(relayer) => relayer.replace_transaction(tx).await,
            NetworkTransaction::Solana(_) => solana_not_supported(),
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
        relayer_repository: Arc<RelayerRepositoryStorage>,
        transaction_repository: Arc<InMemoryTransactionRepository>,
        job_producer: Arc<JobProducer>,
    ) -> Result<NetworkTransaction, TransactionError>;
}
pub struct RelayerTransactionFactory;

#[allow(dead_code)]
impl RelayerTransactionFactory {
    pub fn create_transaction(
        relayer: RelayerRepoModel,
        signer: SignerRepoModel,
        relayer_repository: Arc<RelayerRepositoryStorage>,
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
                let transaction_counter_service = TransactionCounterService::new(
                    relayer.id.clone(),
                    relayer.address.clone(),
                    transaction_counter_store,
                );
                let signer_service = EvmSignerFactory::create_evm_signer(&signer)?;

                Ok(NetworkTransaction::Evm(EvmRelayerTransaction::new(
                    relayer,
                    evm_provider,
                    relayer_repository,
                    transaction_repository,
                    transaction_counter_service,
                    job_producer,
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
