use std::sync::Arc;

use super::{Relayer, RelayerError};
use crate::models::{
    NetworkTransactionRequest, RelayerRepoModel, SolanaNetwork, TransactionRepoModel,
};
use crate::AppState;
use async_trait::async_trait;
use log::info;

pub struct SolanaRelayer {
    relayer: RelayerRepoModel,
    network: SolanaNetwork,
    state: Arc<AppState>,
}

impl SolanaRelayer {
    pub fn new(relayer: RelayerRepoModel, state: Arc<AppState>) -> Result<Self, RelayerError> {
        let network = match SolanaNetwork::from_network_str(&relayer.network) {
            Ok(network) => network,
            Err(e) => return Err(RelayerError::NetworkInitError(e.to_string())),
        };

        Ok(Self {
            relayer,
            network,
            state,
        })
    }
}

#[async_trait]
impl Relayer for SolanaRelayer {
    async fn send_transaction(
        &self,
        network_transaction: NetworkTransactionRequest,
    ) -> Result<TransactionRepoModel, RelayerError> {
        let transaction = TransactionRepoModel::try_from((&network_transaction, &self.relayer))?;

        info!("Solana Sending transaction...");
        Ok(transaction)
    }

    async fn get_balance(&self) -> Result<bool, RelayerError> {
        println!("Solana get_balance...");
        Ok(true)
    }

    async fn get_nonce(&self) -> Result<bool, RelayerError> {
        println!("Solana get_nonce...");
        Ok(true)
    }

    async fn delete_pending_transactions(&self) -> Result<bool, RelayerError> {
        println!("Solana delete_pending_transactions...");
        Ok(true)
    }

    async fn cancel_transaction(&self) -> Result<bool, RelayerError> {
        println!("Solana cancel_transaction...");
        Ok(true)
    }

    async fn replace_transaction(&self) -> Result<bool, RelayerError> {
        println!("Solana replace_transaction...");
        Ok(true)
    }

    async fn sign_data(&self) -> Result<bool, RelayerError> {
        println!("Solana sign_data...");
        Ok(true)
    }

    async fn sign_typed_data(&self) -> Result<bool, RelayerError> {
        println!("Solana sign_typed_data...");
        Ok(true)
    }

    async fn rpc(&self) -> Result<bool, RelayerError> {
        println!("Solana rpc...");
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
    //     let relayer = SolanaRelayer::new(network, provider);
    //     assert!(!relayer.paused);
    // }
}
