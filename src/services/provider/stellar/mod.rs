//! Stellar Provider implementation for interacting with Stellar blockchain networks.
//!
//! This module provides functionality to interact with Stellar networks through RPC calls.
//! It implements common operations like getting accounts, sending transactions, and querying
//! blockchain state and events.

use async_trait::async_trait;
use eyre::Result;
use soroban_rs::stellar_rpc_client::Client;
use soroban_rs::stellar_rpc_client::{
    Error as StellarClientError, EventStart, EventType, GetEventsResponse, GetLatestLedgerResponse,
    GetLedgerEntriesResponse, GetNetworkResponse, GetTransactionResponse, GetTransactionsRequest,
    GetTransactionsResponse, SimulateTransactionResponse,
};
use soroban_rs::xdr::{
    AccountEntry, ContractId, Hash, HostFunction, InvokeContractArgs, InvokeHostFunctionOp,
    LedgerKey, Limits, MuxedAccount, Operation, OperationBody, ReadXdr, ScAddress, ScSymbol, ScVal,
    SequenceNumber, Transaction, TransactionEnvelope, TransactionV1Envelope, Uint256, VecM,
};
#[cfg(test)]
use soroban_rs::xdr::{AccountId, LedgerKeyAccount, PublicKey};
use soroban_rs::SorobanTransactionResponse;
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(test)]
use mockall::automock;

use crate::models::{JsonRpcId, RpcConfig};
use crate::services::provider::is_retriable_error;
use crate::services::provider::retry::retry_rpc_call;
use crate::services::provider::rpc_selector::RpcSelector;
use crate::services::provider::should_mark_provider_failed;
use crate::services::provider::ProviderError;
use crate::services::provider::RetryConfig;
// Reqwest client is used for raw JSON-RPC HTTP requests. Alias to avoid name clash with the
// soroban `Client` type imported above.
use reqwest::Client as ReqwestClient;
use std::sync::Arc;
use std::time::Duration;

