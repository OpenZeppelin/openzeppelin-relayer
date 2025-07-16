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
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PluginCallRequest {
    /// Plugin parameters
    pub params: serde_json::Value,
}
