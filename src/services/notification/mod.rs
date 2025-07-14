//! This module provides the `WebhookNotificationService` for sending notifications via webhooks.
use crate::models::{SecretString, WebhookNotification, WebhookResponse};
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD, Engine};
use hmac::{Hmac, Mac};
#[cfg(test)]
use mockall::automock;
use reqwest::Client;
use sha2::Sha256;
use thiserror::Error;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone)]
pub struct WebhookNotificationService {
    client: Client,
    webhook_url: String,
    secret_key: Option<SecretString>,
}

#[cfg_attr(test, automock)]
#[async_trait]
pub trait WebhookNotificationServiceTrait: Send + Sync {
    async fn send_notification(
        &self,
        notification: WebhookNotification,
    ) -> Result<WebhookResponse, WebhookNotificationError>;

    fn sign_payload(
        &self,
        payload: &str,
        secret_key: &SecretString,
    ) -> Result<String, WebhookNotificationError>;
}

#[async_trait]
impl WebhookNotificationServiceTrait for WebhookNotificationService {
    async fn send_notification(
        &self,
        notification: WebhookNotification,
    ) -> Result<WebhookResponse, WebhookNotificationError> {
        self.send_notification(notification).await
    }

    fn sign_payload(
        &self,
        payload: &str,
        secret_key: &SecretString,
    ) -> Result<String, WebhookNotificationError> {
        self.sign_payload(payload, secret_key)
    }
}

impl WebhookNotificationService {
    pub fn new(webhook_url: String, secret_key: Option<SecretString>) -> Self {
        Self {
            client: Client::new(),
            webhook_url,
            secret_key,
        }
    }

    fn sign_payload(
        &self,
        payload: &str,
        secret_key: &SecretString,
    ) -> Result<String, WebhookNotificationError> {
        let mut mac = HmacSha256::new_from_slice(secret_key.to_str().as_bytes())
            .map_err(|e| WebhookNotificationError::SigningError(e.to_string()))?;
        mac.update(payload.as_bytes());
        let result = mac.finalize();
        let code_bytes = result.into_bytes();
        Ok(STANDARD.encode(code_bytes))
    }

    pub async fn send_notification(
        &self,
        notification: WebhookNotification,
    ) -> Result<WebhookResponse, WebhookNotificationError> {
        let payload = serde_json::to_string(&notification)?;

        let response = match self.secret_key.as_ref() {
            Some(key) => {
                let signature = self.sign_payload(&payload, key)?;

                self.client
                    .post(&self.webhook_url)
                    .header("X-Signature", signature)
                    .json(&notification)
                    .send()
                    .await?
            }
            None => {
                self.client
                    .post(&self.webhook_url)
                    .json(&notification)
                    .send()
                    .await?
            }
        };

        if response.status().is_success() {
            Ok(WebhookResponse {
                status: "success".to_string(),
                message: None,
            })
        } else {
            let error_message: String = response.text().await?;
            Err(WebhookNotificationError::WebhookError(error_message))
        }
    }
}

#[derive(Debug, Error)]
#[allow(clippy::enum_variant_names)]
pub enum WebhookNotificationError {
    #[error("Request error: {0}")]
    RequestError(#[from] reqwest::Error),
    #[error("Response error: {0}")]
    ResponseError(#[from] serde_json::Error),
    #[error("Webhook error: {0}")]
    WebhookError(String),
    #[error("Signing error: {0}")]
    SigningError(String),
}

#[cfg(test)]
mod tests {
    use crate::models::U256;
    use crate::models::{
        EvmTransactionResponse, SecretString, TransactionResponse, TransactionStatus,
    };
    use crate::models::{WebhookNotification, WebhookPayload};
    use crate::services::notification::WebhookNotificationService;
    use base64::{engine::general_purpose::STANDARD, Engine};
    use serde_json::json;
    use wiremock::matchers::{header_exists, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn mock_transaction_response() -> TransactionResponse {
        TransactionResponse::Evm(Box::new(EvmTransactionResponse {
            id: "tx_123".to_string(),
            hash: Some("0x123...".to_string()),
            status: TransactionStatus::Pending,
            status_reason: None,
            created_at: "2024-03-20T10:00:00Z".to_string(),
            sent_at: Some("2024-03-20T10:00:01Z".to_string()),
            confirmed_at: None,
            gas_price: Some(0u128),
            gas_limit: Some(21000u64),
            nonce: Some(1u64),
            value: U256::from(0),
            from: "0x123...".to_string(),
            to: Some("0x456...".to_string()),
            relayer_id: "relayer_123".to_string(),
            data: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            signature: None,
            speed: None,
        }))
    }

    #[tokio::test]
    async fn test_successful_notification_with_signature() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/"))
            .and(header_exists("X-Signature"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "status": "success",
                "message": null
            })))
            .mount(&mock_server)
            .await;

        let secret_key = SecretString::new("test_secret");
        let service = WebhookNotificationService::new(
            mock_server.uri().to_string(),
            Some(secret_key.clone()),
        );

        let notification = WebhookNotification {
            id: "123".to_string(),
            event: "test_event".to_string(),
            payload: WebhookPayload::Transaction(mock_transaction_response()),
            timestamp: "2021-01-01T00:00:00Z".to_string(),
        };

        let result = service.send_notification(notification).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_failed_notification_without_signature() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "status": "success",
                "message": null
            })))
            .mount(&mock_server)
            .await;

        let service = WebhookNotificationService::new(mock_server.uri().to_string(), None);

        let notification = WebhookNotification {
            id: "123".to_string(),
            event: "test_event".to_string(),
            payload: WebhookPayload::Transaction(mock_transaction_response()),
            timestamp: "2021-01-01T00:00:00Z".to_string(),
        };

        let result = service.send_notification(notification).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_failed_notification_with_http_error() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(500).set_body_json(json!({
                "status": "error",
                "message": "Internal Server Error"
            })))
            .mount(&mock_server)
            .await;

        let secret_key = SecretString::new("test_secret");
        let service = WebhookNotificationService::new(
            mock_server.uri().to_string(),
            Some(secret_key.clone()),
        );

        let notification = WebhookNotification {
            id: "123".to_string(),
            event: "test_event".to_string(),
            payload: WebhookPayload::Transaction(mock_transaction_response()),
            timestamp: "2021-01-01T00:00:00Z".to_string(),
        };

        let result = service.send_notification(notification).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_sign_payload() {
        let service = WebhookNotificationService::new(
            "http://example.com".to_string(),
            Some(SecretString::new("test_secret")),
        );

        let payload = r#"{"test": "data"}"#;
        let result = service.sign_payload(payload, &SecretString::new("test_secret"));

        // Verify the signature is generated successfully
        assert!(result.is_ok());

        // Verify it's a valid base64 string
        let signature = result.unwrap();
        assert!(STANDARD.decode(&signature).is_ok());

        // Verify deterministic behavior (same input produces same output)
        let second_result = service
            .sign_payload(payload, &SecretString::new("test_secret"))
            .unwrap();
        assert_eq!(signature, second_result);
    }
}
