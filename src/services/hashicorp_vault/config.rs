use secrecy::SecretBox;
use serde::{Deserialize, Serialize};

/// Configuration for HashiCorp Vault Transit Engine
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VaultConfig {
    /// Vault server address (e.g., "https://vault.example.com:8200")
    pub vault_addr: String,
    
    /// AppRole role_id for authentication
    pub role_id: String,
    
    /// AppRole secret_id for authentication
    pub secret_id: String,
    
    /// Name of the key in the Transit backend to use for signing
    pub key_name: String,
    
    /// Optional path to CA certificate for verifying Vault's TLS certificate
    pub ca_cert: Option<String>,
    
    /// Namespace (for Vault Enterprise)
    pub namespace: Option<String>,
}

impl VaultConfig {
    pub fn new(
        vault_addr: String,
        role_id: String,
        secret_id: String,
        key_name: String,
        ca_cert: Option<String>,
        namespace: Option<String>,
    ) -> Self {
        Self {
            vault_addr,
            role_id,
            secret_id: secret_id,
            key_name,
            ca_cert,
            namespace,
        }
    }
    
    pub fn validate(&self) -> Result<(), String> {
        if self.vault_addr.is_empty() {
            return Err("Vault address cannot be empty".to_string());
        }
        
        if self.role_id.is_empty() {
            return Err("AppRole role_id cannot be empty".to_string());
        }
        
        if self.secret_id.is_empty() {
            return Err("AppRole secret_id cannot be empty".to_string());
        }
        
        if self.key_name.is_empty() {
            return Err("Key name cannot be empty".to_string());
        }
        
        Ok(())
    }
}