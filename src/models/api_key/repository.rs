use crate::{
    models::{ApiError, ApiKeyRequest, PermissionGrant, SecretString},
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
    pub created_at: String,
    pub permissions: Vec<PermissionGrant>,
}

impl ApiKeyRepoModel {
    pub fn new(name: String, value: SecretString, permissions: Vec<PermissionGrant>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            value,
            name,
            permissions,
            created_at: Utc::now().to_string(),
        }
    }
}

impl TryFrom<ApiKeyRequest> for ApiKeyRepoModel {
    type Error = ApiError;

    fn try_from(request: ApiKeyRequest) -> Result<Self, Self::Error> {
        Ok(Self {
            id: Uuid::new_v4().to_string(),
            value: SecretString::new(&Uuid::new_v4().to_string()),
            name: request.name,
            permissions: request.permissions,
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
            vec![PermissionGrant::global("relayers:execute")],
        );
        assert_eq!(api_key_repo_model.name, "test-name");
        assert_eq!(api_key_repo_model.value, SecretString::new("test-value"));
        assert_eq!(
            api_key_repo_model.permissions,
            vec![PermissionGrant::global("relayers:execute")]
        );
    }

    #[test]
    fn test_api_key_repo_model_try_from() {
        let api_key_request = ApiKeyRequest {
            name: "test-name".to_string(),
            permissions: vec![PermissionGrant::global("relayers:execute")],
        };
        let api_key_repo_model = ApiKeyRepoModel::try_from(api_key_request).unwrap();
        assert_eq!(api_key_repo_model.name, "test-name");
        assert_eq!(
            api_key_repo_model.permissions,
            vec![PermissionGrant::global("relayers:execute")]
        );
    }
}
