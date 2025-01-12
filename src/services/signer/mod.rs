use alloy::rpc::types::TransactionRequest;
use async_trait::async_trait;
use eyre::Result;

mod evm;
pub use evm::*;

#[async_trait]
pub trait SignerService: Send + Sync {
    async fn sign_transaction(&self, tx: TransactionRequest) -> Result<Vec<u8>>;
}
