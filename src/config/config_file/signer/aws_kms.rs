//! Configuration for Amazon AWS KMS signer
//!
//! This module provides configuration for using Amazon AWS KMS as a signing service.
//! AWS KMS allows you to manage cryptographic keys and perform signing operations
//! without exposing private keys directly to your application.
//!
//! The configuration supports:
//! - AWS Region (aws_region) - important for region-specific key
//! - KMS Key identification (key_id)
//!
//! The AWS authentication is carried out
//! through recommended credential providers as outlined in
//! https://docs.aws.amazon.com/sdk-for-rust/latest/dg/credproviders.html
//!
//! Currently only EVM signing is supported since, as of June 2025,
//! AWS does not support ed25519 scheme

use serde::{Deserialize, Serialize};
use validator::Validate;

use crate::config::{validate_with_validator, ConfigFileError, SignerConfigValidate};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Validate)]
#[serde(deny_unknown_fields)]
pub struct AwsKmsSignerFileConfig {
    pub region: Option<String>,
    #[validate(length(min = 1, message = "key_id cannot be empty"))]
    pub key_id: String,
}

impl SignerConfigValidate for AwsKmsSignerFileConfig {
    fn validate(&self) -> Result<(), ConfigFileError> {
        validate_with_validator(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aws_kms_signer_file_config_valid() {
        let config = AwsKmsSignerFileConfig {
            region: Some("us-east-1".to_string()),
            key_id: "test-key-id".to_string(),
        };

        assert!(
            Validate::validate(&config).is_ok(),
            "Config should pass basic validation"
        );
        assert!(
            SignerConfigValidate::validate(&config).is_ok(),
            "Config should pass signer config validation"
        );
    }

    #[test]
    fn test_aws_signer_file_config_empty_key_id() {
        let config = AwsKmsSignerFileConfig {
            region: Some("us-east-1".to_string()),
            key_id: "".to_string(),
        };

        let result = SignerConfigValidate::validate(&config);
        assert!(
            result.is_err(),
            "Config should not pass the signer config validation"
        );
        if let Err(e) = result {
            let error_message = format!("{:?}", e);
            assert!(error_message.contains("key_id"));
            assert!(error_message.contains("cannot be empty"), "{:?}", e);
        }
    }

    #[test]
    fn test_serde_deserialize() {
        let json = r#"
        {
            "region": "us-east-1",
            "key_id": "test-key-id"
        }
        "#;
        let config: AwsKmsSignerFileConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.region.unwrap().as_str(), "us-east-1");
        assert_eq!(config.key_id.as_str(), "test-key-id");
    }

    #[test]
    fn test_serde_unknown_field() {
        let json = r#"
        {
            "region": "us-east-1",
            "key_id": "test-key-id",
            "unknown_field": "should cause error"
        }
        "#;
        let result: Result<AwsKmsSignerFileConfig, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_serde_serialize_deserialize() {
        let config = AwsKmsSignerFileConfig {
            region: Some("us-east-1".to_string()),
            key_id: "test-key-id".to_string(),
        };
        let serialized = serde_json::to_string(&config).unwrap();
        let deserialized: AwsKmsSignerFileConfig = serde_json::from_str(&serialized).unwrap();
        assert_eq!(config.region, deserialized.region);
        assert_eq!(config.key_id, deserialized.key_id);
    }
}
