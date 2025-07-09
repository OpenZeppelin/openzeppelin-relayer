use crate::{
    domain::SwapResult,
    jobs::NotificationSend,
    models::{
        RelayerRepoModel, RelayerResponse, SignAndSendTransactionResult, SignTransactionResult,
        TransactionRepoModel, TransactionResponse, TransferTransactionResult,
    },
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct WebhookNotification {
    pub id: String,
    pub event: String,
    pub payload: WebhookPayload,
    pub timestamp: String,
}

impl WebhookNotification {
    pub fn new(event: String, payload: WebhookPayload) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            event,
            payload,
            timestamp: Utc::now().to_rfc3339(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct TransactionFailurePayload {
    pub transaction: TransactionResponse,
    pub failure_reason: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct RelayerDisabledPayload {
    pub relayer: RelayerResponse,
    pub disable_reason: String,
}
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct SolanaDexPayload {
    pub swap_results: Vec<SwapResult>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
#[serde(tag = "payload_type")]
pub enum WebhookPayload {
    Transaction(TransactionResponse),
    #[serde(rename = "transaction_failure")]
    TransactionFailure(TransactionFailurePayload),
    #[serde(rename = "relayer_disabled")]
    RelayerDisabled(Box<RelayerDisabledPayload>),
    #[serde(rename = "solana_rpc")]
    SolanaRpc(SolanaWebhookRpcPayload),
    #[serde(rename = "solana_dex")]
    SolanaDex(SolanaDexPayload),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WebhookResponse {
    pub status: String,
    pub message: Option<String>,
}

pub fn produce_transaction_update_notification_payload(
    notification_id: &str,
    transaction: &TransactionRepoModel,
) -> NotificationSend {
    let tx_payload: TransactionResponse = transaction.clone().into();
    NotificationSend::new(
        notification_id.to_string(),
        WebhookNotification::new(
            "transaction_update".to_string(),
            WebhookPayload::Transaction(tx_payload),
        ),
    )
}

pub fn produce_relayer_disabled_payload(
    notification_id: &str,
    relayer: &RelayerRepoModel,
    reason: &str,
) -> NotificationSend {
    let relayer_response: RelayerResponse = relayer.clone().into();
    let payload = RelayerDisabledPayload {
        relayer: relayer_response,
        disable_reason: reason.to_string(),
    };
    NotificationSend::new(
        notification_id.to_string(),
        WebhookNotification::new(
            "relayer_state_update".to_string(),
            WebhookPayload::RelayerDisabled(Box::new(payload)),
        ),
    )
}

#[allow(clippy::enum_variant_names)]
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(untagged)]
pub enum SolanaWebhookRpcPayload {
    SignAndSendTransaction(SignAndSendTransactionResult),
    SignTransaction(SignTransactionResult),
    TransferTransaction(TransferTransactionResult),
}

/// Produces a notification payload for a Solana RPC webhook event
pub fn produce_solana_rpc_webhook_payload(
    notification_id: &str,
    event: String,
    payload: SolanaWebhookRpcPayload,
) -> NotificationSend {
    NotificationSend::new(
        notification_id.to_string(),
        WebhookNotification::new(event, WebhookPayload::SolanaRpc(payload)),
    )
}

pub fn produce_solana_dex_webhook_payload(
    notification_id: &str,
    event: String,
    payload: SolanaDexPayload,
) -> NotificationSend {
    NotificationSend::new(
        notification_id.to_string(),
        WebhookNotification::new(event, WebhookPayload::SolanaDex(payload)),
    )
}
