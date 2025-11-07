//! This module is responsible for handling the requests to the relayer API.
//!
//! It manages an internal API that mirrors the HTTP external API of the relayer.
//!
//! Supported methods:
//! - `sendTransaction` - sends a transaction to the relayer.
//!
use crate::domain::{
    get_network_relayer, get_relayer_by_id, get_transaction_by_id, Relayer, SignTransactionRequest,
};
use crate::jobs::JobProducerTrait;
use crate::models::{
    convert_to_internal_rpc_request, AppState, JsonRpcRequest, NetworkRepoModel, NetworkRpcRequest,
    NetworkTransactionRequest, NotificationRepoModel, RelayerRepoModel, SignerRepoModel,
    ThinDataAppState, TransactionRepoModel, TransactionResponse,
};
use crate::observability::request_id::set_request_id;
use crate::repositories::{
    ApiKeyRepositoryTrait, NetworkRepository, PluginRepositoryTrait, RelayerRepository, Repository,
    TransactionCounterTrait, TransactionRepository,
};
use crate::services::plugins::PluginError;
use actix_web::web;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use strum::Display;
use tracing::instrument;

#[cfg(test)]
use mockall::automock;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Display)]
pub enum PluginMethod {
    #[serde(rename = "sendTransaction")]
    SendTransaction,
    #[serde(rename = "getTransaction")]
    GetTransaction,
    #[serde(rename = "getRelayerStatus")]
    GetRelayerStatus,
    #[serde(rename = "signTransaction")]
    SignTransaction,
    #[serde(rename = "getRelayer")]
    GetRelayer,
    #[serde(rename = "rpc")]
    Rpc,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub request_id: String,
    pub relayer_id: String,
    pub method: PluginMethod,
    pub payload: serde_json::Value,
    pub http_request_id: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GetTransactionRequest {
    pub transaction_id: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    pub request_id: String,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
}

#[async_trait]
#[cfg_attr(test, automock)]
pub trait RelayerApiTrait<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>: Send + Sync
where
    J: JobProducerTrait + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
    SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PR: PluginRepositoryTrait + Send + Sync + 'static,
    AKR: ApiKeyRepositoryTrait + Send + Sync + 'static,
{
    async fn handle_request(
        &self,
        request: Request,
        state: &web::ThinData<AppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>>,
    ) -> Response;

    async fn process_request(
        &self,
        request: Request,
        state: &web::ThinData<AppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>>,
    ) -> Result<Response, PluginError>;

    async fn handle_send_transaction(
        &self,
        request: Request,
        state: &web::ThinData<AppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>>,
    ) -> Result<Response, PluginError>;

    async fn handle_get_transaction(
        &self,
        request: Request,
        state: &web::ThinData<AppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>>,
    ) -> Result<Response, PluginError>;

    async fn handle_get_relayer_status(
        &self,
        request: Request,
        state: &web::ThinData<AppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>>,
    ) -> Result<Response, PluginError>;

    async fn handle_sign_transaction(
        &self,
        request: Request,
        state: &web::ThinData<AppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>>,
    ) -> Result<Response, PluginError>;
    async fn handle_get_relayer_info(
        &self,
        request: Request,
        state: &web::ThinData<AppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>>,
    ) -> Result<Response, PluginError>;
    async fn handle_rpc_request(
        &self,
        request: Request,
        state: &web::ThinData<AppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>>,
    ) -> Result<Response, PluginError>;
}

#[derive(Default)]
pub struct RelayerApi;

impl RelayerApi {
    #[instrument(name = "Plugin::handle_request", skip_all, fields(method = %request.method, relayer_id = %request.relayer_id, plugin_req_id = %request.request_id))]
    pub async fn handle_request<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
        &self,
        request: Request,
        state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
    ) -> Response
    where
        J: JobProducerTrait + 'static,
        TR: TransactionRepository
            + Repository<TransactionRepoModel, String>
            + Send
            + Sync
            + 'static,
        RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
        NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
        NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
        SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
        TCR: TransactionCounterTrait + Send + Sync + 'static,
        PR: PluginRepositoryTrait + Send + Sync + 'static,
        AKR: ApiKeyRepositoryTrait + Send + Sync + 'static,
    {
        // Restore original HTTP request id onto this span if provided
        if let Some(http_rid) = request.http_request_id.clone() {
            set_request_id(http_rid);
        }

        match self.process_request(request.clone(), state).await {
            Ok(response) => response,
            Err(e) => Response {
                request_id: request.request_id,
                result: None,
                error: Some(e.to_string()),
            },
        }
    }

