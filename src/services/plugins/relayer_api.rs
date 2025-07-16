//! This module is responsible for handling the requests to the relayer API.
//!
//! It manages an internal API that mirrors the HTTP external API of the relayer.
//!
//! Supported methods:
//! - `sendTransaction` - sends a transaction to the relayer.
//!
use crate::domain::{get_network_relayer, get_relayer_by_id, get_transaction_by_id, Relayer};
use crate::jobs::JobProducerTrait;
use crate::models::{
    AppState, NetworkRepoModel, NetworkTransactionRequest, NotificationRepoModel, RelayerRepoModel,
    SignerRepoModel, ThinDataAppState, TransactionRepoModel, TransactionResponse,
};
use crate::repositories::{
    NetworkRepository, PluginRepositoryTrait, RelayerRepository, Repository,
    TransactionCounterTrait, TransactionRepository,
};
use actix_web::web;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use strum::Display;

use super::PluginError;

#[cfg(test)]
use mockall::automock;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Display)]
pub enum PluginMethod {
    #[serde(rename = "sendTransaction")]
    SendTransaction,
    #[serde(rename = "getTransaction")]
    GetTransaction,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub request_id: String,
    pub relayer_id: String,
    pub method: PluginMethod,
    pub payload: serde_json::Value,
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
pub trait RelayerApiTrait<J, RR, TR, NR, NFR, SR, TCR, PR>: Send + Sync
where
    J: JobProducerTrait + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
    SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PR: PluginRepositoryTrait + Send + Sync + 'static,
{
    async fn handle_request(
        &self,
        request: Request,
        state: &web::ThinData<AppState<J, RR, TR, NR, NFR, SR, TCR, PR>>,
    ) -> Response;

    async fn process_request(
        &self,
        request: Request,
        state: &web::ThinData<AppState<J, RR, TR, NR, NFR, SR, TCR, PR>>,
    ) -> Result<Response, PluginError>;

    async fn handle_send_transaction(
        &self,
        request: Request,
        state: &web::ThinData<AppState<J, RR, TR, NR, NFR, SR, TCR, PR>>,
    ) -> Result<Response, PluginError>;

    async fn handle_get_transaction(
        &self,
        request: Request,
        state: &web::ThinData<AppState<J, RR, TR, NR, NFR, SR, TCR, PR>>,
    ) -> Result<Response, PluginError>;
}

#[derive(Default)]
pub struct RelayerApi;

impl RelayerApi {
    pub async fn handle_request<J, RR, TR, NR, NFR, SR, TCR, PR>(
        &self,
        request: Request,
        state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR>,
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
    {
        match self.process_request(request.clone(), state).await {
            Ok(response) => response,
            Err(e) => Response {
                request_id: request.request_id,
                result: None,
                error: Some(e.to_string()),
            },
        }
    }

    async fn process_request<J, RR, TR, NR, NFR, SR, TCR, PR>(
        &self,
        request: Request,
        state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR>,
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
    {
        match request.method {
            PluginMethod::SendTransaction => self.handle_send_transaction(request, state).await,
            PluginMethod::GetTransaction => self.handle_get_transaction(request, state).await,
        }
    }

    async fn handle_send_transaction<J, RR, TR, NR, NFR, SR, TCR, PR>(
        &self,
        request: Request,
        state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR>,
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

    async fn handle_get_transaction<J, RR, TR, NR, NFR, SR, TCR, PR>(
        &self,
        request: Request,
        state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR>,
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
}

#[async_trait]
impl<J, RR, TR, NR, NFR, SR, TCR, PR> RelayerApiTrait<J, RR, TR, NR, NFR, SR, TCR, PR>
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
{
    async fn handle_request(
        &self,
        request: Request,
        state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR>,
    ) -> Response {
        self.handle_request(request, state).await
    }

    async fn process_request(
        &self,
        request: Request,
        state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR>,
    ) -> Result<Response, PluginError> {
        self.process_request(request, state).await
    }

    async fn handle_send_transaction(
        &self,
        request: Request,
        state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR>,
    ) -> Result<Response, PluginError> {
        self.handle_send_transaction(request, state).await
    }

    async fn handle_get_transaction(
        &self,
        request: Request,
        state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR>,
    ) -> Result<Response, PluginError> {
        self.handle_get_transaction(request, state).await
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
        };

        let relayer_api = RelayerApi;
        let response = relayer_api
            .handle_request(request.clone(), &web::ThinData(state))
            .await;

        assert!(response.error.is_some());
        let error = response.error.unwrap();
        assert!(error.contains("Transaction with ID test not found"));
    }
}
