//! Core signer domain model and business logic.
//!
//! This module provides the central `Signer` type that represents signers
//! throughout the relayer system, including:
//!
//! - **Domain Model**: Core `Signer` struct with validation and configuration
//! - **Business Logic**: Update operations and validation rules  
//! - **Error Handling**: Comprehensive validation error types
//! - **Interoperability**: Conversions between API, config, and repository representations
//!
//! The signer model supports multiple signer types including local keys, AWS KMS,
//! Google Cloud KMS, Vault, and Turnkey service integrations.

mod repository;
pub use repository::{
    AwsKmsSignerConfigStorage, GoogleCloudKmsSignerConfigStorage,
    GoogleCloudKmsSignerKeyConfigStorage, GoogleCloudKmsSignerServiceAccountConfigStorage,
    LocalSignerConfigStorage, SignerConfigStorage, SignerRepoModel, TurnkeySignerConfigStorage,
    VaultSignerConfigStorage, VaultTransitSignerConfigStorage,
};

mod config;
pub use config::*;

mod request;
pub use request::*;

mod response;
pub use response::*;

use crate::{constants::ID_REGEX, models::SecretString};
use secrets::SecretVec;
use serde::{Deserialize, Serialize, Serializer};
use utoipa::ToSchema;
use validator::Validate;

/// Helper function to serialize secrets as redacted
fn serialize_secret_redacted<S>(_secret: &SecretVec<u8>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str("[REDACTED]")
}

/// Local signer configuration for storing private keys
#[derive(Debug, Clone, Serialize)]
pub struct LocalSignerConfig {
    #[serde(serialize_with = "serialize_secret_redacted")]
    pub raw_key: SecretVec<u8>,
}

impl LocalSignerConfig {
    /// Validates the raw key for cryptographic requirements
    pub fn validate(&self) -> Result<(), SignerValidationError> {
        let key_bytes = self.raw_key.borrow();

        // Check key length - must be exactly 32 bytes for crypto operations
        if key_bytes.len() != 32 {
            return Err(SignerValidationError::InvalidConfig(format!(
                "Raw key must be exactly 32 bytes, got {} bytes",
                key_bytes.len()
            )));
        }

        // Check if key is all zeros (cryptographically invalid)
        if key_bytes.iter().all(|&b| b == 0) {
            return Err(SignerValidationError::InvalidConfig(
                "Raw key cannot be all zeros".to_string(),
            ));
        }

        Ok(())
    }
}

impl<'de> Deserialize<'de> for LocalSignerConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct LocalSignerConfigHelper {
            raw_key: String,
        }

        let helper = LocalSignerConfigHelper::deserialize(deserializer)?;
        let raw_key = if helper.raw_key == "[REDACTED]" {
            // Return a zero-filled SecretVec when deserializing redacted data
            SecretVec::zero(32)
        } else {
            // For actual data, assume it's the raw bytes represented as a string
            // In practice, this would come from proper key loading
            SecretVec::new(helper.raw_key.len(), |v| {
                v.copy_from_slice(helper.raw_key.as_bytes())
            })
        };

        Ok(LocalSignerConfig { raw_key })
    }
}

/// AWS KMS signer configuration
/// The configuration supports:
/// - AWS Region (aws_region) - important for region-specific key
/// - KMS Key identification (key_id)
///
/// The AWS authentication is carried out
/// through recommended credential providers as outlined in
/// https://docs.aws.amazon.com/sdk-for-rust/latest/dg/credproviders.html
/// Currently only EVM signing is supported since, as of June 2025,
/// AWS does not support ed25519 scheme
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct AwsKmsSignerConfig {
    #[validate(length(min = 1, message = "Region cannot be empty"))]
    pub region: Option<String>,
    #[validate(length(min = 1, message = "Key ID cannot be empty"))]
    pub key_id: String,
}

/// Vault signer configuration
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct VaultSignerConfig {
    #[validate(url(message = "Address must be a valid URL"))]
    pub address: String,
    pub namespace: Option<String>,
    #[validate(custom(
        function = "validate_secret_string",
        message = "Role ID cannot be empty"
    ))]
    pub role_id: SecretString,
    #[validate(custom(
        function = "validate_secret_string",
        message = "Secret ID cannot be empty"
    ))]
    pub secret_id: SecretString,
    #[validate(length(min = 1, message = "Vault key name cannot be empty"))]
    pub key_name: String,
    pub mount_point: Option<String>,
}

