//! Error types for configuration system.
//!
//! This module defines all possible error types used in the configuration system.
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ConfigFileError {
    #[error("Invalid ID length: {0}")]
    InvalidIdLength(String),
    #[error("Invalid ID format: {0}")]
    InvalidIdFormat(String),
    #[error("Missing required field: {0}")]
    MissingField(String),
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),
    #[error("Duplicate id error: {0}")]
    DuplicateId(String),
    #[error("Invalid network type: {0}")]
    InvalidNetworkType(String),
    #[error("Invalid network name for {network_type}: {name}")]
    InvalidNetwork { network_type: String, name: String },
    #[error("Invalid policy: {0}")]
    InvalidPolicy(String),
    #[error("Internal error: {0}")]
    InternalError(String),
    #[error("Missing env var: {0}")]
    MissingEnvVar(String),
    #[error("Invalid format: {0}")]
    InvalidFormat(String),
    #[error("File not found: {0}")]
    FileNotFound(String),
    #[error("Invalid reference: {0}")]
    InvalidReference(String),
    #[error("File read error: {0}")]
    FileRead(String),
    #[error("Test Signer error: {0}")]
    TestSigner(String),
    #[error("Incompatible inheritance type: {0}")]
    IncompatibleInheritanceType(String),
    #[error("Circular inheritance detected: {0}")]
    CircularInheritance(String),
    #[error("Maximum inheritance depth exceeded: {0}")]
    MaxInheritanceDepthExceeded(String),
    #[error("Invalid operation: {0}")]
    InvalidOperation(String),
    #[error("Invalid timeout: {0}")]
    InvalidTimeout(u64),
}
