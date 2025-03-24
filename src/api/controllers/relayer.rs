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
        get_network_relayer, get_network_relayer_by_model, get_relayer_by_id,
        get_relayer_transaction_by_model, get_transaction_by_id as get_tx_by_id, JsonRpcRequest,
        Relayer, RelayerUpdateRequest, SignDataRequest, SignDataResponse, SignTypedDataRequest,
        Transaction,
    },
    jobs::JobProducer,
    models::{
        ApiError, ApiResponse, AppState, NetworkRpcRequest, NetworkTransactionRequest, NetworkType,
        PaginationMeta, PaginationQuery, RelayerResponse, TransactionResponse,
    },
    repositories::{RelayerRepository, Repository, TransactionRepository},
};
use actix_web::{web, HttpResponse};
use eyre::Result;

/// Lists all relayers with pagination support.
///
/// # Arguments
///
/// * `query` - The pagination query parameters.
/// * `state` - The application state containing the relayer repository.
///
/// # Returns
///
/// A paginated list of relayers.
pub async fn list_relayers(
    query: PaginationQuery,
    state: web::ThinData<AppState<JobProducer>>,
) -> Result<HttpResponse, ApiError> {
    let relayers = state.relayer_repository.list_paginated(query).await?;

    let mapped_relayers: Vec<RelayerResponse> =
        relayers.items.into_iter().map(|r| r.into()).collect();

    Ok(HttpResponse::Ok().json(ApiResponse::paginated(
        mapped_relayers,
        PaginationMeta {
            total_items: relayers.total,
            current_page: relayers.page,
            per_page: relayers.per_page,
        },
    )))
}

/// Retrieves details of a specific relayer by its ID.
///
/// # Arguments
///
/// * `relayer_id` - The ID of the relayer to retrieve.
/// * `state` - The application state containing the relayer repository.
///
/// # Returns
///
/// The details of the specified relayer.
pub async fn get_relayer(
    relayer_id: String,
    state: web::ThinData<AppState<JobProducer>>,
) -> Result<HttpResponse, ApiError> {
    let relayer = get_relayer_by_id(relayer_id, &state).await?;

    let relayer_response: RelayerResponse = relayer.into();

    Ok(HttpResponse::Ok().json(ApiResponse::success(relayer_response)))
}

/// Updates a relayer's information.
///
/// # Arguments
///
/// * `relayer_id` - The ID of the relayer to update.
/// * `update_req` - The update request containing new relayer data.
/// * `state` - The application state containing the relayer repository.
///
/// # Returns
///
/// The updated relayer information.
pub async fn update_relayer(
    relayer_id: String,
    update_req: RelayerUpdateRequest,
    state: web::ThinData<AppState<JobProducer>>,
) -> Result<HttpResponse, ApiError> {
    let relayer = get_relayer_by_id(relayer_id.clone(), &state).await?;

    if relayer.system_disabled || (relayer.paused && update_req.paused != Some(false)) {
        let error_message = if relayer.system_disabled {
            "Relayer is disabled"
        } else {
            "Relayer is paused"
        };
        return Err(ApiError::BadRequest(error_message.to_string()));
    }

    let updated_relayer = state
        .relayer_repository
        .partial_update(relayer_id.clone(), update_req)
        .await?;

    let relayer_response: RelayerResponse = updated_relayer.into();

    Ok(HttpResponse::Ok().json(ApiResponse::success(relayer_response)))
}

/// Retrieves the status of a specific relayer.
///
/// # Arguments
///
/// * `relayer_id` - The ID of the relayer to check status for.
/// * `state` - The application state containing the relayer repository.
///
/// # Returns
///
/// The status of the specified relayer.
pub async fn get_relayer_status(
    relayer_id: String,
    state: web::ThinData<AppState<JobProducer>>,
) -> Result<HttpResponse, ApiError> {
    let relayer = get_network_relayer(relayer_id, &state).await?;

    let status = relayer.get_status().await?;

    Ok(HttpResponse::Ok().json(ApiResponse::success(status)))
}

/// Retrieves the balance of a specific relayer.
///
/// # Arguments
///
/// * `relayer_id` - The ID of the relayer to check balance for.
/// * `state` - The application state containing the relayer repository.
///
/// # Returns
///
/// The balance of the specified relayer.
pub async fn get_relayer_balance(
    relayer_id: String,
    state: web::ThinData<AppState<JobProducer>>,
) -> Result<HttpResponse, ApiError> {
    let relayer = get_network_relayer(relayer_id, &state).await?;

    let result = relayer.get_balance().await?;

    Ok(HttpResponse::Ok().json(ApiResponse::success(result)))
}

