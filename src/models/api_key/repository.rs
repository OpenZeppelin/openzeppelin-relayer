use crate::models::SecretString;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ApiKeyRepoModel {
    pub id: String,
    pub value: SecretString,
    pub name: String,
    pub allowed_origins: Vec<String>,
    pub created_at: String,
    pub permissions: Vec<String>,
}
