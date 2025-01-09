use std::sync::Arc;

use crate::models::{EvmNetwork, NetworkTransactionRequest, RelayerError};
use crate::models::{NetworkType, RelayerRepoModel, TransactionRepoModel};

use crate::services::EvmProvider;
use crate::AppState;
use async_trait::async_trait;
use eyre::Result;

mod evm_relayer;
mod solana_relayer;
mod stellar_relayer;

pub use evm_relayer::*;
pub use solana_relayer::*;
pub use stellar_relayer::*;

#[async_trait]
pub trait Relayer {
    async fn send_transaction(
        &self,
        tx_request: NetworkTransactionRequest,
    ) -> Result<TransactionRepoModel, RelayerError>;
    async fn get_balance(&self) -> Result<bool, RelayerError>;
    async fn get_nonce(&self) -> Result<bool, RelayerError>;
    async fn delete_pending_transactions(&self) -> Result<bool, RelayerError>;
    async fn cancel_transaction(&self) -> Result<bool, RelayerError>;
    async fn replace_transaction(&self) -> Result<bool, RelayerError>;
    async fn sign_data(&self) -> Result<bool, RelayerError>;
    async fn sign_typed_data(&self) -> Result<bool, RelayerError>;
    async fn rpc(&self) -> Result<bool, RelayerError>;
}

pub trait RelayerModelFactoryTrait {
    fn create_relayer_model(
        model: RelayerRepoModel,
        state: Arc<AppState>,
    ) -> Result<Box<dyn Relayer>, RelayerError>;
}
pub struct RelayerModelFactory;

impl RelayerModelFactoryTrait for RelayerModelFactory {
    fn create_relayer_model(
        model: RelayerRepoModel,
        state: Arc<AppState>,
    ) -> Result<Box<dyn Relayer>, RelayerError> {
        match model.network_type {
            NetworkType::Evm => {
                let network = match EvmNetwork::from_network_str(&model.network) {
                    Ok(network) => network,
                    Err(e) => return Err(RelayerError::NetworkConfiguration(e.to_string())),
                };
                // use first url
                // let rpc = EVMRpcService::new(&network.public_rpc_urls() )?;
                let rpc_urls =
                    network
                        .public_rpc_urls()
                        .ok_or(RelayerError::NetworkConfiguration(
                            "No RPC URLs found".to_string(),
                        ))?;
                let evm_provider = EvmProvider::new(&rpc_urls[0]);
                let relayer = EvmRelayer::new(model, state.clone())?;
                Ok(Box::new(relayer) as Box<dyn Relayer>)
            }
            NetworkType::Solana => {
                let relayer = SolanaRelayer::new(model, state.clone())?;
                Ok(Box::new(relayer))
            }
            NetworkType::Stellar => {
                let relayer = StellarRelayer::new(model, state.clone())?;
                Ok(Box::new(relayer))
            }
        }
    }
}
