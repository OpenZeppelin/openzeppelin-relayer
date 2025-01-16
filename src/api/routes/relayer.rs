use crate::{
    api::controllers::relayer,
    domain::{JsonRpcRequest, SignDataRequest},
    models::{AppState, PaginationQuery},
};
use actix_web::{delete, get, post, put, web, Responder};
use serde::Deserialize;

// list all relayers
#[get("/relayers")]
async fn list_relayers(
    query: web::Query<PaginationQuery>,
    data: web::ThinData<AppState>,
) -> impl Responder {
    relayer::list_relayers(query.into_inner(), data).await
}

// get relayer details
#[get("/relayers/{relayer_id}")]
async fn get_relayer(
    relayer_id: web::Path<String>,
    data: web::ThinData<AppState>,
) -> impl Responder {
    relayer::get_relayer(relayer_id.into_inner(), data).await
}

// get relayer status
#[post("/relayers/{relayer_id}/status")]
async fn get_relayer_status(
    relayer_id: web::Path<String>,
    data: web::ThinData<AppState>,
) -> impl Responder {
    relayer::get_relayer_status(relayer_id.into_inner(), data).await
}

// get relayer balance
#[post("/relayers/{relayer_id}/balance")]
async fn get_relayer_balance(
    relayer_id: web::Path<String>,
    data: web::ThinData<AppState>,
) -> impl Responder {
    relayer::get_relayer_balance(relayer_id.into_inner(), data).await
}

// send transaction
#[post("/relayers/{relayer_id}/transactions")]
async fn send_relayer_transaction(
    relayer_id: web::Path<String>,
    req: web::Json<serde_json::Value>,
    data: web::ThinData<AppState>,
) -> impl Responder {
    relayer::send_transaction(relayer_id.into_inner(), req.into_inner(), data).await
}

#[derive(Deserialize)]
pub struct TransactionPath {
    relayer_id: String,
    transaction_id: String,
}

#[get("/relayers/{relayer_id}/transactions/{transaction_id}")]
async fn get_relayer_transaction_by_id(
    path: web::Path<TransactionPath>,
    data: web::ThinData<AppState>,
) -> impl Responder {
    let path = path.into_inner();
    relayer::get_transaction_by_id(path.relayer_id, path.transaction_id, data).await
}

#[get("/relayers/{relayer_id}/transactions/by-nonce/{nonce}")]
async fn get_relayer_transaction_by_nonce(
    relayer_id: web::Path<String>,
    nonce: web::Path<u64>,
    data: web::ThinData<AppState>,
) -> impl Responder {
    relayer::get_transaction_by_nonce(relayer_id.into_inner(), nonce.into_inner(), data).await
}

#[get("/relayers/{relayer_id}/transactions")]
async fn list_relayer_transactions(
    relayer_id: web::Path<String>,
    query: web::Query<PaginationQuery>,
    data: web::ThinData<AppState>,
) -> impl Responder {
    relayer::list_transactions(relayer_id.into_inner(), query.into_inner(), data).await
}

#[delete("/relayers/{relayer_id}/transactions/pending")]
async fn delete_pending_transactions(
    relayer_id: web::Path<String>,
    data: web::ThinData<AppState>,
) -> impl Responder {
    relayer::delete_pending_transactions(relayer_id.into_inner(), data).await
}

#[delete("/relayers/{relayer_id}/transactions/{transaction_id}")]
async fn cancel_relayer_transaction(
    relayer_id: web::Path<String>,
    transaction_id: web::Path<String>,
    data: web::ThinData<AppState>,
) -> impl Responder {
    relayer::cancel_transaction(relayer_id.into_inner(), transaction_id.into_inner(), data).await
}

#[put("/relayers/{relayer_id}/transactions/{transaction_id}")]
async fn replace_relayer_transaction(
    relayer_id: web::Path<String>,
    transaction_id: web::Path<String>,
    data: web::ThinData<AppState>,
) -> impl Responder {
    relayer::replace_transaction(relayer_id.into_inner(), transaction_id.into_inner(), data).await
}

#[post("/relayers/{relayer_id}/sign")]
async fn relayer_sign(
    relayer_id: web::Path<String>,
    req: web::Json<SignDataRequest>,
    data: web::ThinData<AppState>,
) -> impl Responder {
    relayer::sign_data(relayer_id.into_inner(), req.into_inner(), data).await
}

#[post("/relayers/{relayer_id}/sign-typed-data")]
async fn relayer_sign_typed_data(
    relayer_id: web::Path<String>,
    req: web::Json<SignDataRequest>,
    data: web::ThinData<AppState>,
) -> impl Responder {
    relayer::sign_typed_data(relayer_id.into_inner(), req.into_inner(), data).await
}

#[post("/relayers/{relayer_id}/rpc")]
async fn relayer_rpc(
    relayer_id: web::Path<String>,
    req: web::Json<JsonRpcRequest>,
    data: web::ThinData<AppState>,
) -> impl Responder {
    relayer::relayer_rpc(relayer_id.into_inner(), req.into_inner(), data).await
}

pub fn init(cfg: &mut web::ServiceConfig) {
    cfg.service(list_relayers);
    cfg.service(get_relayer);
    cfg.service(get_relayer_balance);
    cfg.service(get_relayer_transaction_by_id);
    cfg.service(get_relayer_transaction_by_nonce);
    cfg.service(send_relayer_transaction);
    cfg.service(list_relayer_transactions);
    cfg.service(get_relayer_status);
    cfg.service(relayer_sign_typed_data);
    cfg.service(relayer_sign);
    cfg.service(cancel_relayer_transaction);
    cfg.service(delete_pending_transactions);
    cfg.service(relayer_rpc);
}
