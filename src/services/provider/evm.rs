// TODO improve and add missing methods
use alloy::{
    primitives::{TxKind, Uint, U256},
    providers::{Provider, ProviderBuilder, RootProvider},
    rpc::types::{TransactionInput, TransactionRequest},
    transports::http::{Client, Http},
};
use eyre::{eyre, Result};

use crate::models::{EvmTransactionData, TransactionError};

pub struct EvmProvider {
    provider: RootProvider<Http<Client>>,
}

#[allow(dead_code)]
impl EvmProvider {
    pub fn new(url: &str) -> Result<Self> {
        let rpc_url = url.parse()?;
        let provider = ProviderBuilder::new().on_http(rpc_url);
        Ok(Self { provider })
    }

    pub async fn get_balance(&self, address: &str) -> Result<U256> {
        let address = address.parse()?;
        self.provider
            .get_balance(address)
            .await
            .map_err(|e| eyre!("Failed to get balance: {}", e))
    }

    pub async fn get_block_number(&self) -> Result<u64> {
        self.provider
            .get_block_number()
            .await
            .map_err(|e| eyre!("Failed to get block number: {}", e))
    }

    pub async fn estimate_gas(&self, tx: &EvmTransactionData) -> Result<U256> {
        // transform the tx to a transaction request
        let transaction_request = TransactionRequest::try_from(tx)?;
        self.provider
            .estimate_gas(&transaction_request)
            .await
            .map(|gas| U256::from(gas))
            .map_err(|e| eyre!("Failed to estimate gas: {}", e))
    }

    pub async fn send_transaction(&self, tx: TransactionRequest) -> Result<String> {
        let pending_tx = self
            .provider
            .send_transaction(tx)
            .await
            .map_err(|e| eyre!("Failed to send transaction: {}", e))?;

        let tx_hash = pending_tx.tx_hash().to_string();
        Ok(tx_hash)
    }

    pub async fn send_raw_transaction(&self, tx: &[u8]) -> Result<String> {
        let pending_tx = self
            .provider
            .send_raw_transaction(tx)
            .await
            .map_err(|e| eyre!("Failed to send raw transaction: {}", e))?;

        let tx_hash = pending_tx.tx_hash().to_string();
        Ok(tx_hash)
    }

    pub async fn health_check(&self) -> Result<bool> {
        self.get_block_number()
            .await
            .map(|_| true)
            .map_err(|e| eyre!("Health check failed: {}", e))
    }

    pub async fn get_transaction_count(&self, address: &str) -> Result<u64> {
        let address = address.parse()?;
        let result = self
            .provider
            .get_transaction_count(address)
            .await
            .map_err(|e| eyre!("Health check failed: {}", e))?;

        Ok(result)
    }
}

impl TryFrom<&EvmTransactionData> for TransactionRequest {
    type Error = TransactionError;
    fn try_from(tx: &EvmTransactionData) -> Result<Self, Self::Error> {
        Ok(TransactionRequest {
            from: Some(tx.from.clone().parse().map_err(|_| {
                TransactionError::InvalidType("Invalid address format".to_string())
            })?),
            to: Some(TxKind::Call(
                tx.to
                    .clone()
                    .unwrap_or("".to_string())
                    .parse()
                    .map_err(|_| {
                        TransactionError::InvalidType("Invalid address format".to_string())
                    })?,
            )),
            gas_price: Some(
                Uint::<256, 4>::from(tx.gas_price)
                    .try_into()
                    .map_err(|_| TransactionError::InvalidType("Invalid gas price".to_string()))?,
            ),
            gas: Some(
                Uint::<256, 4>::from(tx.gas_limit)
                    .try_into()
                    .map_err(|_| TransactionError::InvalidType("Invalid gas limit".to_string()))?,
            ),
            value: Some(Uint::<256, 4>::from(tx.value)),
            input: TransactionInput::from(tx.data.clone().unwrap_or("".to_string()).into_bytes()),
            nonce: Some(
                Uint::<256, 4>::from(tx.nonce)
                    .try_into()
                    .map_err(|_| TransactionError::InvalidType("Invalid nonce".to_string()))?,
            ),
            chain_id: Some(11155111),
            ..Default::default()
        })
    }
}
