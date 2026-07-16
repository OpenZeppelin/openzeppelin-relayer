// openzeppelin-relayer/src/services/signer/stellar/mod.rs
//! Stellar signer implementation (local keystore, Vault-backed signers, cloud KMS, and Turnkey)

mod aws_kms_signer;
mod google_cloud_kms_signer;
mod local_signer;
mod turnkey_signer;
mod vault_signer;
mod vault_transit_signer;

use async_trait::async_trait;
use aws_kms_signer::*;
use google_cloud_kms_signer::*;
use local_signer::*;
use turnkey_signer::*;
use vault_signer::*;
use vault_transit_signer::*;

use soroban_rs::xdr::SignatureHint;

use crate::{
    domain::{SignDataRequest, SignDataResponse, SignTransactionResponse, SignTypedDataRequest},
    models::{
        Address, NetworkTransactionData, Signer as SignerDomainModel, SignerConfig,
        SignerRepoModel, SignerType, TransactionRepoModel, VaultSignerConfig,
    },
    services::{
        signer::{SignXdrTransactionResponseStellar, Signer, SignerError, SignerFactoryError},
        AwsKmsService, GoogleCloudKmsService, TurnkeyService, VaultConfig, VaultService,
    },
};

use super::DataSignerTrait;

/// Derive a `SignatureHint` (last 4 bytes of the Ed25519 public key) from a Stellar address.
fn derive_signature_hint(address: &Address) -> Result<SignatureHint, SignerError> {
    match address {
        Address::Stellar(addr) => {
            let pk = stellar_strkey::ed25519::PublicKey::from_string(addr).map_err(|e| {
                SignerError::SigningError(format!("Failed to parse Stellar address '{addr}': {e}"))
            })?;
            // pk.0 is [u8; 32], last 4 bytes are the hint
            let hint_bytes: [u8; 4] = pk.0[28..].try_into().map_err(|_| {
                SignerError::SigningError(
                    "Failed to create signature hint from public key".to_string(),
                )
            })?;
            Ok(SignatureHint(hint_bytes))
        }
        _ => Err(SignerError::SigningError(format!(
            "Expected Stellar address, got: {address:?}"
        ))),
    }
}

#[cfg(test)]
use mockall::automock;

#[cfg_attr(test, automock)]
/// Trait defining Stellar-specific signing operations
///
/// This trait extends the basic signing functionality with methods specific
/// to the Stellar blockchain, following the same pattern as SolanaSignTrait.
#[async_trait]
pub trait StellarSignTrait: Sync + Send {
    /// Signs a Stellar transaction in XDR format
    ///
    /// # Arguments
    ///
    /// * `unsigned_xdr` - The unsigned transaction in XDR format
    /// * `network_passphrase` - The network passphrase for the Stellar network
    ///
    /// # Returns
    ///
    /// A signed transaction response containing the signed XDR and signature
    async fn sign_xdr_transaction(
        &self,
        unsigned_xdr: &str,
        network_passphrase: &str,
    ) -> Result<SignXdrTransactionResponseStellar, SignerError>;
}

pub enum StellarSigner {
    Local(Box<LocalSigner>),
    Vault(VaultSigner<VaultService>),
    VaultTransit(VaultTransitSigner),
    GoogleCloudKms(Box<GoogleCloudKmsSigner>),
    AwsKms(AwsKmsSigner),
    Turnkey(TurnkeySigner),
}

#[async_trait]
impl Signer for StellarSigner {
    async fn address(&self) -> Result<Address, SignerError> {
        match self {
            Self::Local(s) => s.address().await,
            Self::Vault(s) => s.address().await,
            Self::VaultTransit(s) => s.address().await,
            Self::GoogleCloudKms(s) => s.address().await,
            Self::AwsKms(s) => s.address().await,
            Self::Turnkey(s) => s.address().await,
        }
    }

    async fn sign_transaction(
        &self,
        tx: NetworkTransactionData,
    ) -> Result<SignTransactionResponse, SignerError> {
        match self {
            Self::Local(s) => s.sign_transaction(tx).await,
            Self::Vault(s) => s.sign_transaction(tx).await,
            Self::VaultTransit(s) => s.sign_transaction(tx).await,
            Self::GoogleCloudKms(s) => s.sign_transaction(tx).await,
            Self::AwsKms(s) => s.sign_transaction(tx).await,
            Self::Turnkey(s) => s.sign_transaction(tx).await,
        }
    }
}