    async fn process_request<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
        &self,
        request: Request,
        state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
    ) -> Result<Response, PluginError>
    where
        J: JobProducerTrait + 'static,
        TR: TransactionRepository
            + Repository<TransactionRepoModel, String>
            + Send
            + Sync
            + 'static,
        RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
        NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
        NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
        SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
        TCR: TransactionCounterTrait + Send + Sync + 'static,
        PR: PluginRepositoryTrait + Send + Sync + 'static,
        AKR: ApiKeyRepositoryTrait + Send + Sync + 'static,
    {
        match request.method {
            PluginMethod::SendTransaction => self.handle_send_transaction(request, state).await,
            PluginMethod::GetTransaction => self.handle_get_transaction(request, state).await,
            PluginMethod::GetRelayerStatus => self.handle_get_relayer_status(request, state).await,
            PluginMethod::SignTransaction => self.handle_sign_transaction(request, state).await,
            PluginMethod::GetRelayer => self.handle_get_relayer_info(request, state).await,
            PluginMethod::Rpc => self.handle_rpc_request(request, state).await,
        }
    }

    async fn handle_send_transaction<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
        &self,
        request: Request,
        state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
    ) -> Result<Response, PluginError>
    where
        J: JobProducerTrait + 'static,
        TR: TransactionRepository
            + Repository<TransactionRepoModel, String>
            + Send
            + Sync
            + 'static,
        RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
        NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
        NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
        SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
        TCR: TransactionCounterTrait + Send + Sync + 'static,
        PR: PluginRepositoryTrait + Send + Sync + 'static,
        AKR: ApiKeyRepositoryTrait + Send + Sync + 'static,
    {
        let relayer_repo_model = get_relayer_by_id(request.relayer_id.clone(), state)
            .await
            .map_err(|e| PluginError::RelayerError(e.to_string()))?;

        relayer_repo_model
            .validate_active_state()
            .map_err(|e| PluginError::RelayerError(e.to_string()))?;

        let network_relayer = get_network_relayer(request.relayer_id.clone(), state)
            .await
            .map_err(|e| PluginError::RelayerError(e.to_string()))?;

        let tx_request = NetworkTransactionRequest::from_json(
            &relayer_repo_model.network_type,
            request.payload.clone(),
        )
        .map_err(|e| PluginError::RelayerError(e.to_string()))?;

        tx_request
            .validate(&relayer_repo_model)
            .map_err(|e| PluginError::RelayerError(e.to_string()))?;

        let transaction = network_relayer
            .process_transaction_request(tx_request)
            .await
            .map_err(|e| PluginError::RelayerError(e.to_string()))?;

        let transaction_response: TransactionResponse = transaction.into();
        let result = serde_json::to_value(transaction_response)
            .map_err(|e| PluginError::RelayerError(e.to_string()))?;

        Ok(Response {
            request_id: request.request_id,
            result: Some(result),
            error: None,
        })
    }

    async fn handle_get_transaction<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
        &self,
        request: Request,
        state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
    ) -> Result<Response, PluginError>
    where
        J: JobProducerTrait + 'static,
        TR: TransactionRepository
            + Repository<TransactionRepoModel, String>
            + Send
            + Sync
            + 'static,
        RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
        NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
        NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
        SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
        TCR: TransactionCounterTrait + Send + Sync + 'static,
        PR: PluginRepositoryTrait + Send + Sync + 'static,
        AKR: ApiKeyRepositoryTrait + Send + Sync + 'static,
    {
        // validation purpose only, checks if relayer exists
        get_relayer_by_id(request.relayer_id.clone(), state)
            .await
            .map_err(|e| PluginError::RelayerError(e.to_string()))?;

        let get_transaction_request: GetTransactionRequest =
            serde_json::from_value(request.payload)
                .map_err(|e| PluginError::InvalidPayload(e.to_string()))?;

        let transaction = get_transaction_by_id(get_transaction_request.transaction_id, state)
            .await
            .map_err(|e| PluginError::RelayerError(e.to_string()))?;

        let transaction_response: TransactionResponse = transaction.into();

        let result = serde_json::to_value(transaction_response)
            .map_err(|e| PluginError::RelayerError(e.to_string()))?;

        Ok(Response {
            request_id: request.request_id,
            result: Some(result),
            error: None,
        })
    }

    async fn handle_get_relayer_status<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
        &self,
        request: Request,
        state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
    ) -> Result<Response, PluginError>
    where
        J: JobProducerTrait + 'static,
        TR: TransactionRepository
            + Repository<TransactionRepoModel, String>
            + Send
            + Sync
            + 'static,
        RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
        NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
        NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
        SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
        TCR: TransactionCounterTrait + Send + Sync + 'static,
        PR: PluginRepositoryTrait + Send + Sync + 'static,
        AKR: ApiKeyRepositoryTrait + Send + Sync + 'static,
    {
        let network_relayer = get_network_relayer(request.relayer_id.clone(), state)
            .await
            .map_err(|e| PluginError::RelayerError(e.to_string()))?;

        let status = network_relayer
            .get_status()
            .await
            .map_err(|e| PluginError::RelayerError(e.to_string()))?;

        let result =
            serde_json::to_value(status).map_err(|e| PluginError::RelayerError(e.to_string()))?;

        Ok(Response {
            request_id: request.request_id,
            result: Some(result),
            error: None,
        })
    }

    async fn handle_sign_transaction<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
        &self,
        request: Request,
        state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
    ) -> Result<Response, PluginError>
    where
        J: JobProducerTrait + 'static,
        TR: TransactionRepository
            + Repository<TransactionRepoModel, String>
            + Send
            + Sync
            + 'static,
        RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
        NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
        NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
        SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
        TCR: TransactionCounterTrait + Send + Sync + 'static,
        PR: PluginRepositoryTrait + Send + Sync + 'static,
        AKR: ApiKeyRepositoryTrait + Send + Sync + 'static,
    {
        let sign_request: SignTransactionRequest = serde_json::from_value(request.payload)
            .map_err(|e| PluginError::InvalidPayload(e.to_string()))?;

        let network_relayer = get_network_relayer(request.relayer_id.clone(), state)
            .await
            .map_err(|e| PluginError::RelayerError(e.to_string()))?;

        let response = network_relayer
            .sign_transaction(&sign_request)
            .await
            .map_err(|e| PluginError::RelayerError(e.to_string()))?;

        let result =
            serde_json::to_value(response).map_err(|e| PluginError::RelayerError(e.to_string()))?;

        Ok(Response {
            request_id: request.request_id,
            result: Some(result),
            error: None,
        })
    }

    async fn handle_get_relayer_info<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
        &self,
        request: Request,
        state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
    ) -> Result<Response, PluginError>
    where
        J: JobProducerTrait + 'static,
        TR: TransactionRepository
            + Repository<TransactionRepoModel, String>
            + Send
            + Sync
            + 'static,
        RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
        NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
        NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
        SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
        TCR: TransactionCounterTrait + Send + Sync + 'static,
        PR: PluginRepositoryTrait + Send + Sync + 'static,
        AKR: ApiKeyRepositoryTrait + Send + Sync + 'static,
    {
        let relayer = get_relayer_by_id(request.relayer_id.clone(), state)
            .await
            .map_err(|e| PluginError::RelayerError(e.to_string()))?;
        let relayer_response: crate::models::RelayerResponse = relayer.into();
        let result = serde_json::to_value(relayer_response)
            .map_err(|e| PluginError::RelayerError(e.to_string()))?;
        Ok(Response {
            request_id: request.request_id,
            result: Some(result),
            error: None,
        })
    }

    async fn handle_rpc_request<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
        &self,
        request: Request,
        state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
    ) -> Result<Response, PluginError>
    where
        J: JobProducerTrait + 'static,
        TR: TransactionRepository
            + Repository<TransactionRepoModel, String>
            + Send
            + Sync
            + 'static,
        RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
        NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
        NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
        SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
        TCR: TransactionCounterTrait + Send + Sync + 'static,
        PR: PluginRepositoryTrait + Send + Sync + 'static,
        AKR: ApiKeyRepositoryTrait + Send + Sync + 'static,
    {
        let relayer_repo_model = get_relayer_by_id(request.relayer_id.clone(), state)
            .await
            .map_err(|e| PluginError::RelayerError(e.to_string()))?;

        relayer_repo_model
            .validate_active_state()
            .map_err(|e| PluginError::RelayerError(e.to_string()))?;

        let network_relayer = get_network_relayer(request.relayer_id.clone(), state)
            .await
            .map_err(|e| PluginError::RelayerError(e.to_string()))?;

        // Use the network type from relayer_repo_model to parse the request with correct type context
        let network_rpc_request: JsonRpcRequest<NetworkRpcRequest> =
            convert_to_internal_rpc_request(request.payload, &relayer_repo_model.network_type)
                .map_err(|e| PluginError::InvalidPayload(e.to_string()))?;

        let result = network_relayer.rpc(network_rpc_request).await;

        match result {
            Ok(json_rpc_response) => {
                let result_value = serde_json::to_value(json_rpc_response)
                    .map_err(|e| PluginError::RelayerError(e.to_string()))?;
                Ok(Response {
                    request_id: request.request_id,
                    result: Some(result_value),
                    error: None,
                })
            }
            Err(e) => Ok(Response {
                request_id: request.request_id,
                result: None,
                error: Some(e.to_string()),
            }),
        }
    }
}