/// Sends a transaction through a specified relayer.
///
/// # Arguments
///
/// * `relayer_id` - The ID of the relayer to send the transaction through.
/// * `request` - The transaction request data.
/// * `state` - The application state containing the relayer repository.
///
/// # Returns
///
/// The response of the transaction processing.
pub async fn send_transaction(
    relayer_id: String,
    request: serde_json::Value,
    state: web::ThinData<AppState<JobProducer>>,
) -> Result<HttpResponse, ApiError> {
    let relayer_repo_model = get_relayer_by_id(relayer_id, &state).await?;
    relayer_repo_model.validate_active_state()?;

    let relayer = get_network_relayer(relayer_repo_model.id.clone(), &state).await?;

    let tx_request: NetworkTransactionRequest =
        NetworkTransactionRequest::from_json(&relayer_repo_model.network_type, request.clone())?;

    tx_request.validate(&relayer_repo_model)?;

    let transaction = relayer.process_transaction_request(tx_request).await?;

    let transaction_response: TransactionResponse = transaction.into();

    Ok(HttpResponse::Ok().json(ApiResponse::success(transaction_response)))
}

/// Retrieves a transaction by its ID for a specific relayer.
///
/// # Arguments
///
/// * `relayer_id` - The ID of the relayer.
/// * `transaction_id` - The ID of the transaction to retrieve.
/// * `state` - The application state containing the transaction repository.
///
/// # Returns
///
/// The details of the specified transaction.
pub async fn get_transaction_by_id(
    relayer_id: String,
    transaction_id: String,
    state: web::ThinData<AppState<JobProducer>>,
) -> Result<HttpResponse, ApiError> {
    if relayer_id.is_empty() || transaction_id.is_empty() {
        return Ok(HttpResponse::Ok().json(ApiResponse::<()>::error(
            "Invalid relayer or transaction ID".to_string(),
        )));
    }
    // validation purpose only, checks if relayer exists
    get_relayer_by_id(relayer_id, &state).await?;

    let transaction = get_tx_by_id(transaction_id, &state).await?;

    let transaction_response: TransactionResponse = transaction.into();

    Ok(HttpResponse::Ok().json(ApiResponse::success(transaction_response)))
}