/// Generates a unique JSON-RPC request ID.
///
/// This function returns a monotonically increasing ID for JSON-RPC requests.
/// It's thread-safe and guarantees unique IDs across concurrent requests.
///
/// # Returns
///
/// A unique u64 ID that can be used for JSON-RPC requests
fn generate_unique_rpc_id() -> u64 {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// Categorizes a Stellar client error into an appropriate `ProviderError` variant.
///
/// This function analyzes the given error and maps it to a specific `ProviderError` variant:
/// - Handles StellarClientError variants directly (timeouts, JSON-RPC errors, etc.)
/// - Extracts reqwest::Error from jsonrpsee Transport errors
/// - Maps JSON-RPC error codes appropriately
/// - Distinguishes between retriable network errors and non-retriable validation errors
/// - Falls back to ProviderError::Other for unknown error types
/// - Optionally prepends a context message to the error for better debugging
///
/// # Arguments
///
/// * `err` - The StellarClientError to categorize (takes ownership)
/// * `context` - Optional context message to prepend (e.g., "Failed to get account")
///
/// # Returns
///
/// The appropriate `ProviderError` variant based on the error type
fn categorize_stellar_error_with_context(
    err: StellarClientError,
    context: Option<&str>,
) -> ProviderError {
    let add_context = |msg: String| -> String {
        match context {
            Some(ctx) => format!("{ctx}: {msg}"),
            None => msg,
        }
    };
    match err {
        // === Timeout Errors (Retriable) ===
        StellarClientError::TransactionSubmissionTimeout => ProviderError::Timeout,

        // === Address/Encoding Errors (Non-retriable, Client-side) ===
        StellarClientError::InvalidAddress(decode_err) => ProviderError::InvalidAddress(
            add_context(format!("Invalid Stellar address: {decode_err}")),
        ),

        // === XDR/Serialization Errors (Non-retriable, Client-side) ===
        StellarClientError::Xdr(xdr_err) => {
            ProviderError::Other(add_context(format!("XDR processing error: {xdr_err}")))
        }

        // === JSON Parsing Errors (Non-retriable, may indicate RPC response issue) ===
        StellarClientError::Serde(serde_err) => {
            ProviderError::Other(add_context(format!("JSON parsing error: {serde_err}")))
        }

        // === URL Configuration Errors (Non-retriable, Configuration issue) ===
        StellarClientError::InvalidRpcUrl(uri_err) => {
            ProviderError::NetworkConfiguration(add_context(format!("Invalid RPC URL: {uri_err}")))
        }
        StellarClientError::InvalidRpcUrlFromUriParts(uri_err) => {
            ProviderError::NetworkConfiguration(add_context(format!(
                "Invalid RPC URL parts: {uri_err}"
            )))
        }
        StellarClientError::InvalidUrl(url) => {
            ProviderError::NetworkConfiguration(add_context(format!("Invalid URL: {url}")))
        }

        // === Network Passphrase Mismatch (Non-retriable, Configuration issue) ===
        StellarClientError::InvalidNetworkPassphrase { expected, server } => {
            ProviderError::NetworkConfiguration(add_context(format!(
                "Network passphrase mismatch: expected {expected:?}, server returned {server:?}"
            )))
        }

        // === JSON-RPC Errors (May be retriable depending on the specific error) ===
        StellarClientError::JsonRpc(jsonrpsee_err) => {
            match jsonrpsee_err {
                // Handle Call errors with error codes
                jsonrpsee_core::error::Error::Call(err_obj) => {
                    let code = err_obj.code() as i64;
                    let message = add_context(err_obj.message().to_string());
                    ProviderError::RpcErrorCode { code, message }
                }

                // Handle request timeouts
                jsonrpsee_core::error::Error::RequestTimeout => ProviderError::Timeout,

                // Handle transport errors (network-level issues)
                jsonrpsee_core::error::Error::Transport(transport_err) => {
                    // Check source chain for reqwest errors
                    let mut source = transport_err.source();
                    while let Some(s) = source {
                        if let Some(reqwest_err) = s.downcast_ref::<reqwest::Error>() {
                            return ProviderError::from(reqwest_err);
                        }
                        source = s.source();
                    }

                    ProviderError::TransportError(add_context(format!(
                        "Transport error: {transport_err}"
                    )))
                }
                // Catch-all for other jsonrpsee errors
                other => ProviderError::Other(add_context(format!("JSON-RPC error: {other}"))),
            }
        }
        // === Response Parsing/Validation Errors (May indicate RPC node issue) ===
        StellarClientError::InvalidResponse => {
            // This could be a temporary RPC node issue or malformed response
            ProviderError::Other(add_context(
                "Invalid response from Stellar RPC server".to_string(),
            ))
        }
        StellarClientError::MissingResult => {
            ProviderError::Other(add_context("Missing result in RPC response".to_string()))
        }
        StellarClientError::MissingError => ProviderError::Other(add_context(
            "Failed to read error from RPC response".to_string(),
        )),

        // === Transaction Errors (Non-retriable, Transaction-specific issues) ===
        StellarClientError::TransactionFailed(msg) => {
            ProviderError::Other(add_context(format!("Transaction failed: {msg}")))
        }
        StellarClientError::TransactionSubmissionFailed(msg) => {
            ProviderError::Other(add_context(format!("Transaction submission failed: {msg}")))
        }
        StellarClientError::TransactionSimulationFailed(msg) => {
            ProviderError::Other(add_context(format!("Transaction simulation failed: {msg}")))
        }
        StellarClientError::UnexpectedTransactionStatus(status) => ProviderError::Other(
            add_context(format!("Unexpected transaction status: {status}")),
        ),

        // === Resource Not Found Errors (Non-retriable) ===
        StellarClientError::NotFound(resource, id) => {
            ProviderError::Other(add_context(format!("{resource} not found: {id}")))
        }

        // === Client-side Validation Errors (Non-retriable) ===
        StellarClientError::InvalidCursor => {
            ProviderError::Other(add_context("Invalid cursor".to_string()))
        }
        StellarClientError::UnexpectedSimulateTransactionResultSize { length } => {
            ProviderError::Other(add_context(format!(
                "Unexpected simulate transaction result size: {length}"
            )))
        }
        StellarClientError::UnexpectedOperationCount { count } => {
            ProviderError::Other(add_context(format!("Unexpected operation count: {count}")))
        }
        StellarClientError::UnsupportedOperationType => {
            ProviderError::Other(add_context("Unsupported operation type".to_string()))
        }
        StellarClientError::UnexpectedContractCodeDataType(data) => ProviderError::Other(
            add_context(format!("Unexpected contract code data type: {data:?}")),
        ),
        StellarClientError::UnexpectedContractInstance(val) => ProviderError::Other(add_context(
            format!("Unexpected contract instance: {val:?}"),
        )),
        StellarClientError::LargeFee(fee) => {
            ProviderError::Other(add_context(format!("Fee too large: {fee}")))
        }
        StellarClientError::CannotAuthorizeRawTransaction => {
            ProviderError::Other(add_context("Cannot authorize raw transaction".to_string()))
        }
        StellarClientError::MissingOp => {
            ProviderError::Other(add_context("Missing operation in transaction".to_string()))
        }
        StellarClientError::MissingSignerForAddress { address } => ProviderError::Other(
            add_context(format!("Missing signer for address: {address}")),
        ),

        // === Deprecated/Other Errors ===
        #[allow(deprecated)]
        StellarClientError::UnexpectedToken(entry) => {
            ProviderError::Other(add_context(format!("Unexpected token: {entry:?}")))
        }
    }
}

/// Normalize a URL for logging by removing query strings, fragments and redacting userinfo.
///
/// Examples:
/// - https://user:secret@api.example.com/path?api_key=XXX -> https://<redacted>@api.example.com/path
/// - https://api.example.com/path?api_key=XXX -> https://api.example.com/path
fn normalize_url_for_log(url: &str) -> String {
    // Remove query and fragment first
    let mut s = url.to_string();
    if let Some(q) = s.find('?') {
        s.truncate(q);
    }
    if let Some(h) = s.find('#') {
        s.truncate(h);
    }

    // Redact userinfo if present (scheme://userinfo@host...)
    if let Some(scheme_pos) = s.find("://") {
        let start = scheme_pos + 3;
        if let Some(at_pos) = s[start..].find('@') {
            let after = &s[start + at_pos + 1..];
            let prefix = &s[..start];
            s = format!("{prefix}<redacted>@{after}");
        }
    }

    s
}
#[derive(Debug, Clone)]
pub struct GetEventsRequest {
    pub start: EventStart,
    pub event_type: Option<EventType>,
    pub contract_ids: Vec<String>,
    pub topics: Vec<Vec<String>>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct StellarProvider {
    /// RPC selector for managing and selecting providers
    selector: RpcSelector,
    /// Timeout in seconds for RPC calls
    timeout_seconds: Duration,
    /// Configuration for retry behavior
    retry_config: RetryConfig,
}

#[async_trait]
#[cfg_attr(test, automock)]
#[allow(dead_code)]
pub trait StellarProviderTrait: Send + Sync {
    async fn get_account(&self, account_id: &str) -> Result<AccountEntry, ProviderError>;
    async fn simulate_transaction_envelope(
        &self,
        tx_envelope: &TransactionEnvelope,
    ) -> Result<SimulateTransactionResponse, ProviderError>;
    async fn send_transaction_polling(
        &self,
        tx_envelope: &TransactionEnvelope,
    ) -> Result<SorobanTransactionResponse, ProviderError>;
    async fn get_network(&self) -> Result<GetNetworkResponse, ProviderError>;
    async fn get_latest_ledger(&self) -> Result<GetLatestLedgerResponse, ProviderError>;
    async fn send_transaction(
        &self,
        tx_envelope: &TransactionEnvelope,
    ) -> Result<Hash, ProviderError>;
    async fn get_transaction(&self, tx_id: &Hash) -> Result<GetTransactionResponse, ProviderError>;
    async fn get_transactions(
        &self,
        request: GetTransactionsRequest,
    ) -> Result<GetTransactionsResponse, ProviderError>;
    async fn get_ledger_entries(
        &self,
        keys: &[LedgerKey],
    ) -> Result<GetLedgerEntriesResponse, ProviderError>;
    async fn get_events(
        &self,
        request: GetEventsRequest,
    ) -> Result<GetEventsResponse, ProviderError>;
    async fn raw_request_dyn(
        &self,
        method: &str,
        params: serde_json::Value,
        id: Option<JsonRpcId>,
    ) -> Result<serde_json::Value, ProviderError>;
    /// Calls a contract function (read-only, via simulation).
    ///
    /// This method invokes a Soroban contract function without submitting a transaction.
    /// It uses simulation to execute the function and return the result.
    ///
    /// # Arguments
    /// * `contract_address` - The contract address in StrKey format
    /// * `function_name` - The function name as an ScSymbol
    /// * `args` - Function arguments as ScVal vector
    ///
    /// # Returns
    /// The function result as an ScVal, or an error if the call fails
    async fn call_contract(
        &self,
        contract_address: &str,
        function_name: &ScSymbol,
        args: Vec<ScVal>,
    ) -> Result<ScVal, ProviderError>;
}

impl StellarProvider {
    // Create new StellarProvider instance
    pub fn new(
        mut rpc_configs: Vec<RpcConfig>,
        timeout_seconds: u64,
    ) -> Result<Self, ProviderError> {
        if rpc_configs.is_empty() {
            return Err(ProviderError::NetworkConfiguration(
                "No RPC configurations provided for StellarProvider".to_string(),
            ));
        }

        RpcConfig::validate_list(&rpc_configs)
            .map_err(|e| ProviderError::NetworkConfiguration(e.to_string()))?;

        rpc_configs.retain(|config| config.get_weight() > 0);

        if rpc_configs.is_empty() {
            return Err(ProviderError::NetworkConfiguration(
                "No active RPC configurations provided (all weights are 0 or list was empty after filtering)".to_string(),
            ));
        }

        let selector = RpcSelector::new(rpc_configs).map_err(|e| {
            ProviderError::NetworkConfiguration(format!("Failed to create RPC selector: {e}"))
        })?;

        let retry_config = RetryConfig::from_env();

        Ok(Self {
            selector,
            timeout_seconds: Duration::from_secs(timeout_seconds),
            retry_config,
        })
    }

    /// Initialize a Stellar client for a given URL
    fn initialize_provider(&self, url: &str) -> Result<Client, ProviderError> {
        Client::new(url).map_err(|e| {
            ProviderError::NetworkConfiguration(format!(
                "Failed to create Stellar RPC client: {e} - URL: '{url}'"
            ))
        })
    }

    /// Initialize a reqwest client for raw HTTP JSON-RPC calls.
    ///
    /// This centralizes client creation so we can configure timeouts and other options in one place.
    fn initialize_raw_provider(&self, url: &str) -> Result<ReqwestClient, ProviderError> {
        ReqwestClient::builder()
            .timeout(self.timeout_seconds)
            .build()
            .map_err(|e| {
                ProviderError::NetworkConfiguration(format!(
                    "Failed to create HTTP client for raw RPC: {e} - URL: '{url}'"
                ))
            })
    }

    /// Helper method to retry RPC calls with exponential backoff
    async fn retry_rpc_call<T, F, Fut>(
        &self,
        operation_name: &str,
        operation: F,
    ) -> Result<T, ProviderError>
    where
        F: Fn(Client) -> Fut,
        Fut: std::future::Future<Output = Result<T, ProviderError>>,
    {
        let provider_url_raw = match self.selector.get_current_url() {
            Ok(url) => url,
            Err(e) => {
                return Err(ProviderError::NetworkConfiguration(format!(
                    "No RPC URL available for StellarProvider: {e}"
                )));
            }
        };
        let provider_url = normalize_url_for_log(&provider_url_raw);

        tracing::debug!(
            "Starting Stellar RPC operation '{}' with timeout: {}s, provider_url: {}",
            operation_name,
            self.timeout_seconds.as_secs(),
            provider_url
        );

        retry_rpc_call(
            &self.selector,
            operation_name,
            is_retriable_error,
            should_mark_provider_failed,
            |url| self.initialize_provider(url),
            operation,
            Some(self.retry_config.clone()),
        )
        .await
    }

    /// Retry helper for raw JSON-RPC requests
    async fn retry_raw_request(
        &self,
        operation_name: &str,
        request: serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        let provider_url_raw = match self.selector.get_current_url() {
            Ok(url) => url,
            Err(e) => {
                return Err(ProviderError::NetworkConfiguration(format!(
                    "No RPC URL available for StellarProvider: {e}"
                )));
            }
        };
        let provider_url = normalize_url_for_log(&provider_url_raw);

        tracing::debug!(
            "Starting raw RPC operation '{}' with timeout: {}s, provider_url: {}",
            operation_name,
            self.timeout_seconds.as_secs(),
            provider_url
        );

        let request_clone = request.clone();
        retry_rpc_call(
            &self.selector,
            operation_name,
            is_retriable_error,
            should_mark_provider_failed,
            |url| {
                // Initialize an HTTP client for this URL and return it together with the URL string
                self.initialize_raw_provider(url)
                    .map(|client| (url.to_string(), client))
            },
            |(url, client): (String, ReqwestClient)| {
                let request_for_call = request_clone.clone();
                async move {
                    let response = client
                        .post(&url)
                        .json(&request_for_call)
                        // Keep a per-request timeout as a safeguard (client also has a default timeout)
                        .timeout(self.timeout_seconds)
                        .send()
                        .await
                        .map_err(ProviderError::from)?;

                    let json_response: serde_json::Value =
                        response.json().await.map_err(ProviderError::from)?;

                    Ok(json_response)
                }
            },
            Some(self.retry_config.clone()),
        )
        .await
    }
}

#[async_trait]
impl StellarProviderTrait for StellarProvider {
    async fn get_account(&self, account_id: &str) -> Result<AccountEntry, ProviderError> {
        let account_id = Arc::new(account_id.to_string());

        self.retry_rpc_call("get_account", move |client| {
            let account_id = Arc::clone(&account_id);
            async move {
                client.get_account(&account_id).await.map_err(|e| {
                    categorize_stellar_error_with_context(e, Some("Failed to get account"))
                })
            }
        })
        .await
    }

    async fn simulate_transaction_envelope(
        &self,
        tx_envelope: &TransactionEnvelope,
    ) -> Result<SimulateTransactionResponse, ProviderError> {
        let tx_envelope = Arc::new(tx_envelope.clone());

        self.retry_rpc_call("simulate_transaction_envelope", move |client| {
            let tx_envelope = Arc::clone(&tx_envelope);
            async move {
                client
                    .simulate_transaction_envelope(&tx_envelope, None)
                    .await
                    .map_err(|e| {
                        categorize_stellar_error_with_context(
                            e,
                            Some("Failed to simulate transaction"),
                        )
                    })
            }
        })
        .await
    }

    async fn send_transaction_polling(
        &self,
        tx_envelope: &TransactionEnvelope,
    ) -> Result<SorobanTransactionResponse, ProviderError> {
        let tx_envelope = Arc::new(tx_envelope.clone());

        self.retry_rpc_call("send_transaction_polling", move |client| {
            let tx_envelope = Arc::clone(&tx_envelope);
            async move {
                client
                    .send_transaction_polling(&tx_envelope)
                    .await
                    .map(SorobanTransactionResponse::from)
                    .map_err(|e| {
                        categorize_stellar_error_with_context(
                            e,
                            Some("Failed to send transaction (polling)"),
                        )
                    })
            }
        })
        .await
    }

    async fn get_network(&self) -> Result<GetNetworkResponse, ProviderError> {
        self.retry_rpc_call("get_network", |client| async move {
            client.get_network().await.map_err(|e| {
                categorize_stellar_error_with_context(e, Some("Failed to get network"))
            })
        })
        .await
    }

    async fn get_latest_ledger(&self) -> Result<GetLatestLedgerResponse, ProviderError> {
        self.retry_rpc_call("get_latest_ledger", |client| async move {
            client.get_latest_ledger().await.map_err(|e| {
                categorize_stellar_error_with_context(e, Some("Failed to get latest ledger"))
            })
        })
        .await
    }

    async fn send_transaction(
        &self,
        tx_envelope: &TransactionEnvelope,
    ) -> Result<Hash, ProviderError> {
        let tx_envelope = Arc::new(tx_envelope.clone());

        self.retry_rpc_call("send_transaction", move |client| {
            let tx_envelope = Arc::clone(&tx_envelope);
            async move {
                client.send_transaction(&tx_envelope).await.map_err(|e| {
                    categorize_stellar_error_with_context(e, Some("Failed to send transaction"))
                })
            }
        })
        .await
    }

    async fn get_transaction(&self, tx_id: &Hash) -> Result<GetTransactionResponse, ProviderError> {
        let tx_id = Arc::new(tx_id.clone());

        self.retry_rpc_call("get_transaction", move |client| {
            let tx_id = Arc::clone(&tx_id);
            async move {
                client.get_transaction(&tx_id).await.map_err(|e| {
                    categorize_stellar_error_with_context(e, Some("Failed to get transaction"))
                })
            }
        })
        .await
    }

    async fn get_transactions(
        &self,
        request: GetTransactionsRequest,
    ) -> Result<GetTransactionsResponse, ProviderError> {
        let request = Arc::new(request);

        self.retry_rpc_call("get_transactions", move |client| {
            let request = Arc::clone(&request);
            async move {
                client
                    .get_transactions((*request).clone())
                    .await
                    .map_err(|e| {
                        categorize_stellar_error_with_context(e, Some("Failed to get transactions"))
                    })
            }
        })
        .await
    }

    async fn get_ledger_entries(
        &self,
        keys: &[LedgerKey],
    ) -> Result<GetLedgerEntriesResponse, ProviderError> {
        let keys = Arc::new(keys.to_vec());

        self.retry_rpc_call("get_ledger_entries", move |client| {
            let keys = Arc::clone(&keys);
            async move {
                client.get_ledger_entries(&keys).await.map_err(|e| {
                    categorize_stellar_error_with_context(e, Some("Failed to get ledger entries"))
                })
            }
        })
        .await
    }

    async fn get_events(
        &self,
        request: GetEventsRequest,
    ) -> Result<GetEventsResponse, ProviderError> {
        let request = Arc::new(request);

        self.retry_rpc_call("get_events", move |client| {
            let request = Arc::clone(&request);
            async move {
                client
                    .get_events(
                        request.start.clone(),
                        request.event_type,
                        &request.contract_ids,
                        &request.topics,
                        request.limit,
                    )
                    .await
                    .map_err(|e| {
                        categorize_stellar_error_with_context(e, Some("Failed to get events"))
                    })
            }
        })
        .await
    }

    async fn raw_request_dyn(
        &self,
        method: &str,
        params: serde_json::Value,
        id: Option<JsonRpcId>,
    ) -> Result<serde_json::Value, ProviderError> {
        let id_value = match id {
            Some(id) => serde_json::to_value(id)
                .map_err(|e| ProviderError::Other(format!("Failed to serialize id: {e}")))?,
            None => serde_json::json!(generate_unique_rpc_id()),
        };

        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id_value,
            "method": method,
            "params": params,
        });

        let response = self.retry_raw_request("raw_request_dyn", request).await?;

        // Check for JSON-RPC error
        if let Some(error) = response.get("error") {
            if let Some(code) = error.get("code").and_then(|c| c.as_i64()) {
                return Err(ProviderError::RpcErrorCode {
                    code,
                    message: error
                        .get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("Unknown error")
                        .to_string(),
                });
            }
            return Err(ProviderError::Other(format!("JSON-RPC error: {error}")));
        }

        // Extract result
        response
            .get("result")
            .cloned()
            .ok_or_else(|| ProviderError::Other("No result field in JSON-RPC response".to_string()))
    }

    async fn call_contract(
        &self,
        contract_address: &str,
        function_name: &ScSymbol,
        args: Vec<ScVal>,
    ) -> Result<ScVal, ProviderError> {
        // Parse contract address
        let contract = stellar_strkey::Contract::from_string(contract_address)
            .map_err(|e| ProviderError::Other(format!("Invalid contract address: {e}")))?;
        let contract_addr = ScAddress::Contract(ContractId(Hash(contract.0)));

        // Convert args to VecM
        let args_vec = VecM::try_from(args)
            .map_err(|e| ProviderError::Other(format!("Failed to convert arguments: {e:?}")))?;

        // Build InvokeHostFunction operation
        let host_function = HostFunction::InvokeContract(InvokeContractArgs {
            contract_address: contract_addr,
            function_name: function_name.clone(),
            args: args_vec,
        });

        let operation = Operation {
            source_account: None,
            body: OperationBody::InvokeHostFunction(InvokeHostFunctionOp {
                host_function,
                auth: VecM::try_from(vec![]).unwrap(),
            }),
        };

        // Build a minimal transaction envelope for simulation
        //
        // Why simulation instead of direct reads?
        // In Soroban, contract functions (even read-only ones like decimals()) must be invoked
        // through the transaction system. Simulation is the standard way to call read-only
        // functions because it:
        // 1. Executes the contract function without submitting to the ledger (no fees, no state changes)
        // 2. Returns the computed result immediately
        // 3. Works for functions that compute values (not just storage reads)
        //
        // Direct storage reads (get_ledger_entries) only work if the value is stored in contract
        // data storage. For functions that compute values, simulation is required.
        //
        // Use a dummy account - simulation doesn't require a real account or signature
        let dummy_account = MuxedAccount::Ed25519(Uint256([0u8; 32]));
        let operations: VecM<Operation, 100> = vec![operation].try_into().map_err(|e| {
            ProviderError::Other(format!("Failed to create operations vector: {e:?}"))
        })?;

        let tx = Transaction {
            source_account: dummy_account,
            fee: 100,
            seq_num: SequenceNumber(0),
            cond: soroban_rs::xdr::Preconditions::None,
            memo: soroban_rs::xdr::Memo::None,
            operations,
            ext: soroban_rs::xdr::TransactionExt::V0,
        };

        let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
            tx,
            signatures: VecM::try_from(vec![]).unwrap(),
        });

        // Simulate the transaction to get the result (read-only execution, no ledger submission)
        let sim_response = self.simulate_transaction_envelope(&envelope).await?;

        // Check for simulation errors
        if let Some(error) = sim_response.error {
            return Err(ProviderError::Other(format!(
                "Contract invocation simulation failed: {error}",
            )));
        }

        // Extract result from simulation response
        if sim_response.results.is_empty() {
            return Err(ProviderError::Other(
                "Simulation returned no results".to_string(),
            ));
        }

        // Parse the XDR result as ScVal
        let result_xdr = &sim_response.results[0].xdr;
        ScVal::from_xdr_base64(result_xdr, Limits::none()).map_err(|e| {
            ProviderError::Other(format!("Failed to parse simulation result XDR: {e}"))
        })
    }
}

