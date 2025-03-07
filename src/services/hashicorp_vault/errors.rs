use thiserror::Error;

pub type VaultResult<T> = Result<T, VaultError>;

#[derive(Error, Debug)]
pub enum VaultError {
    #[error("Authentication error: {0}")]
    Authentication(String),
    
    #[error("Network error: {0}")]
    Network(String),
    
    #[error("Vault operation error: {0}")]
    Operation(String),
    
    #[error("Configuration error: {0}")]
    Configuration(String),
    
    #[error("Response parsing error: {0}")]
    ResponseParsing(String),
}