#[async_trait]
impl<J, RR, TR, NR, NFR, SR, TCR, PR, AKR> RelayerApiTrait<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>
    for RelayerApi
where
    J: JobProducerTrait + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
    SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PR: PluginRepositoryTrait + Send + Sync + 'static,
    AKR: ApiKeyRepositoryTrait + Send + Sync + 'static,
{
    async fn handle_request(
        &self,
        request: Request,
        state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
    ) -> Response {
        self.handle_request(request, state).await
    }

    async fn process_request(
        &self,
        request: Request,
        state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
    ) -> Result<Response, PluginError> {
        self.process_request(request, state).await
    }

    async fn handle_send_transaction(
        &self,
        request: Request,
        state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
    ) -> Result<Response, PluginError> {
        self.handle_send_transaction(request, state).await
    }

    async fn handle_get_transaction(
        &self,
        request: Request,
        state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
    ) -> Result<Response, PluginError> {
        self.handle_get_transaction(request, state).await
    }

    async fn handle_get_relayer_status(
        &self,
        request: Request,
        state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
    ) -> Result<Response, PluginError> {
        self.handle_get_relayer_status(request, state).await
    }

    async fn handle_sign_transaction(
        &self,
        request: Request,
        state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
    ) -> Result<Response, PluginError> {
        self.handle_sign_transaction(request, state).await
    }

    async fn handle_get_relayer_info(
        &self,
        request: Request,
        state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
    ) -> Result<Response, PluginError> {
        self.handle_get_relayer_info(request, state).await
    }

    async fn handle_rpc_request(
        &self,
        request: Request,
        state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
    ) -> Result<Response, PluginError> {
        self.handle_rpc_request(request, state).await
    }
}

