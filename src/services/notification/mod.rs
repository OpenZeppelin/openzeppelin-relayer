//! This module provides the `WebhookNotificationService` for sending notifications via webhooks.
use crate::models::{WebhookNotification, WebhookResponse};
use base64::{engine::general_purpose::STANDARD, Engine};
use hmac::{Hmac, Mac};
use reqwest::Client;
use sha2::Sha256;
use thiserror::Error;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone)]
pub struct WebhookNotificationService {
    client: Client,
    webhook_url: String,
    secret_key: Option<String>,
}

impl WebhookNotificationService {
    pub fn new(webhook_url: String, secret_key: Option<String>) -> Self {
        Self {
            client: Client::new(),
            webhook_url,
            secret_key,
        }
    }

    fn sign_payload(
        &self,
        payload: &str,
        secret_key: &str,
    ) -> Result<String, WebhookNotificationError> {
        let mut mac = HmacSha256::new_from_slice(secret_key.as_bytes())
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
