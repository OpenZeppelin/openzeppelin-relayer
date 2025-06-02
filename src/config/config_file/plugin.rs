use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PluginFileConfig {
    pub id: String,
    pub path: String,
}
