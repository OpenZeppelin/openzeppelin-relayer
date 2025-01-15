//! # Relayer Controller
//!
//! Handles HTTP endpoints for relayer operations including:
//! - Listing relayers
//! - Getting relayer details
//! - Submitting transactions
//! - Signing messages
//! - JSON-RPC proxy
use crate::{
    domain::{
        JsonRpcRequest, RelayerFactory, RelayerFactoryTrait, RelayerTransactionFactory,
        SignDataRequest,
    },
    models::{
        ApiResponse, NetworkTransactionRequest, NetworkType, RelayerResponse, TransactionResponse,
    },
    repositories::Repository,
    ApiError, AppState,
};
use actix_web::{web, HttpResponse};
use eyre::{Context, Result};
use log::info;

pub async fn list_relayers(state: web::Data<AppState>) -> Result<HttpResponse, ApiError> {
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
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
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

pub async fn get_relayer_status(
    relayer_id: String,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let relayer_repo_model = state
        .relayer_repository
        .get_by_id(relayer_id.to_string())
        .await
        .wrap_err_with(|| format!("Failed to fetch relayer with ID {}", relayer_id))?;
    info!("Relayer: {:?}", relayer_repo_model);

    let relayer = RelayerFactory::create_relayer(
        relayer_repo_model,
        state.relayer_repository(),
        state.transaction_repository(),
    )
    .unwrap();

    let status = relayer.get_status().await?;

    Ok(HttpResponse::Ok().json(serde_json::json!(ApiResponse {
        success: true,
        data: Some(status),
        error: None,
    })))
}

pub async fn get_relayer_balance(
    relayer_id: String,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let relayer_repo_model = state
        .relayer_repository
        .get_by_id(relayer_id.to_string())
        .await
        .wrap_err_with(|| format!("Failed to fetch relayer with ID {}", relayer_id))?;
    info!("Relayer: {:?}", relayer_repo_model);

    let relayer = RelayerFactory::create_relayer(
        relayer_repo_model,
        state.relayer_repository(),
        state.transaction_repository(),
    )
    .unwrap();

    let result = relayer.get_balance().await?;

    Ok(HttpResponse::Ok().json(serde_json::json!(ApiResponse {
        success: true,
        data: Some(result),
        error: None,
    })))
}

pub async fn send_transaction(
    relayer_id: String,
    state: web::Data<AppState>,
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

    let relayer = RelayerFactory::create_relayer(
        relayer_repo_model,
        state.relayer_repository(),
        state.transaction_repository(),
    )
    .unwrap();

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
    state: web::Data<AppState>,
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
    state: web::Data<AppState>,
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
    state: web::Data<AppState>,
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

pub async fn delete_pending_transactions(
    relayer_id: String,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let relayer_repo_model = state
        .relayer_repository
        .get_by_id(relayer_id.to_string())
        .await?;

    let relayer = RelayerFactory::create_relayer(
        relayer_repo_model,
        state.relayer_repository(),
        state.transaction_repository(),
    )
    .unwrap();

    relayer.delete_pending_transactions().await?;

    Ok(
        HttpResponse::Ok().json(serde_json::json!(ApiResponse::<()> {
            success: true,
            data: None,
            error: None,
        })),
    )
}

pub async fn cancel_transaction(
    relayer_id: String,
    transaction_id: String,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let relayer = state
        .relayer_repository
        .get_by_id(relayer_id.to_string())
        .await?;

    let transaction_to_cancel = state
        .transaction_repository
        .get_by_id(transaction_id)
        .await?;

    let relayer_transaction = RelayerTransactionFactory::create_transaction(
        relayer,
        state.relayer_repository(),
        state.transaction_repository(),
    )
    .unwrap();

    let canceled_transaction = relayer_transaction
        .cancel_transaction(transaction_to_cancel)
        .await?;

    Ok(HttpResponse::Ok().json(serde_json::json!(ApiResponse {
        success: true,
        data: Some(canceled_transaction),
        error: None,
    })))
}

pub async fn replace_transaction(
    relayer_id: String,
    transaction_id: String,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let relayer = state
        .relayer_repository
        .get_by_id(relayer_id.to_string())
        .await?;

    let transaction_to_replace = state
        .transaction_repository
        .get_by_id(transaction_id)
        .await?;

    let relayer_transaction = RelayerTransactionFactory::create_transaction(
        relayer,
        state.relayer_repository(),
        state.transaction_repository(),
    )
    .unwrap();

    let replaced_transaction = relayer_transaction
        .replace_transaction(transaction_to_replace)
        .await?;

    Ok(HttpResponse::Ok().json(serde_json::json!(ApiResponse {
        success: true,
        data: Some(replaced_transaction),
        error: None,
    })))
}

pub async fn sign_data(
    relayer_id: String,
    request: serde_json::Value,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let relayer_repo_model = state
        .relayer_repository
        .get_by_id(relayer_id.to_string())
        .await?;

    let relayer = RelayerFactory::create_relayer(
        relayer_repo_model,
        state.relayer_repository(),
        state.transaction_repository(),
    )
    .unwrap();

    let sign_request: SignDataRequest = serde_json::from_value(request)
        .map_err(|e| ApiError::BadRequest(format!("Invalid Sign Typed Data request: {}", e)))?;

    let result = relayer.sign_data(sign_request).await?;

    Ok(HttpResponse::Ok().json(serde_json::json!(ApiResponse {
        success: true,
        data: Some(result),
        error: None,
    })))
}

pub async fn sign_typed_data(
    relayer_id: String,
    request: serde_json::Value,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let relayer_repo_model = state
        .relayer_repository
        .get_by_id(relayer_id.to_string())
        .await?;

    let relayer = RelayerFactory::create_relayer(
        relayer_repo_model,
        state.relayer_repository(),
        state.transaction_repository(),
    )
    .unwrap();

    let sign_request: SignDataRequest = serde_json::from_value(request)
        .map_err(|e| ApiError::BadRequest(format!("Invalid Sign Typed Data request: {}", e)))?;

    let result = relayer.sign_typed_data(sign_request).await?;

    Ok(HttpResponse::Ok().json(serde_json::json!(ApiResponse {
        success: true,
        data: Some(result),
        error: None,
    })))
}

pub async fn relayer_rpc(
    relayer_id: String,
    request: serde_json::Value,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let relayer_repo_model = state
        .relayer_repository
        .get_by_id(relayer_id.to_string())
        .await?;

    let relayer = RelayerFactory::create_relayer(
        relayer_repo_model,
        state.relayer_repository(),
        state.transaction_repository(),
    )
    .unwrap();

    let json_rpc_request: JsonRpcRequest = serde_json::from_value(request)
        .map_err(|e| ApiError::BadRequest(format!("Invalid JSON-RPC request: {}", e)))?;

    let result = relayer.rpc(json_rpc_request).await?;

    Ok(HttpResponse::Ok().json(serde_json::json!(result)))
}