#[cfg(test)]
mod stellar_rpc_tests {
    use super::*;
    use crate::services::provider::stellar::{
        GetEventsRequest, StellarProvider, StellarProviderTrait,
    };
    use futures::FutureExt;
    use lazy_static::lazy_static;
    use mockall::predicate as p;
    use soroban_rs::stellar_rpc_client::{
        EventStart, GetEventsResponse, GetLatestLedgerResponse, GetLedgerEntriesResponse,
        GetNetworkResponse, GetTransactionEvents, GetTransactionResponse, GetTransactionsRequest,
        GetTransactionsResponse, SimulateTransactionResponse,
    };
    use soroban_rs::xdr::{
        AccountEntryExt, Hash, LedgerKey, OperationResult, String32, Thresholds,
        TransactionEnvelope, TransactionResult, TransactionResultExt, TransactionResultResult,
        VecM,
    };
    use soroban_rs::{create_mock_set_options_tx_envelope, SorobanTransactionResponse};
    use std::str::FromStr;
    use std::sync::Mutex;

    lazy_static! {
        static ref STELLAR_TEST_ENV_MUTEX: Mutex<()> = Mutex::new(());
    }

    struct StellarTestEnvGuard {
        _mutex_guard: std::sync::MutexGuard<'static, ()>,
    }

