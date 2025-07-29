//! # Relayer Controller
//!
//! Handles HTTP endpoints for relayer operations including:
//! - Listing relayers
//! - Getting relayer details
//! - Creating relayers
//! - Updating relayers
//! - Deleting relayers
//! - Submitting transactions
//! - Signing messages
//! - JSON-RPC proxy
use crate::{
    domain::{
        get_network_relayer, get_network_relayer_by_model, get_relayer_by_id,
        get_relayer_transaction_by_model, get_transaction_by_id as get_tx_by_id, Relayer,
        RelayerFactory, RelayerFactoryTrait, SignDataRequest, SignDataResponse,
        SignTypedDataRequest, Transaction,
    },
    jobs::JobProducerTrait,
    models::{
        convert_to_internal_rpc_request, deserialize_policy_for_network_type, ApiError,
        ApiResponse, CreateRelayerRequest, DefaultAppState, NetworkRepoModel,
        NetworkTransactionRequest, NetworkType, NotificationRepoModel, PaginationMeta,
        PaginationQuery, Relayer as RelayerDomainModel, RelayerRepoModel, RelayerRepoUpdater,
        RelayerResponse, Signer as SignerDomainModel, SignerRepoModel, ThinDataAppState,
        TransactionRepoModel, TransactionResponse, TransactionStatus, UpdateRelayerRequestRaw,
    },
    repositories::{
        NetworkRepository, PluginRepositoryTrait, RelayerRepository, Repository,
        TransactionCounterTrait, TransactionRepository,
    },
    services::{Signer, SignerFactory},
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
pub async fn list_relayers<J, RR, TR, NR, NFR, SR, TCR, PR>(
    query: PaginationQuery,
    state: ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR>,
) -> Result<HttpResponse, ApiError>
where
    J: JobProducerTrait + Send + Sync + 'static,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
    SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PR: PluginRepositoryTrait + Send + Sync + 'static,
{
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
pub async fn get_relayer<J, RR, TR, NR, NFR, SR, TCR, PR>(
    relayer_id: String,
    state: ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR>,
) -> Result<HttpResponse, ApiError>
where
    J: JobProducerTrait + Send + Sync + 'static,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
    SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PR: PluginRepositoryTrait + Send + Sync + 'static,
{
    let relayer = get_relayer_by_id(relayer_id, &state).await?;

    let relayer_response: RelayerResponse = relayer.into();

    Ok(HttpResponse::Ok().json(ApiResponse::success(relayer_response)))
}

/// Creates a new relayer.
///
/// # Arguments
///
/// * `request` - The relayer creation request.
/// * `state` - The application state containing the relayer repository.
///
/// # Returns
///
/// The created relayer or an error if creation fails.
///
/// # Validation
///
/// This endpoint performs comprehensive dependency validation before creating the relayer:
/// - **Signer Validation**: Ensures the specified signer exists in the system
/// - **Signer Uniqueness**: Validates that the signer is not already in use by another relayer on the same network
/// - **Notification Validation**: If a notification ID is provided, validates it exists
/// - **Network Validation**: Confirms the specified network exists for the given network type
///
/// All validations must pass before the relayer is created, ensuring referential integrity and security constraints.
pub async fn create_relayer<J, RR, TR, NR, NFR, SR, TCR, PR>(
    request: CreateRelayerRequest,
    state: ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR>,
) -> Result<HttpResponse, ApiError>
where
    J: JobProducerTrait + Send + Sync + 'static,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
    SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PR: PluginRepositoryTrait + Send + Sync + 'static,
{
    // Convert request to domain relayer (validates automatically)
    let relayer = RelayerDomainModel::try_from(request)?;

    // Check if signer exists
    let signer_model = state
        .signer_repository
        .get_by_id(relayer.signer_id.clone())
        .await?;

    // Check if network exists for the given network type
    let network = state
        .network_repository
        .get_by_name(relayer.network_type, &relayer.network)
        .await?;

    if network.is_none() {
        return Err(ApiError::BadRequest(format!(
            "Network '{}' not found for network type '{}'. Please ensure the network configuration exists.",
            relayer.network,
            relayer.network_type
        )));
    }

    // Check if signer is already in use by another relayer on the same network
    let relayers = state
        .relayer_repository
        .list_by_signer_id(&relayer.signer_id)
        .await?;
    if let Some(existing_relayer) = relayers.iter().find(|r| r.network == relayer.network) {
        return Err(ApiError::BadRequest(format!(
            "Cannot create relayer: signer '{}' is already in use by relayer '{}' on network '{}'. Each signer can only be connected to one relayer per network for security reasons. Please use a different signer or create the relayer on a different network.",
            relayer.signer_id, existing_relayer.id, relayer.network
        )));
    }

    // Check if notification exists (if provided)
    if let Some(notification_id) = &relayer.notification_id {
        let _notification = state
            .notification_repository
            .get_by_id(notification_id.clone())
            .await?;
    }

    // Convert domain model to repository model
    let mut relayer_model = RelayerRepoModel::from(relayer);

    // get address from signer and set it to relayer model
    let signer_service = SignerFactory::create_signer(
        &relayer_model.network_type,
        &SignerDomainModel::from(signer_model.clone()),
    )
    .await
    .map_err(|e| ApiError::InternalError(e.to_string()))?;
    let address = signer_service
        .address()
        .await
        .map_err(|e| ApiError::InternalError(e.to_string()))?;
    relayer_model.address = address.to_string();

    let created_relayer = state.relayer_repository.create(relayer_model).await?;

    let relayer =
        RelayerFactory::create_relayer(created_relayer.clone(), signer_model, &state).await?;

    relayer.initialize_relayer().await?;

    let response = RelayerResponse::from(created_relayer);
    Ok(HttpResponse::Created().json(ApiResponse::success(response)))
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
pub async fn update_relayer<J, RR, TR, NR, NFR, SR, TCR, PR>(
    relayer_id: String,
    patch: serde_json::Value,
    state: ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR>,
) -> Result<HttpResponse, ApiError>
where
    J: JobProducerTrait + Send + Sync + 'static,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
    SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PR: PluginRepositoryTrait + Send + Sync + 'static,
{
    let relayer = get_relayer_by_id(relayer_id.clone(), &state).await?;

    // convert patch to UpdateRelayerRequest to validate
    let update_request: UpdateRelayerRequestRaw = serde_json::from_value(patch.clone())
        .map_err(|e| ApiError::BadRequest(format!("Invalid update request: {}", e)))?;

    if let Some(policies) = update_request.policies {
        deserialize_policy_for_network_type(&policies, relayer.network_type)
            .map_err(|e| ApiError::BadRequest(format!("Invalid policy: {}", e)))?;
    }

    if relayer.system_disabled {
        return Err(ApiError::BadRequest("Relayer is disabled".to_string()));
    }

    // Check if notification exists (if setting one) by extracting from JSON patch
    if let Some(notification_id) = update_request.notification_id {
        state
            .notification_repository
            .get_by_id(notification_id.to_string())
            .await?;
    }

    // Apply JSON merge patch directly to domain object
    let updated_domain = RelayerDomainModel::from(relayer.clone())
        .apply_json_patch(&patch)
        .map_err(ApiError::from)?;

    // Use existing RelayerRepoUpdater to preserve runtime fields
    let updated_repo_model =
        RelayerRepoUpdater::from_existing(relayer).apply_domain_update(updated_domain);

    let saved_relayer = state
        .relayer_repository
        .update(relayer_id.clone(), updated_repo_model)
        .await?;

    let relayer_response: RelayerResponse = saved_relayer.into();
    Ok(HttpResponse::Ok().json(ApiResponse::success(relayer_response)))
}

/// Deletes a relayer by ID.
///
/// # Arguments
///
/// * `relayer_id` - The ID of the relayer to delete.
/// * `state` - The application state containing the relayer repository.
///
/// # Returns
///
/// A success response or an error if deletion fails.
///
/// # Security
///
/// This endpoint ensures that relayers cannot be deleted if they have any pending
/// or active transactions. This prevents data loss and maintains system integrity.
pub async fn delete_relayer<J, RR, TR, NR, NFR, SR, TCR, PR>(
    relayer_id: String,
    state: ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR>,
) -> Result<HttpResponse, ApiError>
where
    J: JobProducerTrait + Send + Sync + 'static,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
    SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PR: PluginRepositoryTrait + Send + Sync + 'static,
{
    // Check if the relayer exists
    let _relayer = get_relayer_by_id(relayer_id.clone(), &state).await?;

    // Check if the relayer has any transactions (pending or otherwise)
    let transactions = state
        .transaction_repository
        .find_by_status(
            &relayer_id,
            &[
                TransactionStatus::Pending,
                TransactionStatus::Sent,
                TransactionStatus::Submitted,
            ],
        )
        .await?;

    if !transactions.is_empty() {
        return Err(ApiError::BadRequest(format!(
            "Cannot delete relayer '{}' because it has {} transaction(s). Please wait for all transactions to complete or cancel them before deleting the relayer.",
            relayer_id,
            transactions.len()
        )));
    }

    // Safe to delete - no transactions associated with this relayer
    state.relayer_repository.delete_by_id(relayer_id).await?;

    Ok(HttpResponse::Ok().json(ApiResponse::success("Relayer deleted successfully")))
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
pub async fn get_relayer_status<J, RR, TR, NR, NFR, SR, TCR, PR>(
    relayer_id: String,
    state: ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR>,
) -> Result<HttpResponse, ApiError>
where
    J: JobProducerTrait + Send + Sync + 'static,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
    SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PR: PluginRepositoryTrait + Send + Sync + 'static,
{
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
pub async fn get_relayer_balance<J, RR, TR, NR, NFR, SR, TCR, PR>(
    relayer_id: String,
    state: ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR>,
) -> Result<HttpResponse, ApiError>
where
    J: JobProducerTrait + Send + Sync + 'static,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
    SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PR: PluginRepositoryTrait + Send + Sync + 'static,
{
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
    state: web::ThinData<DefaultAppState>,
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
pub async fn get_transaction_by_id<J, RR, TR, NR, NFR, SR, TCR, PR>(
    relayer_id: String,
    transaction_id: String,
    state: ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR>,
) -> Result<HttpResponse, ApiError>
where
    J: JobProducerTrait + Send + Sync + 'static,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
    SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PR: PluginRepositoryTrait + Send + Sync + 'static,
{
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
pub async fn get_transaction_by_nonce<J, RR, TR, NR, NFR, SR, TCR, PR>(
    relayer_id: String,
    nonce: u64,
    state: ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR>,
) -> Result<HttpResponse, ApiError>
where
    J: JobProducerTrait + Send + Sync + 'static,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
    SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PR: PluginRepositoryTrait + Send + Sync + 'static,
{
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
pub async fn list_transactions<J, RR, TR, NR, NFR, SR, TCR, PR>(
    relayer_id: String,
    query: PaginationQuery,
    state: ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR>,
) -> Result<HttpResponse, ApiError>
where
    J: JobProducerTrait + Send + Sync + 'static,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
    SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PR: PluginRepositoryTrait + Send + Sync + 'static,
{
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
/// A success response with details about cancelled and failed transactions.
pub async fn delete_pending_transactions<J, RR, TR, NR, NFR, SR, TCR, PR>(
    relayer_id: String,
    state: ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR>,
) -> Result<HttpResponse, ApiError>
where
    J: JobProducerTrait + Send + Sync + 'static,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
    SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PR: PluginRepositoryTrait + Send + Sync + 'static,
{
    let relayer = get_relayer_by_id(relayer_id, &state).await?;
    relayer.validate_active_state()?;
    let network_relayer = get_network_relayer_by_model(relayer.clone(), &state).await?;

    let result = network_relayer.delete_pending_transactions().await?;

    Ok(HttpResponse::Ok().json(ApiResponse::success(result)))
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
    state: web::ThinData<DefaultAppState>,
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
/// * `request` - The new transaction request data.
/// * `state` - The application state containing the transaction repository.
///
/// # Returns
///
/// The details of the replaced transaction.
pub async fn replace_transaction(
    relayer_id: String,
    transaction_id: String,
    request: serde_json::Value,
    state: web::ThinData<DefaultAppState>,
) -> Result<HttpResponse, ApiError> {
    let relayer = get_relayer_by_id(relayer_id.clone(), &state).await?;
    relayer.validate_active_state()?;

    let new_tx_request: NetworkTransactionRequest =
        NetworkTransactionRequest::from_json(&relayer.network_type, request.clone())?;
    new_tx_request.validate(&relayer)?;

    let transaction_to_replace = state
        .transaction_repository
        .get_by_id(transaction_id)
        .await?;

    let relayer_transaction = get_relayer_transaction_by_model(relayer.clone(), &state).await?;
    let replaced_transaction = relayer_transaction
        .replace_transaction(transaction_to_replace, new_tx_request)
        .await?;

    let transaction_response: TransactionResponse = replaced_transaction.into();

    Ok(HttpResponse::Ok().json(ApiResponse::success(transaction_response)))
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
pub async fn sign_data<J, RR, TR, NR, NFR, SR, TCR, PR>(
    relayer_id: String,
    request: SignDataRequest,
    state: ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR>,
) -> Result<HttpResponse, ApiError>
where
    J: JobProducerTrait + Send + Sync + 'static,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
    SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PR: PluginRepositoryTrait + Send + Sync + 'static,
{
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
pub async fn sign_typed_data<J, RR, TR, NR, NFR, SR, TCR, PR>(
    relayer_id: String,
    request: SignTypedDataRequest,
    state: ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR>,
) -> Result<HttpResponse, ApiError>
where
    J: JobProducerTrait + Send + Sync + 'static,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
    SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PR: PluginRepositoryTrait + Send + Sync + 'static,
{
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
/// * `request` - The raw JSON-RPC request value.
/// * `state` - The application state containing the relayer repository.
///
/// # Returns
///
/// The result of the JSON-RPC call.
pub async fn relayer_rpc<J, RR, TR, NR, NFR, SR, TCR, PR>(
    relayer_id: String,
    request: serde_json::Value,
    state: ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR>,
) -> Result<HttpResponse, ApiError>
where
    J: JobProducerTrait + Send + Sync + 'static,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
    SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PR: PluginRepositoryTrait + Send + Sync + 'static,
{
    let relayer = get_relayer_by_id(relayer_id.clone(), &state).await?;
    relayer.validate_active_state()?;
    let network_relayer = get_network_relayer_by_model(relayer.clone(), &state).await?;

    let internal_request = convert_to_internal_rpc_request(request, &relayer.network_type)?;
    let result = network_relayer.rpc(internal_request).await?;

    Ok(HttpResponse::Ok().json(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        models::{
            ApiResponse, CreateRelayerPolicyRequest, CreateRelayerRequest, RelayerEvmPolicy,
            RelayerNetworkPolicyResponse, RelayerNetworkType, RelayerResponse, RelayerSolanaPolicy,
            RelayerStellarPolicy, SolanaFeePaymentStrategy,
        },
        utils::mocks::mockutils::{
            create_mock_app_state, create_mock_network, create_mock_notification,
            create_mock_relayer, create_mock_signer, create_mock_transaction,
        },
    };
    use actix_web::body::to_bytes;
    use lazy_static::lazy_static;
    use std::env;
    use tokio::sync::Mutex;

    lazy_static! {
        static ref ENV_MUTEX: Mutex<()> = Mutex::new(());
    }

    fn setup_test_env() {
        env::set_var("API_KEY", "7EF1CB7C-5003-4696-B384-C72AF8C3E15D"); // noboost nosemgrep
        env::set_var("REDIS_URL", "redis://localhost:6379");
    }

    fn cleanup_test_env() {
        env::remove_var("API_KEY");
        env::remove_var("REDIS_URL");
    }

    /// Helper function to create a test relayer create request
    fn create_test_relayer_create_request(
        id: Option<String>,
        name: &str,
        network: &str,
        signer_id: &str,
        notification_id: Option<String>,
    ) -> CreateRelayerRequest {
        CreateRelayerRequest {
            id,
            name: name.to_string(),
            network: network.to_string(),
            network_type: RelayerNetworkType::Evm,
            paused: false,
            policies: None,
            signer_id: signer_id.to_string(),
            notification_id,
            custom_rpc_urls: None,
        }
    }

    /// Helper function to create a mock Solana network
    fn create_mock_solana_network() -> crate::models::NetworkRepoModel {
        use crate::config::{NetworkConfigCommon, SolanaNetworkConfig};
        use crate::models::{NetworkConfigData, NetworkRepoModel, NetworkType};

        NetworkRepoModel {
            id: "test".to_string(),
            name: "test".to_string(),
            network_type: NetworkType::Solana,
            config: NetworkConfigData::Solana(SolanaNetworkConfig {
                common: NetworkConfigCommon {
                    network: "test".to_string(),
                    from: None,
                    rpc_urls: Some(vec!["http://localhost:8899".to_string()]),
                    explorer_urls: None,
                    average_blocktime_ms: Some(400),
                    is_testnet: Some(true),
                    tags: None,
                },
            }),
        }
    }

    /// Helper function to create a mock Stellar network
    fn create_mock_stellar_network() -> crate::models::NetworkRepoModel {
        use crate::config::{NetworkConfigCommon, StellarNetworkConfig};
        use crate::models::{NetworkConfigData, NetworkRepoModel, NetworkType};

        NetworkRepoModel {
            id: "test".to_string(),
            name: "test".to_string(),
            network_type: NetworkType::Stellar,
            config: NetworkConfigData::Stellar(StellarNetworkConfig {
                common: NetworkConfigCommon {
                    network: "test".to_string(),
                    from: None,
                    rpc_urls: Some(vec!["https://horizon-testnet.stellar.org".to_string()]),
                    explorer_urls: None,
                    average_blocktime_ms: Some(5000),
                    is_testnet: Some(true),
                    tags: None,
                },
                passphrase: Some("Test Network ; September 2015".to_string()),
            }),
        }
    }

    // CREATE RELAYER TESTS

    #[actix_web::test]
    async fn test_create_relayer_success() {
        let _lock = ENV_MUTEX.lock().await;
        setup_test_env();
        let network = create_mock_network();
        let signer = create_mock_signer();
        let app_state =
            create_mock_app_state(None, Some(vec![signer]), Some(vec![network]), None, None).await;

        let request = create_test_relayer_create_request(
            Some("test-relayer".to_string()),
            "Test Relayer",
            "test", // Using "test" to match the mock network name
            "test", // Using "test" to match the mock signer id
            None,
        );

        let result = create_relayer(request, actix_web::web::ThinData(app_state)).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 201);

        let body = to_bytes(response.into_body()).await.unwrap();
        let api_response: ApiResponse<RelayerResponse> = serde_json::from_slice(&body).unwrap();

        assert!(api_response.success);
        let data = api_response.data.unwrap();
        assert_eq!(data.id, "test-relayer");
        assert_eq!(data.name, "Test Relayer"); // This one keeps custom name from the request
        assert_eq!(data.network, "test");
        cleanup_test_env();
    }

    #[actix_web::test]
    async fn test_create_relayer_with_evm_policies() {
        let _lock = ENV_MUTEX.lock().await;
        setup_test_env();
        let network = create_mock_network();
        let signer = create_mock_signer();
        let app_state =
            create_mock_app_state(None, Some(vec![signer]), Some(vec![network]), None, None).await;

        let mut request = create_test_relayer_create_request(
            Some("test-relayer-policies".to_string()),
            "Test Relayer with Policies",
            "test", // Using "test" to match the mock network name
            "test", // Using "test" to match the mock signer id
            None,
        );

        // Add EVM policies
        request.policies = Some(CreateRelayerPolicyRequest::Evm(RelayerEvmPolicy {
            gas_price_cap: Some(50000000000),
            min_balance: Some(1000000000000000000),
            eip1559_pricing: Some(true),
            private_transactions: Some(false),
            gas_limit_estimation: Some(true),
            whitelist_receivers: Some(vec![
                "0x1234567890123456789012345678901234567890".to_string()
            ]),
        }));

        let result = create_relayer(request, actix_web::web::ThinData(app_state)).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 201);

        let body = to_bytes(response.into_body()).await.unwrap();
        let api_response: ApiResponse<RelayerResponse> = serde_json::from_slice(&body).unwrap();

        assert!(api_response.success);
        let data = api_response.data.unwrap();
        assert_eq!(data.id, "test-relayer-policies");
        assert_eq!(data.name, "Test Relayer with Policies");
        assert_eq!(data.network, "test");

        // Verify policies are present in response
        assert!(data.policies.is_some());
        cleanup_test_env();
    }

    #[actix_web::test]
    async fn test_create_relayer_with_partial_evm_policies() {
        let _lock = ENV_MUTEX.lock().await;
        setup_test_env();
        let network = create_mock_network();
        let signer = create_mock_signer();
        let app_state =
            create_mock_app_state(None, Some(vec![signer]), Some(vec![network]), None, None).await;

        let mut request = create_test_relayer_create_request(
            Some("test-relayer-partial".to_string()),
            "Test Relayer with Partial Policies",
            "test",
            "test",
            None,
        );

        // Add partial EVM policies
        request.policies = Some(CreateRelayerPolicyRequest::Evm(RelayerEvmPolicy {
            gas_price_cap: Some(30000000000),
            eip1559_pricing: Some(false),
            min_balance: None,
            private_transactions: None,
            gas_limit_estimation: None,
            whitelist_receivers: None,
        }));

        let result = create_relayer(request, actix_web::web::ThinData(app_state)).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 201);

        let body = to_bytes(response.into_body()).await.unwrap();
        let api_response: ApiResponse<RelayerResponse> = serde_json::from_slice(&body).unwrap();

        assert!(api_response.success);
        let data = api_response.data.unwrap();
        assert_eq!(data.id, "test-relayer-partial");

        // Verify partial policies are present in response
        assert!(data.policies.is_some());
        cleanup_test_env();
    }

    #[actix_web::test]
    async fn test_create_relayer_with_solana_policies() {
        let _lock = ENV_MUTEX.lock().await;
        setup_test_env();
        let network = create_mock_solana_network();
        let signer = create_mock_signer();
        let app_state =
            create_mock_app_state(None, Some(vec![signer]), Some(vec![network]), None, None).await;

        let mut request = create_test_relayer_create_request(
            Some("test-solana-relayer".to_string()),
            "Test Solana Relayer",
            "test",
            "test",
            None,
        );

        // Change network type to Solana and add Solana policies
        request.network_type = RelayerNetworkType::Solana;
        request.policies = Some(CreateRelayerPolicyRequest::Solana(RelayerSolanaPolicy {
            fee_payment_strategy: Some(SolanaFeePaymentStrategy::Relayer),
            min_balance: Some(5000000),
            max_signatures: Some(10),
            max_tx_data_size: Some(1232),
            max_allowed_fee_lamports: Some(50000),
            allowed_programs: None, // Simplified to avoid validation issues
            allowed_tokens: None,
            fee_margin_percentage: Some(10.0),
            allowed_accounts: None,
            disallowed_accounts: None,
            swap_config: None,
        }));

        let result = create_relayer(request, actix_web::web::ThinData(app_state)).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 201);

        let body = to_bytes(response.into_body()).await.unwrap();
        let api_response: ApiResponse<RelayerResponse> = serde_json::from_slice(&body).unwrap();

        assert!(api_response.success);
        let data = api_response.data.unwrap();
        assert_eq!(data.id, "test-solana-relayer");
        assert_eq!(data.network_type, RelayerNetworkType::Solana);
        assert_eq!(data.name, "Test Solana Relayer");

        // Verify Solana policies are present in response
        assert!(data.policies.is_some());
        // verify policies are correct
        let policies = data.policies.unwrap();
        if let RelayerNetworkPolicyResponse::Solana(solana_policy) = policies {
            assert_eq!(
                solana_policy.fee_payment_strategy,
                Some(SolanaFeePaymentStrategy::Relayer)
            );
            assert_eq!(solana_policy.min_balance, 5000000);
            assert_eq!(solana_policy.max_signatures, Some(10));
            assert_eq!(solana_policy.max_tx_data_size, 1232);
            assert_eq!(solana_policy.max_allowed_fee_lamports, Some(50000));
        } else {
            panic!("Expected Solana policies");
        }
        cleanup_test_env();
    }

    #[actix_web::test]
    async fn test_create_relayer_with_stellar_policies() {
        let _lock = ENV_MUTEX.lock().await;
        setup_test_env();
        let network = create_mock_stellar_network();
        let signer = create_mock_signer();
        let app_state =
            create_mock_app_state(None, Some(vec![signer]), Some(vec![network]), None, None).await;

        let mut request = create_test_relayer_create_request(
            Some("test-stellar-relayer".to_string()),
            "Test Stellar Relayer",
            "test",
            "test",
            None,
        );

        // Change network type to Stellar and add Stellar policies
        request.network_type = RelayerNetworkType::Stellar;
        request.policies = Some(CreateRelayerPolicyRequest::Stellar(RelayerStellarPolicy {
            min_balance: Some(10000000),
            max_fee: Some(100),
            timeout_seconds: Some(30),
        }));

        let result = create_relayer(request, actix_web::web::ThinData(app_state)).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 201);

        let body = to_bytes(response.into_body()).await.unwrap();
        let api_response: ApiResponse<RelayerResponse> = serde_json::from_slice(&body).unwrap();

        assert!(api_response.success);
        let data = api_response.data.unwrap();
        assert_eq!(data.id, "test-stellar-relayer");
        assert_eq!(data.network_type, RelayerNetworkType::Stellar);

        // Verify Stellar policies are present in response
        assert!(data.policies.is_some());
        cleanup_test_env();
    }

    #[actix_web::test]
    async fn test_create_relayer_with_policy_type_mismatch() {
        let _lock = ENV_MUTEX.lock().await;
        setup_test_env();
        let network = create_mock_network();
        let signer = create_mock_signer();
        let app_state =
            create_mock_app_state(None, Some(vec![signer]), Some(vec![network]), None, None).await;

        let mut request = create_test_relayer_create_request(
            Some("test-mismatch-relayer".to_string()),
            "Test Mismatch Relayer",
            "test",
            "test",
            None,
        );

        // Set network type to EVM but provide Solana policies (should fail)
        request.network_type = RelayerNetworkType::Evm;
        request.policies = Some(CreateRelayerPolicyRequest::Solana(
            RelayerSolanaPolicy::default(),
        ));

        let result = create_relayer(request, actix_web::web::ThinData(app_state)).await;

        assert!(result.is_err());
        if let Err(ApiError::BadRequest(msg)) = result {
            assert!(msg.contains("Policy type does not match relayer network type"));
        } else {
            panic!("Expected BadRequest error for policy type mismatch");
        }
        cleanup_test_env();
    }

    #[actix_web::test]
    async fn test_create_relayer_with_notification() {
        let _lock = ENV_MUTEX.lock().await;
        setup_test_env();
        let network = create_mock_network();
        let signer = create_mock_signer();
        let notification = create_mock_notification("test-notification".to_string());
        let app_state =
            create_mock_app_state(None, Some(vec![signer]), Some(vec![network]), None, None).await;

        // Add notification manually since create_mock_app_state doesn't handle notifications
        app_state
            .notification_repository
            .create(notification)
            .await
            .unwrap();

        let request = create_test_relayer_create_request(
            Some("test-relayer".to_string()),
            "Test Relayer",
            "test", // Using "test" to match the mock network name
            "test", // Using "test" to match the mock signer id
            Some("test-notification".to_string()),
        );

        let result = create_relayer(request, actix_web::web::ThinData(app_state)).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 201);
        let body = to_bytes(response.into_body()).await.unwrap();
        let api_response: ApiResponse<RelayerResponse> = serde_json::from_slice(&body).unwrap();

        assert!(api_response.success);
        let data = api_response.data.unwrap();
        assert_eq!(data.notification_id, Some("test-notification".to_string()));
        cleanup_test_env();
    }

    #[actix_web::test]
    async fn test_create_relayer_nonexistent_signer() {
        let network = create_mock_network();
        let app_state = create_mock_app_state(None, None, Some(vec![network]), None, None).await;

        let request = create_test_relayer_create_request(
            Some("test-relayer".to_string()),
            "Test Relayer",
            "test", // Using "test" to match the mock network name
            "nonexistent-signer",
            None,
        );

        let result = create_relayer(request, actix_web::web::ThinData(app_state)).await;

        assert!(result.is_err());
        if let Err(ApiError::NotFound(msg)) = result {
            assert!(msg.contains("Signer with ID nonexistent-signer not found"));
        } else {
            panic!("Expected NotFound error for nonexistent signer");
        }
    }

    #[actix_web::test]
    async fn test_create_relayer_nonexistent_network() {
        let signer = create_mock_signer();
        let app_state = create_mock_app_state(None, Some(vec![signer]), None, None, None).await;

        let request = create_test_relayer_create_request(
            Some("test-relayer".to_string()),
            "Test Relayer",
            "nonexistent-network",
            "test", // Using "test" to match the mock signer id
            None,
        );

        let result = create_relayer(request, actix_web::web::ThinData(app_state)).await;

        assert!(result.is_err());
        if let Err(ApiError::BadRequest(msg)) = result {
            assert!(msg.contains("Network 'nonexistent-network' not found"));
            assert!(msg.contains("network configuration exists"));
        } else {
            panic!("Expected BadRequest error for nonexistent network");
        }
    }

    #[actix_web::test]
    async fn test_create_relayer_signer_already_in_use() {
        let network = create_mock_network();
        let signer = create_mock_signer();
        let mut existing_relayer = create_mock_relayer("existing-relayer".to_string(), false);
        existing_relayer.signer_id = "test".to_string(); // Match the mock signer id
        existing_relayer.network = "test".to_string(); // Match the mock network name
        let app_state = create_mock_app_state(
            Some(vec![existing_relayer]),
            Some(vec![signer]),
            Some(vec![network]),
            None,
            None,
        )
        .await;

        let request = create_test_relayer_create_request(
            Some("test-relayer".to_string()),
            "Test Relayer",
            "test", // Using "test" to match the mock network name
            "test", // Using "test" to match the mock signer id
            None,
        );

        let result = create_relayer(request, actix_web::web::ThinData(app_state)).await;

        assert!(result.is_err());
        if let Err(ApiError::BadRequest(msg)) = result {
            assert!(msg.contains("signer 'test' is already in use"));
            assert!(msg.contains("relayer 'existing-relayer'"));
            assert!(msg.contains("network 'test'"));
            assert!(msg.contains("security reasons"));
        } else {
            panic!("Expected BadRequest error for signer already in use");
        }
    }

    #[actix_web::test]
    async fn test_create_relayer_nonexistent_notification() {
        let network = create_mock_network();
        let signer = create_mock_signer();
        let app_state =
            create_mock_app_state(None, Some(vec![signer]), Some(vec![network]), None, None).await;

        let request = create_test_relayer_create_request(
            Some("test-relayer".to_string()),
            "Test Relayer",
            "test", // Using "test" to match the mock network name
            "test", // Using "test" to match the mock signer id
            Some("nonexistent-notification".to_string()),
        );

        let result = create_relayer(request, actix_web::web::ThinData(app_state)).await;

        assert!(result.is_err());
        if let Err(ApiError::NotFound(msg)) = result {
            assert!(msg.contains("Notification with ID 'nonexistent-notification' not found"));
        } else {
            panic!("Expected NotFound error for nonexistent notification");
        }
    }

    // LIST RELAYERS TESTS

    #[actix_web::test]
    async fn test_list_relayers_success() {
        let relayer1 = create_mock_relayer("relayer-1".to_string(), false);
        let relayer2 = create_mock_relayer("relayer-2".to_string(), false);
        let app_state =
            create_mock_app_state(Some(vec![relayer1, relayer2]), None, None, None, None).await;

        let query = PaginationQuery {
            page: 1,
            per_page: 10,
        };

        let result = list_relayers(query, actix_web::web::ThinData(app_state)).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 200);

        let body = to_bytes(response.into_body()).await.unwrap();
        let api_response: ApiResponse<Vec<RelayerResponse>> =
            serde_json::from_slice(&body).unwrap();

        assert!(api_response.success);
        let data = api_response.data.unwrap();
        assert_eq!(data.len(), 2);
    }

    #[actix_web::test]
    async fn test_list_relayers_empty() {
        let app_state = create_mock_app_state(None, None, None, None, None).await;

        let query = PaginationQuery {
            page: 1,
            per_page: 10,
        };

        let result = list_relayers(query, actix_web::web::ThinData(app_state)).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 200);

        let body = to_bytes(response.into_body()).await.unwrap();
        let api_response: ApiResponse<Vec<RelayerResponse>> =
            serde_json::from_slice(&body).unwrap();

        assert!(api_response.success);
        let data = api_response.data.unwrap();
        assert_eq!(data.len(), 0);
    }

    // GET RELAYER TESTS

    #[actix_web::test]
    async fn test_get_relayer_success() {
        let relayer = create_mock_relayer("test-relayer".to_string(), false);
        let app_state = create_mock_app_state(Some(vec![relayer]), None, None, None, None).await;

        let result = get_relayer(
            "test-relayer".to_string(),
            actix_web::web::ThinData(app_state),
        )
        .await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 200);

        let body = to_bytes(response.into_body()).await.unwrap();
        let api_response: ApiResponse<RelayerResponse> = serde_json::from_slice(&body).unwrap();

        assert!(api_response.success);
        let data = api_response.data.unwrap();
        assert_eq!(data.id, "test-relayer");
        assert_eq!(data.name, "Relayer test-relayer"); // Mock utility creates name as "Relayer {id}"
    }

    #[actix_web::test]
    async fn test_get_relayer_not_found() {
        let app_state = create_mock_app_state(None, None, None, None, None).await;

        let result = get_relayer(
            "nonexistent".to_string(),
            actix_web::web::ThinData(app_state),
        )
        .await;

        assert!(result.is_err());
        if let Err(ApiError::NotFound(msg)) = result {
            assert!(msg.contains("Relayer with ID nonexistent not found"));
        } else {
            panic!("Expected NotFound error");
        }
    }

    // UPDATE RELAYER TESTS

    #[actix_web::test]
    async fn test_update_relayer_success() {
        let relayer = create_mock_relayer("test-relayer".to_string(), false);
        let app_state = create_mock_app_state(Some(vec![relayer]), None, None, None, None).await;

        let patch = serde_json::json!({
            "name": "Updated Relayer Name",
            "paused": true
        });

        let result = update_relayer(
            "test-relayer".to_string(),
            patch,
            actix_web::web::ThinData(app_state),
        )
        .await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 200);

        let body = to_bytes(response.into_body()).await.unwrap();
        let api_response: ApiResponse<RelayerResponse> = serde_json::from_slice(&body).unwrap();

        assert!(api_response.success);
        let data = api_response.data.unwrap();
        assert_eq!(data.name, "Updated Relayer Name");
        assert!(data.paused);
    }

    #[actix_web::test]
    async fn test_update_relayer_system_disabled() {
        let mut relayer = create_mock_relayer("disabled-relayer".to_string(), false);
        relayer.system_disabled = true;
        let app_state = create_mock_app_state(Some(vec![relayer]), None, None, None, None).await;

        let patch = serde_json::json!({
            "name": "Updated Name"
        });

        let result = update_relayer(
            "disabled-relayer".to_string(),
            patch,
            actix_web::web::ThinData(app_state),
        )
        .await;

        assert!(result.is_err());
        if let Err(ApiError::BadRequest(msg)) = result {
            assert!(msg.contains("Relayer is disabled"));
        } else {
            panic!("Expected BadRequest error for disabled relayer");
        }
    }

    #[actix_web::test]
    async fn test_update_relayer_invalid_patch() {
        let relayer = create_mock_relayer("test-relayer".to_string(), false);
        let app_state = create_mock_app_state(Some(vec![relayer]), None, None, None, None).await;

        let patch = serde_json::json!({
            "invalid_field": "value"
        });

        let result = update_relayer(
            "test-relayer".to_string(),
            patch,
            actix_web::web::ThinData(app_state),
        )
        .await;

        assert!(result.is_err());
        if let Err(ApiError::BadRequest(msg)) = result {
            assert!(msg.contains("Invalid update request"));
        } else {
            panic!("Expected BadRequest error for invalid patch");
        }
    }

    #[actix_web::test]
    async fn test_update_relayer_nonexistent() {
        let app_state = create_mock_app_state(None, None, None, None, None).await;

        let patch = serde_json::json!({
            "name": "Updated Name"
        });

        let result = update_relayer(
            "nonexistent-relayer".to_string(),
            patch,
            actix_web::web::ThinData(app_state),
        )
        .await;

        assert!(result.is_err());
        if let Err(ApiError::NotFound(msg)) = result {
            assert!(msg.contains("Relayer with ID nonexistent-relayer not found"));
        } else {
            panic!("Expected NotFound error for nonexistent relayer");
        }
    }

    #[actix_web::test]
    async fn test_update_relayer_set_evm_policies() {
        let relayer = create_mock_relayer("test-relayer".to_string(), false);
        let app_state = create_mock_app_state(Some(vec![relayer]), None, None, None, None).await;

        let patch = serde_json::json!({
            "policies": {
                "gas_price_cap": 50000000000u64,
                "min_balance": 1000000000000000000u64,
                "eip1559_pricing": true,
                "private_transactions": false,
                "gas_limit_estimation": true,
                "whitelist_receivers": ["0x1234567890123456789012345678901234567890"]
            }
        });

        let result = update_relayer(
            "test-relayer".to_string(),
            patch,
            actix_web::web::ThinData(app_state),
        )
        .await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 200);

        let body = to_bytes(response.into_body()).await.unwrap();
        let api_response: ApiResponse<RelayerResponse> = serde_json::from_slice(&body).unwrap();

        assert!(api_response.success);
        let data = api_response.data.unwrap();

        // For now, just verify that the policies field exists
        // The policy validation can be added once we understand the correct structure
        assert!(data.policies.is_some());
    }

    #[actix_web::test]
    async fn test_update_relayer_partial_policy_update() {
        let relayer = create_mock_relayer("test-relayer".to_string(), false);
        let app_state = create_mock_app_state(Some(vec![relayer]), None, None, None, None).await;

        // First update with some policies
        let patch1 = serde_json::json!({
            "policies": {
                "gas_price_cap": 30000000000u64,
                "min_balance": 500000000000000000u64,
                "eip1559_pricing": false
            }
        });

        let result1 = update_relayer(
            "test-relayer".to_string(),
            patch1,
            actix_web::web::ThinData(app_state),
        )
        .await;

        assert!(result1.is_ok());

        // Create fresh app state for second update test
        let relayer2 = create_mock_relayer("test-relayer".to_string(), false);
        let app_state2 = create_mock_app_state(Some(vec![relayer2]), None, None, None, None).await;

        // Second update with only gas_price_cap change
        let patch2 = serde_json::json!({
            "policies": {
                "gas_price_cap": 60000000000u64
            }
        });

        let result2 = update_relayer(
            "test-relayer".to_string(),
            patch2,
            actix_web::web::ThinData(app_state2),
        )
        .await;

        assert!(result2.is_ok());
        let response = result2.unwrap();
        assert_eq!(response.status(), 200);

        let body = to_bytes(response.into_body()).await.unwrap();
        let api_response: ApiResponse<RelayerResponse> = serde_json::from_slice(&body).unwrap();

        assert!(api_response.success);
        let data = api_response.data.unwrap();

        // Just verify policies exist for now
        assert!(data.policies.is_some());
    }

    #[actix_web::test]
    async fn test_update_relayer_unset_notification() {
        let mut relayer = create_mock_relayer("test-relayer".to_string(), false);
        relayer.notification_id = Some("test-notification".to_string());
        let app_state = create_mock_app_state(Some(vec![relayer]), None, None, None, None).await;

        let patch = serde_json::json!({
            "notification_id": null
        });

        let result = update_relayer(
            "test-relayer".to_string(),
            patch,
            actix_web::web::ThinData(app_state),
        )
        .await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 200);

        let body = to_bytes(response.into_body()).await.unwrap();
        let api_response: ApiResponse<RelayerResponse> = serde_json::from_slice(&body).unwrap();

        assert!(api_response.success);
        let data = api_response.data.unwrap();
        assert_eq!(data.notification_id, None);
    }

    #[actix_web::test]
    async fn test_update_relayer_unset_custom_rpc_urls() {
        let mut relayer = create_mock_relayer("test-relayer".to_string(), false);
        relayer.custom_rpc_urls = Some(vec![crate::models::RpcConfig {
            url: "https://custom-rpc.example.com".to_string(),
            weight: 50,
        }]);
        let app_state = create_mock_app_state(Some(vec![relayer]), None, None, None, None).await;

        let patch = serde_json::json!({
            "custom_rpc_urls": null
        });

        let result = update_relayer(
            "test-relayer".to_string(),
            patch,
            actix_web::web::ThinData(app_state),
        )
        .await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 200);

        let body = to_bytes(response.into_body()).await.unwrap();
        let api_response: ApiResponse<RelayerResponse> = serde_json::from_slice(&body).unwrap();

        assert!(api_response.success);
        let data = api_response.data.unwrap();
        assert_eq!(data.custom_rpc_urls, None);
    }

    #[actix_web::test]
    async fn test_update_relayer_set_custom_rpc_urls() {
        let relayer = create_mock_relayer("test-relayer".to_string(), false);
        let app_state = create_mock_app_state(Some(vec![relayer]), None, None, None, None).await;

        let patch = serde_json::json!({
            "custom_rpc_urls": [
                {
                    "url": "https://rpc1.example.com",
                    "weight": 80
                },
                {
                    "url": "https://rpc2.example.com",
                    "weight": 60
                }
            ]
        });

        let result = update_relayer(
            "test-relayer".to_string(),
            patch,
            actix_web::web::ThinData(app_state),
        )
        .await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 200);

        let body = to_bytes(response.into_body()).await.unwrap();
        let api_response: ApiResponse<RelayerResponse> = serde_json::from_slice(&body).unwrap();

        assert!(api_response.success);
        let data = api_response.data.unwrap();

        assert!(data.custom_rpc_urls.is_some());
        let rpc_urls = data.custom_rpc_urls.unwrap();
        assert_eq!(rpc_urls.len(), 2);
        assert_eq!(rpc_urls[0].url, "https://rpc1.example.com");
        assert_eq!(rpc_urls[0].weight, 80);
        assert_eq!(rpc_urls[1].url, "https://rpc2.example.com");
        assert_eq!(rpc_urls[1].weight, 60);
    }

    #[actix_web::test]
    async fn test_update_relayer_clear_policies() {
        let relayer = create_mock_relayer("test-relayer".to_string(), false);
        let app_state = create_mock_app_state(Some(vec![relayer]), None, None, None, None).await;

        let patch = serde_json::json!({
            "policies": null
        });

        let result = update_relayer(
            "test-relayer".to_string(),
            patch,
            actix_web::web::ThinData(app_state),
        )
        .await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 200);

        let body = to_bytes(response.into_body()).await.unwrap();
        let api_response: ApiResponse<RelayerResponse> = serde_json::from_slice(&body).unwrap();

        assert!(api_response.success);
        let data = api_response.data.unwrap();
        assert_eq!(data.policies, None);
    }

    #[actix_web::test]
    async fn test_update_relayer_invalid_policy_structure() {
        let relayer = create_mock_relayer("test-relayer".to_string(), false);
        let app_state = create_mock_app_state(Some(vec![relayer]), None, None, None, None).await;

        let patch = serde_json::json!({
            "policies": {
                "invalid_field_name": "some_value"
            }
        });

        let result = update_relayer(
            "test-relayer".to_string(),
            patch,
            actix_web::web::ThinData(app_state),
        )
        .await;

        assert!(result.is_err());
        if let Err(ApiError::BadRequest(msg)) = result {
            assert!(msg.contains("Invalid policy"));
        } else {
            panic!("Expected BadRequest error for invalid policy structure");
        }
    }

    #[actix_web::test]
    async fn test_update_relayer_invalid_evm_policy_values() {
        let relayer = create_mock_relayer("test-relayer".to_string(), false);
        let app_state = create_mock_app_state(Some(vec![relayer]), None, None, None, None).await;

        let patch = serde_json::json!({
            "policies": {
                "gas_price_cap": "invalid_number",
                "min_balance": -1
            }
        });

        let result = update_relayer(
            "test-relayer".to_string(),
            patch,
            actix_web::web::ThinData(app_state),
        )
        .await;

        assert!(result.is_err());
        if let Err(ApiError::BadRequest(msg)) = result {
            assert!(msg.contains("Invalid policy") || msg.contains("Invalid update request"));
        } else {
            panic!("Expected BadRequest error for invalid policy values");
        }
    }

    #[actix_web::test]
    async fn test_update_relayer_multiple_fields_at_once() {
        let mut relayer = create_mock_relayer("test-relayer".to_string(), false);
        relayer.notification_id = Some("old-notification".to_string());
        let app_state = create_mock_app_state(Some(vec![relayer]), None, None, None, None).await;

        let patch = serde_json::json!({
            "name": "Multi-Update Relayer",
            "paused": true,
            "notification_id": null,
            "policies": {
                "gas_price_cap": 40000000000u64,
                "eip1559_pricing": true
            },
            "custom_rpc_urls": [
                {
                    "url": "https://new-rpc.example.com",
                    "weight": 90
                }
            ]
        });

        let result = update_relayer(
            "test-relayer".to_string(),
            patch,
            actix_web::web::ThinData(app_state),
        )
        .await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 200);

        let body = to_bytes(response.into_body()).await.unwrap();
        let api_response: ApiResponse<RelayerResponse> = serde_json::from_slice(&body).unwrap();

        assert!(api_response.success);
        let data = api_response.data.unwrap();

        // Verify all fields were updated correctly
        assert_eq!(data.name, "Multi-Update Relayer");
        assert!(data.paused);
        assert_eq!(data.notification_id, None);

        // Verify policies and RPC URLs were set
        assert!(data.policies.is_some());
        assert!(data.custom_rpc_urls.is_some());
        let rpc_urls = data.custom_rpc_urls.unwrap();
        assert_eq!(rpc_urls.len(), 1);
        assert_eq!(rpc_urls[0].url, "https://new-rpc.example.com");
        assert_eq!(rpc_urls[0].weight, 90);
    }

    #[actix_web::test]
    async fn test_update_relayer_solana_policies() {
        use crate::models::{
            NetworkType, RelayerNetworkPolicy, RelayerSolanaPolicy, SolanaFeePaymentStrategy,
        };

        // Create a Solana relayer (not the default EVM one)
        let mut solana_relayer = create_mock_relayer("test-solana-relayer".to_string(), false);
        solana_relayer.network_type = NetworkType::Solana;
        solana_relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy::default());

        let app_state =
            create_mock_app_state(Some(vec![solana_relayer]), None, None, None, None).await;

        let patch = serde_json::json!({
            "policies": {
                "fee_payment_strategy": "user",
                "min_balance": 2000000,
                "max_signatures": 5,
                "max_tx_data_size": 800,
                "max_allowed_fee_lamports": 25000,
                "fee_margin_percentage": 15.0
            }
        });

        let result = update_relayer(
            "test-solana-relayer".to_string(),
            patch,
            actix_web::web::ThinData(app_state),
        )
        .await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 200);

        let body = to_bytes(response.into_body()).await.unwrap();
        let api_response: ApiResponse<RelayerResponse> = serde_json::from_slice(&body).unwrap();

        assert!(api_response.success);
        let data = api_response.data.unwrap();

        // Verify Solana policies are present and correctly updated
        assert!(data.policies.is_some());
        let policies = data.policies.unwrap();
        if let RelayerNetworkPolicyResponse::Solana(solana_policy) = policies {
            assert_eq!(
                solana_policy.fee_payment_strategy,
                Some(SolanaFeePaymentStrategy::User)
            );
            assert_eq!(solana_policy.min_balance, 2000000);
            assert_eq!(solana_policy.max_signatures, Some(5));
            assert_eq!(solana_policy.max_tx_data_size, 800);
            assert_eq!(solana_policy.max_allowed_fee_lamports, Some(25000));
            assert_eq!(solana_policy.fee_margin_percentage, Some(15.0));
        } else {
            panic!("Expected Solana policies in response");
        }
    }

    // DELETE RELAYER TESTS

    #[actix_web::test]
    async fn test_delete_relayer_success() {
        let relayer = create_mock_relayer("test-relayer".to_string(), false);
        let app_state = create_mock_app_state(Some(vec![relayer]), None, None, None, None).await;

        let result = delete_relayer(
            "test-relayer".to_string(),
            actix_web::web::ThinData(app_state),
        )
        .await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), 200);

        let body = to_bytes(response.into_body()).await.unwrap();
        let api_response: ApiResponse<String> = serde_json::from_slice(&body).unwrap();

        assert!(api_response.success);
        let data = api_response.data.unwrap();
        assert!(data.contains("Relayer deleted successfully"));
    }

    #[actix_web::test]
    async fn test_delete_relayer_with_transactions() {
        let relayer = create_mock_relayer("relayer-with-tx".to_string(), false);
        let mut transaction = create_mock_transaction();
        transaction.id = "test-tx".to_string();
        transaction.relayer_id = "relayer-with-tx".to_string();
        let app_state = create_mock_app_state(
            Some(vec![relayer]),
            None,
            None,
            None,
            Some(vec![transaction]),
        )
        .await;

        let result = delete_relayer(
            "relayer-with-tx".to_string(),
            actix_web::web::ThinData(app_state),
        )
        .await;

        assert!(result.is_err());
        if let Err(ApiError::BadRequest(msg)) = result {
            assert!(msg.contains("Cannot delete relayer 'relayer-with-tx'"));
            assert!(msg.contains("has 1 transaction(s)"));
            assert!(msg.contains("wait for all transactions to complete"));
        } else {
            panic!("Expected BadRequest error for relayer with transactions");
        }
    }

    #[actix_web::test]
    async fn test_delete_relayer_nonexistent() {
        let app_state = create_mock_app_state(None, None, None, None, None).await;

        let result = delete_relayer(
            "nonexistent-relayer".to_string(),
            actix_web::web::ThinData(app_state),
        )
        .await;

        assert!(result.is_err());
        if let Err(ApiError::NotFound(msg)) = result {
            assert!(msg.contains("Relayer with ID nonexistent-relayer not found"));
        } else {
            panic!("Expected NotFound error for nonexistent relayer");
        }
    }
}
