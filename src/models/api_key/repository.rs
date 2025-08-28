use crate::{
    models::{ApiError, ApiKeyRequest, SecretString},
    utils::{deserialize_secret_string, serialize_secret_string},
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ApiKeyRepoModel {
    pub id: String,
    #[serde(
        serialize_with = "serialize_secret_string",
        deserialize_with = "deserialize_secret_string"
    )]
    pub value: SecretString,
    pub name: String,
    pub allowed_origins: Vec<String>,
    pub created_at: String,
    pub permissions: Vec<String>,
}

impl TryFrom<ApiKeyRequest> for ApiKeyRepoModel {
    type Error = ApiError;

    fn try_from(request: ApiKeyRequest) -> Result<Self, Self::Error> {
        let allowed_origins = request.allowed_origins.unwrap_or(vec!["*".to_string()]);

        Ok(Self {
            id: Uuid::new_v4().to_string(),
            value: SecretString::new(&Uuid::new_v4().to_string()),
            name: request.name,
            permissions: request.permissions,
            allowed_origins,
            created_at: Utc::now().to_string(),
        })
    }
}
