use std::{collections::HashMap, time::Duration};

use serde::{Deserialize, Serialize};
use serde_json::Map;
use utoipa::ToSchema;

use crate::constants::DEFAULT_PLUGIN_TIMEOUT_SECONDS;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PluginModel {
    /// Plugin ID
    pub id: String,
    /// Plugin path
    pub path: String,
    /// Plugin timeout
    #[schema(value_type = u64)]
    pub timeout: Duration,
    /// Whether to include logs in the HTTP response
    #[serde(default)]
    pub emit_logs: bool,
    /// Whether to include traces in the HTTP response
    #[serde(default)]
    pub emit_traces: bool,
    /// Whether to return raw plugin response without ApiResponse wrapper
    #[serde(default)]
    pub raw_response: bool,
    /// Whether to allow GET requests to invoke plugin logic
    #[serde(default)]
    pub allow_get_invocation: bool,
    /// User-defined configuration accessible to the plugin (must be a JSON object)
    #[serde(default)]
    pub config: Option<Map<String, serde_json::Value>>,
    /// Whether to forward plugin logs into the relayer's tracing output
    #[serde(default)]
    pub forward_logs: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PluginCallRequest {
    /// Plugin parameters. If not provided, the entire request body will be used as params.
    #[serde(default)]
    pub params: serde_json::Value,
    /// HTTP headers from the incoming request (injected by the route handler)
    #[serde(default, skip_deserializing)]
    pub headers: Option<HashMap<String, Vec<String>>>,
    /// Wildcard route from the endpoint (e.g., "" for /call, "/verify" for /call/verify)
    #[serde(default, skip_deserializing)]
    pub route: Option<String>,
    /// HTTP method used for the request (e.g., "GET" or "POST")
    #[serde(default, skip_deserializing)]
    pub method: Option<String>,
    /// Query parameters from the request URL
    #[serde(default, skip_deserializing)]
    pub query: Option<HashMap<String, Vec<String>>>,
}

/// Request model for updating an existing plugin.
/// All fields are optional to allow partial updates.
/// Note: `id` and `path` are not updateable after creation.
#[derive(Debug, Clone, Serialize, Deserialize, Default, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdatePluginRequest {
    /// Plugin timeout in seconds
    #[schema(value_type = Option<u64>)]
    pub timeout: Option<u64>,
    /// Whether to include logs in the HTTP response
    pub emit_logs: Option<bool>,
    /// Whether to include traces in the HTTP response
    pub emit_traces: Option<bool>,
    /// Whether to return raw plugin response without ApiResponse wrapper
    pub raw_response: Option<bool>,
    /// Whether to allow GET requests to invoke plugin logic
    pub allow_get_invocation: Option<bool>,
    /// User-defined configuration accessible to the plugin (must be a JSON object)
    /// Use `null` to clear the config
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<Option<Map<String, serde_json::Value>>>,
    /// Whether to forward plugin logs into the relayer's tracing output
    pub forward_logs: Option<bool>,
}

/// Validation errors for plugin updates
#[derive(Debug, thiserror::Error)]
pub enum PluginValidationError {
    #[error("Invalid timeout: {0}")]
    InvalidTimeout(String),
}

impl PluginModel {
    /// Apply an update request to this plugin model.
    /// Returns the updated plugin model or a validation error.
    pub fn apply_update(&self, update: UpdatePluginRequest) -> Result<Self, PluginValidationError> {
        let mut updated = self.clone();

        if let Some(timeout_secs) = update.timeout {
            if timeout_secs == 0 {
                return Err(PluginValidationError::InvalidTimeout(
                    "Timeout must be greater than 0".to_string(),
                ));
            }
            updated.timeout = Duration::from_secs(timeout_secs);
        }

        if let Some(emit_logs) = update.emit_logs {
            updated.emit_logs = emit_logs;
        }

        if let Some(emit_traces) = update.emit_traces {
            updated.emit_traces = emit_traces;
        }

        if let Some(raw_response) = update.raw_response {
            updated.raw_response = raw_response;
        }

        if let Some(allow_get_invocation) = update.allow_get_invocation {
            updated.allow_get_invocation = allow_get_invocation;
        }

        // config uses Option<Option<...>> to distinguish between:
        // - None: field not provided, don't change
        // - Some(None): explicitly set to null, clear the config
        // - Some(Some(value)): update to new value
        if let Some(config) = update.config {
            updated.config = config;
        }

        if let Some(forward_logs) = update.forward_logs {
            updated.forward_logs = forward_logs;
        }

        Ok(updated)
    }
}

impl Default for PluginModel {
    fn default() -> Self {
        Self {
            id: String::new(),
            path: String::new(),
            timeout: Duration::from_secs(DEFAULT_PLUGIN_TIMEOUT_SECONDS),
            emit_logs: false,
            emit_traces: false,
            raw_response: false,
            allow_get_invocation: false,
            config: None,
            forward_logs: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_plugin() -> PluginModel {
        PluginModel {
            id: "test-plugin".to_string(),
            path: "plugins/test.ts".to_string(),
            timeout: Duration::from_secs(30),
            emit_logs: false,
            emit_traces: false,
            raw_response: false,
            allow_get_invocation: false,
            config: None,
            forward_logs: false,
        }
    }

    #[test]
    fn test_apply_update_timeout() {
        let plugin = create_test_plugin();
        let update = UpdatePluginRequest {
            timeout: Some(60),
            ..Default::default()
        };

        let updated = plugin.apply_update(update).unwrap();
        assert_eq!(updated.timeout, Duration::from_secs(60));
        // Other fields unchanged
        assert_eq!(updated.emit_logs, false);
    }

    #[test]
    fn test_apply_update_timeout_zero_fails() {
        let plugin = create_test_plugin();
        let update = UpdatePluginRequest {
            timeout: Some(0),
            ..Default::default()
        };

        let result = plugin.apply_update(update);
        assert!(result.is_err());
    }

    #[test]
    fn test_apply_update_all_fields() {
        let plugin = create_test_plugin();
        let mut config_map = Map::new();
        config_map.insert("key".to_string(), serde_json::json!("value"));

        let update = UpdatePluginRequest {
            timeout: Some(120),
            emit_logs: Some(true),
            emit_traces: Some(true),
            raw_response: Some(true),
            allow_get_invocation: Some(true),
            config: Some(Some(config_map.clone())),
            forward_logs: Some(true),
        };

        let updated = plugin.apply_update(update).unwrap();
        assert_eq!(updated.timeout, Duration::from_secs(120));
        assert!(updated.emit_logs);
        assert!(updated.emit_traces);
        assert!(updated.raw_response);
        assert!(updated.allow_get_invocation);
        assert_eq!(updated.config, Some(config_map));
        assert!(updated.forward_logs);
    }

    #[test]
    fn test_apply_update_clear_config() {
        let mut plugin = create_test_plugin();
        let mut config_map = Map::new();
        config_map.insert("key".to_string(), serde_json::json!("value"));
        plugin.config = Some(config_map);

        // Clear config by setting to null
        let update = UpdatePluginRequest {
            config: Some(None),
            ..Default::default()
        };

        let updated = plugin.apply_update(update).unwrap();
        assert!(updated.config.is_none());
    }

    #[test]
    fn test_apply_update_no_changes() {
        let plugin = create_test_plugin();
        let update = UpdatePluginRequest::default();

        let updated = plugin.apply_update(update).unwrap();
        assert_eq!(updated.id, plugin.id);
        assert_eq!(updated.path, plugin.path);
        assert_eq!(updated.timeout, plugin.timeout);
    }
}
