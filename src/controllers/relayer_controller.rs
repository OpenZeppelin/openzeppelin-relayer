use std::sync::Arc;

use crate::models::NetworkType;
use crate::services::{RelayerModelFactory, RelayerModelFactoryTrait};
use crate::{
    models::{ApiResponse, NetworkTransactionRequest},
    repositories::Repository,
    AppState, RelayerApiError,
};
use actix_web::HttpResponse;
use log::info;

pub async fn list_relayers(state: &AppState) -> Result<HttpResponse, RelayerApiError> {
    let relayers = state.relayer_repository.list_all().await?;
    info!("Relayers: {:?}", relayers);

    Ok(HttpResponse::Ok().json(serde_json::json!(ApiResponse {
        success: true,
        data: Some(relayers),
        error: None,
    })))
}

pub async fn get_relayer(
    relayer_id: String,
    state: &AppState,
) -> Result<HttpResponse, RelayerApiError> {
    let relayer = state
        .relayer_repository
        .get_by_id(relayer_id.to_string())
        .await?;
    info!("Relayer: {:?}", relayer);

    Ok(HttpResponse::Ok().json(serde_json::json!(ApiResponse {
        success: true,
        data: Some(relayer),
        error: None,
    })))
}

pub async fn get_relayer_status(relayer_id: String) -> Result<HttpResponse, RelayerApiError> {
    Ok(HttpResponse::Ok().json(serde_json::json!(ApiResponse {
        success: true,
        data: Some(relayer_id),
        error: None,
    })))
}

pub async fn get_relayer_balance(relayer_id: String) -> Result<HttpResponse, RelayerApiError> {
    Ok(HttpResponse::Ok().json(serde_json::json!(ApiResponse {
        success: true,
        data: Some(relayer_id),
        error: None,
    })))
}

pub async fn get_relayer_nonce(relayer_id: String) -> Result<HttpResponse, RelayerApiError> {
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
) -> Result<HttpResponse, RelayerApiError> {
    let relayer_repo_model = state
        .relayer_repository
        .get_by_id(relayer_id.to_string())
        .await?;
    info!("Relayer: {:?}", relayer_repo_model);

    let tx_request: NetworkTransactionRequest =
        NetworkTransactionRequest::from_json(&relayer_repo_model.network_type, request.clone())?;

    let relayer_model =
        RelayerModelFactory::create_relayer_model(relayer_repo_model, state.clone()).unwrap();

    let transaction = relayer_model.send_transaction(tx_request).await?;

    Ok(HttpResponse::Ok().json(serde_json::json!(ApiResponse {
        success: true,
        data: Some(transaction),
        error: None,
    })))
}

pub async fn get_transaction_by_id(
    relayer_id: String,
    transaction_id: String,
    state: &AppState,
) -> Result<HttpResponse, RelayerApiError> {
    state
        .relayer_repository
        .get_by_id(relayer_id.to_string())
        .await?;

    let transaction = state
        .transaction_repository
        .get_by_id(transaction_id.to_string())
        .await?;

    Ok(HttpResponse::Ok().json(serde_json::json!(ApiResponse {
        success: true,
        data: Some(transaction),
        error: None,
    })))
}

pub async fn get_transaction_by_nonce(
    relayer_id: String,
    nonce: u64,
    state: &AppState,
) -> Result<HttpResponse, RelayerApiError> {
    let relayer = state
        .relayer_repository
        .get_by_id(relayer_id.to_string())
        .await?;

    // get by nonce is only supported for EVM network
    if relayer.network_type != NetworkType::Evm {
        return Err(RelayerApiError::NotSupported(
            "Nonce lookup only supported for EVM networks".into(),
        ));
    }

    let transaction = state
        .transaction_repository
        .find_by_nonce(&relayer_id, nonce) // New optimized method
        .await?
        .ok_or_else(|| {
            RelayerApiError::NotFound(format!("Transaction with nonce {} not found", nonce))
        })?;

    Ok(HttpResponse::Ok().json(ApiResponse {
        success: true,
        data: Some(transaction),
        error: None,
    }))
}

pub async fn list_transactions(
    relayer_id: String,
    state: &AppState,
) -> Result<HttpResponse, RelayerApiError> {
    state
        .relayer_repository
        .get_by_id(relayer_id.to_string())
        .await?;

    let transactions = state
        .transaction_repository
        .find_by_relayer_id(&relayer_id)
        .await?;

    Ok(HttpResponse::Ok().json(serde_json::json!(ApiResponse {
        success: true,
        data: Some(transactions),
        error: None,
    })))
}

pub async fn delete_pending_transactions(
    relayer_id: String,
) -> Result<HttpResponse, RelayerApiError> {
    Ok(HttpResponse::Ok().json(serde_json::json!(ApiResponse {
        success: true,
        data: Some(relayer_id),
        error: None,
    })))
}

pub async fn cancel_transaction(
    relayer_id: String,
    transaction_id: String,
) -> Result<HttpResponse, RelayerApiError> {
    Ok(HttpResponse::Ok().json(serde_json::json!(ApiResponse {
        success: true,
        data: Some(relayer_id),
        error: None,
    })))
}

pub async fn replace_transaction(
    relayer_id: String,
    transaction_id: String,
) -> Result<HttpResponse, RelayerApiError> {
    Ok(HttpResponse::Ok().json(serde_json::json!(ApiResponse {
        success: true,
        data: Some(relayer_id),
        error: None,
    })))
}

pub async fn sign_data(relayer_id: String) -> Result<HttpResponse, RelayerApiError> {
    Ok(HttpResponse::Ok().json(serde_json::json!(ApiResponse {
        success: true,
        data: Some(relayer_id),
        error: None,
    })))
}

pub async fn sign_typed_data(relayer_id: String) -> Result<HttpResponse, RelayerApiError> {
    Ok(HttpResponse::Ok().json(serde_json::json!(ApiResponse {
        success: true,
        data: Some(relayer_id),
        error: None,
    })))
}

pub async fn relayer_rpc(
    relayer_id: String,
    request: serde_json::Value,
) -> Result<HttpResponse, RelayerApiError> {
    Ok(HttpResponse::Ok().json(serde_json::json!(ApiResponse {
        success: true,
        data: Some(true),
        error: None,
    })))
}