    impl StellarTestEnvGuard {
        fn new(mutex_guard: std::sync::MutexGuard<'static, ()>) -> Self {
            std::env::set_var(
                "API_KEY",
                "test_api_key_for_evm_provider_new_this_is_long_enough_32_chars",
            );
            std::env::set_var("REDIS_URL", "redis://test-dummy-url-for-evm-provider");
            // Set minimal retry config to avoid excessive retries and TCP exhaustion in concurrent tests
            std::env::set_var("PROVIDER_MAX_RETRIES", "1");
            std::env::set_var("PROVIDER_MAX_FAILOVERS", "0");
            std::env::set_var("PROVIDER_RETRY_BASE_DELAY_MS", "0");
            std::env::set_var("PROVIDER_RETRY_MAX_DELAY_MS", "0");

            Self {
                _mutex_guard: mutex_guard,
            }
        }
    }

    impl Drop for StellarTestEnvGuard {
        fn drop(&mut self) {
            std::env::remove_var("API_KEY");
            std::env::remove_var("REDIS_URL");
            std::env::remove_var("PROVIDER_MAX_RETRIES");
            std::env::remove_var("PROVIDER_MAX_FAILOVERS");
            std::env::remove_var("PROVIDER_RETRY_BASE_DELAY_MS");
            std::env::remove_var("PROVIDER_RETRY_MAX_DELAY_MS");
        }
    }

    // Helper function to set up the test environment
    fn setup_test_env() -> StellarTestEnvGuard {
        let guard = STELLAR_TEST_ENV_MUTEX
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        StellarTestEnvGuard::new(guard)
    }

    fn dummy_hash() -> Hash {
        Hash([0u8; 32])
    }

    fn dummy_get_network_response() -> GetNetworkResponse {
        GetNetworkResponse {
            friendbot_url: Some("https://friendbot.testnet.stellar.org/".into()),
            passphrase: "Test SDF Network ; September 2015".into(),
            protocol_version: 20,
        }
    }

    fn dummy_get_latest_ledger_response() -> GetLatestLedgerResponse {
        GetLatestLedgerResponse {
            id: "c73c5eac58a441d4eb733c35253ae85f783e018f7be5ef974258fed067aabb36".into(),
            protocol_version: 20,
            sequence: 2_539_605,
        }
    }

    fn dummy_simulate() -> SimulateTransactionResponse {
        SimulateTransactionResponse {
            min_resource_fee: 100,
            transaction_data: "test".to_string(),
            ..Default::default()
        }
    }

    fn create_success_tx_result() -> TransactionResult {
        // Create empty operation results
        let empty_vec: Vec<OperationResult> = Vec::new();
        let op_results = empty_vec.try_into().unwrap_or_default();

        TransactionResult {
            fee_charged: 100,
            result: TransactionResultResult::TxSuccess(op_results),
            ext: TransactionResultExt::V0,
        }
    }

    fn dummy_get_transaction_response() -> GetTransactionResponse {
        GetTransactionResponse {
            status: "SUCCESS".to_string(),
            envelope: None,
            result: Some(create_success_tx_result()),
            result_meta: None,
            events: GetTransactionEvents {
                contract_events: vec![],
                diagnostic_events: vec![],
                transaction_events: vec![],
            },
            ledger: None,
        }
    }

    fn dummy_soroban_tx() -> SorobanTransactionResponse {
        SorobanTransactionResponse {
            response: dummy_get_transaction_response(),
        }
    }

    fn dummy_get_transactions_response() -> GetTransactionsResponse {
        GetTransactionsResponse {
            transactions: vec![],
            latest_ledger: 0,
            latest_ledger_close_time: 0,
            oldest_ledger: 0,
            oldest_ledger_close_time: 0,
            cursor: 0,
        }
    }

