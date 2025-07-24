//! Notification handling worker implementation.
//!
//! This module implements the notification handling worker that processes
//! notification jobs from the queue.

use actix_web::web::ThinData;
use apalis::prelude::{Attempt, Data, *};
use eyre::Result;
use log::info;

use crate::{
    constants::WORKER_DEFAULT_MAXIMUM_RETRIES,
    jobs::{handle_result, Job, NotificationSend},
    models::DefaultAppState,
    repositories::Repository,
    services::WebhookNotificationService,
};

/// Handles incoming notification jobs from the queue.
///
/// # Arguments
/// * `job` - The notification job containing recipient and message details
/// * `context` - Application state containing notification services
///
/// # Returns
/// * `Result<(), Error>` - Success or failure of notification processing
pub async fn notification_handler(
    job: Job<NotificationSend>,
    context: Data<ThinData<DefaultAppState>>,
    attempt: Attempt,
) -> Result<(), Error> {
    info!("handling notification: {:?}", job.data);

    let result = handle_request(job.data, context).await;

    handle_result(
        result,
        attempt,
        "Notification",
        WORKER_DEFAULT_MAXIMUM_RETRIES,
    )
}

async fn handle_request(
    request: NotificationSend,
    context: Data<ThinData<DefaultAppState>>,
) -> Result<()> {
    info!("sending notification: {:?}", request);
    let notification = context
        .notification_repository
        .get_by_id(request.notification_id)
        .await?;

    let notification_service =
        WebhookNotificationService::new(notification.url, notification.signing_key);

    notification_service
        .send_notification(request.notification)
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        EvmTransactionResponse, NetworkType, RelayerDisabledPayload, RelayerEvmPolicy,
        RelayerNetworkPolicyResponse, RelayerResponse, TransactionResponse, TransactionStatus,
        WebhookNotification, WebhookPayload, U256,
    };

    #[tokio::test]
    async fn test_notification_job_creation() {
        // Create a basic notification webhook payload
        let payload = WebhookPayload::Transaction(TransactionResponse::Evm(Box::new(
            EvmTransactionResponse {
                id: "tx123".to_string(),
                hash: Some("0x123".to_string()),
                status: TransactionStatus::Confirmed,
                status_reason: None,
                created_at: "2025-01-27T15:31:10.777083+00:00".to_string(),
                sent_at: Some("2025-01-27T15:31:10.777083+00:00".to_string()),
                confirmed_at: Some("2025-01-27T15:31:10.777083+00:00".to_string()),
                gas_price: Some(1000000000),
                gas_limit: Some(21000),
                nonce: Some(1),
                value: U256::from(1000000000000000000_u64),
                from: "0xabc".to_string(),
                to: Some("0xdef".to_string()),
                relayer_id: "relayer-1".to_string(),
                data: None,
                max_fee_per_gas: None,
                max_priority_fee_per_gas: None,
                signature: None,
                speed: None,
            },
        )));

        // Create a notification
        let notification = WebhookNotification::new("test_event".to_string(), payload);
        let notification_job =
            NotificationSend::new("notification-1".to_string(), notification.clone());

        // Create the job
        let job = Job::new(crate::jobs::JobType::NotificationSend, notification_job);

        // Test the job structure
        assert_eq!(job.data.notification_id, "notification-1");
        assert_eq!(job.data.notification.event, "test_event");
    }

    #[tokio::test]
    async fn test_notification_job_with_different_payloads() {
        // Test with different payload types

        let transaction_payload = WebhookPayload::Transaction(TransactionResponse::Evm(Box::new(
            EvmTransactionResponse {
                id: "tx123".to_string(),
                hash: Some("0x123".to_string()),
                status: TransactionStatus::Confirmed,
                status_reason: None,
                created_at: "2025-01-27T15:31:10.777083+00:00".to_string(),
                sent_at: Some("2025-01-27T15:31:10.777083+00:00".to_string()),
                confirmed_at: Some("2025-01-27T15:31:10.777083+00:00".to_string()),
                gas_price: Some(1000000000),
                gas_limit: Some(21000),
                nonce: Some(1),
                value: U256::from(1000000000000000000_u64),
                from: "0xabc".to_string(),
                to: Some("0xdef".to_string()),
                relayer_id: "relayer-1".to_string(),
                data: None,
                max_fee_per_gas: None,
                max_priority_fee_per_gas: None,
                signature: None,
                speed: None,
            },
        )));

        let string_notification =
            WebhookNotification::new("transaction_payload".to_string(), transaction_payload);
        let job = NotificationSend::new("notification-string".to_string(), string_notification);
        assert_eq!(job.notification.event, "transaction_payload");

        let relayer_disabled = WebhookPayload::RelayerDisabled(Box::new(RelayerDisabledPayload {
            relayer: RelayerResponse {
                id: "relayer-1".to_string(),
                name: "relayer-1".to_string(),
                network: "ethereum".to_string(),
                network_type: NetworkType::Evm,
                paused: false,
                policies: Some(RelayerNetworkPolicyResponse::Evm(
                    RelayerEvmPolicy {
                        gas_price_cap: None,
                        whitelist_receivers: None,
                        eip1559_pricing: None,
                        private_transactions: Some(false),
                        min_balance: Some(0),
                        gas_limit_estimation: None,
                    }
                    .into(),
                )),
                signer_id: "signer-1".to_string(),
                notification_id: None,
                custom_rpc_urls: None,
                address: Some("0xabc".to_string()),
                system_disabled: Some(false),
            },
            disable_reason: "test".to_string(),
        }));
        let object_notification =
            WebhookNotification::new("object_event".to_string(), relayer_disabled);
        let job = NotificationSend::new("notification-object".to_string(), object_notification);
        assert_eq!(job.notification.event, "object_event");
    }
}
