//! # Vault Transit Signer for Stellar
//!
//! This module provides a Stellar signer implementation that uses HashiCorp Vault's Transit
//! engine for secure Ed25519 signing operations without exporting the private key.

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use soroban_rs::xdr::{
    DecoratedSignature, Hash, Limits, ReadXdr, Signature, SignatureHint, Transaction,
    TransactionEnvelope, WriteXdr,
};
use tokio::sync::OnceCell;
use tracing::debug;

use crate::{
    domain::{
        attach_signatures_to_envelope, parse_transaction_xdr,
        stellar::{create_signature_payload, create_transaction_signature_payload},
        SignTransactionResponse, SignXdrTransactionResponseStellar,
    },
    models::{Address, NetworkTransactionData, Signer as SignerDomainModel, SignerError},
    services::{signer::Signer, VaultService, VaultServiceTrait},
    utils::base64_decode,
};

use super::StellarSignTrait;

pub type DefaultVaultService = VaultService;

pub struct VaultTransitSigner<T = DefaultVaultService>
where
    T: VaultServiceTrait,
{
    vault_service: T,
    pubkey: String,
    key_name: String,
    cached_hint: OnceCell<SignatureHint>,
}

impl VaultTransitSigner<DefaultVaultService> {
    /// Builds a Stellar Vault Transit signer from a validated signer model.
    pub fn new(
        signer_model: &SignerDomainModel,
        vault_service: DefaultVaultService,
    ) -> Result<Self, SignerError> {
        let config = signer_model
            .config
            .get_vault_transit()
            .ok_or_else(|| SignerError::Configuration("vault transit config not found".into()))?;

        Ok(Self {
            vault_service,
            pubkey: config.pubkey.clone(),
            key_name: config.key_name.clone(),
            cached_hint: OnceCell::new(),
        })
    }
}

#[cfg(test)]
impl<T: VaultServiceTrait> VaultTransitSigner<T> {
    /// Builds a test signer from a signer model and injected Vault service.
    pub fn new_with_service(
        signer_model: &SignerDomainModel,
        vault_service: T,
    ) -> Result<Self, SignerError> {
        let config = signer_model
            .config
            .get_vault_transit()
            .ok_or_else(|| SignerError::Configuration("vault transit config not found".into()))?;

        Ok(Self {
            vault_service,
            pubkey: config.pubkey.clone(),
            key_name: config.key_name.clone(),
            cached_hint: OnceCell::new(),
        })
    }

    /// Builds a test signer directly from raw constructor inputs.
    pub fn new_for_testing(key_name: String, pubkey: String, vault_service: T) -> Self {
        Self {
            vault_service,
            pubkey,
            key_name,
            cached_hint: OnceCell::new(),
        }
    }
}

impl<T: VaultServiceTrait> VaultTransitSigner<T> {
    /// Converts the configured Vault Transit public key into a Stellar address.
    fn stellar_address_from_pubkey(&self) -> Result<Address, SignerError> {
        let raw_pubkey =
            base64_decode(&self.pubkey).map_err(|e| SignerError::KeyError(e.to_string()))?;
        let public_key_bytes: [u8; 32] = raw_pubkey.as_slice().try_into().map_err(|_| {
            SignerError::KeyError(format!(
                "Invalid Stellar Vault Transit public key length: expected 32 bytes, got {}",
                raw_pubkey.len()
            ))
        })?;

        let stellar_address = stellar_strkey::ed25519::PublicKey(public_key_bytes).to_string();
        Ok(Address::Stellar(stellar_address))
    }

    /// Requests a signature from Vault Transit and validates the Ed25519 length.
    async fn sign_hash(&self, hash: &[u8]) -> Result<Vec<u8>, SignerError> {
        let vault_signature_str = self.vault_service.sign(&self.key_name, hash).await?;

        debug!(vault_signature_str = %vault_signature_str, "vault signature string");

        let base64_sig = vault_signature_str
            .strip_prefix("vault:v1:")
            .unwrap_or(&vault_signature_str);

        let signature_bytes = base64_decode(base64_sig)
            .map_err(|e| SignerError::SigningError(format!("Failed to decode signature: {e}")))?;

        if signature_bytes.len() != 64 {
            return Err(SignerError::SigningError(format!(
                "Vault Transit returned invalid Ed25519 signature length: expected 64 bytes, got {}",
                signature_bytes.len()
            )));
        }

        Ok(signature_bytes)
    }