/// Vault Transit signer configuration
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct VaultTransitSignerConfig {
    #[validate(length(min = 1, message = "Key name cannot be empty"))]
    pub key_name: String,
    #[validate(url(message = "Address must be a valid URL"))]
    pub address: String,
    pub namespace: Option<String>,
    #[validate(custom(
        function = "validate_secret_string",
        message = "Role ID cannot be empty"
    ))]
    pub role_id: SecretString,
    #[validate(custom(
        function = "validate_secret_string",
        message = "Secret ID cannot be empty"
    ))]
    pub secret_id: SecretString,
    #[validate(length(min = 1, message = "pubkey cannot be empty"))]
    pub pubkey: String,
    pub mount_point: Option<String>,
}

/// Turnkey signer configuration
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct TurnkeySignerConfig {
    #[validate(length(min = 1, message = "API public key cannot be empty"))]
    pub api_public_key: String,
    #[validate(custom(
        function = "validate_secret_string",
        message = "API private key cannot be empty"
    ))]
    pub api_private_key: SecretString,
    #[validate(length(min = 1, message = "Organization ID cannot be empty"))]
    pub organization_id: String,
    #[validate(length(min = 1, message = "Private key ID cannot be empty"))]
    pub private_key_id: String,
    #[validate(length(min = 1, message = "Public key cannot be empty"))]
    pub public_key: String,
}

/// Google Cloud KMS service account configuration
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct GoogleCloudKmsSignerServiceAccountConfig {
    #[validate(custom(
        function = "validate_secret_string",
        message = "Private key cannot be empty"
    ))]
    pub private_key: SecretString,
    #[validate(custom(
        function = "validate_secret_string",
        message = "Private key ID cannot be empty"
    ))]
    pub private_key_id: SecretString,
    #[validate(length(min = 1, message = "Project ID cannot be empty"))]
    pub project_id: String,
    #[validate(custom(
        function = "validate_secret_string",
        message = "Client email cannot be empty"
    ))]
    pub client_email: SecretString,
    #[validate(length(min = 1, message = "Client ID cannot be empty"))]
    pub client_id: String,
    #[validate(url(message = "Auth URI must be a valid URL"))]
    pub auth_uri: String,
    #[validate(url(message = "Token URI must be a valid URL"))]
    pub token_uri: String,
    #[validate(url(message = "Auth provider x509 cert URL must be a valid URL"))]
    pub auth_provider_x509_cert_url: String,
    #[validate(url(message = "Client x509 cert URL must be a valid URL"))]
    pub client_x509_cert_url: String,
    pub universe_domain: String,
}

/// Google Cloud KMS key configuration
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct GoogleCloudKmsSignerKeyConfig {
    pub location: String,
    #[validate(length(min = 1, message = "Key ring ID cannot be empty"))]
    pub key_ring_id: String,
    #[validate(length(min = 1, message = "Key ID cannot be empty"))]
    pub key_id: String,
    pub key_version: u32,
}

/// Google Cloud KMS signer configuration
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct GoogleCloudKmsSignerConfig {
    #[validate(nested)]
    pub service_account: GoogleCloudKmsSignerServiceAccountConfig,
    #[validate(nested)]
    pub key: GoogleCloudKmsSignerKeyConfig,
}

/// Custom validator for SecretString
fn validate_secret_string(secret: &SecretString) -> Result<(), validator::ValidationError> {
    if secret.to_str().is_empty() {
        return Err(validator::ValidationError::new("empty_secret"));
    }
    Ok(())
}

/// Domain signer configuration enum containing all supported signer types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SignerConfig {
    Local(LocalSignerConfig),
    Vault(VaultSignerConfig),
    VaultTransit(VaultTransitSignerConfig),
    AwsKms(AwsKmsSignerConfig),
    Turnkey(TurnkeySignerConfig),
    GoogleCloudKms(GoogleCloudKmsSignerConfig),
}