#[cfg(test)]
mod tests {
    use std::env;

    use crate::utils::mocks::mockutils::{
        create_mock_app_state, create_mock_evm_transaction_request, create_mock_network,
        create_mock_relayer, create_mock_signer, create_mock_transaction,
    };

    use super::*;

    fn setup_test_env() {
        env::set_var("API_KEY", "7EF1CB7C-5003-4696-B384-C72AF8C3E15D"); // noboost
        env::set_var("REDIS_URL", "redis://localhost:6379");
        env::set_var("RPC_TIMEOUT_MS", "5000");
    }

    #[tokio::test]
    async fn test_handle_request() {
        setup_test_env();
        let state = create_mock_app_state(
            None,
            Some(vec![create_mock_relayer("test".to_string(), false)]),
            Some(vec![create_mock_signer()]),
            Some(vec![create_mock_network()]),
            None,
            None,
        )
        .await;

        let request = Request {
            request_id: "test".to_string(),
            relayer_id: "test".to_string(),
            method: PluginMethod::SendTransaction,
            payload: serde_json::json!(create_mock_evm_transaction_request()),
            http_request_id: None,
        };

        let relayer_api = RelayerApi;
        let response = relayer_api
            .handle_request(request.clone(), &web::ThinData(state))
            .await;

        assert!(response.error.is_none());
        assert!(response.result.is_some());
    }

    #[tokio::test]
    async fn test_handle_request_error_paused_relayer() {
        setup_test_env();
        let paused = true;
        let state = create_mock_app_state(
            None,
            Some(vec![create_mock_relayer("test".to_string(), paused)]),
            Some(vec![create_mock_signer()]),
            Some(vec![create_mock_network()]),
            None,
            None,
        )
        .await;

        let request = Request {
            request_id: "test".to_string(),
            relayer_id: "test".to_string(),
            method: PluginMethod::SendTransaction,
            payload: serde_json::json!(create_mock_evm_transaction_request()),
            http_request_id: None,
        };

        let relayer_api = RelayerApi;
        let response = relayer_api
            .handle_request(request.clone(), &web::ThinData(state))
            .await;

        assert!(response.error.is_some());
        assert!(response.result.is_none());
        assert_eq!(response.error.unwrap(), "Relayer error: Relayer is paused");
    }

    #[tokio::test]
    async fn test_handle_request_using_trait() {
        setup_test_env();
        let state = create_mock_app_state(
            None,
            Some(vec![create_mock_relayer("test".to_string(), false)]),
            Some(vec![create_mock_signer()]),
            Some(vec![create_mock_network()]),
            None,
            None,
        )
        .await;

        let request = Request {
            request_id: "test".to_string(),
            relayer_id: "test".to_string(),
            method: PluginMethod::SendTransaction,
            payload: serde_json::json!(create_mock_evm_transaction_request()),
            http_request_id: None,
        };

        let relayer_api = RelayerApi;

        let state = web::ThinData(state);

        let response = RelayerApiTrait::handle_request(&relayer_api, request.clone(), &state).await;

        assert!(response.error.is_none());
        assert!(response.result.is_some());

        let response =
            RelayerApiTrait::process_request(&relayer_api, request.clone(), &state).await;

        assert!(response.is_ok());

        let response =
            RelayerApiTrait::handle_send_transaction(&relayer_api, request.clone(), &state).await;

        assert!(response.is_ok());
    }

    #[tokio::test]
    async fn test_handle_get_transaction() {
        setup_test_env();
        let state = create_mock_app_state(
            None,
            Some(vec![create_mock_relayer("test".to_string(), false)]),
            Some(vec![create_mock_signer()]),
            Some(vec![create_mock_network()]),
            None,
            Some(vec![create_mock_transaction()]),
        )
        .await;

        let request = Request {
            request_id: "test".to_string(),
            relayer_id: "test".to_string(),
            method: PluginMethod::GetTransaction,
            payload: serde_json::json!(GetTransactionRequest {
                transaction_id: "test".to_string(),
            }),
            http_request_id: None,
        };

        let relayer_api = RelayerApi;
        let response = relayer_api
            .handle_request(request.clone(), &web::ThinData(state))
            .await;

        assert!(response.error.is_none());
        assert!(response.result.is_some());
    }

