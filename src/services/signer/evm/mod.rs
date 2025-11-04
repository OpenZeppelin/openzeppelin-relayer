//! # EVM Signer Implementations
//!
//! This module provides various signer implementations for Ethereum Virtual Machine (EVM)
//! transactions and data signing, including support for EIP-712 typed data.
//!
//! ## Architecture
//!
//! ```text
//! EVM Signer (trait implementations)
//!   ├── LocalSigner             - Encrypted JSON keystore (development/testing)
//!   ├── AwsKmsSigner           - AWS Key Management Service
//!   ├── GoogleCloudKmsSigner   - Google Cloud Key Management Service
//!   ├── VaultSigner            - HashiCorp Vault KV2 backend
//!   ├── TurnkeySigner          - Turnkey API backend
//!   └── CdpSigner              - Coinbase Developer Platform
//! ```
//!
//! ## Features
//!
//! - **Transaction Signing**: Support for both legacy and EIP-1559 transactions
//! - **Data Signing**: EIP-191 personal message signing
//! - **Typed Data**: EIP-712 structured data signing
//! - **Multi-Backend**: Pluggable signer backends with consistent API
//!
//! ## Security
//!
//! All implementations follow Ethereum signing standards and include:
//! - Signature malleability protection (EIP-2 low-s normalization)
//! - Proper v-value handling for different transaction types
//! - Input validation and error handling

mod aws_kms_signer;
mod cdp_signer;
mod google_cloud_kms_signer;
mod local_signer;
mod turnkey_signer;
pub(crate) mod utils;
mod vault_signer;
use aws_kms_signer::*;
use cdp_signer::*;
use google_cloud_kms_signer::*;
use local_signer::*;
use oz_keystore::HashicorpCloudClient;
use turnkey_signer::*;
use vault_signer::*;

use async_trait::async_trait;
use std::sync::Arc;

use crate::{
    domain::{
        SignDataRequest, SignDataResponse, SignDataResponseEvm, SignTransactionResponse,
        SignTypedDataRequest,
    },
    models::{
        Address, NetworkTransactionData, Signer as SignerDomainModel, SignerConfig,
        SignerRepoModel, SignerType, TransactionRepoModel, VaultSignerConfig,
    },
    services::{
        signer::Signer,
        signer::SignerError,
        signer::SignerFactoryError,
        turnkey::TurnkeyService,
        vault::{VaultConfig, VaultService, VaultServiceTrait},
        AwsKmsService, CdpService, GoogleCloudKmsService, TurnkeyServiceTrait,
    },
};
use eyre::Result;

// EIP-712 and ECDSA Constants
const EIP712_PREFIX: [u8; 2] = [0x19, 0x01];
const EIP712_MESSAGE_SIZE: usize = 66; // 2 (prefix) + 32 (domain) + 32 (struct)

/// SECP256K1 signature length: 32 bytes (r) + 32 bytes (s) + 1 byte (v)
const SECP256K1_SIGNATURE_LENGTH: usize = 65;

/// Keccak256 hash output length
const HASH_LENGTH: usize = 32;

/// Validates and decodes a hex string
///
/// # Arguments
/// * `value` - The hex string to decode (may have optional "0x" prefix)
/// * `field_name` - Name of the field for error messages
///
/// # Returns
/// * `Ok(Vec<u8>)` - The decoded bytes
/// * `Err(SignerError)` - If the hex string is invalid
fn validate_and_decode_hex(value: &str, field_name: &str) -> Result<Vec<u8>, SignerError> {
    let hex_str = value.strip_prefix("0x").unwrap_or(value);

    // Check for invalid characters and report position
    if let Some((pos, ch)) = hex_str
        .chars()
        .enumerate()
        .find(|(_, c)| !c.is_ascii_hexdigit())
    {
        return Err(SignerError::SigningError(format!(
            "Invalid {} hex: non-hexadecimal character '{}' at position {} (input: {}...)",
            field_name,
            ch,
            pos,
            &hex_str[..hex_str.len().min(16)] // Show first 16 chars
        )));
    }

    // Decode the hex string
    hex::decode(hex_str).map_err(|e| {
        SignerError::SigningError(format!(
            "Invalid {} hex: failed to decode - {} (input: {}...)",
            field_name,
            e,
            &hex_str[..hex_str.len().min(16)]
        ))
    })
}

