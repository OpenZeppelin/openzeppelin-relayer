use crate::models::PermissionGrant;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct ApiKeyRequest {
    pub name: String,
    pub permissions: Vec<PermissionGrant>,
}