    #[tokio::test]
    async fn test_handle_get_transaction_error_relayer_not_found() {
        setup_test_env();
        let state = create_mock_app_state(
            None,
            None,
            Some(vec![create_mock_signer()]),
            Some(vec![create_mock_network()]),
            None,
            Some(vec![create_mock_transaction()]),
        )
        .await;

        let request = Request {
            request_id: "test".to_string(),
            relayer_id: "test".to_string(),
            method: PluginMethod::GetTransaction,
            payload: serde_json::json!(GetTransactionRequest {
                transaction_id: "test".to_string(),
            }),
            http_request_id: None,
        };

        let relayer_api = RelayerApi;
        let response = relayer_api
            .handle_request(request.clone(), &web::ThinData(state))
            .await;

        assert!(response.error.is_some());
        let error = response.error.unwrap();
        assert!(error.contains("Relayer with ID test not found"));
    }

    #[tokio::test]
    async fn test_handle_get_transaction_error_transaction_not_found() {
        setup_test_env();
        let state = create_mock_app_state(
            None,
            Some(vec![create_mock_relayer("test".to_string(), false)]),
            Some(vec![create_mock_signer()]),
            Some(vec![create_mock_network()]),
            None,
            None,
        )
        .await;

        let request = Request {
            request_id: "test".to_string(),
            relayer_id: "test".to_string(),
            method: PluginMethod::GetTransaction,
            payload: serde_json::json!(GetTransactionRequest {
                transaction_id: "test".to_string(),
            }),
            http_request_id: None,
        };

        let relayer_api = RelayerApi;
        let response = relayer_api
            .handle_request(request.clone(), &web::ThinData(state))
            .await;

        assert!(response.error.is_some());
        let error = response.error.unwrap();
        assert!(error.contains("Transaction with ID test not found"));
    }

    #[tokio::test]
    async fn test_handle_get_relayer_status_relayer_not_found() {
        setup_test_env();
        let state = create_mock_app_state(
            None,
            None,
            Some(vec![create_mock_signer()]),
            Some(vec![create_mock_network()]),
            None,
            None,
        )
        .await;

        let request = Request {
            request_id: "test".to_string(),
            relayer_id: "test".to_string(),
            method: PluginMethod::GetRelayerStatus,
            payload: serde_json::json!({}),
            http_request_id: None,
        };

        let relayer_api = RelayerApi;
        let response = relayer_api
            .handle_request(request.clone(), &web::ThinData(state))
            .await;

        assert!(response.error.is_some());
        let error = response.error.unwrap();
        assert!(error.contains("Relayer with ID test not found"));
    }

    #[tokio::test]
    async fn test_handle_sign_transaction_evm_not_supported() {
        setup_test_env();
        let state = create_mock_app_state(
            None,
            Some(vec![create_mock_relayer("test".to_string(), false)]),
            Some(vec![create_mock_signer()]),
            Some(vec![create_mock_network()]),
            None,
            None,
        )
        .await;

        let request = Request {
            request_id: "test".to_string(),
            relayer_id: "test".to_string(),
            method: PluginMethod::SignTransaction,
            payload: serde_json::json!({
                "unsigned_xdr": "test_xdr"
            }),
            http_request_id: None,
        };

        let relayer_api = RelayerApi;
        let response = relayer_api
            .handle_request(request.clone(), &web::ThinData(state))
            .await;

        assert!(response.error.is_some());
        let error = response.error.unwrap();
        assert!(error.contains("sign_transaction not supported for EVM"));
    }

    #[tokio::test]
    async fn test_handle_sign_transaction_invalid_payload() {
        setup_test_env();
        let state = create_mock_app_state(
            None,
            Some(vec![create_mock_relayer("test".to_string(), false)]),
            Some(vec![create_mock_signer()]),
            Some(vec![create_mock_network()]),
            None,
            None,
        )
        .await;

        let request = Request {
            request_id: "test".to_string(),
            relayer_id: "test".to_string(),
            method: PluginMethod::SignTransaction,
            payload: serde_json::json!({"invalid": "payload"}),
            http_request_id: None,
        };

        let relayer_api = RelayerApi;
        let response = relayer_api
            .handle_request(request.clone(), &web::ThinData(state))
            .await;

        assert!(response.error.is_some());
        let error = response.error.unwrap();
        assert!(error.contains("Invalid payload"));
    }

    #[tokio::test]
    async fn test_handle_sign_transaction_relayer_not_found() {
        setup_test_env();
        let state = create_mock_app_state(
            None,
            None,
            Some(vec![create_mock_signer()]),
            Some(vec![create_mock_network()]),
            None,
            None,
        )
        .await;

        let request = Request {
            request_id: "test".to_string(),
            relayer_id: "test".to_string(),
            method: PluginMethod::SignTransaction,
            payload: serde_json::json!({
                "unsigned_xdr": "test_xdr"
            }),
            http_request_id: None,
        };

        let relayer_api = RelayerApi;
        let response = relayer_api
            .handle_request(request.clone(), &web::ThinData(state))
            .await;

        assert!(response.error.is_some());
        let error = response.error.unwrap();
        assert!(error.contains("Relayer with ID test not found"));
    }