/// Constructs an EIP-712 message hash from domain separator and struct hash
///
/// This function implements the EIP-712 typed data signing specification:
/// <https://eips.ethereum.org/EIPS/eip-712>
///
/// # EIP-712 Format
///
/// The message hash is constructed as:
/// ```text
/// keccak256("\x19\x01" ‖ domainSeparator ‖ hashStruct(message))
/// ```
///
/// # Security Considerations
///
/// **CRITICAL SECURITY REQUIREMENTS:**
///
/// - **Domain Separator Uniqueness**: The domain separator MUST be unique to your dapp
///   to prevent cross-contract replay attacks. Include at minimum:
///   - Contract name
///   - Contract version
///   - Chain ID (prevents cross-chain replays)
///   - Verifying contract address
///
/// - **Hash Struct Integrity**: The hashStruct MUST uniquely identify your message type
///   and data. Collisions allow signature reuse across different messages.
///
/// - **Replay Protection**: Always include nonces or timestamps in your message struct
///   to prevent replay attacks within the same contract.
///
/// - **Chain ID**: MUST be included in domain separator to prevent signatures from
///   being valid on multiple chains (mainnet, testnet, etc.)
///
/// # Arguments
///
/// * `request` - The typed data signing request containing:
///   - `domain_separator`: 32-byte hash uniquely identifying the signing domain
///   - `hash_struct_message`: 32-byte hash of the structured message data
///
/// # Returns
///
/// * `Ok([u8; 32])` - The 32-byte message hash ready for signing
/// * `Err(SignerError)` - If validation fails or hex decoding fails
///
/// # Errors
///
/// Returns `SignerError::SigningError` if:
/// - Domain separator is not exactly 32 bytes
/// - Hash struct is not exactly 32 bytes
/// - Input contains invalid hexadecimal characters
///
/// # Examples
///
/// ```ignore
/// use crate::domain::SignTypedDataRequest;
/// use crate::services::signer::evm::construct_eip712_message_hash;
///
/// let request = SignTypedDataRequest {
///     // 32 bytes as hex (with or without 0x prefix)
///     domain_separator: "a".repeat(64),
///     hash_struct_message: "b".repeat(64),
/// };
///
/// let hash = construct_eip712_message_hash(&request)?;
/// // hash is now ready for signing
/// ```
pub fn construct_eip712_message_hash(
    request: &SignTypedDataRequest,
) -> Result<[u8; 32], SignerError> {
    // Decode and validate domain separator
    let domain_separator = validate_and_decode_hex(&request.domain_separator, "domain separator")?;

    // Decode and validate hash struct message
    let hash_struct = validate_and_decode_hex(&request.hash_struct_message, "hash struct message")?;

    // Validate lengths (both must be exactly 32 bytes)
    if domain_separator.len() != HASH_LENGTH {
        return Err(SignerError::SigningError(format!(
            "Invalid domain separator length: expected {} bytes, got {}",
            HASH_LENGTH,
            domain_separator.len()
        )));
    }
    if hash_struct.len() != HASH_LENGTH {
        return Err(SignerError::SigningError(format!(
            "Invalid hash struct length: expected {} bytes, got {}",
            HASH_LENGTH,
            hash_struct.len()
        )));
    }

    // Construct EIP-712 message: "\x19\x01" ++ domainSeparator ++ hashStruct(message)
    // Use fixed-size array instead of Vec for better performance (size known at compile time)
    let mut eip712_message = [0u8; EIP712_MESSAGE_SIZE];
    eip712_message[0..2].copy_from_slice(&EIP712_PREFIX);
    eip712_message[2..34].copy_from_slice(&domain_separator);
    eip712_message[34..66].copy_from_slice(&hash_struct);

    // Hash the EIP-712 message
    use alloy::primitives::keccak256;
    let message_hash = keccak256(eip712_message);

    Ok(message_hash.into())
}

