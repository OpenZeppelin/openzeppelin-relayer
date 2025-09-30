use std::time::Duration;

use serde::{Deserialize, Serialize};
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
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PluginCallRequest {
    /// Plugin parameters
    pub params: serde_json::Value,
}
