use std::sync::Arc;

use crate::domain::{RelayerFactory, RelayerFactoryTrait};
use crate::models::{NetworkType, RelayerResponse, TransactionResponse};
use crate::{
    models::{ApiResponse, NetworkTransactionRequest},
    repositories::Repository,
    ApiError, AppState,
};
use actix_web::HttpResponse;
use eyre::{Context, Result};
use log::info;

pub async fn list_relayers(state: &AppState) -> Result<HttpResponse, ApiError> {
    let relayers = state.relayer_repository.list_all().await?;

    info!("Relayers: {:?}", relayers);

    Ok(HttpResponse::Ok().json(serde_json::json!(ApiResponse {
        success: true,
        data: Some(relayers),
        error: None,
    })))
}

pub async fn get_relayer(relayer_id: String, state: &AppState) -> Result<HttpResponse, ApiError> {
    let relayer = state
        .relayer_repository
        .get_by_id(relayer_id.to_string())
        .await?;

    info!("Relayer: {:?}", relayer);

    let relayer_response: RelayerResponse = relayer.into();

    Ok(HttpResponse::Ok().json(serde_json::json!(ApiResponse {
        success: true,
        data: Some(relayer_response),
        error: None,
    })))
}

pub async fn get_relayer_status(relayer_id: String) -> Result<HttpResponse, ApiError> {
    Ok(HttpResponse::Ok().json(serde_json::json!(ApiResponse {
        success: true,
        data: Some(relayer_id),
        error: None,
    })))
}

pub async fn get_relayer_balance(relayer_id: String) -> Result<HttpResponse, ApiError> {
    Ok(HttpResponse::Ok().json(serde_json::json!(ApiResponse {
        success: true,
        data: Some(relayer_id),
        error: None,
    })))
}

pub async fn get_relayer_nonce(relayer_id: String) -> Result<HttpResponse, ApiError> {
    Ok(HttpResponse::Ok().json(serde_json::json!(ApiResponse {
        success: true,
        data: Some(relayer_id),
        error: None,
    })))
}

pub async fn send_transaction(
    relayer_id: String,
    state: Arc<AppState>,
    request: serde_json::Value,
) -> Result<HttpResponse, ApiError> {
    let relayer_repo_model = state
        .relayer_repository
        .get_by_id(relayer_id.to_string())
        .await
        .wrap_err_with(|| format!("Failed to fetch relayer with ID {}", relayer_id))?;
    info!("Relayer: {:?}", relayer_repo_model);

    let tx_request: NetworkTransactionRequest =
        NetworkTransactionRequest::from_json(&relayer_repo_model.network_type, request.clone())?;

    let relayer = RelayerFactory::create_relayer(relayer_repo_model, state.clone()).unwrap();

    let transaction = relayer.send_transaction(tx_request).await?;

    let transaction_response: TransactionResponse = transaction.into();

    Ok(HttpResponse::Ok().json(serde_json::json!(ApiResponse {
        success: true,
        data: Some(transaction_response),
        error: None,
    })))
}

pub async fn get_transaction_by_id(
    relayer_id: String,
    transaction_id: String,
    state: &AppState,
) -> Result<HttpResponse, ApiError> {
    state
        .relayer_repository
        .get_by_id(relayer_id.to_string())
        .await?;

    let transaction = state
        .transaction_repository
        .get_by_id(transaction_id.to_string())
        .await?;

    let transaction_response: TransactionResponse = transaction.into();

    Ok(HttpResponse::Ok().json(serde_json::json!(ApiResponse {
        success: true,
        data: Some(transaction_response),
        error: None,
    })))
}

pub async fn get_transaction_by_nonce(
    relayer_id: String,
    nonce: u64,
    state: &AppState,
) -> Result<HttpResponse, ApiError> {
    let relayer = state
        .relayer_repository
        .get_by_id(relayer_id.to_string())
        .await?;

    // get by nonce is only supported for EVM network
    if relayer.network_type != NetworkType::Evm {
        return Err(ApiError::NotSupported(
            "Nonce lookup only supported for EVM networks".into(),
        ));
    }

    let transaction = state
        .transaction_repository
        .find_by_nonce(&relayer_id, nonce)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Transaction with nonce {} not found", nonce)))?;

    let transaction_response: TransactionResponse = transaction.into();

    Ok(HttpResponse::Ok().json(ApiResponse {
        success: true,
        data: Some(transaction_response),
        error: None,
    }))
}

pub async fn list_transactions(
    relayer_id: String,
    state: &AppState,
) -> Result<HttpResponse, ApiError> {
    state
        .relayer_repository
        .get_by_id(relayer_id.to_string())
        .await?;

    let transactions = state
        .transaction_repository
        .find_by_relayer_id(&relayer_id)
        .await?;

    let transaction_response_list: Vec<TransactionResponse> =
        transactions.into_iter().map(|t| t.into()).collect();

    Ok(HttpResponse::Ok().json(serde_json::json!(ApiResponse {
        success: true,
        data: Some(transaction_response_list),
        error: None,
    })))
}

pub async fn delete_pending_transactions(relayer_id: String) -> Result<HttpResponse, ApiError> {
    Ok(HttpResponse::Ok().json(serde_json::json!(ApiResponse {
        success: true,
        data: Some(relayer_id),
        error: None,
    })))
}

pub async fn cancel_transaction(
    relayer_id: String,
    _transaction_id: String,
) -> Result<HttpResponse, ApiError> {
    Ok(HttpResponse::Ok().json(serde_json::json!(ApiResponse {
        success: true,
        data: Some(relayer_id),
        error: None,
    })))
}

pub async fn replace_transaction(
    relayer_id: String,
    _transaction_id: String,
) -> Result<HttpResponse, ApiError> {
    Ok(HttpResponse::Ok().json(serde_json::json!(ApiResponse {
        success: true,
        data: Some(relayer_id),
        error: None,
    })))
}

pub async fn sign_data(relayer_id: String) -> Result<HttpResponse, ApiError> {
    Ok(HttpResponse::Ok().json(serde_json::json!(ApiResponse {
        success: true,
        data: Some(relayer_id),
        error: None,
    })))
}

pub async fn sign_typed_data(relayer_id: String) -> Result<HttpResponse, ApiError> {
    Ok(HttpResponse::Ok().json(serde_json::json!(ApiResponse {
        success: true,
        data: Some(relayer_id),
        error: None,
    })))
}

pub async fn relayer_rpc(
    _relayer_id: String,
    _request: serde_json::Value,
) -> Result<HttpResponse, ApiError> {
    Ok(HttpResponse::Ok().json(serde_json::json!(ApiResponse {
        success: true,
        data: Some(true),
        error: None,
    })))
}