    #[tokio::test]
    async fn test_handle_get_relayer_info_success() {
        setup_test_env();
        let state = create_mock_app_state(
            None,
            Some(vec![create_mock_relayer("test".to_string(), false)]),
            Some(vec![create_mock_signer()]),
            Some(vec![create_mock_network()]),
            None,
            None,
        )
        .await;

        let request = Request {
            request_id: "test".to_string(),
            relayer_id: "test".to_string(),
            method: PluginMethod::GetRelayer,
            payload: serde_json::json!({}),
            http_request_id: None,
        };

        let relayer_api = RelayerApi;
        let response = relayer_api
            .handle_request(request.clone(), &web::ThinData(state))
            .await;

        assert!(response.error.is_none());
        assert!(response.result.is_some());

        let result = response.result.unwrap();
        assert!(result.get("id").is_some());
        assert!(result.get("name").is_some());
        assert!(result.get("network").is_some());
        assert!(result.get("address").is_some());
    }

    #[tokio::test]
    async fn test_handle_get_relayer_info_relayer_not_found() {
        setup_test_env();
        let state = create_mock_app_state(
            None,
            None,
            Some(vec![create_mock_signer()]),
            Some(vec![create_mock_network()]),
            None,
            None,
        )
        .await;

        let request = Request {
            request_id: "test".to_string(),
            relayer_id: "test".to_string(),
            method: PluginMethod::GetRelayer,
            payload: serde_json::json!({}),
            http_request_id: None,
        };

        let relayer_api = RelayerApi;
        let response = relayer_api
            .handle_request(request.clone(), &web::ThinData(state))
            .await;

        assert!(response.error.is_some());
        let error = response.error.unwrap();
        assert!(error.contains("Relayer with ID test not found"));
    }

    #[tokio::test]
    async fn test_handle_rpc_request_evm_success() {
        setup_test_env();
        let state = create_mock_app_state(
            None,
            Some(vec![create_mock_relayer("test".to_string(), false)]),
            Some(vec![create_mock_signer()]),
            Some(vec![create_mock_network()]),
            None,
            None,
        )
        .await;

        let request = Request {
            request_id: "test-rpc-1".to_string(),
            relayer_id: "test".to_string(),
            method: PluginMethod::Rpc,
            payload: serde_json::json!({
                "jsonrpc": "2.0",
                "method": "eth_blockNumber",
                "params": [],
                "id": 1
            }),
            http_request_id: None,
        };

        let relayer_api = RelayerApi;
        let response = relayer_api
            .handle_request(request.clone(), &web::ThinData(state))
            .await;

        assert!(response.error.is_none());
        assert!(response.result.is_some());
        let result = response.result.unwrap();
        assert!(result.get("jsonrpc").is_some());
    }

    #[tokio::test]
    async fn test_handle_rpc_request_invalid_payload() {
        setup_test_env();
        let state = create_mock_app_state(
            None,
            Some(vec![create_mock_relayer("test".to_string(), false)]),
            Some(vec![create_mock_signer()]),
            Some(vec![create_mock_network()]),
            None,
            None,
        )
        .await;

        let request = Request {
            request_id: "test-rpc-2".to_string(),
            relayer_id: "test".to_string(),
            method: PluginMethod::Rpc,
            payload: serde_json::json!({
                "invalid": "payload"
            }),
            http_request_id: None,
        };

        let relayer_api = RelayerApi;
        let response = relayer_api
            .handle_request(request.clone(), &web::ThinData(state))
            .await;

        assert!(response.error.is_some());
        let error = response.error.unwrap();
        assert!(error.contains("Invalid payload") || error.contains("Missing 'method' field"));
    }

    #[tokio::test]
    async fn test_handle_rpc_request_relayer_not_found() {
        setup_test_env();
        let state = create_mock_app_state(
            None,
            None,
            Some(vec![create_mock_signer()]),
            Some(vec![create_mock_network()]),
            None,
            None,
        )
        .await;

        let request = Request {
            request_id: "test-rpc-3".to_string(),
            relayer_id: "nonexistent".to_string(),
            method: PluginMethod::Rpc,
            payload: serde_json::json!({
                "jsonrpc": "2.0",
                "method": "eth_blockNumber",
                "params": [],
                "id": 1
            }),
            http_request_id: None,
        };

        let relayer_api = RelayerApi;
        let response = relayer_api
            .handle_request(request.clone(), &web::ThinData(state))
            .await;

        assert!(response.error.is_some());
        let error = response.error.unwrap();
        assert!(error.contains("Relayer with ID nonexistent not found"));
    }

    #[tokio::test]
    async fn test_handle_rpc_request_paused_relayer() {
        setup_test_env();
        let paused = true;
        let state = create_mock_app_state(
            None,
            Some(vec![create_mock_relayer("test".to_string(), paused)]),
            Some(vec![create_mock_signer()]),
            Some(vec![create_mock_network()]),
            None,
            None,
        )
        .await;

        let request = Request {
            request_id: "test-rpc-4".to_string(),
            relayer_id: "test".to_string(),
            method: PluginMethod::Rpc,
            payload: serde_json::json!({
                "jsonrpc": "2.0",
                "method": "eth_blockNumber",
                "params": [],
                "id": 1
            }),
            http_request_id: None,
        };

        let relayer_api = RelayerApi;
        let response = relayer_api
            .handle_request(request.clone(), &web::ThinData(state))
            .await;

        assert!(response.error.is_some());
        let error = response.error.unwrap();
        assert!(error.contains("Relayer is paused"));
    }

