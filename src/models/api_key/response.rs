use crate::models::{ApiError, ApiKeyRepoModel, PermissionGrant};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ApiKeyListResponse {
    pub id: String,
    pub name: String,
    pub created_at: String,
    pub permissions: Vec<PermissionGrant>,
}

impl TryFrom<ApiKeyRepoModel> for ApiKeyListResponse {
    type Error = ApiError;

    fn try_from(api_key: ApiKeyRepoModel) -> Result<Self, ApiError> {
        Ok(ApiKeyListResponse {
            id: api_key.id,
            name: api_key.name,
            created_at: api_key.created_at,
            permissions: api_key.permissions,
        })
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ApiKeyCreateResponse {
    pub id: String,
    pub value: String,
    pub name: String,
    pub created_at: String,
    pub permissions: Vec<PermissionGrant>,
}

impl TryFrom<ApiKeyRepoModel> for ApiKeyCreateResponse {
    type Error = ApiError;

    fn try_from(api_key: ApiKeyRepoModel) -> Result<Self, ApiError> {
        Ok(ApiKeyCreateResponse {
            id: api_key.id,
            value: api_key.value.to_str().to_string(),
            name: api_key.name,
            created_at: api_key.created_at,
            permissions: api_key.permissions,
        })
    }
}
