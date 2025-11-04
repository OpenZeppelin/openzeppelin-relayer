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
pub struct RelayerEnabledPayload {
    pub relayer: RelayerResponse,
    pub retry_count: u32,
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
    #[serde(rename = "relayer_enabled")]
    RelayerEnabled(Box<RelayerEnabledPayload>),
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

pub fn produce_relayer_enabled_payload(
    notification_id: &str,
    relayer: &RelayerRepoModel,
    retry_count: u32,
) -> NotificationSend {
    let relayer_response: RelayerResponse = relayer.clone().into();
    let payload = RelayerEnabledPayload {
        relayer: relayer_response,
        retry_count,
    };
    NotificationSend::new(
        notification_id.to_string(),
        WebhookNotification::new(
            "relayer_state_update".to_string(),
            WebhookPayload::RelayerEnabled(Box::new(payload)),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::mocks::mockutils::{create_mock_relayer, create_mock_transaction};

    #[test]
    fn test_webhook_notification_new() {
        let payload = WebhookPayload::Transaction(create_mock_transaction().into());
        let notification = WebhookNotification::new("test_event".to_string(), payload.clone());

        // Verify structure
        assert!(!notification.id.is_empty(), "Should have an ID");
        assert_eq!(notification.event, "test_event");
        assert_eq!(notification.payload, payload);
        assert!(
            !notification.timestamp.is_empty(),
            "Should have a timestamp"
        );

        // Verify ID is a valid UUID
        assert!(
            Uuid::parse_str(&notification.id).is_ok(),
            "ID should be a valid UUID"
        );

        // Verify timestamp is valid RFC3339
        assert!(
            chrono::DateTime::parse_from_rfc3339(&notification.timestamp).is_ok(),
            "Timestamp should be valid RFC3339"
        );
    }

    #[test]
    fn test_webhook_notification_unique_ids() {
        let payload = WebhookPayload::Transaction(create_mock_transaction().into());
        let notification1 = WebhookNotification::new("event".to_string(), payload.clone());
        let notification2 = WebhookNotification::new("event".to_string(), payload);

        // Each notification should have a unique ID
        assert_ne!(
            notification1.id, notification2.id,
            "Each notification should have a unique UUID"
        );
    }

    #[test]
    fn test_produce_transaction_update_notification_payload() {
        let transaction = create_mock_transaction();
        let notification_id = "test-notification-id";

        let result = produce_transaction_update_notification_payload(notification_id, &transaction);

        // Verify notification_id
        assert_eq!(result.notification_id, notification_id);

        // Verify webhook structure
        assert_eq!(result.notification.event, "transaction_update");

        // Verify payload type
        match result.notification.payload {
            WebhookPayload::Transaction(TransactionResponse::Evm(tx)) => {
                assert_eq!(tx.id, transaction.id);
                assert_eq!(tx.relayer_id, transaction.relayer_id);
            }
            _ => panic!("Expected Transaction(Evm) payload"),
        }
    }

    #[test]
    fn test_produce_relayer_disabled_payload() {
        let relayer = create_mock_relayer("test-relayer".to_string(), false);
        let notification_id = "test-notification-id";
        let reason = "RPC endpoint validation failed";

        let result = produce_relayer_disabled_payload(notification_id, &relayer, reason);

        // Verify notification_id
        assert_eq!(result.notification_id, notification_id);

        // Verify webhook structure
        assert_eq!(result.notification.event, "relayer_state_update");

        // Verify payload type and content
        match result.notification.payload {
            WebhookPayload::RelayerDisabled(payload) => {
                assert_eq!(payload.relayer.id, relayer.id);
                assert_eq!(payload.disable_reason, reason);
            }
            _ => panic!("Expected RelayerDisabled payload"),
        }
    }

    #[test]
    fn test_produce_relayer_disabled_payload_with_sensitive_info() {
        let relayer = create_mock_relayer("test-relayer".to_string(), false);
        let notification_id = "test-notification-id";
        // This should be a safe description (from safe_description())
        let reason = "RPC endpoint validation failed";

        let result = produce_relayer_disabled_payload(notification_id, &relayer, reason);

        match result.notification.payload {
            WebhookPayload::RelayerDisabled(payload) => {
                // Verify it doesn't contain sensitive details
                assert!(!payload.disable_reason.contains("http://"));
                assert!(!payload.disable_reason.contains("https://"));
                assert_eq!(payload.disable_reason, reason);
            }
            _ => panic!("Expected RelayerDisabled payload"),
        }
    }

    #[test]
    fn test_produce_relayer_enabled_payload() {
        let relayer = create_mock_relayer("test-relayer".to_string(), true);
        let notification_id = "test-notification-id";
        let retry_count = 5;

        let result = produce_relayer_enabled_payload(notification_id, &relayer, retry_count);

        // Verify notification_id
        assert_eq!(result.notification_id, notification_id);

        // Verify webhook structure
        assert_eq!(result.notification.event, "relayer_state_update");

        // Verify payload type and content
        match result.notification.payload {
            WebhookPayload::RelayerEnabled(payload) => {
                assert_eq!(payload.relayer.id, relayer.id);
                assert_eq!(payload.retry_count, retry_count);
            }
            _ => panic!("Expected RelayerEnabled payload"),
        }
    }

    #[test]
    fn test_produce_relayer_enabled_payload_with_zero_retries() {
        let relayer = create_mock_relayer("test-relayer".to_string(), true);
        let notification_id = "test-notification-id";

        let result = produce_relayer_enabled_payload(notification_id, &relayer, 0);

        match result.notification.payload {
            WebhookPayload::RelayerEnabled(payload) => {
                assert_eq!(payload.retry_count, 0);
            }
            _ => panic!("Expected RelayerEnabled payload"),
        }
    }

    #[test]
    fn test_produce_solana_rpc_webhook_payload() {
        use crate::models::EncodedSerializedTransaction;

        let notification_id = "test-notification-id";
        let event = "solana_sign_transaction".to_string();
        let solana_payload = SolanaWebhookRpcPayload::SignTransaction(SignTransactionResult {
            transaction: EncodedSerializedTransaction::new("test-transaction".to_string()),
            signature: "test-signature".to_string(),
        });

        let result =
            produce_solana_rpc_webhook_payload(notification_id, event.clone(), solana_payload);

        // Verify notification_id
        assert_eq!(result.notification_id, notification_id);

        // Verify webhook structure
        assert_eq!(result.notification.event, event);

        // Verify payload type
        match result.notification.payload {
            WebhookPayload::SolanaRpc(SolanaWebhookRpcPayload::SignTransaction(sig)) => {
                assert_eq!(sig.signature, "test-signature");
            }
            _ => panic!("Expected SolanaRpc SignTransaction payload"),
        }
    }

    #[test]
    fn test_produce_solana_dex_webhook_payload() {
        let notification_id = "test-notification-id";
        let event = "solana_swap_completed".to_string();
        let swap_results = vec![];
        let dex_payload = SolanaDexPayload { swap_results };

        let result =
            produce_solana_dex_webhook_payload(notification_id, event.clone(), dex_payload.clone());

        // Verify notification_id
        assert_eq!(result.notification_id, notification_id);

        // Verify webhook structure
        assert_eq!(result.notification.event, event);

        // Verify payload type
        match result.notification.payload {
            WebhookPayload::SolanaDex(payload) => {
                assert_eq!(payload.swap_results.len(), 0);
            }
            _ => panic!("Expected SolanaDex payload"),
        }
    }

    #[test]
    fn test_webhook_payload_serialization_transaction() {
        let transaction = create_mock_transaction();
        let payload = WebhookPayload::Transaction(transaction.into());

        let serialized = serde_json::to_value(&payload).unwrap();

        // Verify it has the correct tag
        assert_eq!(serialized["payload_type"], "transaction");
    }

    #[test]
    fn test_webhook_payload_serialization_relayer_disabled() {
        let relayer = create_mock_relayer("test".to_string(), false);
        let payload = WebhookPayload::RelayerDisabled(Box::new(RelayerDisabledPayload {
            relayer: relayer.into(),
            disable_reason: "Test reason".to_string(),
        }));

        let serialized = serde_json::to_value(&payload).unwrap();

        // Verify it has the correct tag
        assert_eq!(serialized["payload_type"], "relayer_disabled");
        assert!(serialized["disable_reason"].is_string());
    }

    #[test]
    fn test_webhook_payload_serialization_relayer_enabled() {
        let relayer = create_mock_relayer("test".to_string(), false);
        let payload = WebhookPayload::RelayerEnabled(Box::new(RelayerEnabledPayload {
            relayer: relayer.into(),
            retry_count: 3,
        }));

        let serialized = serde_json::to_value(&payload).unwrap();

        // Verify it has the correct tag
        assert_eq!(serialized["payload_type"], "relayer_enabled");
        assert_eq!(serialized["retry_count"], 3);
    }

    #[test]
    fn test_notification_send_structure() {
        let transaction = create_mock_transaction();
        let notification_id = "test-notification-id";

        let notification_send =
            produce_transaction_update_notification_payload(notification_id, &transaction);

        // Verify the NotificationSend can be serialized (for job queue)
        let serialized = serde_json::to_string(&notification_send);
        assert!(
            serialized.is_ok(),
            "NotificationSend should be serializable"
        );

        // Verify it can be deserialized back
        let deserialized: Result<NotificationSend, _> = serde_json::from_str(&serialized.unwrap());
        assert!(
            deserialized.is_ok(),
            "NotificationSend should be deserializable"
        );
    }

    #[test]
    fn test_relayer_disabled_and_enabled_use_same_event() {
        let relayer = create_mock_relayer("test".to_string(), false);

        let disabled_notification =
            produce_relayer_disabled_payload("notif-id", &relayer, "reason");
        let enabled_notification = produce_relayer_enabled_payload("notif-id", &relayer, 1);

        // Both should use the same event type for consistency
        assert_eq!(
            disabled_notification.notification.event,
            enabled_notification.notification.event
        );
        assert_eq!(
            disabled_notification.notification.event,
            "relayer_state_update"
        );
    }
}
