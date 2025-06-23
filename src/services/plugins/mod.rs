//! Plugins service module for handling plugins execution and interaction with relayer
use crate::models::PluginCallRequest;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[cfg(test)]
use mockall::automock;

#[derive(Debug, Serialize, Deserialize)]
pub struct PluginCallResponse {
    pub success: bool,
}

pub struct PluginService {}

#[async_trait]
#[cfg_attr(test, automock)]
pub trait PluginServiceTrait {
    fn new() -> Self;
    async fn call_plugin(
        &self,
        path: &str,
        plugin_call_request: PluginCallRequest,
    ) -> PluginCallResponse;
}

#[async_trait]
impl PluginServiceTrait for PluginService {
    fn new() -> Self {
        Self {}
    }
    async fn call_plugin(
        &self,
        _path: &str,
        _plugin_call_request: PluginCallRequest,
    ) -> PluginCallResponse {
        PluginCallResponse { success: true }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_call_plugin() {
        let plugin_service = PluginService::new();
        let result = plugin_service
            .call_plugin(
                "test-plugin",
                PluginCallRequest {
                    params: serde_json::Value::Null,
                },
            )
            .await;
        assert!(result.success);
    }
}
