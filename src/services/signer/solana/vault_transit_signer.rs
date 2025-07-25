//! # Vault Transit Signer for Solana
//!
//! This module provides a Solana signer implementation that uses HashiCorp Vault's Transit engine
//! for secure key management and signing operations.
use std::str::FromStr;

use async_trait::async_trait;
use base64::Engine;
use log::{debug, info};
use solana_sdk::{
    instruction::Instruction,
    message::Message,
    pubkey::{self, Pubkey},
    signature::{Keypair, Signature},
    signer::{SeedDerivable, Signer as SolanaSigner},
    transaction::Transaction,
};
use vaultrs::client::VaultClient;
use vaultrs::client::VaultClientSettingsBuilder;
use vaultrs::error::ClientError;
use vaultrs::transit;

use crate::{
    domain::{
        SignDataRequest, SignDataResponse, SignDataResponseEvm, SignTransactionResponse,
        SignTypedDataRequest,
    },
    models::{
        Address, NetworkTransactionData, Signer as SignerDomainModel, SignerError,
        TransactionRepoModel,
    },
    services::{Signer, VaultConfig, VaultService, VaultServiceTrait},
    utils::{base64_decode, base64_encode},
};

use super::SolanaSignTrait;

pub type DefaultVaultService = VaultService;

pub struct VaultTransitSigner<T = DefaultVaultService>
where
    T: VaultServiceTrait,
{
    vault_service: T,
    pubkey: String,
    key_name: String,
}

impl VaultTransitSigner<DefaultVaultService> {
    pub fn new(signer_model: &SignerDomainModel, vault_service: DefaultVaultService) -> Self {
        let config = signer_model
            .config
            .get_vault_transit()
            .expect("vault transit config not found");

        Self {
            vault_service,
            pubkey: config.pubkey.clone(),
            key_name: config.key_name.clone(),
        }
    }
}

#[cfg(test)]
impl<T: VaultServiceTrait> VaultTransitSigner<T> {
    pub fn new_with_service(signer_model: &SignerDomainModel, vault_service: T) -> Self {
        let config = signer_model
            .config
            .get_vault_transit()
            .expect("vault transit config not found");

        Self {
            vault_service,
            pubkey: config.pubkey.clone(),
            key_name: config.key_name.clone(),
        }
    }

    pub fn new_for_testing(key_name: String, pubkey: String, vault_service: T) -> Self {
        Self {
            vault_service,
            pubkey,
            key_name,
        }
    }
}

#[async_trait]
impl<T: VaultServiceTrait> SolanaSignTrait for VaultTransitSigner<T> {
    async fn pubkey(&self) -> Result<Address, SignerError> {
        let raw_pubkey =
            base64_decode(&self.pubkey).map_err(|e| SignerError::KeyError(e.to_string()))?;
        let pubkey = bs58::encode(&raw_pubkey).into_string();
        let address: Address = Address::Solana(pubkey);

        Ok(address)
    }

    async fn sign(&self, message: &[u8]) -> Result<Signature, SignerError> {
        let vault_signature_str = self.vault_service.sign(&self.key_name, message).await?;

        debug!("vault_signature_str: {}", vault_signature_str);

        let base64_sig = vault_signature_str
            .strip_prefix("vault:v1:")
            .unwrap_or(&vault_signature_str);

        let sig_bytes = base64_decode(base64_sig)
            .map_err(|e| SignerError::SigningError(format!("Failed to decode signature: {}", e)))?;

        Ok(Signature::try_from(sig_bytes.as_slice()).map_err(|e| {
            SignerError::SigningError(format!("Failed to create signature from bytes: {}", e))
        })?)
    }
}

#[async_trait]
impl<T: VaultServiceTrait> Signer for VaultTransitSigner<T> {
    async fn address(&self) -> Result<Address, SignerError> {
        let raw_pubkey =
            base64_decode(&self.pubkey).map_err(|e| SignerError::KeyError(e.to_string()))?;
        let pubkey = bs58::encode(&raw_pubkey).into_string();
        let address: Address = Address::Solana(pubkey);

        Ok(address)
    }