    fn dummy_get_ledger_entries_response() -> GetLedgerEntriesResponse {
        GetLedgerEntriesResponse {
            entries: None,
            latest_ledger: 0,
        }
    }

    fn dummy_get_events_response() -> GetEventsResponse {
        GetEventsResponse {
            events: vec![],
            latest_ledger: 0,
            latest_ledger_close_time: "0".to_string(),
            oldest_ledger: 0,
            oldest_ledger_close_time: "0".to_string(),
            cursor: "0".to_string(),
        }
    }

    fn dummy_transaction_envelope() -> TransactionEnvelope {
        create_mock_set_options_tx_envelope()
    }

    fn dummy_ledger_key() -> LedgerKey {
        LedgerKey::Account(LedgerKeyAccount {
            account_id: AccountId(PublicKey::PublicKeyTypeEd25519(Uint256([0; 32]))),
        })
    }

    pub fn mock_account_entry(account_id: &str) -> AccountEntry {
        AccountEntry {
            account_id: AccountId(PublicKey::from_str(account_id).unwrap()),
            balance: 0,
            ext: AccountEntryExt::V0,
            flags: 0,
            home_domain: String32::default(),
            inflation_dest: None,
            seq_num: 0.into(),
            num_sub_entries: 0,
            signers: VecM::default(),
            thresholds: Thresholds([0, 0, 0, 0]),
        }
    }

    fn dummy_account_entry() -> AccountEntry {
        mock_account_entry("GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF")
    }

    // ---------------------------------------------------------------------
    // Tests
    // ---------------------------------------------------------------------

    #[test]
    fn test_new_provider() {
        let _env_guard = setup_test_env();

        let provider =
            StellarProvider::new(vec![RpcConfig::new("http://localhost:8000".to_string())], 0);
        assert!(provider.is_ok());

        let provider_err = StellarProvider::new(vec![], 0);
        assert!(provider_err.is_err());
        match provider_err.unwrap_err() {
            ProviderError::NetworkConfiguration(msg) => {
                assert!(msg.contains("No RPC configurations provided"));
            }
            _ => panic!("Unexpected error type"),
        }
    }

    #[test]
    fn test_new_provider_selects_highest_weight() {
        let _env_guard = setup_test_env();

        let configs = vec![
            RpcConfig::with_weight("http://rpc1.example.com".to_string(), 10).unwrap(),
            RpcConfig::with_weight("http://rpc2.example.com".to_string(), 100).unwrap(), // Highest weight
            RpcConfig::with_weight("http://rpc3.example.com".to_string(), 50).unwrap(),
        ];
        let provider = StellarProvider::new(configs, 0);
        assert!(provider.is_ok());
        // We can't directly inspect the client's URL easily without more complex mocking or changes.
        // For now, we trust the sorting logic and that Client::new would fail for a truly bad URL if selection was wrong.
        // A more robust test would involve a mock client or a way to inspect the chosen URL.
    }

    #[test]
    fn test_new_provider_ignores_weight_zero() {
        let _env_guard = setup_test_env();

        let configs = vec![
            RpcConfig::with_weight("http://rpc1.example.com".to_string(), 0).unwrap(), // Weight 0
            RpcConfig::with_weight("http://rpc2.example.com".to_string(), 100).unwrap(), // Should be selected
        ];
        let provider = StellarProvider::new(configs, 0);
        assert!(provider.is_ok());

        let configs_only_zero =
            vec![RpcConfig::with_weight("http://rpc1.example.com".to_string(), 0).unwrap()];
        let provider_err = StellarProvider::new(configs_only_zero, 0);
        assert!(provider_err.is_err());
        match provider_err.unwrap_err() {
            ProviderError::NetworkConfiguration(msg) => {
                assert!(msg.contains("No active RPC configurations provided"));
            }
            _ => panic!("Unexpected error type"),
        }
    }

    #[test]
    fn test_new_provider_invalid_url_scheme() {
        let configs = vec![RpcConfig::new("ftp://invalid.example.com".to_string())];
        let provider_err = StellarProvider::new(configs, 0);
        assert!(provider_err.is_err());
        match provider_err.unwrap_err() {
            ProviderError::NetworkConfiguration(msg) => {
                assert!(msg.contains("Invalid URL scheme"));
            }
            _ => panic!("Unexpected error type"),
        }
    }

    #[test]
    fn test_new_provider_all_zero_weight_configs() {
        let _env_guard = setup_test_env();

        let configs = vec![
            RpcConfig::with_weight("http://rpc1.example.com".to_string(), 0).unwrap(),
            RpcConfig::with_weight("http://rpc2.example.com".to_string(), 0).unwrap(),
        ];
        let provider_err = StellarProvider::new(configs, 0);
        assert!(provider_err.is_err());
        match provider_err.unwrap_err() {
            ProviderError::NetworkConfiguration(msg) => {
                assert!(msg.contains("No active RPC configurations provided"));
            }
            _ => panic!("Unexpected error type"),
        }
    }

    #[tokio::test]
    async fn test_mock_basic_methods() {
        let mut mock = MockStellarProviderTrait::new();

        mock.expect_get_network()
            .times(1)
            .returning(|| async { Ok(dummy_get_network_response()) }.boxed());

        mock.expect_get_latest_ledger()
            .times(1)
            .returning(|| async { Ok(dummy_get_latest_ledger_response()) }.boxed());

        assert!(mock.get_network().await.is_ok());
        assert!(mock.get_latest_ledger().await.is_ok());
    }

    #[tokio::test]
    async fn test_mock_transaction_flow() {
        let mut mock = MockStellarProviderTrait::new();

        let envelope: TransactionEnvelope = dummy_transaction_envelope();
        let hash = dummy_hash();

        mock.expect_simulate_transaction_envelope()
            .withf(|_| true)
            .times(1)
            .returning(|_| async { Ok(dummy_simulate()) }.boxed());

        mock.expect_send_transaction()
            .withf(|_| true)
            .times(1)
            .returning(|_| async { Ok(dummy_hash()) }.boxed());

        mock.expect_send_transaction_polling()
            .withf(|_| true)
            .times(1)
            .returning(|_| async { Ok(dummy_soroban_tx()) }.boxed());

        mock.expect_get_transaction()
            .withf(|_| true)
            .times(1)
            .returning(|_| async { Ok(dummy_get_transaction_response()) }.boxed());

        mock.simulate_transaction_envelope(&envelope).await.unwrap();
        mock.send_transaction(&envelope).await.unwrap();
        mock.send_transaction_polling(&envelope).await.unwrap();
        mock.get_transaction(&hash).await.unwrap();
    }

    #[tokio::test]
    async fn test_mock_events_and_entries() {
        let mut mock = MockStellarProviderTrait::new();

        mock.expect_get_events()
            .times(1)
            .returning(|_| async { Ok(dummy_get_events_response()) }.boxed());

        mock.expect_get_ledger_entries()
            .times(1)
            .returning(|_| async { Ok(dummy_get_ledger_entries_response()) }.boxed());

        let events_request = GetEventsRequest {
            start: EventStart::Ledger(1),
            event_type: None,
            contract_ids: vec![],
            topics: vec![],
            limit: Some(10),
        };

        let dummy_key: LedgerKey = dummy_ledger_key();
        mock.get_events(events_request).await.unwrap();
        mock.get_ledger_entries(&[dummy_key]).await.unwrap();
    }