#[async_trait]
impl StellarSignTrait for StellarSigner {
    async fn sign_xdr_transaction(
        &self,
        unsigned_xdr: &str,
        network_passphrase: &str,
    ) -> Result<SignXdrTransactionResponseStellar, SignerError> {
        match self {
            Self::Local(s) => {
                s.sign_xdr_transaction(unsigned_xdr, network_passphrase)
                    .await
            }
            Self::Vault(s) => {
                s.sign_xdr_transaction(unsigned_xdr, network_passphrase)
                    .await
            }
            Self::VaultTransit(s) => {
                s.sign_xdr_transaction(unsigned_xdr, network_passphrase)
                    .await
            }
            Self::GoogleCloudKms(s) => {
                s.sign_xdr_transaction(unsigned_xdr, network_passphrase)
                    .await
            }
            Self::AwsKms(s) => {
                s.sign_xdr_transaction(unsigned_xdr, network_passphrase)
                    .await
            }
            Self::Turnkey(s) => {
                s.sign_xdr_transaction(unsigned_xdr, network_passphrase)
                    .await
            }
        }
    }
}

pub struct StellarSignerFactory;

impl StellarSignerFactory {
    /// Creates a Stellar signer implementation from the provided signer configuration.
    pub async fn create_stellar_signer(
        m: SignerDomainModel,
    ) -> Result<StellarSigner, SignerFactoryError> {
        // Taken by value (like create_evm_signer): an `async fn` holding a
        // `&SignerDomainModel` across `.await` is `!Send` because `Signer` is `!Sync`,
        // and these futures are spawned on the multi-thread runtime.
        let signer = match &m.config {
            SignerConfig::Local(_) => {
                let local_signer = LocalSigner::new(&m)?;
                StellarSigner::Local(Box::new(local_signer))
            }
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

                StellarSigner::Vault(VaultSigner::new(
                    m.id.clone(),
                    config.clone(),
                    vault_service,
                ))
            }
            SignerConfig::GoogleCloudKms(config) => {
                let service = GoogleCloudKmsService::new(config)
                    .map_err(|e| SignerFactoryError::CreationFailed(e.to_string()))?;
                StellarSigner::GoogleCloudKms(Box::new(GoogleCloudKmsSigner::new(service)))
            }
            SignerConfig::Turnkey(config) => {
                let service = TurnkeyService::new(config.clone())
                    .map_err(|e| SignerFactoryError::CreationFailed(e.to_string()))?;
                StellarSigner::Turnkey(TurnkeySigner::new(service))
            }
            SignerConfig::AwsKms(config) => {
                // Async construction (mirrors create_evm_signer): `block_on` here would
                // park the carrier worker thread and not drive the reactor — under the
                // multi-thread runtime a burst of cold-cache builds could park all workers.
                let aws_kms_service = AwsKmsService::new(config.clone()).await.map_err(|e| {
                    SignerFactoryError::InvalidConfig(format!(
                        "Failed to create AWS KMS service: {e}"
                    ))
                })?;
                StellarSigner::AwsKms(AwsKmsSigner::new(aws_kms_service))
            }
            SignerConfig::VaultTransit(config) => {
                let vault_service = VaultService::new(VaultConfig {
                    address: config.address.clone(),
                    namespace: config.namespace.clone(),
                    role_id: config.role_id.clone(),
                    secret_id: config.secret_id.clone(),
                    mount_path: config
                        .mount_point
                        .clone()
                        .unwrap_or_else(|| "transit".to_string()),
                    token_ttl: None,
                });

                StellarSigner::VaultTransit(VaultTransitSigner::new(&m, vault_service).map_err(
                    |e| {
                        SignerFactoryError::InvalidConfig(format!(
                            "Failed to create Vault Transit signer: {e}"
                        ))
                    },
                )?)
            }
            SignerConfig::AzureKeyVault(_) => {
                return Err(SignerFactoryError::UnsupportedType(
                    "Azure Key Vault".into(),
                ))
            }
            SignerConfig::Cdp(_) => return Err(SignerFactoryError::UnsupportedType("CDP".into())),
        };
        Ok(signer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        AzureKeyVaultSignerConfig, LocalSignerConfig, SecretString, StellarTransactionData,
        TransactionInput, TurnkeySignerConfig, VaultTransitSignerConfig,
    };
    use base64::Engine;
    use secrets::SecretVec;
    use soroban_rs::xdr::{
        Limits, SequenceNumber, TransactionEnvelope, TransactionV0, TransactionV0Envelope, Uint256,
        WriteXdr,
    };