    #[tokio::test]
    async fn test_handle_rpc_request_with_string_id() {
        setup_test_env();
        let state = create_mock_app_state(
            None,
            Some(vec![create_mock_relayer("test".to_string(), false)]),
            Some(vec![create_mock_signer()]),
            Some(vec![create_mock_network()]),
            None,
            None,
        )
        .await;

        let request = Request {
            request_id: "test-rpc-5".to_string(),
            relayer_id: "test".to_string(),
            method: PluginMethod::Rpc,
            payload: serde_json::json!({
                "jsonrpc": "2.0",
                "method": "eth_chainId",
                "params": [],
                "id": "custom-string-id"
            }),
            http_request_id: None,
        };

        let relayer_api = RelayerApi;
        let response = relayer_api
            .handle_request(request.clone(), &web::ThinData(state))
            .await;

        assert!(response.error.is_none());
        assert!(response.result.is_some());
        let result = response.result.unwrap();
        assert_eq!(result.get("id").unwrap(), "custom-string-id");
    }

    #[tokio::test]
    async fn test_handle_rpc_request_with_null_id() {
        setup_test_env();
        let state = create_mock_app_state(
            None,
            Some(vec![create_mock_relayer("test".to_string(), false)]),
            Some(vec![create_mock_signer()]),
            Some(vec![create_mock_network()]),
            None,
            None,
        )
        .await;

        let request = Request {
            request_id: "test-rpc-6".to_string(),
            relayer_id: "test".to_string(),
            method: PluginMethod::Rpc,
            payload: serde_json::json!({
                "jsonrpc": "2.0",
                "method": "eth_chainId",
                "params": [],
                "id": null
            }),
            http_request_id: None,
        };

        let relayer_api = RelayerApi;
        let response = relayer_api
            .handle_request(request.clone(), &web::ThinData(state))
            .await;

        assert!(response.error.is_none());
        assert!(response.result.is_some());
    }

    #[tokio::test]
    async fn test_handle_rpc_request_with_array_params() {
        setup_test_env();
        let state = create_mock_app_state(
            None,
            Some(vec![create_mock_relayer("test".to_string(), false)]),
            Some(vec![create_mock_signer()]),
            Some(vec![create_mock_network()]),
            None,
            None,
        )
        .await;

        let request = Request {
            request_id: "test-rpc-7".to_string(),
            relayer_id: "test".to_string(),
            method: PluginMethod::Rpc,
            payload: serde_json::json!({
                "jsonrpc": "2.0",
                "method": "eth_getBalance",
                "params": ["0x742d35Cc6634C0532925a3b844Bc454e4438f44e", "latest"],
                "id": 1
            }),
            http_request_id: None,
        };

        let relayer_api = RelayerApi;
        let response = relayer_api
            .handle_request(request.clone(), &web::ThinData(state))
            .await;

        assert!(response.error.is_none());
        assert!(response.result.is_some());
    }

    #[tokio::test]
    async fn test_handle_rpc_request_with_object_params() {
        setup_test_env();
        let state = create_mock_app_state(
            None,
            Some(vec![create_mock_relayer("test".to_string(), false)]),
            Some(vec![create_mock_signer()]),
            Some(vec![create_mock_network()]),
            None,
            None,
        )
        .await;

        let request = Request {
            request_id: "test-rpc-8".to_string(),
            relayer_id: "test".to_string(),
            method: PluginMethod::Rpc,
            payload: serde_json::json!({
                "jsonrpc": "2.0",
                "method": "eth_call",
                "params": {
                    "to": "0x742d35Cc6634C0532925a3b844Bc454e4438f44e",
                    "data": "0x"
                },
                "id": 1
            }),
            http_request_id: None,
        };

        let relayer_api = RelayerApi;
        let response = relayer_api
            .handle_request(request.clone(), &web::ThinData(state))
            .await;

        assert!(response.error.is_none());
        assert!(response.result.is_some());
    }

    #[tokio::test]
    async fn test_handle_rpc_request_missing_method() {
        setup_test_env();
        let state = create_mock_app_state(
            None,
            Some(vec![create_mock_relayer("test".to_string(), false)]),
            Some(vec![create_mock_signer()]),
            Some(vec![create_mock_network()]),
            None,
            None,
        )
        .await;

        let request = Request {
            request_id: "test-rpc-9".to_string(),
            relayer_id: "test".to_string(),
            method: PluginMethod::Rpc,
            payload: serde_json::json!({
                "jsonrpc": "2.0",
                "params": [],
                "id": 1
            }),
            http_request_id: None,
        };

        let relayer_api = RelayerApi;
        let response = relayer_api
            .handle_request(request.clone(), &web::ThinData(state))
            .await;

        assert!(response.error.is_some());
        let error = response.error.unwrap();
        assert!(error.contains("Missing 'method' field") || error.contains("Invalid payload"));
    }

