use crate::models::{
    NetworkTransactionRequest, RelayerRepoModel, StellarNetwork, TransactionRepoModel,
};
use crate::AppState;
use async_trait::async_trait;
use eyre::Result;
use log::info;
use std::sync::Arc;

use super::{Relayer, RelayerError};

#[allow(dead_code)]
pub struct StellarRelayer {
    relayer: RelayerRepoModel,
    network: StellarNetwork,
    state: Arc<AppState>,
}

impl StellarRelayer {
    pub fn new(relayer: RelayerRepoModel, state: Arc<AppState>) -> Result<Self, RelayerError> {
        let network = match StellarNetwork::from_network_str(&relayer.network) {
            Ok(network) => network,
            Err(e) => return Err(RelayerError::NetworkConfiguration(e.to_string())),
        };
        Ok(Self {
            network,
            relayer,
            state,
        })
    }
}

#[async_trait]
impl Relayer for StellarRelayer {
    async fn send_transaction(
        &self,
        network_transaction: NetworkTransactionRequest,
    ) -> Result<TransactionRepoModel, RelayerError> {
        let transaction = TransactionRepoModel::try_from((&network_transaction, &self.relayer))?;

        info!("Stellar Sending transaction...");
        Ok(transaction)
    }

    async fn get_balance(&self) -> Result<bool, RelayerError> {
        println!("Stellar get_balance...");
        Ok(true)
    }

    async fn get_nonce(&self) -> Result<bool, RelayerError> {
        println!("Stellar get_nonce...");
        Ok(true)
    }

    async fn delete_pending_transactions(&self) -> Result<bool, RelayerError> {
        println!("Stellar delete_pending_transactions...");
        Ok(true)
    }

    async fn cancel_transaction(&self) -> Result<bool, RelayerError> {
        println!("Stellar cancel_transaction...");
        Ok(true)
    }

    async fn replace_transaction(&self) -> Result<bool, RelayerError> {
        println!("Stellar replace_transaction...");
        Ok(true)
    }

    async fn sign_data(&self) -> Result<bool, RelayerError> {
        println!("Stellar sign_data...");
        Ok(true)
    }

    async fn sign_typed_data(&self) -> Result<bool, RelayerError> {
        println!("Stellar sign_typed_data...");
        Ok(true)
    }

    async fn rpc(&self) -> Result<bool, RelayerError> {
        println!("Stellar rpc...");
        Ok(true)
    }
}