    /// Returns deterministic private key bytes for local Stellar signer tests.
    fn test_key_bytes() -> SecretVec<u8> {
        let seed = [1u8; 32];
        SecretVec::new(seed.len(), |v| v.copy_from_slice(&seed))
    }

    /// Builds a local Stellar signer model backed by deterministic test key material.
    fn create_local_signer_model() -> SignerDomainModel {
        SignerDomainModel {
            id: "test".to_string(),
            config: SignerConfig::Local(LocalSignerConfig {
                raw_key: test_key_bytes(),
            }),
        }
    }

    /// Builds a Vault Transit signer model with deterministic public key material.
    fn create_vault_transit_signer_model() -> SignerDomainModel {
        let pubkey = base64::engine::general_purpose::STANDARD.encode([1u8; 32]);
        SignerDomainModel {
            id: "test".to_string(),
            config: SignerConfig::VaultTransit(VaultTransitSignerConfig {
                key_name: "test-key".to_string(),
                address: "https://vault.test.com".to_string(),
                namespace: None,
                role_id: SecretString::new("test-role-id"),
                secret_id: SecretString::new("test-secret-id"),
                pubkey,
                mount_point: None,
            }),
        }
    }

    /// Builds a Turnkey signer model with deterministic public key material.
    fn create_turnkey_signer_model() -> SignerDomainModel {
        SignerDomainModel {
            id: "test".to_string(),
            config: SignerConfig::Turnkey(TurnkeySignerConfig {
                api_private_key: SecretString::new(
                    "1111111111111111111111111111111111111111111111111111111111111111",
                ),
                api_public_key: "api-public-key".to_string(),
                organization_id: "organization-id".to_string(),
                private_key_id: "private-key-id".to_string(),
                public_key: "0101010101010101010101010101010101010101010101010101010101010101"
                    .to_string(),
            }),
        }
    }

    /// Creates a minimal unsigned Stellar transaction envelope encoded as base64 XDR.
    fn create_unsigned_xdr(source_account: &str) -> String {
        let source_pk = stellar_strkey::ed25519::PublicKey::from_string(source_account).unwrap();
        let tx = TransactionV0 {
            source_account_ed25519: Uint256(source_pk.0),
            fee: 100,
            seq_num: SequenceNumber(1),
            time_bounds: None,
            memo: soroban_rs::xdr::Memo::None,
            operations: vec![].try_into().unwrap(),
            ext: soroban_rs::xdr::TransactionV0Ext::V0,
        };

        TransactionEnvelope::TxV0(TransactionV0Envelope {
            tx,
            signatures: vec![].try_into().unwrap(),
        })
        .to_xdr_base64(Limits::none())
        .unwrap()
    }

    /// Builds minimal Stellar transaction data for module-level dispatch tests.
    fn create_stellar_transaction_data(source_account: String) -> StellarTransactionData {
        StellarTransactionData {
            source_account,
            fee: Some(100),
            sequence_number: Some(1),
            transaction_input: TransactionInput::Operations(vec![]),
            memo: None,
            valid_until: None,
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            signatures: Vec::new(),
            hash: None,
            simulation_transaction_data: None,
            signed_envelope_xdr: None,
            transaction_result_xdr: None,
        }
    }

    #[test]
    fn test_derive_signature_hint_valid_stellar_address() {
        let pk = stellar_strkey::ed25519::PublicKey([0u8; 32]);
        let address = Address::Stellar(pk.to_string());

        let hint = derive_signature_hint(&address).unwrap();
        // Last 4 bytes of all-zero key
        assert_eq!(hint.0, [0u8; 4]);
    }

