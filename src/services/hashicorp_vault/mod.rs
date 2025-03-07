// mod client;
// mod config;
// mod errors;

// use async_trait::async_trait;
// use client::VaultTransitClient;
// pub use config::VaultConfig;
// pub use errors::{VaultError, VaultResult};
// use std::sync::Arc;

// /// Service for interacting with HashiCorp Vault Transit Engine
// /// to perform cryptographic operations, particularly for signing transactions.
// #[derive(Clone)]
// pub struct HashiCorpVaultService {
//     client: Arc<VaultTransitClient>,
//     key_name: String,
// }

// impl HashiCorpVaultService {
//     /// Create a new HashiCorpVaultService instance with AppRole authentication
//     pub async fn new(config: VaultConfig) -> VaultResult<Self> {
//         let client = VaultTransitClient::new(
//             &config.vault_addr,
//             &config.role_id,
//             &config.secret_id,
//             config.ca_cert.as_deref(),
//         )
//         .await?;

//         Ok(Self {
//             client: Arc::new(client),
//             key_name: config.key_name,
//         })
//     }

//     /// Sign data using Vault's transit backend
//     pub async fn sign(&self, data: &[u8]) -> VaultResult<Vec<u8>> {
//         let signature = self.client.sign(&self.key_name, data).await?;
//         Ok(signature)
//     }

//     /// Verify a signature using Vault's transit backend
//     pub async fn verify(&self, data: &[u8], signature: &[u8]) -> VaultResult<bool> {
//         self.client.verify(&self.key_name, data, signature).await
//     }

//     /// Get the public key from Vault's transit backend
//     pub async fn get_public_key(&self) -> VaultResult<Vec<u8>> {
//         self.client.get_public_key(&self.key_name).await
//     }

//     /// Check if the key exists in Vault's transit backend
//     pub async fn key_exists(&self) -> VaultResult<bool> {
//         self.client.key_exists(&self.key_name).await
//     }

//     /// Renew the authentication token
//     pub async fn renew_token(&self) -> VaultResult<()> {
//         self.client.renew_token().await
//     }
// }

// /// Trait for services that can sign arbitrary data
// #[async_trait]
// pub trait SigningService: Send + Sync {
//     async fn sign(&self, data: &[u8]) -> VaultResult<Vec<u8>>;
//     async fn get_public_key(&self) -> VaultResult<Vec<u8>>;
// }

// #[async_trait]
// impl SigningService for HashiCorpVaultService {
//     async fn sign(&self, data: &[u8]) -> VaultResult<Vec<u8>> {
//         self.sign(data).await
//     }

//     async fn get_public_key(&self) -> VaultResult<Vec<u8>> {
//         self.get_public_key().await
//     }
// }
