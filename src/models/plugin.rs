use std::{collections::HashMap, time::Duration};

use serde::{Deserialize, Serialize};
use serde_json::Map;
use utoipa::ToSchema;

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
    pub config: Option<Map<String, serde_json::Value>>,
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