    #[test]
    fn test_derive_signature_hint_extracts_last_four_bytes() {
        let mut key_bytes = [0u8; 32];
        key_bytes[28] = 0xAA;
        key_bytes[29] = 0xBB;
        key_bytes[30] = 0xCC;
        key_bytes[31] = 0xDD;
        let pk = stellar_strkey::ed25519::PublicKey(key_bytes);
        let address = Address::Stellar(pk.to_string());

        let hint = derive_signature_hint(&address).unwrap();
        assert_eq!(hint.0, [0xAA, 0xBB, 0xCC, 0xDD]);
    }

    #[test]
    fn test_derive_signature_hint_invalid_stellar_address() {
        let address = Address::Stellar("INVALID_ADDRESS".to_string());
        let result = derive_signature_hint(&address);
        assert!(result.is_err());
        match result.unwrap_err() {
            SignerError::SigningError(msg) => {
                assert!(msg.contains("Failed to parse Stellar address"));
            }
            e => panic!("Expected SigningError, got: {e:?}"),
        }
    }

    #[test]
    fn test_derive_signature_hint_non_stellar_address() {
        let address = Address::Evm([0u8; 20]);
        let result = derive_signature_hint(&address);
        assert!(result.is_err());
        match result.unwrap_err() {
            SignerError::SigningError(msg) => {
                assert!(msg.contains("Expected Stellar address"));
            }
            e => panic!("Expected SigningError, got: {e:?}"),
        }
    }

    #[test]
    fn test_derive_signature_hint_solana_address_rejected() {
        let address = Address::Solana("SomeBase58Address".to_string());
        let result = derive_signature_hint(&address);
        assert!(result.is_err());
    }

    #[tokio::test]
    /// Verifies that the factory creates a local Stellar signer.
    async fn test_create_stellar_signer_local() {
        let signer = StellarSignerFactory::create_stellar_signer(create_local_signer_model())
            .await
            .unwrap();

        assert!(matches!(signer, StellarSigner::Local(_)));
    }

    #[tokio::test]
    /// Verifies that the factory creates a Vault-backed Stellar signer.
    async fn test_create_stellar_signer_vault() {
        let signer_model = SignerDomainModel {
            id: "test".to_string(),
            config: SignerConfig::Vault(VaultSignerConfig {
                address: "https://vault.test.com".to_string(),
                namespace: Some("test-namespace".to_string()),
                role_id: SecretString::new("test-role-id"),
                secret_id: SecretString::new("test-secret-id"),
                key_name: "test-key".to_string(),
                mount_point: Some("secret".to_string()),
            }),
        };

        let signer = StellarSignerFactory::create_stellar_signer(signer_model)
            .await
            .unwrap();

        assert!(matches!(signer, StellarSigner::Vault(_)));
    }

    #[tokio::test]
    /// Verifies that the factory creates a Vault Transit Stellar signer.
    async fn test_create_stellar_signer_vault_transit() {
        let signer =
            StellarSignerFactory::create_stellar_signer(create_vault_transit_signer_model())
                .await
                .unwrap();

        assert!(matches!(signer, StellarSigner::VaultTransit(_)));
    }

    #[tokio::test]
    /// Verifies that the factory creates a Turnkey-backed Stellar signer.
    async fn test_create_stellar_signer_turnkey() {
        let signer = StellarSignerFactory::create_stellar_signer(create_turnkey_signer_model())
            .await
            .unwrap();

        assert!(matches!(signer, StellarSigner::Turnkey(_)));
    }

    #[tokio::test]
    /// Verifies that Azure Key Vault remains unsupported for Stellar signers.
    async fn test_create_stellar_signer_azure_key_vault_unsupported() {
        let signer_model = SignerDomainModel {
            id: "test".to_string(),
            config: SignerConfig::AzureKeyVault(AzureKeyVaultSignerConfig {
                auth_type: None,
                tenant_id: Some(SecretString::new("tenant-id")),
                client_id: Some(SecretString::new("client-id")),
                client_secret: Some(SecretString::new("client-secret")),
                federated_token_file: None,
                vault_url: SecretString::new("https://example.vault.azure.net"),
                key_name: SecretString::new("test-key"),
                key_version: None,
            }),
        };

        let result = StellarSignerFactory::create_stellar_signer(signer_model).await;

        assert!(matches!(
            result,
            Err(SignerFactoryError::UnsupportedType(ref provider))
                if provider == "Azure Key Vault"
        ));
    }