    /// Signs a parsed Stellar envelope using Vault Transit.
    async fn sign_envelope(
        &self,
        envelope: &TransactionEnvelope,
        network_id: &Hash,
    ) -> Result<DecoratedSignature, SignerError> {
        let payload = create_signature_payload(envelope, network_id)
            .map_err(|e| SignerError::SigningError(format!("Failed to create payload: {e}")))?;

        let payload_bytes = payload
            .to_xdr(Limits::none())
            .map_err(|e| SignerError::SigningError(format!("Failed to serialize payload: {e}")))?;

        let hash = Sha256::digest(&payload_bytes);
        let signature_bytes = self.sign_hash(&hash).await?;

        self.create_decorated_signature(signature_bytes).await
    }

    /// Signs an operations-based Stellar transaction by constructing its signature payload.
    async fn sign_transaction_directly(
        &self,
        transaction: &Transaction,
        network_id: &Hash,
    ) -> Result<DecoratedSignature, SignerError> {
        let payload = create_transaction_signature_payload(transaction, network_id);

        let payload_bytes = payload
            .to_xdr(Limits::none())
            .map_err(|e| SignerError::SigningError(format!("Failed to serialize payload: {e}")))?;

        let hash = Sha256::digest(&payload_bytes);
        let signature_bytes = self.sign_hash(&hash).await?;

        self.create_decorated_signature(signature_bytes).await
    }

    /// Wraps raw signature bytes with the cached Stellar signature hint.
    async fn create_decorated_signature(
        &self,
        signature_bytes: Vec<u8>,
    ) -> Result<DecoratedSignature, SignerError> {
        let hint = self.get_signature_hint().await?;
        let signature_bytes_m =
            soroban_rs::xdr::BytesM::try_from(signature_bytes).map_err(|_| {
                SignerError::SigningError(
                    "Failed to convert signature to BytesM format".to_string(),
                )
            })?;

        Ok(DecoratedSignature {
            hint,
            signature: Signature(signature_bytes_m),
        })
    }

    /// Computes and caches the Stellar signature hint derived from the configured public key.
    async fn get_signature_hint(&self) -> Result<SignatureHint, SignerError> {
        self.cached_hint
            .get_or_try_init(|| async {
                let address = self.stellar_address_from_pubkey()?;
                super::derive_signature_hint(&address)
            })
            .await
            .cloned()
    }
}

#[async_trait]
impl<T: VaultServiceTrait> Signer for VaultTransitSigner<T> {
    /// Returns the Stellar address derived from the configured Vault Transit public key.
    async fn address(&self) -> Result<Address, SignerError> {
        self.stellar_address_from_pubkey()
    }

    /// Signs Stellar transaction data using Vault Transit for either operations or XDR inputs.
    async fn sign_transaction(
        &self,
        tx: NetworkTransactionData,
    ) -> Result<SignTransactionResponse, SignerError> {
        let stellar_data = tx
            .get_stellar_transaction_data()
            .map_err(|e| SignerError::SigningError(format!("Failed to get tx data: {e}")))?;

        let passphrase = &stellar_data.network_passphrase;
        let hash_bytes: [u8; 32] = Sha256::digest(passphrase.as_bytes()).into();
        let network_id = Hash(hash_bytes);

        let signature = match &stellar_data.transaction_input {
            crate::models::TransactionInput::Operations(_) => {
                let transaction = Transaction::try_from(stellar_data).map_err(|e| {
                    SignerError::SigningError(format!(
                        "Failed to build Stellar transaction from operations: {e}"
                    ))
                })?;

                self.sign_transaction_directly(&transaction, &network_id)
                    .await?
            }
            crate::models::TransactionInput::UnsignedXdr(xdr)
            | crate::models::TransactionInput::SignedXdr { xdr, .. }
            | crate::models::TransactionInput::SorobanGasAbstraction { xdr, .. } => {
                let envelope =
                    TransactionEnvelope::from_xdr_base64(xdr, Limits::none()).map_err(|e| {
                        SignerError::SigningError(format!(
                            "Failed to parse Stellar transaction XDR '{}...': {}",
                            &xdr[..std::cmp::min(50, xdr.len())],
                            e
                        ))
                    })?;

                self.sign_envelope(&envelope, &network_id).await?
            }
        };

        Ok(SignTransactionResponse::Stellar(
            crate::domain::SignTransactionResponseStellar { signature },
        ))
    }
}

