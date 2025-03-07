// use super::errors::{VaultError, VaultResult};
// use base64::{engine::general_purpose, Engine};
// use log::{debug, error, info};
// use reqwest::{
//     header::{HeaderMap, HeaderValue, ACCEPT, CONTENT_TYPE},
//     Client, ClientBuilder,
// };
// use serde::{Deserialize, Serialize};
// use std::time::{Duration, Instant};

// const TOKEN_RENEWAL_INTERVAL: Duration = Duration::from_secs(3600); // 1 hour

// pub struct VaultTransitClient {
//     client: Client,
//     base_url: String,
//     auth_token: String,
//     token_expiry: Instant,
//     role_id: String,
//     secret_id: String,
// }

// #[derive(Serialize)]
// struct AppRoleAuthRequest {
//     role_id: String,
//     secret_id: String,
// }

// #[derive(Deserialize)]
// struct AuthResponse {
//     auth: Auth,
// }

// #[derive(Deserialize)]
// struct Auth {
//     client_token: String,
//     lease_duration: u64,
// }

// #[derive(Serialize)]
// struct SignRequest {
//     input: String,
//     hash_algorithm: String,
// }

// #[derive(Deserialize)]
// struct SignResponse {
//     data: SignData,
// }

// #[derive(Deserialize)]
// struct SignData {
//     signature: String,
// }

// #[derive(Deserialize)]
// struct VerifyResponse {
//     data: VerifyData,
// }

// #[derive(Deserialize)]
// struct VerifyData {
//     valid: bool,
// }

// #[derive(Serialize)]
// struct VerifyRequest {
//     input: String,
//     signature: String,
//     hash_algorithm: String,
// }

// #[derive(Deserialize)]
// struct PublicKeyResponse {
//     data: PublicKeyData,
// }

// #[derive(Deserialize)]
// struct PublicKeyData {
//     public_key: String,
// }

// #[derive(Deserialize)]
// struct KeysResponse {
//     data: KeysData,
// }

// #[derive(Deserialize)]
// struct KeysData {
//     keys: Vec<String>,
// }

// impl VaultTransitClient {
//     pub async fn new(
//         base_url: &str,
//         role_id: &str,
//         secret_id: &str,
//         ca_cert_path: Option<&str>,
//     ) -> VaultResult<Self> {
//         let mut client_builder = ClientBuilder::new()
//             .timeout(Duration::from_secs(30))
//             .pool_max_idle_per_host(10);

//         if let Some(ca_path) = ca_cert_path {
//             let cert_data = tokio::fs::read(ca_path)
//                 .await
//                 .map_err(|e| VaultError::Configuration(format!("Failed to read CA cert: {}", e)))?;

//             let cert = reqwest::Certificate::from_pem(&cert_data)
//                 .map_err(|e| VaultError::Configuration(format!("Invalid CA cert: {}", e)))?;

//             client_builder = client_builder.add_root_certificate(cert);
//         }

//         let client = client_builder.build().map_err(|e| {
//             VaultError::Configuration(format!("Failed to build HTTP client: {}", e))
//         })?;

//         let mut vault_client = Self {
//             client,
//             base_url: base_url.trim_end_matches('/').to_string(),
//             auth_token: String::new(),
//             token_expiry: Instant::now(),
//             role_id: role_id.to_string(),
//             secret_id: secret_id.to_string(),
//         };

//         // Authenticate and get initial token
//         vault_client.authenticate().await?;

//         Ok(vault_client)
//     }

//     async fn authenticate(&mut self) -> VaultResult<()> {
//         debug!("Authenticating with Vault using AppRole");

//         let auth_data = AppRoleAuthRequest {
//             role_id: self.role_id.clone(),
//             secret_id: self.secret_id.clone(),
//         };

//         let response = self
//             .client
//             .post(&format!("{}/v1/auth/approle/login", self.base_url))
//             .json(&auth_data)
//             .send()
//             .await
//             .map_err(|e| VaultError::Network(format!("Failed to send auth request: {}", e)))?;

//         if !response.status().is_success() {
//             let status = response.status();
//             let error_text = response.text().await.unwrap_or_default();
//             return Err(VaultError::Authentication(format!(
//                 "Authentication failed: {} - {}",
//                 status, error_text
//             )));
//         }

