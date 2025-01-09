use std::sync::Arc;

use super::Relayer;
use crate::models::{EvmNetwork, NetworkTransactionRequest, RelayerError};
use crate::repositories::{RelayerRepoModel, TransactionRepoModel};
use crate::services::EvmProvider;
use crate::AppState;
use async_trait::async_trait;
use log::info;

pub struct EvmRelayer {
    relayer: RelayerRepoModel,
    network: EvmNetwork,
    state: Arc<AppState>,
    evm_provider: EvmProvider,
}

impl EvmRelayer {
    pub fn new(relayer: RelayerRepoModel, state: Arc<AppState>, evm_provider: EvmProvider) -> Result<Self, RelayerError> {
        let network = match EvmNetwork::from_network_str(&relayer.network) {
            Ok(network) => network,
            Err(e) => return Err(RelayerError::NetworkInitError(e.to_string())),
        };

        Ok(Self {
            relayer,
            network,
            state,
            evm_provider,
        })
    }
}

#[async_trait]
impl Relayer for EvmRelayer {
    async fn send_transaction(
        &self,
        network_transaction: NetworkTransactionRequest,
    ) -> Result<TransactionRepoModel, RelayerError> {
        // create
        let transaction = TransactionRepoModel::try_from((&network_transaction, &self.relayer))?;

        // send TODO
        info!("EVM Sending transaction...");
        Ok(transaction)
    }

    async fn get_balance(&self) -> Result<bool, RelayerError> {
        println!("EVM get_balance...");
        Ok(true)
    }

    async fn get_nonce(&self) -> Result<bool, RelayerError> {
        println!("EVM get_nonce...");
        Ok(true)
    }

    async fn delete_pending_transactions(&self) -> Result<bool, RelayerError> {
        println!("EVM delete_pending_transactions...");
        Ok(true)
    }

    async fn cancel_transaction(&self) -> Result<bool, RelayerError> {
        println!("EVM cancel_transaction...");
        Ok(true)
    }

    async fn replace_transaction(&self) -> Result<bool, RelayerError> {
        println!("EVM replace_transaction...");
        Ok(true)
    }

    async fn sign_data(&self) -> Result<bool, RelayerError> {
        println!("EVM sign_data...");
        Ok(true)
    }

    async fn sign_typed_data(&self) -> Result<bool, RelayerError> {
        println!("EVM sign_typed_data...");
        Ok(true)
    }

    async fn rpc(&self) -> Result<bool, RelayerError> {
        println!("EVM rpc...");
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    // use super::*;

    // #[test]
    // fn test_new_evm_relayer() {
    //     let network = EvmNetwork::from_named(EvmNamedNetwork::Mainnet);
    //     let provider = EvmProvider::new();
    //     let relayer = EvmRelayer::new(network, provider);
    //     assert!(!relayer.paused);
    // }
}
