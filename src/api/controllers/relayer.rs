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
        JsonRpcRequest, Relayer, RelayerFactory, RelayerFactoryTrait, RelayerTransactionFactory,
        SignDataRequest, Transaction,
    },
    models::{
        ApiResponse, NetworkTransactionRequest, NetworkType, PaginationMeta, PaginationQuery,
        RelayerResponse, TransactionResponse,
    },
    repositories::Repository,
    ApiError, AppState,
};
use actix_web::{web, HttpResponse};
use eyre::{Context, Result};
use log::info;

pub async fn list_relayers(
    query: PaginationQuery,
    state: web::ThinData<AppState>,
) -> Result<HttpResponse, ApiError> {
    let relayers = state.relayer_repository.list_paginated(query).await?;

    info!("Relayers: {:?}", relayers);

    Ok(HttpResponse::Ok().json(ApiResponse::paginated(
        relayers.items,
        PaginationMeta {
            total_items: relayers.total,
            current_page: relayers.page,
            per_page: relayers.per_page,
        },
    )))
}

pub async fn get_relayer(
    relayer_id: String,
    state: web::ThinData<AppState>,
) -> Result<HttpResponse, ApiError> {
    let relayer = state
        .relayer_repository
        .get_by_id(relayer_id.to_string())
        .await?;

    info!("Relayer: {:?}", relayer);

    let relayer_response: RelayerResponse = relayer.into();

    Ok(HttpResponse::Ok().json(ApiResponse::success(relayer_response)))
}

pub async fn get_relayer_status(
    relayer_id: String,
    state: web::ThinData<AppState>,
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
    )?;

    let status = relayer.get_status().await?;

    Ok(HttpResponse::Ok().json(ApiResponse::success(status)))
}

pub async fn get_relayer_balance(
    relayer_id: String,
    state: web::ThinData<AppState>,
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
    )?;

    let result = relayer.get_balance().await?;

    Ok(HttpResponse::Ok().json(ApiResponse::success(result)))
}

pub async fn send_transaction(
    relayer_id: String,
    request: serde_json::Value,
    state: web::ThinData<AppState>,
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
    )?;

    let transaction = relayer.send_transaction(tx_request).await?;

    let transaction_response: TransactionResponse = transaction.into();

    Ok(HttpResponse::Ok().json(ApiResponse::success(transaction_response)))
}

pub async fn get_transaction_by_id(
    relayer_id: String,
    transaction_id: String,
    state: web::ThinData<AppState>,
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

    Ok(HttpResponse::Ok().json(ApiResponse::success(transaction_response)))
}

pub async fn get_transaction_by_nonce(
    relayer_id: String,
    nonce: u64,
    state: web::ThinData<AppState>,
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

    Ok(HttpResponse::Ok().json(ApiResponse::success(transaction_response)))
}

pub async fn list_transactions(
    relayer_id: String,
    query: PaginationQuery,
    state: web::ThinData<AppState>,
) -> Result<HttpResponse, ApiError> {
    state
        .relayer_repository
        .get_by_id(relayer_id.to_string())
        .await?;

    let transactions = state
        .transaction_repository
        .find_by_relayer_id(&relayer_id, query)
        .await?;

    let transaction_response_list: Vec<TransactionResponse> =
        transactions.items.into_iter().map(|t| t.into()).collect();

    Ok(HttpResponse::Ok().json(ApiResponse::paginated(
        transaction_response_list,
        PaginationMeta {
            total_items: transactions.total,
            current_page: transactions.page,
            per_page: transactions.per_page,
        },
    )))
}

pub async fn delete_pending_transactions(
    relayer_id: String,
    state: web::ThinData<AppState>,
) -> Result<HttpResponse, ApiError> {
    let relayer_repo_model = state
        .relayer_repository
        .get_by_id(relayer_id.to_string())
        .await?;

    let relayer = RelayerFactory::create_relayer(
        relayer_repo_model,
        state.relayer_repository(),
        state.transaction_repository(),
    )?;

    relayer.delete_pending_transactions().await?;

    Ok(HttpResponse::Ok().json(ApiResponse::success(())))
}

pub async fn cancel_transaction(
    relayer_id: String,
    transaction_id: String,
    state: web::ThinData<AppState>,
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
    )?;

    let canceled_transaction = relayer_transaction
        .cancel_transaction(transaction_to_cancel)
        .await?;

    Ok(HttpResponse::Ok().json(ApiResponse::success(canceled_transaction)))
}

pub async fn replace_transaction(
    relayer_id: String,
    transaction_id: String,
    state: web::ThinData<AppState>,
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
    )?;

    let replaced_transaction = relayer_transaction
        .replace_transaction(transaction_to_replace)
        .await?;

    Ok(HttpResponse::Ok().json(ApiResponse::success(replaced_transaction)))
}

pub async fn sign_data(
    relayer_id: String,
    request: SignDataRequest,
    state: web::ThinData<AppState>,
) -> Result<HttpResponse, ApiError> {
    let relayer_repo_model = state
        .relayer_repository
        .get_by_id(relayer_id.to_string())
        .await?;

    let relayer = RelayerFactory::create_relayer(
        relayer_repo_model,
        state.relayer_repository(),
        state.transaction_repository(),
    )?;

    let result = relayer.sign_data(request).await?;

    Ok(HttpResponse::Ok().json(ApiResponse::success(result)))
}

pub async fn sign_typed_data(
    relayer_id: String,
    request: SignDataRequest,
    state: web::ThinData<AppState>,
) -> Result<HttpResponse, ApiError> {
    let relayer_repo_model = state
        .relayer_repository
        .get_by_id(relayer_id.to_string())
        .await?;

    let relayer = RelayerFactory::create_relayer(
        relayer_repo_model,
        state.relayer_repository(),
        state.transaction_repository(),
    )?;

    let result = relayer.sign_typed_data(request).await?;

    Ok(HttpResponse::Ok().json(ApiResponse::success(result)))
}

pub async fn relayer_rpc(
    relayer_id: String,
    request: JsonRpcRequest,
    state: web::ThinData<AppState>,
) -> Result<HttpResponse, ApiError> {
    let relayer_repo_model = state
        .relayer_repository
        .get_by_id(relayer_id.to_string())
        .await?;

    let relayer = RelayerFactory::create_relayer(
        relayer_repo_model,
        state.relayer_repository(),
        state.transaction_repository(),
    )?;

    let result = relayer.rpc(request).await?;

    Ok(HttpResponse::Ok().json(ApiResponse::success(result)))
}