impl SignerConfig {
    /// Validates the configuration using the appropriate validator
    pub fn validate(&self) -> Result<(), SignerValidationError> {
        match self {
            Self::Local(config) => config.validate(),
            Self::AwsKms(config) => Validate::validate(config).map_err(|e| {
                SignerValidationError::InvalidConfig(format!(
                    "AWS KMS validation failed: {}",
                    format_validation_errors(&e)
                ))
            }),
            Self::Vault(config) => Validate::validate(config).map_err(|e| {
                SignerValidationError::InvalidConfig(format!(
                    "Vault validation failed: {}",
                    format_validation_errors(&e)
                ))
            }),
            Self::VaultTransit(config) => Validate::validate(config).map_err(|e| {
                SignerValidationError::InvalidConfig(format!(
                    "Vault Transit validation failed: {}",
                    format_validation_errors(&e)
                ))
            }),
            Self::Turnkey(config) => Validate::validate(config).map_err(|e| {
                SignerValidationError::InvalidConfig(format!(
                    "Turnkey validation failed: {}",
                    format_validation_errors(&e)
                ))
            }),
            Self::GoogleCloudKms(config) => Validate::validate(config).map_err(|e| {
                SignerValidationError::InvalidConfig(format!(
                    "Google Cloud KMS validation failed: {}",
                    format_validation_errors(&e)
                ))
            }),
        }
    }

    /// Get local signer config if this is a local signer
    pub fn get_local(&self) -> Option<&LocalSignerConfig> {
        match self {
            Self::Local(config) => Some(config),
            _ => None,
        }
    }

    /// Get AWS KMS signer config if this is an AWS KMS signer
    pub fn get_aws_kms(&self) -> Option<&AwsKmsSignerConfig> {
        match self {
            Self::AwsKms(config) => Some(config),
            _ => None,
        }
    }

    /// Get Vault signer config if this is a Vault signer
    pub fn get_vault(&self) -> Option<&VaultSignerConfig> {
        match self {
            Self::Vault(config) => Some(config),
            _ => None,
        }
    }

    /// Get Vault Transit signer config if this is a Vault Transit signer
    pub fn get_vault_transit(&self) -> Option<&VaultTransitSignerConfig> {
        match self {
            Self::VaultTransit(config) => Some(config),
            _ => None,
        }
    }

    /// Get Turnkey signer config if this is a Turnkey signer
    pub fn get_turnkey(&self) -> Option<&TurnkeySignerConfig> {
        match self {
            Self::Turnkey(config) => Some(config),
            _ => None,
        }
    }

    /// Get Google Cloud KMS signer config if this is a Google Cloud KMS signer
    pub fn get_google_cloud_kms(&self) -> Option<&GoogleCloudKmsSignerConfig> {
        match self {
            Self::GoogleCloudKms(config) => Some(config),
            _ => None,
        }
    }

    /// Get the signer type from the configuration
    pub fn get_signer_type(&self) -> SignerType {
        match self {
            Self::Local(_) => SignerType::Local,
            Self::AwsKms(_) => SignerType::AwsKms,
            Self::Vault(_) => SignerType::Vault,
            Self::VaultTransit(_) => SignerType::VaultTransit,
            Self::Turnkey(_) => SignerType::Turnkey,
            Self::GoogleCloudKms(_) => SignerType::GoogleCloudKms,
        }
    }
}

/// Helper function to format validation errors
fn format_validation_errors(errors: &validator::ValidationErrors) -> String {
    let mut messages = Vec::new();

    for (field, field_errors) in errors.field_errors().iter() {
        let field_msgs: Vec<String> = field_errors
            .iter()
            .map(|error| error.message.clone().unwrap_or_default().to_string())
            .collect();
        messages.push(format!("{}: {}", field, field_msgs.join(", ")));
    }

    for (struct_field, kind) in errors.errors().iter() {
        if let validator::ValidationErrorsKind::Struct(nested) = kind {
            let nested_msgs = format_validation_errors(nested);
            messages.push(format!("{}.{}", struct_field, nested_msgs));
        }
    }

    messages.join("; ")
}

/// Core signer domain model containing both metadata and configuration
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct Signer {
    #[validate(
        length(min = 1, max = 36, message = "ID must be between 1 and 36 characters"),
        regex(
            path = "*ID_REGEX",
            message = "ID must contain only letters, numbers, dashes and underscores"
        )
    )]
    pub id: String,
    pub config: SignerConfig,
}