    #[tokio::test]
    /// Verifies that CDP remains unsupported for Stellar signers.
    async fn test_create_stellar_signer_cdp_unsupported() {
        let signer_model = SignerDomainModel {
            id: "test".to_string(),
            config: SignerConfig::Cdp(crate::models::CdpSignerConfig {
                api_key_id: "api-key-id".to_string(),
                api_key_secret: SecretString::new("dGVzdA=="),
                wallet_secret: SecretString::new("dGVzdA=="),
                account_address: "0xb726167dc2ef2ac582f0a3de4c08ac4abb90626a".to_string(),
            }),
        };

        let result = StellarSignerFactory::create_stellar_signer(signer_model).await;

        assert!(matches!(
            result,
            Err(SignerFactoryError::UnsupportedType(ref provider)) if provider == "CDP"
        ));
    }

    #[tokio::test]
    /// Verifies that address dispatch works for the local Stellar signer variant.
    async fn test_stellar_signer_local_address_dispatch() {
        let signer = StellarSignerFactory::create_stellar_signer(create_local_signer_model())
            .await
            .unwrap();

        let address = signer.address().await.unwrap();

        match address {
            Address::Stellar(addr) => {
                assert!(addr.starts_with('G'));
                assert!(!addr.is_empty());
            }
            _ => panic!("Expected Stellar address"),
        }
    }

    #[tokio::test]
    /// Verifies that address dispatch works for the Vault Transit Stellar signer variant.
    async fn test_stellar_signer_vault_transit_address_dispatch() {
        let signer =
            StellarSignerFactory::create_stellar_signer(create_vault_transit_signer_model())
                .await
                .unwrap();

        let address = signer.address().await.unwrap();

        match address {
            Address::Stellar(addr) => {
                assert!(addr.starts_with('G'));
                assert_eq!(addr.len(), 56);
            }
            _ => panic!("Expected Stellar address"),
        }
    }

    #[tokio::test]
    /// Verifies that address dispatch works for the Turnkey Stellar signer variant.
    async fn test_stellar_signer_turnkey_address_dispatch() {
        let signer = StellarSignerFactory::create_stellar_signer(create_turnkey_signer_model())
            .await
            .unwrap();

        let address = signer.address().await.unwrap();

        match address {
            Address::Stellar(addr) => {
                assert!(addr.starts_with('G'));
                assert_eq!(addr.len(), 56);
            }
            _ => panic!("Expected Stellar address"),
        }
    }

    #[tokio::test]
    /// Verifies that transaction signing dispatch works for the local Stellar signer variant.
    async fn test_stellar_signer_local_sign_transaction_dispatch() {
        let signer = StellarSignerFactory::create_stellar_signer(create_local_signer_model())
            .await
            .unwrap();
        let source_account = signer.address().await.unwrap().to_string();

        let result = signer
            .sign_transaction(NetworkTransactionData::Stellar(
                create_stellar_transaction_data(source_account),
            ))
            .await
            .unwrap();

        match result {
            SignTransactionResponse::Stellar(response) => {
                assert_eq!(response.signature.hint.0.len(), 4);
                assert_eq!(response.signature.signature.0.len(), 64);
            }
            _ => panic!("Expected Stellar signature response"),
        }
    }

    #[tokio::test]
    /// Verifies that XDR signing dispatch works for the local Stellar signer variant.
    async fn test_stellar_signer_local_sign_xdr_transaction_dispatch() {
        let signer = StellarSignerFactory::create_stellar_signer(create_local_signer_model())
            .await
            .unwrap();
        let source_account = signer.address().await.unwrap().to_string();
        let unsigned_xdr = create_unsigned_xdr(&source_account);

        let result = signer
            .sign_xdr_transaction(&unsigned_xdr, "Test SDF Network ; September 2015")
            .await
            .unwrap();

        assert!(!result.signed_xdr.is_empty());
        assert_eq!(result.signature.hint.0.len(), 4);
        assert_eq!(result.signature.signature.0.len(), 64);
    }
}