/// Validates signature length and formats it into a SignDataResponse::Evm
///
/// This helper eliminates duplication across all EVM signer implementations by providing
/// a single point for signature validation and formatting.
///
/// # Arguments
/// * `signature_bytes` - The raw signature bytes (expected to be 65 bytes: r + s + v)
/// * `signer_name` - Name of the signer for error messages (e.g., "AWS KMS", "Turnkey")
///
/// # Returns
/// * `Ok(SignDataResponse)` - A properly formatted EVM signature response
/// * `Err(SignerError)` - If the signature length is not 65 bytes
///
/// # Format
/// The signature is split into:
/// - `r`: First 32 bytes (hex encoded)
/// - `s`: Next 32 bytes (hex encoded)
/// - `v`: Last byte (recovery ID, typically 27 or 28)
/// - `sig`: Full 65 bytes (hex encoded)
pub(crate) fn validate_and_format_signature(
    signature_bytes: &[u8],
    signer_name: &str,
) -> Result<SignDataResponse, SignerError> {
    if signature_bytes.len() != SECP256K1_SIGNATURE_LENGTH {
        return Err(SignerError::SigningError(format!(
            "Invalid signature length from {}: expected {} bytes, got {}",
            signer_name,
            SECP256K1_SIGNATURE_LENGTH,
            signature_bytes.len()
        )));
    }

    let r = hex::encode(&signature_bytes[0..32]);
    let s = hex::encode(&signature_bytes[32..64]);
    let v = signature_bytes[64];

    Ok(SignDataResponse::Evm(SignDataResponseEvm {
        r,
        s,
        v,
        sig: hex::encode(signature_bytes),
    }))
}

#[async_trait]
pub trait DataSignerTrait: Send + Sync {
    /// Signs arbitrary message data
    async fn sign_data(&self, request: SignDataRequest) -> Result<SignDataResponse, SignerError>;

    /// Signs EIP-712 typed data
    async fn sign_typed_data(
        &self,
        request: SignTypedDataRequest,
    ) -> Result<SignDataResponse, SignerError>;
}

pub enum EvmSigner {
    Local(LocalSigner),
    Vault(VaultSigner<VaultService>),
    Turnkey(TurnkeySigner),
    Cdp(CdpSigner),
    AwsKms(AwsKmsSigner),
    GoogleCloudKms(GoogleCloudKmsSigner),
}

#[async_trait]
impl Signer for EvmSigner {
    async fn address(&self) -> Result<Address, SignerError> {
        match self {
            Self::Local(signer) => signer.address().await,
            Self::Vault(signer) => signer.address().await,
            Self::Turnkey(signer) => signer.address().await,
            Self::Cdp(signer) => signer.address().await,
            Self::AwsKms(signer) => signer.address().await,
            Self::GoogleCloudKms(signer) => signer.address().await,
        }
    }

    async fn sign_transaction(
        &self,
        transaction: NetworkTransactionData,
    ) -> Result<SignTransactionResponse, SignerError> {
        match self {
            Self::Local(signer) => signer.sign_transaction(transaction).await,
            Self::Vault(signer) => signer.sign_transaction(transaction).await,
            Self::Turnkey(signer) => signer.sign_transaction(transaction).await,
            Self::Cdp(signer) => signer.sign_transaction(transaction).await,
            Self::AwsKms(signer) => signer.sign_transaction(transaction).await,
            Self::GoogleCloudKms(signer) => signer.sign_transaction(transaction).await,
        }
    }
}

#[async_trait]
impl DataSignerTrait for EvmSigner {
    async fn sign_data(&self, request: SignDataRequest) -> Result<SignDataResponse, SignerError> {
        match self {
            Self::Local(signer) => signer.sign_data(request).await,
            Self::Vault(signer) => signer.sign_data(request).await,
            Self::Turnkey(signer) => signer.sign_data(request).await,
            Self::Cdp(signer) => signer.sign_data(request).await,
            Self::AwsKms(signer) => signer.sign_data(request).await,
            Self::GoogleCloudKms(signer) => signer.sign_data(request).await,
        }
    }

    async fn sign_typed_data(
        &self,
        request: SignTypedDataRequest,
    ) -> Result<SignDataResponse, SignerError> {
        match self {
            Self::Local(signer) => signer.sign_typed_data(request).await,
            Self::Vault(signer) => signer.sign_typed_data(request).await,
            Self::Turnkey(signer) => signer.sign_typed_data(request).await,
            Self::Cdp(signer) => signer.sign_typed_data(request).await,
            Self::AwsKms(signer) => signer.sign_typed_data(request).await,
            Self::GoogleCloudKms(signer) => signer.sign_typed_data(request).await,
        }
    }
}