/// Signer type enum used for validation and API responses
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum SignerType {
    Local,
    #[serde(rename = "aws_kms")]
    AwsKms,
    #[serde(rename = "google_cloud_kms")]
    GoogleCloudKms,
    Vault,
    #[serde(rename = "vault_transit")]
    VaultTransit,
    Turnkey,
}

impl Signer {
    /// Creates a new signer with configuration
    pub fn new(id: String, config: SignerConfig) -> Self {
        Self { id, config }
    }

    /// Gets the signer type from the configuration
    pub fn signer_type(&self) -> SignerType {
        self.config.get_signer_type()
    }

    /// Validates the signer using both struct validation and config validation
    pub fn validate(&self) -> Result<(), SignerValidationError> {
        // First validate struct-level constraints (ID format, etc.)
        Validate::validate(self).map_err(|validation_errors| {
            // Convert validator errors to our custom error type
            // Return the first error for simplicity
            for (field, errors) in validation_errors.field_errors() {
                if let Some(error) = errors.first() {
                    let field_str = field.as_ref();
                    return match (field_str, error.code.as_ref()) {
                        ("id", "length") => SignerValidationError::InvalidIdFormat,
                        ("id", "regex") => SignerValidationError::InvalidIdFormat,
                        _ => SignerValidationError::InvalidIdFormat, // fallback
                    };
                }
            }
            // Fallback error
            SignerValidationError::InvalidIdFormat
        })?;

        // Then validate the configuration
        self.config.validate()?;

        Ok(())
    }
}

/// Validation errors for signers
#[derive(Debug, thiserror::Error)]
pub enum SignerValidationError {
    #[error("Signer ID cannot be empty")]
    EmptyId,
    #[error("Signer ID must contain only letters, numbers, dashes and underscores and must be at most 36 characters long")]
    InvalidIdFormat,
    #[error("Invalid signer configuration: {0}")]
    InvalidConfig(String),
}

