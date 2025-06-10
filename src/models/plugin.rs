use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct PluginModel {
    /// Plugin ID
    pub id: String,
    /// Plugin path
    pub path: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PluginCallRequest {
    /// Plugin ID
    pub plugin_id: String,
    /// Plugin parameters
    pub params: serde_json::Value,
}
