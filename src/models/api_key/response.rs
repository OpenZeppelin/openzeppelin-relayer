use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ApiKeyResponse {
    pub id: String,
    pub name: String,
    pub allowed_origins: Vec<String>,
    pub created_at: String,
    pub permissions: Vec<String>,
}