/// Centralized conversion from SignerValidationError to ApiError
impl From<SignerValidationError> for crate::models::ApiError {
    fn from(error: SignerValidationError) -> Self {
        use crate::models::ApiError;

        ApiError::BadRequest(match error {
            SignerValidationError::EmptyId => "ID cannot be empty".to_string(),
            SignerValidationError::InvalidIdFormat => {
                "ID must contain only letters, numbers, dashes and underscores and must be at most 36 characters long".to_string()
            }
            SignerValidationError::InvalidConfig(msg) => format!("Invalid signer configuration: {}", msg),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_local_signer() {
        let config = SignerConfig::Local(LocalSignerConfig {
            raw_key: SecretVec::new(32, |v| v.fill(1)),
        });

        let signer = Signer::new("valid-id".to_string(), config);

        assert!(signer.validate().is_ok());
        assert_eq!(signer.signer_type(), SignerType::Local);
    }

    #[test]
    fn test_valid_aws_kms_signer() {
        let config = SignerConfig::AwsKms(AwsKmsSignerConfig {
            region: Some("us-east-1".to_string()),
            key_id: "test-key-id".to_string(),
        });

        let signer = Signer::new("aws-signer".to_string(), config);

        assert!(signer.validate().is_ok());
        assert_eq!(signer.signer_type(), SignerType::AwsKms);
    }

    #[test]
    fn test_empty_id() {
        let config = SignerConfig::Local(LocalSignerConfig {
            raw_key: SecretVec::new(32, |v| v.fill(1)), // Use valid non-zero key
        });

        let signer = Signer::new("".to_string(), config);

        assert!(matches!(
            signer.validate(),
            Err(SignerValidationError::InvalidIdFormat)
        ));
    }

    #[test]
    fn test_id_too_long() {
        let config = SignerConfig::Local(LocalSignerConfig {
            raw_key: SecretVec::new(32, |v| v.fill(1)), // Use valid non-zero key
        });

        let signer = Signer::new("a".repeat(37), config);

        assert!(matches!(
            signer.validate(),
            Err(SignerValidationError::InvalidIdFormat)
        ));
    }

    #[test]
    fn test_invalid_id_format() {
        let config = SignerConfig::Local(LocalSignerConfig {
            raw_key: SecretVec::new(32, |v| v.fill(1)), // Use valid non-zero key
        });

        let signer = Signer::new("invalid@id".to_string(), config);

        assert!(matches!(
            signer.validate(),
            Err(SignerValidationError::InvalidIdFormat)
        ));
    }

    #[test]
    fn test_local_signer_invalid_key_length() {
        let config = SignerConfig::Local(LocalSignerConfig {
            raw_key: SecretVec::new(16, |v| v.fill(1)), // Invalid length: 16 bytes instead of 32
        });

        let signer = Signer::new("valid-id".to_string(), config);

        let result = signer.validate();
        assert!(result.is_err());
        if let Err(SignerValidationError::InvalidConfig(msg)) = result {
            assert!(msg.contains("Raw key must be exactly 32 bytes"));
            assert!(msg.contains("got 16 bytes"));
        } else {
            panic!("Expected InvalidConfig error for invalid key length");
        }
    }

    #[test]
    fn test_local_signer_all_zero_key() {
        let config = SignerConfig::Local(LocalSignerConfig {
            raw_key: SecretVec::new(32, |v| v.fill(0)), // Invalid: all zeros
        });

        let signer = Signer::new("valid-id".to_string(), config);

        let result = signer.validate();
        assert!(result.is_err());
        if let Err(SignerValidationError::InvalidConfig(msg)) = result {
            assert_eq!(msg, "Raw key cannot be all zeros");
        } else {
            panic!("Expected InvalidConfig error for all-zero key");
        }
    }

    #[test]
    fn test_local_signer_valid_key() {
        let config = SignerConfig::Local(LocalSignerConfig {
            raw_key: SecretVec::new(32, |v| v.fill(1)), // Valid: 32 bytes, non-zero
        });

        let signer = Signer::new("valid-id".to_string(), config);

        assert!(signer.validate().is_ok());
    }

    #[test]
    fn test_signer_type_serialization() {
        use serde_json::{from_str, to_string};

        assert_eq!(to_string(&SignerType::Local).unwrap(), "\"local\"");
        assert_eq!(to_string(&SignerType::AwsKms).unwrap(), "\"aws_kms\"");
        assert_eq!(
            to_string(&SignerType::GoogleCloudKms).unwrap(),
            "\"google_cloud_kms\""
        );
        assert_eq!(
            to_string(&SignerType::VaultTransit).unwrap(),
            "\"vault_transit\""
        );

        assert_eq!(
            from_str::<SignerType>("\"local\"").unwrap(),
            SignerType::Local
        );
        assert_eq!(
            from_str::<SignerType>("\"aws_kms\"").unwrap(),
            SignerType::AwsKms
        );
    }

    #[test]
    fn test_config_accessor_methods() {
        // Test Local config accessor
        let local_config = LocalSignerConfig {
            raw_key: SecretVec::new(32, |v| v.fill(1)),
        };
        let config = SignerConfig::Local(local_config);
        assert!(config.get_local().is_some());
        assert!(config.get_aws_kms().is_none());

        // Test AWS KMS config accessor
        let aws_config = AwsKmsSignerConfig {
            region: Some("us-east-1".to_string()),
            key_id: "test-key".to_string(),
        };
        let config = SignerConfig::AwsKms(aws_config);
        assert!(config.get_aws_kms().is_some());
        assert!(config.get_local().is_none());
    }

    #[test]
    fn test_error_conversion_to_api_error() {
        let error = SignerValidationError::InvalidIdFormat;
        let api_error: crate::models::ApiError = error.into();

        if let crate::models::ApiError::BadRequest(msg) = api_error {
            assert!(msg.contains("ID must contain only letters, numbers, dashes and underscores"));
        } else {
            panic!("Expected BadRequest error");
        }
    }

    #[test]
    fn test_valid_vault_signer() {
        let config = SignerConfig::Vault(VaultSignerConfig {
            address: "https://vault.example.com".to_string(),
            namespace: Some("test".to_string()),
            role_id: SecretString::new("role-id"),
            secret_id: SecretString::new("secret-id"),
            key_name: "test-key".to_string(),
            mount_point: None,
        });

        let signer = Signer::new("vault-signer".to_string(), config);
        assert!(signer.validate().is_ok());
        assert_eq!(signer.signer_type(), SignerType::Vault);
    }

    #[test]
    fn test_invalid_vault_signer_url() {
        let config = SignerConfig::Vault(VaultSignerConfig {
            address: "not-a-url".to_string(),
            namespace: Some("test".to_string()),
            role_id: SecretString::new("role-id"),
            secret_id: SecretString::new("secret-id"),
            key_name: "test-key".to_string(),
            mount_point: None,
        });

        let signer = Signer::new("vault-signer".to_string(), config);
        let result = signer.validate();
        assert!(result.is_err());
        if let Err(SignerValidationError::InvalidConfig(msg)) = result {
            assert!(msg.contains("Address must be a valid URL"));
        } else {
            panic!("Expected InvalidConfig error for invalid URL");
        }
    }

    #[test]
    fn test_valid_google_cloud_kms_signer() {
        let config = SignerConfig::GoogleCloudKms(GoogleCloudKmsSignerConfig {
            service_account: GoogleCloudKmsSignerServiceAccountConfig {
                private_key: SecretString::new("private-key"),
                private_key_id: SecretString::new("key-id"),
                project_id: "project".to_string(),
                client_email: SecretString::new("client@example.com"),
                client_id: "client-id".to_string(),
                auth_uri: "https://accounts.google.com/o/oauth2/auth".to_string(),
                token_uri: "https://oauth2.googleapis.com/token".to_string(),
                auth_provider_x509_cert_url: "https://www.googleapis.com/oauth2/v1/certs"
                    .to_string(),
                client_x509_cert_url: "https://www.googleapis.com/robot/v1/metadata/x509/test"
                    .to_string(),
                universe_domain: "googleapis.com".to_string(),
            },
            key: GoogleCloudKmsSignerKeyConfig {
                location: "us-central1".to_string(),
                key_ring_id: "test-ring".to_string(),
                key_id: "test-key".to_string(),
                key_version: 1,
            },
        });

        let signer = Signer::new("gcp-kms-signer".to_string(), config);
        assert!(signer.validate().is_ok());
        assert_eq!(signer.signer_type(), SignerType::GoogleCloudKms);
    }

    #[test]
    fn test_invalid_google_cloud_kms_urls() {
        let config = SignerConfig::GoogleCloudKms(GoogleCloudKmsSignerConfig {
            service_account: GoogleCloudKmsSignerServiceAccountConfig {
                private_key: SecretString::new("private-key"),
                private_key_id: SecretString::new("key-id"),
                project_id: "project".to_string(),
                client_email: SecretString::new("client@example.com"),
                client_id: "client-id".to_string(),
                auth_uri: "not-a-url".to_string(), // Invalid URL
                token_uri: "https://oauth2.googleapis.com/token".to_string(),
                auth_provider_x509_cert_url: "https://www.googleapis.com/oauth2/v1/certs"
                    .to_string(),
                client_x509_cert_url: "https://www.googleapis.com/robot/v1/metadata/x509/test"
                    .to_string(),
                universe_domain: "googleapis.com".to_string(),
            },
            key: GoogleCloudKmsSignerKeyConfig {
                location: "us-central1".to_string(),
                key_ring_id: "test-ring".to_string(),
                key_id: "test-key".to_string(),
                key_version: 1,
            },
        });

        let signer = Signer::new("gcp-kms-signer".to_string(), config);
        let result = signer.validate();
        assert!(result.is_err());
        if let Err(SignerValidationError::InvalidConfig(msg)) = result {
            assert!(msg.contains("Auth URI must be a valid URL"));
        } else {
            panic!("Expected InvalidConfig error for invalid URL");
        }
    }

    #[test]
    fn test_secret_string_validation() {
        // Test empty secret
        let result = validate_secret_string(&SecretString::new(""));
        if let Err(e) = result {
            assert_eq!(e.code, "empty_secret");
        } else {
            panic!("Expected validation error for empty secret");
        }

        // Test valid secret
        let result = validate_secret_string(&SecretString::new("secret"));
        assert!(result.is_ok());
    }

    #[test]
    fn test_validation_error_formatting() {
        // Create an invalid config to trigger multiple nested validation errors
        let invalid_config = GoogleCloudKmsSignerConfig {
            service_account: GoogleCloudKmsSignerServiceAccountConfig {
                private_key: SecretString::new(""), // Invalid: empty
                private_key_id: SecretString::new("key-id"),
                project_id: "project".to_string(),
                client_email: SecretString::new("client@example.com"),
                client_id: "".to_string(),         // Invalid: empty
                auth_uri: "not-a-url".to_string(), // Invalid: not a URL
                token_uri: "https://oauth2.googleapis.com/token".to_string(),
                auth_provider_x509_cert_url: "https://www.googleapis.com/oauth2/v1/certs"
                    .to_string(),
                client_x509_cert_url: "https://www.googleapis.com/robot/v1/metadata/x509/test"
                    .to_string(),
                universe_domain: "googleapis.com".to_string(),
            },
            key: GoogleCloudKmsSignerKeyConfig {
                location: "us-central1".to_string(),
                key_ring_id: "".to_string(), // Invalid: empty
                key_id: "test-key".to_string(),
                key_version: 1,
            },
        };

        let errors = invalid_config.validate().unwrap_err();

        // Format the errors using the helper function
        let formatted = format_validation_errors(&errors);

        println!("formatted: {}", formatted);

        // Check that messages from nested fields are correctly formatted
        assert!(formatted.contains("client_id: Client ID cannot be empty"));
        assert!(formatted.contains("private_key: Private key cannot be empty"));
        assert!(formatted.contains("auth_uri: Auth URI must be a valid URL"));
        assert!(formatted.contains("key_ring_id: Key ring ID cannot be empty"));
    }

    #[test]
    fn test_config_type_getters() {
        // Test Vault config getter
        let vault_config = VaultSignerConfig {
            address: "https://vault.example.com".to_string(),
            namespace: None,
            role_id: SecretString::new("role"),
            secret_id: SecretString::new("secret"),
            key_name: "key".to_string(),
            mount_point: None,
        };
        let config = SignerConfig::Vault(vault_config);
        assert!(config.get_vault().is_some());

        // Test VaultTransit config getter
        let vault_transit_config = VaultTransitSignerConfig {
            key_name: "key".to_string(),
            address: "https://vault.example.com".to_string(),
            namespace: None,
            role_id: SecretString::new("role"),
            secret_id: SecretString::new("secret"),
            pubkey: "pubkey".to_string(),
            mount_point: None,
        };
        let config = SignerConfig::VaultTransit(vault_transit_config);
        assert!(config.get_vault_transit().is_some());
        assert!(config.get_turnkey().is_none());

        // Test Turnkey config getter
        let turnkey_config = TurnkeySignerConfig {
            api_public_key: "public".to_string(),
            api_private_key: SecretString::new("private"),
            organization_id: "org".to_string(),
            private_key_id: "key-id".to_string(),
            public_key: "pubkey".to_string(),
        };
        let config = SignerConfig::Turnkey(turnkey_config);
        assert!(config.get_turnkey().is_some());
        assert!(config.get_google_cloud_kms().is_none());

        // Test Google Cloud KMS config getter
        let gcp_config = GoogleCloudKmsSignerConfig {
            service_account: GoogleCloudKmsSignerServiceAccountConfig {
                private_key: SecretString::new("private-key"),
                private_key_id: SecretString::new("key-id"),
                project_id: "project".to_string(),
                client_email: SecretString::new("client@example.com"),
                client_id: "client-id".to_string(),
                auth_uri: "https://accounts.google.com/o/oauth2/auth".to_string(),
                token_uri: "https://oauth2.googleapis.com/token".to_string(),
                auth_provider_x509_cert_url: "https://www.googleapis.com/oauth2/v1/certs"
                    .to_string(),
                client_x509_cert_url: "https://www.googleapis.com/robot/v1/metadata/x509/test"
                    .to_string(),
                universe_domain: "googleapis.com".to_string(),
            },
            key: GoogleCloudKmsSignerKeyConfig {
                location: "us-central1".to_string(),
                key_ring_id: "test-ring".to_string(),
                key_id: "test-key".to_string(),
                key_version: 1,
            },
        };
        let config = SignerConfig::GoogleCloudKms(gcp_config);
        assert!(config.get_google_cloud_kms().is_some());
        assert!(config.get_local().is_none());
    }
}
