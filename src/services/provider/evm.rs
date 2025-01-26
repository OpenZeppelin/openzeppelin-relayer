// TODO improve and add missing methods
use alloy::{
    primitives::U256,
    providers::{Provider, ProviderBuilder, RootProvider},
    rpc::types::TransactionRequest,
    transports::http::{Client, Http},
};
use eyre::{eyre, Result};

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

    pub async fn estimate_gas(&self, tx: &TransactionRequest) -> Result<U256> {
        self.provider
            .estimate_gas(tx)
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
}