/// Retrieves a transaction by its nonce for a specific relayer.
///
/// # Arguments
///
/// * `relayer_id` - The ID of the relayer.
/// * `nonce` - The nonce of the transaction to retrieve.
/// * `state` - The application state containing the transaction repository.
///
/// # Returns
///
/// The details of the specified transaction.
pub async fn get_transaction_by_nonce(
    relayer_id: String,
    nonce: u64,
    state: web::ThinData<AppState<JobProducer>>,
) -> Result<HttpResponse, ApiError> {
    let relayer = get_relayer_by_id(relayer_id.clone(), &state).await?;

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

/// Lists all transactions for a specific relayer with pagination support.
///
/// # Arguments
///
/// * `relayer_id` - The ID of the relayer.
/// * `query` - The pagination query parameters.
/// * `state` - The application state containing the transaction repository.
///
/// # Returns
///
/// A paginated list of transactions
pub async fn list_transactions(
    relayer_id: String,
    query: PaginationQuery,
    state: web::ThinData<AppState<JobProducer>>,
) -> Result<HttpResponse, ApiError> {
    get_relayer_by_id(relayer_id.clone(), &state).await?;

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

/// Deletes all pending transactions for a specific relayer.
///
/// # Arguments
///
/// * `relayer_id` - The ID of the relayer.
/// * `state` - The application state containing the relayer repository.
///
/// # Returns
///
/// A success response if the operation was successful.
pub async fn delete_pending_transactions(
    relayer_id: String,
    state: web::ThinData<AppState<JobProducer>>,
) -> Result<HttpResponse, ApiError> {
    let relayer = get_relayer_by_id(relayer_id, &state).await?;
    relayer.validate_active_state()?;
    let network_relayer = get_network_relayer_by_model(relayer.clone(), &state).await?;

    network_relayer.delete_pending_transactions().await?;

    Ok(HttpResponse::Ok().json(ApiResponse::success(())))
}

/// Cancels a specific transaction for a relayer.
///
/// # Arguments
///
/// * `relayer_id` - The ID of the relayer.
/// * `transaction_id` - The ID of the transaction to cancel.
/// * `state` - The application state containing the transaction repository.
///
/// # Returns
///
/// The details of the canceled transaction.
pub async fn cancel_transaction(
    relayer_id: String,
    transaction_id: String,
    state: web::ThinData<AppState<JobProducer>>,
) -> Result<HttpResponse, ApiError> {
    let relayer = get_relayer_by_id(relayer_id.clone(), &state).await?;
    relayer.validate_active_state()?;

    let relayer_transaction = get_relayer_transaction_by_model(relayer.clone(), &state).await?;

    let transaction_to_cancel = get_tx_by_id(transaction_id, &state).await?;

    let canceled_transaction = relayer_transaction
        .cancel_transaction(transaction_to_cancel)
        .await?;

    let transaction_response: TransactionResponse = canceled_transaction.into();

    Ok(HttpResponse::Ok().json(ApiResponse::success(transaction_response)))
}

/// Replaces a specific transaction for a relayer.
///
/// # Arguments
///
/// * `relayer_id` - The ID of the relayer.
/// * `transaction_id` - The ID of the transaction to replace.
/// * `state` - The application state containing the transaction repository.
///
/// # Returns
///
/// The details of the replaced transaction.
pub async fn replace_transaction(
    relayer_id: String,
    transaction_id: String,
    state: web::ThinData<AppState<JobProducer>>,
) -> Result<HttpResponse, ApiError> {
    let relayer = get_relayer_by_id(relayer_id.clone(), &state).await?;
    relayer.validate_active_state()?;

    let relayer_transaction = get_relayer_transaction_by_model(relayer.clone(), &state).await?;

    let transaction_to_replace = state
        .transaction_repository
        .get_by_id(transaction_id)
        .await?;

    let replaced_transaction = relayer_transaction
        .replace_transaction(transaction_to_replace)
        .await?;

    Ok(HttpResponse::Ok().json(ApiResponse::success(replaced_transaction)))
}

/// Signs data using a specific relayer.
///
/// # Arguments
///
/// * `relayer_id` - The ID of the relayer.
/// * `request` - The sign data request.
/// * `state` - The application state containing the relayer repository.
///
/// # Returns
///
/// The signed data response.
pub async fn sign_data(
    relayer_id: String,
    request: SignDataRequest,
    state: web::ThinData<AppState<JobProducer>>,
) -> Result<HttpResponse, ApiError> {
    let relayer = get_relayer_by_id(relayer_id.clone(), &state).await?;
    relayer.validate_active_state()?;
    let network_relayer = get_network_relayer_by_model(relayer, &state).await?;

    let result = network_relayer.sign_data(request).await?;

    if let SignDataResponse::Evm(sign) = result {
        Ok(HttpResponse::Ok().json(ApiResponse::success(sign)))
    } else {
        Err(ApiError::NotSupported("Sign data not supported".into()))
    }
}

/// Signs typed data using a specific relayer.
///
/// # Arguments
///
/// * `relayer_id` - The ID of the relayer.
/// * `request` - The sign typed data request.
/// * `state` - The application state containing the relayer repository.
///
/// # Returns
///
/// The signed typed data response.
pub async fn sign_typed_data(
    relayer_id: String,
    request: SignTypedDataRequest,
    state: web::ThinData<AppState<JobProducer>>,
) -> Result<HttpResponse, ApiError> {
    let relayer = get_relayer_by_id(relayer_id.clone(), &state).await?;
    relayer.validate_active_state()?;
    let network_relayer = get_network_relayer_by_model(relayer, &state).await?;

    let result = network_relayer.sign_typed_data(request).await?;

    Ok(HttpResponse::Ok().json(ApiResponse::success(result)))
}

/// Performs a JSON-RPC call through a specific relayer.
///
/// # Arguments
///
/// * `relayer_id` - The ID of the relayer.
/// * `request` - The JSON-RPC request.
/// * `state` - The application state containing the relayer repository.
///
/// # Returns
///
/// The result of the JSON-RPC call.
pub async fn relayer_rpc(
    relayer_id: String,
    request: JsonRpcRequest<NetworkRpcRequest>,
    state: web::ThinData<AppState<JobProducer>>,
) -> Result<HttpResponse, ApiError> {
    let relayer = get_relayer_by_id(relayer_id.clone(), &state).await?;
    relayer.validate_active_state()?;
    let network_relayer = get_network_relayer_by_model(relayer, &state).await?;

    let result = network_relayer.rpc(request).await?;

    Ok(HttpResponse::Ok().json(result))
}
