// use alloy::primitives::Address;
// use async_trait::async_trait;
// use eyre::Result;
// use thiserror::Error;

// #[derive(Error, Debug)]
// pub enum SignerError {
//     #[error("Failed to sign transaction: {0}")]
//     SigningError(String),

//     #[error("Invalid key format: {0}")]
//     KeyError(String),

//     #[error("Provider error: {0}")]
//     ProviderError(String),
// }

// #[async_trait]
// pub trait Signer: Send + Sync {
//     /// Returns the signer's ethereum address
//     async fn address(&self) -> Result<Address, SignerError>;

//     /// Signs an Ethereum transaction
//     async fn sign_transaction(
//         &self,
//         transaction: TransactionRequest,
//     ) -> Result<Vec<u8>, SignerError>;
// }

// // Example local implementation
// pub struct LocalSigner {
//     private_key: SecretKey,
// }

// #[async_trait]
// impl Signer for LocalSigner {
//     async fn address(&self) -> Result<Address, SignerError> {
//         // Implementation
//         todo!()
//     }

//     async fn sign_transaction(
//         &self,
//         transaction: TransactionRequest,
//     ) -> Result<Vec<u8>, SignerError> {
//         // Implementation
//         todo!()
//     }
// }
