use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct PluginModel {
    /// Plugin ID
    pub id: String,
    /// Plugin path
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginCallRequest {
    /// Plugin parameters
    pub params: serde_json::Value,
}