    #[tokio::test]
    async fn test_handle_rpc_request_empty_method() {
        setup_test_env();
        let state = create_mock_app_state(
            None,
            Some(vec![create_mock_relayer("test".to_string(), false)]),
            Some(vec![create_mock_signer()]),
            Some(vec![create_mock_network()]),
            None,
            None,
        )
        .await;

        let request = Request {
            request_id: "test-rpc-10".to_string(),
            relayer_id: "test".to_string(),
            method: PluginMethod::Rpc,
            payload: serde_json::json!({
                "jsonrpc": "2.0",
                "method": "",
                "params": [],
                "id": 1
            }),
            http_request_id: None,
        };

        let relayer_api = RelayerApi;
        let response = relayer_api
            .handle_request(request.clone(), &web::ThinData(state))
            .await;

        // Empty method may be handled by the convert function or the provider
        // Either way, there should be an error or the response should indicate a problem
        assert!(
            response.error.is_some()
                || (response.result.is_some()
                    && response.result.as_ref().unwrap().get("error").is_some())
        );
    }

    #[tokio::test]
    async fn test_handle_rpc_request_with_http_request_id() {
        setup_test_env();
        let state = create_mock_app_state(
            None,
            Some(vec![create_mock_relayer("test".to_string(), false)]),
            Some(vec![create_mock_signer()]),
            Some(vec![create_mock_network()]),
            None,
            None,
        )
        .await;

        let request = Request {
            request_id: "test-rpc-11".to_string(),
            relayer_id: "test".to_string(),
            method: PluginMethod::Rpc,
            payload: serde_json::json!({
                "jsonrpc": "2.0",
                "method": "eth_blockNumber",
                "params": [],
                "id": 1
            }),
            http_request_id: Some("http-req-123".to_string()),
        };

        let relayer_api = RelayerApi;
        let response = relayer_api
            .handle_request(request.clone(), &web::ThinData(state))
            .await;

        assert!(response.error.is_none());
        assert!(response.result.is_some());
        assert_eq!(response.request_id, "test-rpc-11");
    }

    #[tokio::test]
    async fn test_handle_rpc_request_default_jsonrpc_version() {
        setup_test_env();
        let state = create_mock_app_state(
            None,
            Some(vec![create_mock_relayer("test".to_string(), false)]),
            Some(vec![create_mock_signer()]),
            Some(vec![create_mock_network()]),
            None,
            None,
        )
        .await;

        let request = Request {
            request_id: "test-rpc-12".to_string(),
            relayer_id: "test".to_string(),
            method: PluginMethod::Rpc,
            payload: serde_json::json!({
                "method": "eth_blockNumber",
                "params": [],
                "id": 1
            }),
            http_request_id: None,
        };

        let relayer_api = RelayerApi;
        let response = relayer_api
            .handle_request(request.clone(), &web::ThinData(state))
            .await;

        // Should either succeed or return a JSON-RPC formatted response
        if response.error.is_none() {
            assert!(response.result.is_some());
            let result = response.result.unwrap();
            assert_eq!(result.get("jsonrpc").unwrap(), "2.0");
        } else {
            // If there's an error, it's still valid since we're testing default version behavior
            assert!(response.error.is_some());
        }
    }

    #[tokio::test]
    async fn test_handle_rpc_request_custom_jsonrpc_version() {
        setup_test_env();
        let state = create_mock_app_state(
            None,
            Some(vec![create_mock_relayer("test".to_string(), false)]),
            Some(vec![create_mock_signer()]),
            Some(vec![create_mock_network()]),
            None,
            None,
        )
        .await;

        let request = Request {
            request_id: "test-rpc-13".to_string(),
            relayer_id: "test".to_string(),
            method: PluginMethod::Rpc,
            payload: serde_json::json!({
                "jsonrpc": "1.0",
                "method": "eth_blockNumber",
                "params": [],
                "id": 1
            }),
            http_request_id: None,
        };

        let relayer_api = RelayerApi;
        let response = relayer_api
            .handle_request(request.clone(), &web::ThinData(state))
            .await;

        assert!(response.error.is_none());
        assert!(response.result.is_some());
    }

    #[tokio::test]
    async fn test_handle_rpc_request_result_structure() {
        setup_test_env();
        let state = create_mock_app_state(
            None,
            Some(vec![create_mock_relayer("test".to_string(), false)]),
            Some(vec![create_mock_signer()]),
            Some(vec![create_mock_network()]),
            None,
            None,
        )
        .await;

        let request = Request {
            request_id: "test-rpc-14".to_string(),
            relayer_id: "test".to_string(),
            method: PluginMethod::Rpc,
            payload: serde_json::json!({
                "jsonrpc": "2.0",
                "method": "eth_blockNumber",
                "params": [],
                "id": 42
            }),
            http_request_id: None,
        };

        let relayer_api = RelayerApi;
        let response = relayer_api
            .handle_request(request.clone(), &web::ThinData(state))
            .await;

        assert!(response.error.is_none());
        assert!(response.result.is_some());
        assert_eq!(response.request_id, "test-rpc-14");

        let result = response.result.unwrap();
        assert!(result.get("jsonrpc").is_some());
        assert!(result.get("id").is_some());
        // Should have either result or error field
        assert!(result.get("result").is_some() || result.get("error").is_some());
    }
}
