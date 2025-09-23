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

impl ApiKeyRepoModel {
    pub fn new(
        name: String,
        value: SecretString,
        permissions: Vec<String>,
        allowed_origins: Vec<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            value,
            name,
            permissions,
            allowed_origins,
            created_at: Utc::now().to_string(),
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_key_repo_model_new() {
        let api_key_repo_model = ApiKeyRepoModel::new(
            "test-name".to_string(),
            SecretString::new("test-value"),
            vec!["relayer:all:execute".to_string()],
            vec!["*".to_string()],
        );
        assert_eq!(api_key_repo_model.name, "test-name");
        assert_eq!(api_key_repo_model.value, SecretString::new("test-value"));
        assert_eq!(
            api_key_repo_model.permissions,
            vec!["relayer:all:execute".to_string()]
        );
        assert_eq!(api_key_repo_model.allowed_origins, vec!["*".to_string()]);
    }

    #[test]
    fn test_api_key_repo_model_try_from() {
        let api_key_request = ApiKeyRequest {
            name: "test-name".to_string(),
            permissions: vec!["relayer:all:execute".to_string()],
            allowed_origins: Some(vec!["*".to_string()]),
        };
        let api_key_repo_model = ApiKeyRepoModel::try_from(api_key_request).unwrap();
        assert_eq!(api_key_repo_model.name, "test-name");
        assert_eq!(
            api_key_repo_model.permissions,
            vec!["relayer:all:execute".to_string()]
        );
        assert_eq!(api_key_repo_model.allowed_origins, vec!["*".to_string()]);
    }
}
