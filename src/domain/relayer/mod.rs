use std::sync::Arc;

use crate::models::{
    EvmNetwork, NetworkTransactionRequest, NetworkType, RelayerError, RelayerRepoModel,
    TransactionRepoModel,
};

use crate::{
    repositories::{InMemoryRelayerRepository, InMemoryTransactionRepository},
    services::EvmProvider,
};
use async_trait::async_trait;
use eyre::Result;

mod evm;
mod solana;
mod stellar;

pub use evm::*;
pub use solana::*;
pub use stellar::*;

#[async_trait]
#[allow(dead_code)]
pub trait Relayer {
    async fn send_transaction(
        &self,
        tx_request: NetworkTransactionRequest,
    ) -> Result<TransactionRepoModel, RelayerError>;
    async fn get_balance(&self) -> Result<bool, RelayerError>;
    async fn get_nonce(&self) -> Result<bool, RelayerError>;
    async fn delete_pending_transactions(&self) -> Result<bool, RelayerError>;
    async fn sign_data(&self) -> Result<bool, RelayerError>;
    async fn sign_typed_data(&self) -> Result<bool, RelayerError>;
    async fn rpc(&self) -> Result<bool, RelayerError>;
    async fn get_status(&self) -> Result<bool, RelayerError>;
}

pub trait RelayerFactoryTrait {
    fn create_relayer(
        model: RelayerRepoModel,
        relayer_repository: Arc<InMemoryRelayerRepository>,
        transaction_repository: Arc<InMemoryTransactionRepository>,
    ) -> Result<Box<dyn Relayer>, RelayerError>;
}
pub struct RelayerFactory;

impl RelayerFactoryTrait for RelayerFactory {
    fn create_relayer(
        relayer: RelayerRepoModel,
        relayer_repository: Arc<InMemoryRelayerRepository>,
        transaction_repository: Arc<InMemoryTransactionRepository>,
    ) -> Result<Box<dyn Relayer>, RelayerError> {
        match relayer.network_type {
            NetworkType::Evm => {
                let network = match EvmNetwork::from_network_str(&relayer.network) {
                    Ok(network) => network,
                    Err(e) => return Err(RelayerError::NetworkConfiguration(e.to_string())),
                };
                let rpc_url = network
                    .public_rpc_urls()
                    .and_then(|urls| urls.first().cloned())
                    .ok_or_else(|| {
                        RelayerError::NetworkConfiguration("No RPC URLs configured".to_string())
                    })?;
                let evm_provider: EvmProvider = EvmProvider::new(rpc_url).unwrap();
                let relayer = EvmRelayer::new(
                    relayer,
                    evm_provider,
                    network,
                    relayer_repository,
                    transaction_repository,
                )?;

                Ok(Box::new(relayer) as Box<dyn Relayer>)
            }
            NetworkType::Solana => {
                let relayer =
                    SolanaRelayer::new(relayer, relayer_repository, transaction_repository)?;
                Ok(Box::new(relayer))
            }
            NetworkType::Stellar => {
                let relayer =
                    StellarRelayer::new(relayer, relayer_repository, transaction_repository)?;
                Ok(Box::new(relayer))
            }
        }
    }
}
