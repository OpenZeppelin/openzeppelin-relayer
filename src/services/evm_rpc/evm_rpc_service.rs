// use alloy::{
//     primitives::U256,
//     providers::{Provider, ProviderBuilder},
//     rpc::client::{ClientBuilder, RpcClient},
//     transports::http::Http,
// };
// use serde::{Deserialize, Serialize};

// use crate::models::RelayerError;

// #[derive(Debug, Serialize, Deserialize, Clone)]
// pub enum RpcError {
//     ProviderError(String),
//     InvalidAddress(String),
//     NetworkError(String),
// }

// #[derive(Debug)]
// pub struct RpcConfig {
//     pub urls: Vec<String>,
//     pub retry_attempts: u32,
//     pub timeout_secs: u64,
// }

// #[derive(Debug, Clone)]
// pub struct EVMRpcService {
//     primary_client: RpcClient<Http>,
//     fallback_clients: Vec<RpcClient<Http>>,
//     retry_attempts: u32,
//     network: String,
// }

// impl EVMRpcService {
//     pub fn new(url: &str) -> Result<Self, RpcError> {
//         let provider = ProviderBuilder::new().on_http(url.parse().unwrap());

//         Ok(EVMRpcService {
//             provider,
//             network: url.to_string(),
//         })
//     }

//     // pub fn new(urls: &[String]) -> Result<Self, RelayerError> {
//     //     if urls.is_empty() {
//     //         return Err(RpcError::ProviderError("No RPC URLs provided".into()));
//     //     }

//     //     let primary_url = urls[0].clone();
//     //     let fallback_urls: Vec<String> = urls[1..].to_vec();

//     //     let config = RpcConfig {
//     //         primary_url,
//     //         fallback_urls,
//     //         retry_count: 3,
//     //     };

//     //     Ok(Self {
//     //         config,
//     //         client: reqwest::Client::new(),
//     //     })
//     // }

//     pub fn new(urls: &[String]) -> Result<Self, RpcError> {
//         if urls.is_empty() {
//             return Err(RpcError::ProviderError("No RPC URLs provided".into()));
//         }

//         let primary_client = ClientBuilder::default().http(
//             urls[0]
//                 .parse()
//                 .map_err(|e| RpcError::ProviderError(e.to_string()))?,
//         );

//         let mut fallback_clients = Vec::new();
//         for url in &urls[1..] {
//             let client = ClientBuilder::default()
//                 .http(url)
//                 .build(Http::new(
//                     url.parse()
//                         .map_err(|e| RpcError::ProviderError(e.to_string()))?,
//                 ))
//                 .map_err(|e| RpcError::ProviderError(e.to_string()))?;
//             fallback_clients.push(client);
//         }

//         Ok(Self {
//             primary_client,
//             fallback_clients,
//             retry_attempts: 3,
//         })
//     }

//     pub async fn get_block_number(&self) -> Result<U256, RpcError> {
//         self.provider
//             .get_block_number()
//             .await
//             .map(|block_number| U256::from(block_number)) // Convert u64 to U256
//             .map_err(|e| RpcError::ProviderError(e.to_string()))
//     }

//     pub async fn get_balance(&self, address: &str) -> Result<U256, RpcError> {
//         let address = address
//             .parse()
//             .map_err(|_| RpcError::InvalidAddress(address.to_string()))?;
//         self.provider
//             .get_balance(address)
//             .await
//             .map(|balance| U256::from(balance)) // Convert U256 to U256
//             .map_err(|e| RpcError::ProviderError(e.to_string()))
//     }

//     pub async fn get_transaction_count(&self, address: &str) -> Result<U256, RpcError> {
//         let address = address
//             .parse()
//             .map_err(|_| RpcError::InvalidAddress(address.to_string()))?;
//         self.provider
//             .get_transaction_count(address)
//             .await
//             .map(|count| U256::from(count)) // Convert U256 to U256
//             .map_err(|e| RpcError::ProviderError(e.to_string()))
//     }

//     pub async fn send_raw_transaction(&self, tx: &[u8]) -> Result<String, RpcError> {
//         self.provider
//             .send_raw_transaction(tx)
//             .await
//             .map(|pending_tx| pending_tx.tx_hash().to_string())
//             .map_err(|e| RpcError::ProviderError(e.to_string()))
//     }
// }