    async fn sign_transaction(
        &self,
        _transaction: NetworkTransactionData,
    ) -> Result<SignTransactionResponse, SignerError> {
        Err(SignerError::NotImplemented(
            "sign_transaction is not implemented".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        models::{
            SecretString, Signer as SignerDomainModel, SignerConfig, SolanaTransactionData,
            VaultTransitSignerConfig,
        },
        services::{vault::VaultError, MockVaultServiceTrait},
    };
    use mockall::predicate::*;

    fn create_test_signer_model() -> SignerDomainModel {
        SignerDomainModel {
            id: "test-vault-transit-signer".to_string(),
            config: SignerConfig::VaultTransit(VaultTransitSignerConfig {
                key_name: "transit-key".to_string(),
                address: "https://vault.example.com".to_string(),
                namespace: None,
                role_id: SecretString::new("role-123"),
                secret_id: SecretString::new("secret-456"),
                pubkey: "9zzYYGQM9prm/xXgn6Vwas/TVgteDaACCm1zW1ouKQs=".to_string(),
                mount_point: None,
            }),
        }
    }

    #[test]
    fn test_new_with_service() {
        let model = create_test_signer_model();
        let mock_vault_service = MockVaultServiceTrait::new();

        let signer = VaultTransitSigner::new_with_service(&model, mock_vault_service);

        assert_eq!(signer.key_name, "transit-key");
        assert_eq!(
            signer.pubkey,
            "9zzYYGQM9prm/xXgn6Vwas/TVgteDaACCm1zW1ouKQs="
        );
    }

    #[test]
    fn test_new_for_testing() {
        let mock_vault_service = MockVaultServiceTrait::new();

        let signer = VaultTransitSigner::new_for_testing(
            "test-key".to_string(),
            "test-pubkey".to_string(),
            mock_vault_service,
        );

        assert_eq!(signer.key_name, "test-key");
        assert_eq!(signer.pubkey, "test-pubkey");
    }
    #[tokio::test]
    async fn test_sign_with_mock() {
        let mut mock_vault_service = MockVaultServiceTrait::new();
        let key_name = "test-key";
        let test_message = b"hello world";

        let mock_sig_bytes = [1u8; 64];
        let mock_sig_base64 = base64::engine::general_purpose::STANDARD.encode(mock_sig_bytes);
        let mock_vault_signature = format!("vault:v1:{}", mock_sig_base64);

        mock_vault_service
            .expect_sign()
            .with(eq(key_name), eq(test_message.to_vec()))
            .times(1)
            .returning(move |_, _| {
                let mock_vault_signature = mock_vault_signature.clone();
                Box::pin(async move { Ok(mock_vault_signature) })
            });

        let signer = VaultTransitSigner::new_for_testing(
            key_name.to_string(),
            "9zzYYGQM9prm/xXgn6Vwas/TVgteDaACCm1zW1ouKQs=".to_string(),
            mock_vault_service,
        );

        let result = signer.sign(test_message).await;

        assert!(result.is_ok());
        let signature = result.unwrap();
        assert_eq!(signature.as_ref(), &mock_sig_bytes);
    }

    #[tokio::test]
    async fn test_sign_transaction_with_mock() {
        let mock_vault_service = MockVaultServiceTrait::new();
        let key_name = "test-key";

        let signer = VaultTransitSigner::new_for_testing(
            key_name.to_string(),
            "9zzYYGQM9prm/xXgn6Vwas/TVgteDaACCm1zW1ouKQs=".to_string(),
            mock_vault_service,
        );
        let transaction_data = NetworkTransactionData::Solana(SolanaTransactionData {
            fee_payer: "test".to_string(),
            hash: None,
            recent_blockhash: None,
            instructions: vec![],
        });

        let result = signer.sign_transaction(transaction_data).await;

        match result {
            Err(SignerError::NotImplemented(msg)) => {
                assert_eq!(msg, "sign_transaction is not implemented".to_string());
            }
            _ => panic!("Expected SignerError::NotImplemented"),
        }
    }

    #[tokio::test]
    async fn test_pubkey_returns_correct_address() {
        let mock_vault_service = MockVaultServiceTrait::new();
        let base64_pubkey = "9zzYYGQM9prm/xXgn6Vwas/TVgteDaACCm1zW1ouKQs=";

        let signer = VaultTransitSigner::new_for_testing(
            "test-key".to_string(),
            base64_pubkey.to_string(),
            mock_vault_service,
        );

        let result = signer.pubkey().await;
        let result_address = signer.address().await;

        // Assert
        assert!(result.is_ok());
        assert!(result_address.is_ok());
        match result.unwrap() {
            Address::Solana(pubkey) => {
                // The expected base58 encoded representation of the public key
                assert_eq!(pubkey, "He7WmJPCHfaJYHhMqK7QePfRT1JC5JC4UXxf3gnQhN3L");
            }
            _ => panic!("Expected Address::Solana variant"),
        }
        match result_address.unwrap() {
            Address::Solana(pubkey) => {
                // The expected base58 encoded representation of the public key
                assert_eq!(pubkey, "He7WmJPCHfaJYHhMqK7QePfRT1JC5JC4UXxf3gnQhN3L");
            }
            _ => panic!("Expected Address::Solana variant"),
        }
    }
}