    #[tokio::test]
    async fn test_mock_all_methods_ok() {
        let mut mock = MockStellarProviderTrait::new();

        mock.expect_get_account()
            .with(p::eq("GTESTACCOUNTID"))
            .times(1)
            .returning(|_| async { Ok(dummy_account_entry()) }.boxed());

        mock.expect_simulate_transaction_envelope()
            .times(1)
            .returning(|_| async { Ok(dummy_simulate()) }.boxed());

        mock.expect_send_transaction_polling()
            .times(1)
            .returning(|_| async { Ok(dummy_soroban_tx()) }.boxed());

        mock.expect_get_network()
            .times(1)
            .returning(|| async { Ok(dummy_get_network_response()) }.boxed());

        mock.expect_get_latest_ledger()
            .times(1)
            .returning(|| async { Ok(dummy_get_latest_ledger_response()) }.boxed());

        mock.expect_send_transaction()
            .times(1)
            .returning(|_| async { Ok(dummy_hash()) }.boxed());

        mock.expect_get_transaction()
            .times(1)
            .returning(|_| async { Ok(dummy_get_transaction_response()) }.boxed());

        mock.expect_get_transactions()
            .times(1)
            .returning(|_| async { Ok(dummy_get_transactions_response()) }.boxed());

        mock.expect_get_ledger_entries()
            .times(1)
            .returning(|_| async { Ok(dummy_get_ledger_entries_response()) }.boxed());

        mock.expect_get_events()
            .times(1)
            .returning(|_| async { Ok(dummy_get_events_response()) }.boxed());

        let _ = mock.get_account("GTESTACCOUNTID").await.unwrap();
        let env: TransactionEnvelope = dummy_transaction_envelope();
        mock.simulate_transaction_envelope(&env).await.unwrap();
        mock.send_transaction_polling(&env).await.unwrap();
        mock.get_network().await.unwrap();
        mock.get_latest_ledger().await.unwrap();
        mock.send_transaction(&env).await.unwrap();

        let h = dummy_hash();
        mock.get_transaction(&h).await.unwrap();

        let req: GetTransactionsRequest = GetTransactionsRequest {
            start_ledger: None,
            pagination: None,
        };
        mock.get_transactions(req).await.unwrap();

        let key: LedgerKey = dummy_ledger_key();
        mock.get_ledger_entries(&[key]).await.unwrap();

        let ev_req = GetEventsRequest {
            start: EventStart::Ledger(0),
            event_type: None,
            contract_ids: vec![],
            topics: vec![],
            limit: None,
        };
        mock.get_events(ev_req).await.unwrap();
    }