//         let auth_response: AuthResponse = response.json().await.map_err(|e| {
//             VaultError::ResponseParsing(format!("Failed to parse auth response: {}", e))
//         })?;

//         self.auth_token = auth_response.auth.client_token;

//         // Set token expiry to slightly less than the lease duration to ensure we renew before expiry
//         let lease_secs = auth_response.auth.lease_duration;
//         let renewal_secs = lease_secs.saturating_sub(60); // Renew 1 minute before expiry
//         self.token_expiry = Instant::now() + Duration::from_secs(renewal_secs);

//         info!(
//             "Successfully authenticated with Vault. Token valid for {} seconds",
//             lease_secs
//         );

//         Ok(())
//     }

//     pub async fn renew_token(&self) -> VaultResult<()> {
//         if Instant::now() >= self.token_expiry {
//             debug!("Renewing Vault authentication token");

//             let headers = self.build_auth_headers()?;

//             let response = self
//                 .client
//                 .post(&format!("{}/v1/auth/token/renew-self", self.base_url))
//                 .headers(headers)
//                 .send()
//                 .await
//                 .map_err(|e| VaultError::Network(format!("Failed to renew token: {}", e)))?;

//             if !response.status().is_success() {
//                 let status = response.status();
//                 let error_text = response.text().await.unwrap_or_default();
//                 return Err(VaultError::Authentication(format!(
//                     "Token renewal failed: {} - {}",
//                     status,
//                     error_text
//                 )));
//             }

//             info!("Successfully renewed Vault token");
//         }

//         Ok(())
//     }

//     fn build_auth_headers(&self) -> VaultResult<HeaderMap> {
//         let mut headers = HeaderMap::new();
//         headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
//         headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

//         let auth_header = format!("Bearer {}", self.auth_token);
//         headers.insert(
//             reqwest::header::AUTHORIZATION,
//             HeaderValue::from_str(&auth_header)
//                 .map_err(|e| VaultError::Configuration(format!("Invalid auth header: {}", e)))?,
//         );

//         Ok(headers)
//     }

//     async fn ensure_authenticated(&self) -> VaultResult<()> {
//         if Instant::now() >= self.token_expiry {
//             // Use clone to avoid borrowing self as mutable in this function
//             let mut client_clone = self.clone();
//             client_clone.authenticate().await?;

//             // Update the original client's token and expiry
//             // Note: This is a bit of a hack since we need interior mutability
//             // In a production system, you might want to use a mutex or atomic reference
//             // to properly handle concurrent access
//             unsafe {
//                 let self_mut = &mut *(self as *const Self as *mut Self);
//                 self_mut.auth_token = client_clone.auth_token;
//                 self_mut.token_expiry = client_clone.token_expiry;
//             }
//         }
//         Ok(())
//     }

//     pub async fn sign(&self, key_name: &str, data: &[u8]) -> VaultResult<Vec<u8>> {
//         self.ensure_authenticated().await?;

//         let base64_data = general_purpose::STANDARD.encode(data);

//         let sign_request = SignRequest {
//             input: base64_data,
//             hash_algorithm: "sha2-256".to_string(),
//         };

//         let headers = self.build_auth_headers()?;

//         let response = self
//             .client
//             .post(&format!(
//                 "{}/v1/transit/sign/{}/sha2-256",
//                 self.base_url, key_name
//             ))
//             .headers(headers)
//             .json(&sign_request)
//             .send()
//             .await
//             .map_err(|e| VaultError::Network(format!("Failed to send sign request: {}", e)))?;

//         if !response.status().is_success() {
//           let status = response.status();
//           let error_text = response.text().await.unwrap_or_default();
//             return Err(VaultError::Operation(format!(
//                 "Signing operation failed: {} - {}",
//                 status,
//                 error_text
//             )));
//         }

//         let sign_response: SignResponse = response.json().await.map_err(|e| {
//             VaultError::ResponseParsing(format!("Failed to parse sign response: {}", e))
//         })?;

//         // The signature from Vault is usually in the format "vault:v1:signature_data"
//         let signature_parts: Vec<&str> = sign_response.data.signature.split(':').collect();
//         let signature_base64 = if signature_parts.len() >= 3 {
//             signature_parts[2]
//         } else {
//             return Err(VaultError::ResponseParsing(
//                 "Invalid signature format".to_string(),
//             ));
//         };

