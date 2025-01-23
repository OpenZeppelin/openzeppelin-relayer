use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SignerType {
    Local,
    AwsKms,
    Vault,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SignerPassphrase {
    Env { name: String },
    Plain { value: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct SignerRepoModel {
    pub id: String,
    pub signer_type: SignerType,
    pub path: Option<String>,
    pub passphrase: Option<SignerPassphrase>,
}
