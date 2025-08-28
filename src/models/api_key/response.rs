use crate::models::{ApiError, ApiKeyRepoModel};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ApiKeyResponse {
    pub id: String,
    pub name: String,
    pub allowed_origins: Vec<String>,
    pub created_at: String,
    pub permissions: Vec<String>,
}

impl TryFrom<ApiKeyRepoModel> for ApiKeyResponse {
    type Error = ApiError;

    fn try_from(api_key: ApiKeyRepoModel) -> Result<Self, ApiError> {
        Ok(ApiKeyResponse {
            id: api_key.id,
            name: api_key.name,
            allowed_origins: api_key.allowed_origins,
            created_at: api_key.created_at,
            permissions: api_key.permissions,
        })
    }
}