#[async_trait]
impl<T: VaultServiceTrait> StellarSignTrait for VaultTransitSigner<T> {
    /// Signs an unsigned Stellar XDR envelope and returns the signed XDR plus decorated signature.
    async fn sign_xdr_transaction(
        &self,
        unsigned_xdr: &str,
        network_passphrase: &str,
    ) -> Result<SignXdrTransactionResponseStellar, SignerError> {
        debug!("Signing Stellar XDR transaction with Vault Transit");

        let mut envelope = parse_transaction_xdr(unsigned_xdr, false)
            .map_err(|e| SignerError::SigningError(format!("Invalid XDR: {e}")))?;

        let hash_bytes: [u8; 32] = Sha256::digest(network_passphrase.as_bytes()).into();
        let network_id = Hash(hash_bytes);

        let signature = self.sign_envelope(&envelope, &network_id).await?;

        attach_signatures_to_envelope(&mut envelope, vec![signature.clone()])
            .map_err(|e| SignerError::SigningError(format!("Failed to attach signature: {e}")))?;

        let signed_xdr = envelope.to_xdr_base64(Limits::none()).map_err(|e| {
            SignerError::SigningError(format!("Failed to serialize signed XDR: {e}"))
        })?;

        Ok(SignXdrTransactionResponseStellar {
            signed_xdr,
            signature,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        models::{
            LocalSignerConfig, SecretString, SignerConfig, StellarTransactionData,
            TransactionInput, VaultTransitSignerConfig,
        },
        services::{vault::VaultError, MockVaultServiceTrait},
    };
    use base64::Engine;
    use mockall::predicate::*;
    use secrets::SecretVec;
    use soroban_rs::xdr::{SequenceNumber, TransactionV0, TransactionV0Envelope, Uint256};

    /// Returns deterministic public key bytes for Vault Transit unit tests.
    fn create_test_public_key_bytes() -> [u8; 32] {
        [7u8; 32]
    }

    /// Encodes the deterministic test public key as base64, matching Vault output format.
    fn create_test_pubkey_base64() -> String {
        base64::engine::general_purpose::STANDARD.encode(create_test_public_key_bytes())
    }

    /// Converts the deterministic test public key into a Stellar account address.
    fn create_test_address() -> String {
        stellar_strkey::ed25519::PublicKey(create_test_public_key_bytes()).to_string()
    }

    /// Builds a valid Vault Transit signer model for Stellar unit tests.
    fn create_test_signer_model() -> SignerDomainModel {
        SignerDomainModel {
            id: "test-vault-transit-signer".to_string(),
            config: SignerConfig::VaultTransit(VaultTransitSignerConfig {
                key_name: "transit-key".to_string(),
                address: "https://vault.example.com".to_string(),
                namespace: None,
                role_id: SecretString::new("role-123"),
                secret_id: SecretString::new("secret-456"),
                pubkey: create_test_pubkey_base64(),
                mount_point: None,
            }),
        }
    }

    /// Creates a minimal unsigned Stellar XDR envelope for signing tests.
    fn create_unsigned_xdr(source_address: &str) -> String {
        let source_pk = stellar_strkey::ed25519::PublicKey::from_string(source_address).unwrap();
        let tx = TransactionV0 {
            source_account_ed25519: Uint256(source_pk.0),
            fee: 100,
            seq_num: SequenceNumber(1),
            time_bounds: None,
            memo: soroban_rs::xdr::Memo::None,
            operations: vec![].try_into().unwrap(),
            ext: soroban_rs::xdr::TransactionV0Ext::V0,
        };

        let envelope = TransactionEnvelope::TxV0(TransactionV0Envelope {
            tx,
            signatures: vec![].try_into().unwrap(),
        });

        envelope.to_xdr_base64(Limits::none()).unwrap()
    }

    #[test]
    /// Verifies that constructor state is copied from Vault Transit config.
    fn test_new_with_service() {
        let model = create_test_signer_model();
        let mock_vault_service = MockVaultServiceTrait::new();

        let signer = VaultTransitSigner::new_with_service(&model, mock_vault_service).unwrap();

        assert_eq!(signer.key_name, "transit-key");
        assert_eq!(signer.pubkey, create_test_pubkey_base64());
    }

    #[test]
    /// Verifies that missing Vault Transit config surfaces as a configuration error.
    fn test_new_with_service_missing_config_returns_error() {
        let model = SignerDomainModel {
            id: "test-vault-transit-signer".to_string(),
            config: SignerConfig::Local(LocalSignerConfig {
                raw_key: SecretVec::new(32, |v| v.copy_from_slice(&[1u8; 32])),
            }),
        };
        let mock_vault_service = MockVaultServiceTrait::new();

        let result = VaultTransitSigner::new_with_service(&model, mock_vault_service);

        assert!(matches!(
            result,
            Err(SignerError::Configuration(ref msg)) if msg == "vault transit config not found"
        ));
    }

    #[tokio::test]
    /// Verifies that address resolution returns the expected Stellar StrKey.
    async fn test_address_returns_stellar_strkey() {
        let mock_vault_service = MockVaultServiceTrait::new();
        let signer = VaultTransitSigner::new_for_testing(
            "test-key".to_string(),
            create_test_pubkey_base64(),
            mock_vault_service,
        );

        let result = signer.address().await.unwrap();
        assert_eq!(result, Address::Stellar(create_test_address()));
    }

    #[tokio::test]
    /// Verifies that a signed XDR response contains a decorated signature and signed envelope.
    async fn test_sign_xdr_transaction_success() {
        let test_address = create_test_address();
        let unsigned_xdr = create_unsigned_xdr(&test_address);

        let mut mock_vault_service = MockVaultServiceTrait::new();
        mock_vault_service
            .expect_sign()
            .times(1)
            .with(eq("transit-key"), always())
            .returning(|_, _| {
                let signature = vec![1u8; 64];
                let encoded = base64::engine::general_purpose::STANDARD.encode(signature);
                Box::pin(async move { Ok(format!("vault:v1:{encoded}")) })
            });

        let signer = VaultTransitSigner::new_for_testing(
            "transit-key".to_string(),
            create_test_pubkey_base64(),
            mock_vault_service,
        );

        let result = signer
            .sign_xdr_transaction(&unsigned_xdr, "Test SDF Network ; September 2015")
            .await
            .unwrap();

        assert!(!result.signed_xdr.is_empty());
        assert_eq!(result.signature.hint.0.len(), 4);
        assert_eq!(result.signature.signature.0.len(), 64);

        let signed_envelope =
            TransactionEnvelope::from_xdr_base64(&result.signed_xdr, Limits::none()).unwrap();
        match signed_envelope {
            TransactionEnvelope::TxV0(v0_env) => assert_eq!(v0_env.signatures.len(), 1),
            _ => panic!("Expected V0 envelope"),
        }
    }

    #[tokio::test]
    /// Verifies that invalid signature sizes returned by Vault Transit are rejected.
    async fn test_sign_transaction_invalid_signature_length() {
        let mut mock_vault_service = MockVaultServiceTrait::new();
        mock_vault_service.expect_sign().times(1).returning(|_, _| {
            let encoded = base64::engine::general_purpose::STANDARD.encode(vec![2u8; 32]);
            Box::pin(async move { Ok(format!("vault:v1:{encoded}")) })
        });

        let signer = VaultTransitSigner::new_for_testing(
            "transit-key".to_string(),
            create_test_pubkey_base64(),
            mock_vault_service,
        );

        let tx_data = StellarTransactionData {
            source_account: create_test_address(),
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
        };

        let result = signer
            .sign_transaction(NetworkTransactionData::Stellar(tx_data))
            .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            SignerError::SigningError(msg) => {
                assert!(msg.contains("invalid Ed25519 signature length"));
                assert!(msg.contains("expected 64 bytes, got 32"));
            }
            other => panic!("Expected SigningError, got {other:?}"),
        }
    }

    #[tokio::test]
    /// Verifies that Vault signing failures propagate through transaction signing.
    async fn test_sign_propagates_vault_error() {
        let mut mock_vault_service = MockVaultServiceTrait::new();
        mock_vault_service.expect_sign().times(1).returning(|_, _| {
            Box::pin(async move { Err(VaultError::SigningError("vault unavailable".into())) })
        });

        let signer = VaultTransitSigner::new_for_testing(
            "transit-key".to_string(),
            create_test_pubkey_base64(),
            mock_vault_service,
        );

        let tx_data = StellarTransactionData {
            source_account: create_test_address(),
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
        };

        let result = signer
            .sign_transaction(NetworkTransactionData::Stellar(tx_data))
            .await;

        assert!(result.is_err());
    }
}