pub struct EvmSignerFactory;

impl EvmSignerFactory {
    pub async fn create_evm_signer(
        signer_model: SignerDomainModel,
    ) -> Result<EvmSigner, SignerFactoryError> {
        let signer = match &signer_model.config {
            SignerConfig::Local(_) => EvmSigner::Local(LocalSigner::new(&signer_model)?),
            SignerConfig::Vault(config) => {
                let vault_config = VaultConfig::new(
                    config.address.clone(),
                    config.role_id.clone(),
                    config.secret_id.clone(),
                    config.namespace.clone(),
                    config
                        .mount_point
                        .clone()
                        .unwrap_or_else(|| "secret".to_string()),
                    None,
                );
                let vault_service = VaultService::new(vault_config);

                EvmSigner::Vault(VaultSigner::new(
                    signer_model.id.clone(),
                    config.clone(),
                    vault_service,
                ))
            }
            SignerConfig::AwsKms(config) => {
                let aws_service = AwsKmsService::new(config.clone()).await.map_err(|e| {
                    SignerFactoryError::CreationFailed(format!("AWS KMS service error: {}", e))
                })?;
                EvmSigner::AwsKms(AwsKmsSigner::new(aws_service))
            }
            SignerConfig::VaultTransit(_) => {
                return Err(SignerFactoryError::UnsupportedType("Vault Transit".into()));
            }
            SignerConfig::Turnkey(config) => {
                let turnkey_service = TurnkeyService::new(config.clone()).map_err(|e| {
                    SignerFactoryError::CreationFailed(format!("Turnkey service error: {}", e))
                })?;
                EvmSigner::Turnkey(TurnkeySigner::new(turnkey_service))
            }
            SignerConfig::Cdp(config) => {
                let cdp_signer = CdpSigner::new(config.clone()).map_err(|e| {
                    SignerFactoryError::CreationFailed(format!("CDP service error: {}", e))
                })?;
                EvmSigner::Cdp(cdp_signer)
            }
            SignerConfig::GoogleCloudKms(config) => {
                let gcp_service = GoogleCloudKmsService::new(config).map_err(|e| {
                    SignerFactoryError::CreationFailed(format!(
                        "Google Cloud KMS service error: {}",
                        e
                    ))
                })?;
                EvmSigner::GoogleCloudKms(GoogleCloudKmsSigner::new(gcp_service))
            }
        };

        Ok(signer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        AwsKmsSignerConfig, CdpSignerConfig, EvmTransactionData, GoogleCloudKmsSignerConfig,
        GoogleCloudKmsSignerKeyConfig, GoogleCloudKmsSignerServiceAccountConfig, LocalSignerConfig,
        SecretString, SignerConfig, SignerRepoModel, TurnkeySignerConfig, VaultTransitSignerConfig,
        U256,
    };
    use futures;
    use mockall::predicate::*;
    use secrets::SecretVec;
    use std::str::FromStr;
    use std::sync::Arc;

    fn test_key_bytes() -> SecretVec<u8> {
        let key_bytes =
            hex::decode("0000000000000000000000000000000000000000000000000000000000000001")
                .unwrap();
        SecretVec::new(key_bytes.len(), |v| v.copy_from_slice(&key_bytes))
    }

    fn test_key_address() -> Address {
        Address::Evm([
            126, 95, 69, 82, 9, 26, 105, 18, 93, 93, 252, 183, 184, 194, 101, 144, 41, 57, 91, 223,
        ])
    }

    #[tokio::test]
    async fn test_create_evm_signer_local() {
        let signer_model = SignerDomainModel {
            id: "test".to_string(),
            config: SignerConfig::Local(LocalSignerConfig {
                raw_key: test_key_bytes(),
            }),
        };

        let signer = EvmSignerFactory::create_evm_signer(signer_model)
            .await
            .unwrap();

        assert!(matches!(signer, EvmSigner::Local(_)));
    }

    #[tokio::test]
    async fn test_create_evm_signer_test() {
        let signer_model = SignerDomainModel {
            id: "test".to_string(),
            config: SignerConfig::Local(LocalSignerConfig {
                raw_key: test_key_bytes(),
            }),
        };

        let signer = EvmSignerFactory::create_evm_signer(signer_model)
            .await
            .unwrap();

        assert!(matches!(signer, EvmSigner::Local(_)));
    }

    #[tokio::test]
    async fn test_create_evm_signer_vault() {
        let signer_model = SignerDomainModel {
            id: "test".to_string(),
            config: SignerConfig::Vault(VaultSignerConfig {
                address: "https://vault.test.com".to_string(),
                namespace: Some("test-namespace".to_string()),
                role_id: crate::models::SecretString::new("test-role-id"),
                secret_id: crate::models::SecretString::new("test-secret-id"),
                key_name: "test-key".to_string(),
                mount_point: Some("secret".to_string()),
            }),
        };

        let signer = EvmSignerFactory::create_evm_signer(signer_model)
            .await
            .unwrap();

        assert!(matches!(signer, EvmSigner::Vault(_)));
    }

    #[tokio::test]
    async fn test_create_evm_signer_aws_kms() {
        let signer_model = SignerDomainModel {
            id: "test".to_string(),
            config: SignerConfig::AwsKms(AwsKmsSignerConfig {
                region: Some("us-east-1".to_string()),
                key_id: "test-key-id".to_string(),
            }),
        };

        let signer = EvmSignerFactory::create_evm_signer(signer_model)
            .await
            .unwrap();

        assert!(matches!(signer, EvmSigner::AwsKms(_)));
    }

    #[tokio::test]
    async fn test_create_evm_signer_vault_transit() {
        let signer_model = SignerDomainModel {
            id: "test".to_string(),
            config: SignerConfig::VaultTransit(VaultTransitSignerConfig {
                key_name: "test".to_string(),
                address: "address".to_string(),
                namespace: None,
                role_id: SecretString::new("test-role"),
                secret_id: SecretString::new("test-secret"),
                pubkey: "pubkey".to_string(),
                mount_point: None,
            }),
        };

        let result = EvmSignerFactory::create_evm_signer(signer_model).await;

        assert!(matches!(
            result,
            Err(SignerFactoryError::UnsupportedType(_))
        ));
    }

    #[tokio::test]
    async fn test_create_evm_signer_turnkey() {
        let signer_model = SignerDomainModel {
            id: "test".to_string(),
            config: SignerConfig::Turnkey(TurnkeySignerConfig {
                api_private_key: SecretString::new("api_private_key"),
                api_public_key: "api_public_key".to_string(),
                organization_id: "organization_id".to_string(),
                private_key_id: "private_key_id".to_string(),
                public_key: "047d3bb8e0317927700cf19fed34e0627367be1390ec247dddf8c239e4b4321a49aea80090e49b206b6a3e577a4f11d721ab063482001ee10db40d6f2963233eec".to_string(),
            }),
        };

        let signer = EvmSignerFactory::create_evm_signer(signer_model)
            .await
            .unwrap();
        let signer_address = signer.address().await.unwrap();

        assert_eq!(
            "0xb726167dc2ef2ac582f0a3de4c08ac4abb90626a",
            signer_address.to_string()
        );
    }

    #[tokio::test]
    async fn test_create_evm_signer_cdp() {
        let signer_model = SignerDomainModel {
            id: "test".to_string(),
            config: SignerConfig::Cdp(CdpSignerConfig {
                api_key_id: "test-api-key-id".to_string(),
                api_key_secret: SecretString::new("test-api-key-secret"),
                wallet_secret: SecretString::new("test-wallet-secret"),
                account_address: "0xb726167dc2ef2ac582f0a3de4c08ac4abb90626a".to_string(),
            }),
        };

        let signer = EvmSignerFactory::create_evm_signer(signer_model)
            .await
            .unwrap();

        assert!(matches!(signer, EvmSigner::Cdp(_)));
    }

    #[tokio::test]
    async fn test_address_evm_signer_local() {
        let signer_model = SignerDomainModel {
            id: "test".to_string(),
            config: SignerConfig::Local(LocalSignerConfig {
                raw_key: test_key_bytes(),
            }),
        };

        let signer = EvmSignerFactory::create_evm_signer(signer_model)
            .await
            .unwrap();
        let signer_address = signer.address().await.unwrap();

        assert_eq!(test_key_address(), signer_address);
    }

    #[tokio::test]
    async fn test_address_evm_signer_test() {
        let signer_model = SignerDomainModel {
            id: "test".to_string(),
            config: SignerConfig::Local(LocalSignerConfig {
                raw_key: test_key_bytes(),
            }),
        };

        let signer = EvmSignerFactory::create_evm_signer(signer_model)
            .await
            .unwrap();
        let signer_address = signer.address().await.unwrap();

        assert_eq!(test_key_address(), signer_address);
    }

    #[tokio::test]
    async fn test_address_evm_signer_turnkey() {
        let signer_model = SignerDomainModel {
            id: "test".to_string(),
            config: SignerConfig::Turnkey(TurnkeySignerConfig {
                api_private_key: SecretString::new("api_private_key"),
                api_public_key: "api_public_key".to_string(),
                organization_id: "organization_id".to_string(),
                private_key_id: "private_key_id".to_string(),
                public_key: "047d3bb8e0317927700cf19fed34e0627367be1390ec247dddf8c239e4b4321a49aea80090e49b206b6a3e577a4f11d721ab063482001ee10db40d6f2963233eec".to_string(),
            }),
        };

        let signer = EvmSignerFactory::create_evm_signer(signer_model)
            .await
            .unwrap();
        let signer_address = signer.address().await.unwrap();

        assert_eq!(
            "0xb726167dc2ef2ac582f0a3de4c08ac4abb90626a",
            signer_address.to_string()
        );
    }

    #[tokio::test]
    async fn test_address_evm_signer_cdp() {
        let signer_model = SignerDomainModel {
            id: "test".to_string(),
            config: SignerConfig::Cdp(CdpSignerConfig {
                api_key_id: "test-api-key-id".to_string(),
                api_key_secret: SecretString::new("test-api-key-secret"),
                wallet_secret: SecretString::new("test-wallet-secret"),
                account_address: "0xb726167dc2ef2ac582f0a3de4c08ac4abb90626a".to_string(),
            }),
        };

        let signer = EvmSignerFactory::create_evm_signer(signer_model)
            .await
            .unwrap();
        let signer_address = signer.address().await.unwrap();

        assert_eq!(
            "0xb726167dc2ef2ac582f0a3de4c08ac4abb90626a",
            signer_address.to_string()
        );
    }

    #[tokio::test]
    async fn test_sign_data_evm_signer_local() {
        let signer_model = SignerDomainModel {
            id: "test".to_string(),
            config: SignerConfig::Local(LocalSignerConfig {
                raw_key: test_key_bytes(),
            }),
        };

        let signer = EvmSignerFactory::create_evm_signer(signer_model)
            .await
            .unwrap();
        let request = SignDataRequest {
            message: "Test message".to_string(),
        };

        let result = signer.sign_data(request).await;

        assert!(result.is_ok());

        let response = result.unwrap();
        assert!(matches!(response, SignDataResponse::Evm(_)));

        if let SignDataResponse::Evm(sig) = response {
            assert_eq!(sig.r.len(), 64); // 32 bytes in hex
            assert_eq!(sig.s.len(), 64); // 32 bytes in hex
            assert!(sig.v == 27 || sig.v == 28); // Valid v values
            assert_eq!(sig.sig.len(), 130); // 65 bytes in hex
        }
    }

    #[tokio::test]
    async fn test_sign_transaction_evm() {
        let signer_model = SignerDomainModel {
            id: "test".to_string(),
            config: SignerConfig::Local(LocalSignerConfig {
                raw_key: test_key_bytes(),
            }),
        };

        let signer = EvmSignerFactory::create_evm_signer(signer_model)
            .await
            .unwrap();

        let transaction_data = NetworkTransactionData::Evm(EvmTransactionData {
            from: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            to: Some("0x742d35Cc6634C0532925a3b844Bc454e4438f44f".to_string()),
            gas_price: Some(20000000000),
            gas_limit: Some(21000),
            nonce: Some(0),
            value: U256::from(1000000000000000000u64),
            data: Some("0x".to_string()),
            chain_id: 1,
            hash: None,
            signature: None,
            raw: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            speed: None,
        });

        let result = signer.sign_transaction(transaction_data).await;

        assert!(result.is_ok());

        let signed_tx = result.unwrap();

        assert!(matches!(signed_tx, SignTransactionResponse::Evm(_)));

        if let SignTransactionResponse::Evm(evm_tx) = signed_tx {
            assert!(!evm_tx.hash.is_empty());
            assert!(!evm_tx.raw.is_empty());
            assert!(!evm_tx.signature.sig.is_empty());
        }
    }

    #[tokio::test]
    async fn test_create_evm_signer_google_cloud_kms() {
        let signer_model = SignerDomainModel {
            id: "test".to_string(),
            config: SignerConfig::GoogleCloudKms(GoogleCloudKmsSignerConfig {
                service_account: GoogleCloudKmsSignerServiceAccountConfig {
                    project_id: "project_id".to_string(),
                    private_key_id: SecretString::new("private_key_id"),
                    private_key: SecretString::new("-----BEGIN EXAMPLE PRIVATE KEY-----\nFAKEKEYDATA\n-----END EXAMPLE PRIVATE KEY-----\n"),
                    client_email: SecretString::new("client_email@example.com"),
                    client_id: "client_id".to_string(),
                    auth_uri: "https://accounts.google.com/o/oauth2/auth".to_string(),
                    token_uri: "https://oauth2.googleapis.com/token".to_string(),
                    auth_provider_x509_cert_url: "https://www.googleapis.com/oauth2/v1/certs".to_string(),
                    client_x509_cert_url: "https://www.googleapis.com/robot/v1/metadata/x509/client_email%40example.com".to_string(),
                    universe_domain: "googleapis.com".to_string(),
                },
                key: GoogleCloudKmsSignerKeyConfig {
                    location: "global".to_string(),
                    key_id: "id".to_string(),
                    key_ring_id: "key_ring".to_string(),
                    key_version: 1,
                },
            }),
        };

        let result = EvmSignerFactory::create_evm_signer(signer_model).await;

        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), EvmSigner::GoogleCloudKms(_)));
    }

    #[tokio::test]
    async fn test_sign_data_with_different_message_types() {
        let signer_model = SignerDomainModel {
            id: "test".to_string(),
            config: SignerConfig::Local(LocalSignerConfig {
                raw_key: test_key_bytes(),
            }),
        };

        let signer = EvmSignerFactory::create_evm_signer(signer_model)
            .await
            .unwrap();

        // Test with various message types
        let long_message = "a".repeat(1000);
        let test_cases = vec![
            ("Simple message", "Test message"),
            ("Empty message", ""),
            ("Unicode message", "🚀 Test message with émojis"),
            ("Long message", long_message.as_str()),
            ("JSON message", r#"{"test": "value", "number": 123}"#),
        ];

        for (name, message) in test_cases {
            let request = SignDataRequest {
                message: message.to_string(),
            };

            let result = signer.sign_data(request).await;
            assert!(result.is_ok(), "Failed to sign {}", name);

            if let Ok(SignDataResponse::Evm(sig)) = result {
                assert_eq!(sig.r.len(), 64, "Invalid r length for {}", name);
                assert_eq!(sig.s.len(), 64, "Invalid s length for {}", name);
                assert!(sig.v == 27 || sig.v == 28, "Invalid v value for {}", name);
                assert_eq!(sig.sig.len(), 130, "Invalid signature length for {}", name);
            } else {
                panic!("Expected EVM signature for {}", name);
            }
        }
    }

    // Edge case tests for EIP-712 and signature validation
    #[tokio::test]
    async fn test_eip712_hash_with_0x_prefix() {
        let request_with_prefix = SignTypedDataRequest {
            domain_separator: format!("0x{}", "a".repeat(64)),
            hash_struct_message: format!("0x{}", "b".repeat(64)),
        };

        let request_without_prefix = SignTypedDataRequest {
            domain_separator: "a".repeat(64),
            hash_struct_message: "b".repeat(64),
        };

        let hash1 = construct_eip712_message_hash(&request_with_prefix).unwrap();
        let hash2 = construct_eip712_message_hash(&request_without_prefix).unwrap();

        assert_eq!(
            hash1, hash2,
            "Hash should be same with or without 0x prefix"
        );
    }

    #[tokio::test]
    async fn test_eip712_deterministic() {
        let request = SignTypedDataRequest {
            domain_separator: "a".repeat(64),
            hash_struct_message: "b".repeat(64),
        };

        let hash1 = construct_eip712_message_hash(&request).unwrap();
        let hash2 = construct_eip712_message_hash(&request).unwrap();

        assert_eq!(hash1, hash2, "Hash must be deterministic");
    }

    #[tokio::test]
    async fn test_eip712_invalid_domain_length() {
        let request = SignTypedDataRequest {
            domain_separator: "a".repeat(30), // Too short
            hash_struct_message: "b".repeat(64),
        };

        let result = construct_eip712_message_hash(&request);
        assert!(result.is_err());
        if let Err(e) = result {
            assert!(e.to_string().contains("Invalid domain separator length"));
        }
    }

    #[tokio::test]
    async fn test_eip712_invalid_hash_struct_length() {
        let request = SignTypedDataRequest {
            domain_separator: "a".repeat(64),
            hash_struct_message: "b".repeat(30), // Too short
        };

        let result = construct_eip712_message_hash(&request);
        assert!(result.is_err());
        if let Err(e) = result {
            assert!(e.to_string().contains("Invalid hash struct length"));
        }
    }

    #[tokio::test]
    async fn test_eip712_invalid_hex_characters() {
        let request = SignTypedDataRequest {
            domain_separator: "zzzz".to_string(), // Invalid hex
            hash_struct_message: "b".repeat(64),
        };

        let result = construct_eip712_message_hash(&request);
        assert!(result.is_err());
        if let Err(e) = result {
            assert!(e.to_string().contains("non-hexadecimal character"));
        }
    }

    #[tokio::test]
    async fn test_eip712_invalid_hex_at_specific_position() {
        let request = SignTypedDataRequest {
            domain_separator: format!("{}z{}", "a".repeat(10), "a".repeat(53)), // 'z' at position 10
            hash_struct_message: "b".repeat(64),
        };

        let result = construct_eip712_message_hash(&request);
        assert!(result.is_err());
        if let Err(e) = result {
            let err_msg = e.to_string();
            assert!(err_msg.contains("non-hexadecimal character"));
            assert!(err_msg.contains("position 10")); // Error should report exact position
        }
    }

    #[test]
    fn test_signature_validation_wrong_length() {
        let sig_64_bytes = vec![0u8; 64];
        let result = validate_and_format_signature(&sig_64_bytes, "TestSigner");
        assert!(result.is_err());
        if let Err(e) = result {
            assert!(e.to_string().contains("expected 65 bytes"));
        }

        let sig_66_bytes = vec![0u8; 66];
        let result = validate_and_format_signature(&sig_66_bytes, "TestSigner");
        assert!(result.is_err());

        let sig_0_bytes = vec![];
        let result = validate_and_format_signature(&sig_0_bytes, "TestSigner");
        assert!(result.is_err());
    }

    #[test]
    fn test_signature_validation_correct_length() {
        let sig_65_bytes = vec![0u8; 65];
        let result = validate_and_format_signature(&sig_65_bytes, "TestSigner");
        assert!(result.is_ok());

        if let Ok(SignDataResponse::Evm(sig)) = result {
            assert_eq!(sig.r.len(), 64); // 32 bytes as hex
            assert_eq!(sig.s.len(), 64); // 32 bytes as hex
            assert_eq!(sig.v, 0); // Last byte
            assert_eq!(sig.sig.len(), 130); // 65 bytes as hex
        }
    }

    #[tokio::test]
    async fn test_eip712_odd_length_hex_string() {
        // Odd-length hex strings should fail during decoding
        let request = SignTypedDataRequest {
            domain_separator: "a".repeat(63), // Odd length
            hash_struct_message: "b".repeat(64),
        };

        let result = construct_eip712_message_hash(&request);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_eip712_mixed_case_hex() {
        // Mixed case hex should work
        let request = SignTypedDataRequest {
            domain_separator: "AaBbCcDdEeFf11223344556677889900AaBbCcDdEeFf11223344556677889900"
                .to_string(),
            hash_struct_message: "b".repeat(64),
        };

        let result = construct_eip712_message_hash(&request);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validate_and_decode_hex_error_includes_input_preview() {
        let long_invalid_hex = format!("{}z{}", "a".repeat(20), "a".repeat(100));
        let result = validate_and_decode_hex(&long_invalid_hex, "test_field");

        assert!(result.is_err());
        if let Err(e) = result {
            let err_msg = e.to_string();
            // Should include first 16 chars of input for debugging
            assert!(err_msg.contains("input:"));
            assert!(err_msg.len() < 200); // Error message should be reasonably sized
        }
    }
}