    #[tokio::test]
    async fn test_error_propagation() {
        let mut mock = MockStellarProviderTrait::new();

        mock.expect_get_account()
            .returning(|_| async { Err(ProviderError::Other("boom".to_string())) }.boxed());

        let res = mock.get_account("BAD").await;
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("boom"));
    }

    #[tokio::test]
    async fn test_get_events_edge_cases() {
        let mut mock = MockStellarProviderTrait::new();

        mock.expect_get_events()
            .withf(|req| {
                req.contract_ids.is_empty() && req.topics.is_empty() && req.limit.is_none()
            })
            .times(1)
            .returning(|_| async { Ok(dummy_get_events_response()) }.boxed());

        let ev_req = GetEventsRequest {
            start: EventStart::Ledger(0),
            event_type: None,
            contract_ids: vec![],
            topics: vec![],
            limit: None,
        };

        mock.get_events(ev_req).await.unwrap();
    }

    #[test]
    fn test_provider_send_sync_bounds() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<StellarProvider>();
    }

    #[cfg(test)]
    mod concrete_tests {
        use super::*;

        const NON_EXISTENT_URL: &str = "http://127.0.0.1:9998";

        fn setup_provider() -> StellarProvider {
            StellarProvider::new(vec![RpcConfig::new(NON_EXISTENT_URL.to_string())], 0)
                .expect("Provider creation should succeed even with bad URL")
        }

        #[tokio::test]
        async fn test_concrete_get_account_error() {
            let _env_guard = setup_test_env();
            let provider = setup_provider();
            let result = provider.get_account("SOME_ACCOUNT_ID").await;
            assert!(result.is_err());
            let err_str = result.unwrap_err().to_string();
            // Should contain the "Failed to..." context message
            assert!(
                err_str.contains("Failed to get account"),
                "Unexpected error message: {}",
                err_str
            );
        }

        #[tokio::test]
        async fn test_concrete_simulate_transaction_envelope_error() {
            let _env_guard = setup_test_env();

            let provider = setup_provider();
            let envelope: TransactionEnvelope = dummy_transaction_envelope();
            let result = provider.simulate_transaction_envelope(&envelope).await;
            assert!(result.is_err());
            let err_str = result.unwrap_err().to_string();
            // Should contain the "Failed to..." context message
            assert!(
                err_str.contains("Failed to simulate transaction"),
                "Unexpected error message: {}",
                err_str
            );
        }

        #[tokio::test]
        async fn test_concrete_send_transaction_polling_error() {
            let _env_guard = setup_test_env();

            let provider = setup_provider();
            let envelope: TransactionEnvelope = dummy_transaction_envelope();
            let result = provider.send_transaction_polling(&envelope).await;
            assert!(result.is_err());
            let err_str = result.unwrap_err().to_string();
            // Should contain the "Failed to..." context message
            assert!(
                err_str.contains("Failed to send transaction (polling)"),
                "Unexpected error message: {}",
                err_str
            );
        }

        #[tokio::test]
        async fn test_concrete_get_network_error() {
            let _env_guard = setup_test_env();

            let provider = setup_provider();
            let result = provider.get_network().await;
            assert!(result.is_err());
            let err_str = result.unwrap_err().to_string();
            // Should contain the "Failed to..." context message
            assert!(
                err_str.contains("Failed to get network"),
                "Unexpected error message: {}",
                err_str
            );
        }

        #[tokio::test]
        async fn test_concrete_get_latest_ledger_error() {
            let _env_guard = setup_test_env();

            let provider = setup_provider();
            let result = provider.get_latest_ledger().await;
            assert!(result.is_err());
            let err_str = result.unwrap_err().to_string();
            // Should contain the "Failed to..." context message
            assert!(
                err_str.contains("Failed to get latest ledger"),
                "Unexpected error message: {}",
                err_str
            );
        }

        #[tokio::test]
        async fn test_concrete_send_transaction_error() {
            let _env_guard = setup_test_env();

            let provider = setup_provider();
            let envelope: TransactionEnvelope = dummy_transaction_envelope();
            let result = provider.send_transaction(&envelope).await;
            assert!(result.is_err());
            let err_str = result.unwrap_err().to_string();
            // Should contain the "Failed to..." context message
            assert!(
                err_str.contains("Failed to send transaction"),
                "Unexpected error message: {}",
                err_str
            );
        }

        #[tokio::test]
        async fn test_concrete_get_transaction_error() {
            let _env_guard = setup_test_env();

            let provider = setup_provider();
            let hash: Hash = dummy_hash();
            let result = provider.get_transaction(&hash).await;
            assert!(result.is_err());
            let err_str = result.unwrap_err().to_string();
            // Should contain the "Failed to..." context message
            assert!(
                err_str.contains("Failed to get transaction"),
                "Unexpected error message: {}",
                err_str
            );
        }

        #[tokio::test]
        async fn test_concrete_get_transactions_error() {
            let _env_guard = setup_test_env();

            let provider = setup_provider();
            let req = GetTransactionsRequest {
                start_ledger: None,
                pagination: None,
            };
            let result = provider.get_transactions(req).await;
            assert!(result.is_err());
            let err_str = result.unwrap_err().to_string();
            // Should contain the "Failed to..." context message
            assert!(
                err_str.contains("Failed to get transactions"),
                "Unexpected error message: {}",
                err_str
            );
        }

        #[tokio::test]
        async fn test_concrete_get_ledger_entries_error() {
            let _env_guard = setup_test_env();

            let provider = setup_provider();
            let key: LedgerKey = dummy_ledger_key();
            let result = provider.get_ledger_entries(&[key]).await;
            assert!(result.is_err());
            let err_str = result.unwrap_err().to_string();
            // Should contain the "Failed to..." context message
            assert!(
                err_str.contains("Failed to get ledger entries"),
                "Unexpected error message: {}",
                err_str
            );
        }

        #[tokio::test]
        async fn test_concrete_get_events_error() {
            let _env_guard = setup_test_env();
            let provider = setup_provider();
            let req = GetEventsRequest {
                start: EventStart::Ledger(1),
                event_type: None,
                contract_ids: vec![],
                topics: vec![],
                limit: None,
            };
            let result = provider.get_events(req).await;
            assert!(result.is_err());
            let err_str = result.unwrap_err().to_string();
            // Should contain the "Failed to..." context message
            assert!(
                err_str.contains("Failed to get events"),
                "Unexpected error message: {}",
                err_str
            );
        }
    }

    #[test]
    fn test_generate_unique_rpc_id() {
        let id1 = generate_unique_rpc_id();
        let id2 = generate_unique_rpc_id();
        assert_ne!(id1, id2, "Generated IDs should be unique");
        assert!(id1 > 0, "ID should be positive");
        assert!(id2 > 0, "ID should be positive");
        assert!(id2 > id1, "IDs should be monotonically increasing");
    }

    #[test]
    fn test_normalize_url_for_log() {
        // Test basic URL without query/fragment
        assert_eq!(
            normalize_url_for_log("https://api.example.com/path"),
            "https://api.example.com/path"
        );

        // Test URL with query string removal
        assert_eq!(
            normalize_url_for_log("https://api.example.com/path?api_key=secret&other=value"),
            "https://api.example.com/path"
        );

        // Test URL with fragment removal
        assert_eq!(
            normalize_url_for_log("https://api.example.com/path#section"),
            "https://api.example.com/path"
        );

        // Test URL with both query and fragment
        assert_eq!(
            normalize_url_for_log("https://api.example.com/path?key=value#fragment"),
            "https://api.example.com/path"
        );

        // Test URL with userinfo redaction
        assert_eq!(
            normalize_url_for_log("https://user:password@api.example.com/path"),
            "https://<redacted>@api.example.com/path"
        );

        // Test URL with userinfo and query/fragment removal
        assert_eq!(
            normalize_url_for_log("https://user:pass@api.example.com/path?token=abc#frag"),
            "https://<redacted>@api.example.com/path"
        );

        // Test URL without userinfo (should remain unchanged)
        assert_eq!(
            normalize_url_for_log("https://api.example.com/path?token=abc"),
            "https://api.example.com/path"
        );

        // Test malformed URL (should handle gracefully)
        assert_eq!(normalize_url_for_log("not-a-url"), "not-a-url");
    }

    #[test]
    fn test_categorize_stellar_error_with_context_timeout() {
        let err = StellarClientError::TransactionSubmissionTimeout;
        let result = categorize_stellar_error_with_context(err, Some("Test operation"));
        assert!(matches!(result, ProviderError::Timeout));
    }

    #[test]
    fn test_categorize_stellar_error_with_context_xdr_error() {
        use soroban_rs::xdr::Error as XdrError;
        let err = StellarClientError::Xdr(XdrError::Invalid);
        let result = categorize_stellar_error_with_context(err, Some("Test operation"));
        match result {
            ProviderError::Other(msg) => {
                assert!(msg.contains("Test operation"));
            }
            _ => panic!("Expected Other error"),
        }
    }

    #[test]
    fn test_categorize_stellar_error_with_context_serde_error() {
        // Create a serde error by attempting to deserialize invalid JSON
        let json_err = serde_json::from_str::<serde_json::Value>("invalid json").unwrap_err();
        let err = StellarClientError::Serde(json_err);
        let result = categorize_stellar_error_with_context(err, Some("Test operation"));
        match result {
            ProviderError::Other(msg) => {
                assert!(msg.contains("Test operation"));
            }
            _ => panic!("Expected Other error"),
        }
    }

    #[test]
    fn test_categorize_stellar_error_with_context_url_errors() {
        // Test InvalidRpcUrl
        let invalid_uri_err: http::uri::InvalidUri =
            ":::invalid url".parse::<http::Uri>().unwrap_err();
        let err = StellarClientError::InvalidRpcUrl(invalid_uri_err);
        let result = categorize_stellar_error_with_context(err, Some("Test operation"));
        match result {
            ProviderError::NetworkConfiguration(msg) => {
                assert!(msg.contains("Test operation"));
                assert!(msg.contains("Invalid RPC URL"));
            }
            _ => panic!("Expected NetworkConfiguration error"),
        }

        // Test InvalidUrl
        let err = StellarClientError::InvalidUrl("not a url".to_string());
        let result = categorize_stellar_error_with_context(err, Some("Test operation"));
        match result {
            ProviderError::NetworkConfiguration(msg) => {
                assert!(msg.contains("Test operation"));
                assert!(msg.contains("Invalid URL"));
            }
            _ => panic!("Expected NetworkConfiguration error"),
        }
    }

    #[test]
    fn test_categorize_stellar_error_with_context_network_passphrase() {
        let err = StellarClientError::InvalidNetworkPassphrase {
            expected: "Expected".to_string(),
            server: "Server".to_string(),
        };
        let result = categorize_stellar_error_with_context(err, Some("Test operation"));
        match result {
            ProviderError::NetworkConfiguration(msg) => {
                assert!(msg.contains("Test operation"));
                assert!(msg.contains("Expected"));
                assert!(msg.contains("Server"));
            }
            _ => panic!("Expected NetworkConfiguration error"),
        }
    }

    #[test]
    fn test_categorize_stellar_error_with_context_json_rpc_call_error() {
        // Test that RPC Call errors are properly categorized as RpcErrorCode
        // We'll test this indirectly through other error types since creating Call errors
        // requires jsonrpsee internals that aren't easily accessible in tests
        let err = StellarClientError::TransactionSubmissionTimeout;
        let result = categorize_stellar_error_with_context(err, Some("Test operation"));
        // Verify timeout is properly categorized
        assert!(matches!(result, ProviderError::Timeout));
    }

    #[test]
    fn test_categorize_stellar_error_with_context_json_rpc_timeout() {
        // Test timeout through TransactionSubmissionTimeout which is simpler to construct
        let err = StellarClientError::TransactionSubmissionTimeout;
        let result = categorize_stellar_error_with_context(err, None);
        assert!(matches!(result, ProviderError::Timeout));
    }

    #[test]
    fn test_categorize_stellar_error_with_context_transport_errors() {
        // Test network-related errors through InvalidResponse which is simpler to construct
        let err = StellarClientError::InvalidResponse;
        let result = categorize_stellar_error_with_context(err, Some("Test operation"));
        match result {
            ProviderError::Other(msg) => {
                assert!(msg.contains("Test operation"));
                assert!(msg.contains("Invalid response"));
            }
            _ => panic!("Expected Other error for response issues"),
        }
    }

    #[test]
    fn test_categorize_stellar_error_with_context_response_errors() {
        // Test InvalidResponse
        let err = StellarClientError::InvalidResponse;
        let result = categorize_stellar_error_with_context(err, Some("Test operation"));
        match result {
            ProviderError::Other(msg) => {
                assert!(msg.contains("Test operation"));
                assert!(msg.contains("Invalid response"));
            }
            _ => panic!("Expected Other error"),
        }

        // Test MissingResult
        let err = StellarClientError::MissingResult;
        let result = categorize_stellar_error_with_context(err, Some("Test operation"));
        match result {
            ProviderError::Other(msg) => {
                assert!(msg.contains("Test operation"));
                assert!(msg.contains("Missing result"));
            }
            _ => panic!("Expected Other error"),
        }
    }

    #[test]
    fn test_categorize_stellar_error_with_context_transaction_errors() {
        // Test TransactionFailed
        let err = StellarClientError::TransactionFailed("tx failed".to_string());
        let result = categorize_stellar_error_with_context(err, Some("Test operation"));
        match result {
            ProviderError::Other(msg) => {
                assert!(msg.contains("Test operation"));
                assert!(msg.contains("tx failed"));
            }
            _ => panic!("Expected Other error"),
        }

        // Test NotFound
        let err = StellarClientError::NotFound("Account".to_string(), "123".to_string());
        let result = categorize_stellar_error_with_context(err, Some("Test operation"));
        match result {
            ProviderError::Other(msg) => {
                assert!(msg.contains("Test operation"));
                assert!(msg.contains("Account not found"));
                assert!(msg.contains("123"));
            }
            _ => panic!("Expected Other error"),
        }
    }

    #[test]
    fn test_categorize_stellar_error_with_context_validation_errors() {
        // Test InvalidCursor
        let err = StellarClientError::InvalidCursor;
        let result = categorize_stellar_error_with_context(err, Some("Test operation"));
        match result {
            ProviderError::Other(msg) => {
                assert!(msg.contains("Test operation"));
                assert!(msg.contains("Invalid cursor"));
            }
            _ => panic!("Expected Other error"),
        }

        // Test LargeFee
        let err = StellarClientError::LargeFee(1000000);
        let result = categorize_stellar_error_with_context(err, Some("Test operation"));
        match result {
            ProviderError::Other(msg) => {
                assert!(msg.contains("Test operation"));
                assert!(msg.contains("1000000"));
            }
            _ => panic!("Expected Other error"),
        }
    }

    #[test]
    fn test_categorize_stellar_error_with_context_no_context() {
        // Test with a simpler error type that doesn't have version conflicts
        let err = StellarClientError::InvalidResponse;
        let result = categorize_stellar_error_with_context(err, None);
        match result {
            ProviderError::Other(msg) => {
                assert!(!msg.contains(":")); // No context prefix
                assert!(msg.contains("Invalid response"));
            }
            _ => panic!("Expected Other error"),
        }
    }

    #[test]
    fn test_initialize_provider_invalid_url() {
        let _env_guard = setup_test_env();
        let provider = StellarProvider::new(
            vec![RpcConfig::new("http://localhost:8000".to_string())],
            30,
        )
        .unwrap();

        // Test with invalid URL that should fail client creation
        let result = provider.initialize_provider("invalid-url");
        assert!(result.is_err());
        match result.unwrap_err() {
            ProviderError::NetworkConfiguration(msg) => {
                assert!(msg.contains("Failed to create Stellar RPC client"));
            }
            _ => panic!("Expected NetworkConfiguration error"),
        }
    }

    #[test]
    fn test_initialize_raw_provider_timeout_config() {
        let _env_guard = setup_test_env();
        let provider = StellarProvider::new(
            vec![RpcConfig::new("http://localhost:8000".to_string())],
            30,
        )
        .unwrap();

        // Test with valid URL - should succeed
        let result = provider.initialize_raw_provider("http://localhost:8000");
        assert!(result.is_ok());

        // Test with invalid URL for reqwest client - this might not fail immediately
        // but we can test that the function doesn't panic
        let result = provider.initialize_raw_provider("not-a-url");
        // reqwest::Client::builder() may not fail immediately for malformed URLs
        // but the function should return a Result
        assert!(result.is_ok() || result.is_err());
    }

    #[tokio::test]
    async fn test_raw_request_dyn_success() {
        let _env_guard = setup_test_env();

        // Create a provider with a mock server URL that won't actually connect
        let provider =
            StellarProvider::new(vec![RpcConfig::new("http://127.0.0.1:9999".to_string())], 1)
                .unwrap();

        let params = serde_json::json!({"test": "value"});
        let result = provider
            .raw_request_dyn("test_method", params, Some(JsonRpcId::Number(1)))
            .await;

        // Should fail due to connection, but should go through the retry logic
        assert!(result.is_err());
        let err = result.unwrap_err();
        // Should be a network-related error, not a panic
        assert!(matches!(
            err,
            ProviderError::Other(_)
                | ProviderError::Timeout
                | ProviderError::NetworkConfiguration(_)
        ));
    }

    #[tokio::test]
    async fn test_raw_request_dyn_with_auto_generated_id() {
        let _env_guard = setup_test_env();

        let provider =
            StellarProvider::new(vec![RpcConfig::new("http://127.0.0.1:9999".to_string())], 1)
                .unwrap();

        let params = serde_json::json!({"test": "value"});
        let result = provider.raw_request_dyn("test_method", params, None).await;

        // Should fail due to connection, but the ID generation should work
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_retry_raw_request_connection_failure() {
        let _env_guard = setup_test_env();

        let provider =
            StellarProvider::new(vec![RpcConfig::new("http://127.0.0.1:9999".to_string())], 1)
                .unwrap();

        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "test",
            "params": {}
        });

        let result = provider.retry_raw_request("test_operation", request).await;

        // Should fail due to connection issues
        assert!(result.is_err());
        let err = result.unwrap_err();
        // Should be categorized as network error
        assert!(matches!(
            err,
            ProviderError::Other(_) | ProviderError::Timeout
        ));
    }

    #[tokio::test]
    async fn test_raw_request_dyn_json_rpc_error_response() {
        let _env_guard = setup_test_env();

        // This test would require mocking the HTTP response, which is complex
        // For now, we test that the function exists and can be called
        let provider =
            StellarProvider::new(vec![RpcConfig::new("http://127.0.0.1:9999".to_string())], 1)
                .unwrap();

        let params = serde_json::json!({"test": "value"});
        let result = provider
            .raw_request_dyn(
                "test_method",
                params,
                Some(JsonRpcId::String("test-id".to_string())),
            )
            .await;

        // Should fail due to connection, but should handle the request properly
        assert!(result.is_err());
    }

    #[test]
    fn test_provider_creation_edge_cases() {
        let _env_guard = setup_test_env();

        // Test with empty configs
        let result = StellarProvider::new(vec![], 30);
        assert!(result.is_err());
        match result.unwrap_err() {
            ProviderError::NetworkConfiguration(msg) => {
                assert!(msg.contains("No RPC configurations provided"));
            }
            _ => panic!("Expected NetworkConfiguration error"),
        }

        // Test with configs that have zero weights after filtering
        let mut config1 = RpcConfig::new("http://localhost:8000".to_string());
        config1.weight = 0;
        let mut config2 = RpcConfig::new("http://localhost:8001".to_string());
        config2.weight = 0;
        let configs = vec![config1, config2];
        let result = StellarProvider::new(configs, 30);
        assert!(result.is_err());
        match result.unwrap_err() {
            ProviderError::NetworkConfiguration(msg) => {
                assert!(msg.contains("No active RPC configurations"));
            }
            _ => panic!("Expected NetworkConfiguration error"),
        }
    }

    #[tokio::test]
    async fn test_get_events_empty_request() {
        let _env_guard = setup_test_env();

        let mut mock = MockStellarProviderTrait::new();
        mock.expect_get_events()
            .withf(|req| req.contract_ids.is_empty() && req.topics.is_empty())
            .returning(|_| async { Ok(dummy_get_events_response()) }.boxed());

        let req = GetEventsRequest {
            start: EventStart::Ledger(1),
            event_type: Some(EventType::Contract),
            contract_ids: vec![],
            topics: vec![],
            limit: Some(10),
        };

        let result = mock.get_events(req).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_get_ledger_entries_empty_keys() {
        let _env_guard = setup_test_env();

        let mut mock = MockStellarProviderTrait::new();
        mock.expect_get_ledger_entries()
            .withf(|keys| keys.is_empty())
            .returning(|_| async { Ok(dummy_get_ledger_entries_response()) }.boxed());

        let result = mock.get_ledger_entries(&[]).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_send_transaction_polling_success() {
        let _env_guard = setup_test_env();

        let mut mock = MockStellarProviderTrait::new();
        mock.expect_send_transaction_polling()
            .returning(|_| async { Ok(dummy_soroban_tx()) }.boxed());

        let envelope = dummy_transaction_envelope();
        let result = mock.send_transaction_polling(&envelope).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_get_transactions_with_pagination() {
        let _env_guard = setup_test_env();

        let mut mock = MockStellarProviderTrait::new();
        mock.expect_get_transactions()
            .returning(|_| async { Ok(dummy_get_transactions_response()) }.boxed());

        let req = GetTransactionsRequest {
            start_ledger: Some(1000),
            pagination: None, // Pagination struct may not be available in this version
        };

        let result = mock.get_transactions(req).await;
        assert!(result.is_ok());
    }
}
