use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct ApiKeyRequest {
    pub name: String,
    pub permissions: Vec<String>,
    pub allowed_origins: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ApiKeyModel {
    pub id: String,
    pub value: String,
    pub name: String,
    pub allowed_origins: Vec<String>,
    pub created_at: String,
    pub permissions: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ApiKeyResponse {
    pub id: String,
    pub name: String,
    pub allowed_origins: Vec<String>,
    pub created_at: String,
    pub permissions: Vec<String>,
}