//         // Decode the base64 signature
//         let signature = general_purpose::STANDARD
//             .decode(signature_base64)
//             .map_err(|e| {
//                 VaultError::ResponseParsing(format!("Failed to decode signature: {}", e))
//             })?;

//         Ok(signature)
//     }

//     pub async fn verify(&self, key_name: &str, data: &[u8], signature: &[u8]) -> VaultResult<bool> {
//         self.ensure_authenticated().await?;

//         let base64_data = general_purpose::STANDARD.encode(data);
//         let base64_signature = general_purpose::STANDARD.encode(signature);

//         let verify_request = VerifyRequest {
//             input: base64_data,
//             signature: format!("vault:v1:{}", base64_signature), // Format expected by Vault
//             hash_algorithm: "sha2-256".to_string(),
//         };

//         let headers = self.build_auth_headers()?;

//         let response = self
//             .client
//             .post(&format!(
//                 "{}/v1/transit/verify/{}/sha2-256",
//                 self.base_url, key_name
//             ))
//             .headers(headers)
//             .json(&verify_request)
//             .send()
//             .await
//             .map_err(|e| VaultError::Network(format!("Failed to send verify request: {}", e)))?;

//         if !response.status().is_success() {
//           let status = response.status();
//           let error_text = response.text().await.unwrap_or_default();
//             return Err(VaultError::Operation(format!(
//                 "Verification operation failed: {} - {}",
//                 status,
//                 error_text
//             )));
//         }

//         let verify_response: VerifyResponse = response.json().await.map_err(|e| {
//             VaultError::ResponseParsing(format!("Failed to parse verify response: {}", e))
//         })?;

//         Ok(verify_response.data.valid)
//     }

//     pub async fn get_public_key(&self, key_name: &str) -> VaultResult<Vec<u8>> {
//         self.ensure_authenticated().await?;

//         let headers = self.build_auth_headers()?;

//         let response = self
//             .client
//             .get(&format!("{}/v1/transit/keys/{}", self.base_url, key_name))
//             .headers(headers)
//             .send()
//             .await
//             .map_err(|e| VaultError::Network(format!("Failed to get public key: {}", e)))?;

//         if !response.status().is_success() {
//           let status = response.status();
//           let error_text = response.text().await.unwrap_or_default();
//             return Err(VaultError::Operation(format!(
//                 "Get public key operation failed: {} - {}",
//                 status,
//                 error_text
//             )));
//         }

//         let key_response: PublicKeyResponse = response.json().await.map_err(|e| {
//             VaultError::ResponseParsing(format!("Failed to parse public key response: {}", e))
//         })?;

//         // The public key from Vault is PEM encoded
//         Ok(key_response.data.public_key.into_bytes())
//     }

//     pub async fn key_exists(&self, key_name: &str) -> VaultResult<bool> {
//         self.ensure_authenticated().await?;

//         let headers = self.build_auth_headers()?;

//         let response = self
//             .client
//             .get(&format!("{}/v1/transit/keys", self.base_url))
//             .headers(headers)
//             .send()
//             .await
//             .map_err(|e| VaultError::Network(format!("Failed to list keys: {}", e)))?;

//         if !response.status().is_success() {
//           let status = response.status();
//           let error_text = response.text().await.unwrap_or_default();
//             return Err(VaultError::Operation(format!(
//                 "List keys operation failed: {} - {}",
//                 status,
//                 error_text
//             )));
//         }

//         let keys_response: KeysResponse = response.json().await.map_err(|e| {
//             VaultError::ResponseParsing(format!("Failed to parse keys response: {}", e))
//         })?;

//         Ok(keys_response.data.keys.contains(&key_name.to_string()))
//     }
// }

// impl Clone for VaultTransitClient {
//     fn clone(&self) -> Self {
//         Self {
//             client: self.client.clone(),
//             base_url: self.base_url.clone(),
//             auth_token: self.auth_token.clone(),
//             token_expiry: self.token_expiry,
//             role_id: self.role_id.clone(),
//             secret_id: self.secret_id.clone(),
//         }
//     }
// }
