use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct ApiKeyRequest {
    pub name: String,
    pub permissions: Vec<String>,
    pub allowed_origins: Option<Vec<String>>,
}
