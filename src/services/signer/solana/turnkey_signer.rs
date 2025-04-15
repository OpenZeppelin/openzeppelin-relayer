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

use crate::{
    domain::{
        SignDataRequest, SignDataResponse, SignDataResponseEvm, SignTransactionResponse,
        SignTypedDataRequest,
    },
    models::{
        Address, NetworkTransactionData, SignerError, SignerRepoModel, TransactionRepoModel,
        TurnkeySignerConfig,
    },
    services::{Signer, TurnkeyService, TurnkeyServiceTrait},
    utils::{base64_decode, base64_encode},
};

use super::SolanaSignTrait;

pub type DefaultTurnkeyService = TurnkeyService;

pub struct TurnkeySigner<T = DefaultTurnkeyService>
where
    T: TurnkeyServiceTrait,
{
    turnkey_service: T,
    config: TurnkeySignerConfig,
}

impl TurnkeySigner<DefaultTurnkeyService> {
    pub fn new(config: &TurnkeySignerConfig, turnkey_service: DefaultTurnkeyService) -> Self {
        Self {
            turnkey_service,
            config: config.clone(),
        }
    }
}

#[cfg(test)]
impl<T: TurnkeyServiceTrait> TurnkeySigner<T> {
    pub fn new_with_service(signer_model: &SignerRepoModel, turnkey_service: T) -> Self {
        let config = signer_model
            .config
            .get_turnkey()
            .expect("turnkey config not found");

        Self {
            turnkey_service,
            config: config.clone(),
        }
    }

    pub fn new_for_testing(config: TurnkeySignerConfig, turnkey_service: T) -> Self {
        Self {
            turnkey_service,
            config,
        }
    }
}

#[async_trait]
impl<T: TurnkeyServiceTrait> SolanaSignTrait for TurnkeySigner<T> {
    fn pubkey(&self) -> Result<Address, SignerError> {
        let raw_pubkey = hex::decode(&self.config.public_key)
            .map_err(|e| SignerError::KeyError(format!("Invalid public key: {}", e)))?;
        let pubkey = bs58::encode(&raw_pubkey).into_string();
        let address: Address = Address::Solana(pubkey);

        Ok(address)
    }

    async fn sign(&self, message: &[u8]) -> Result<Signature, SignerError> {
        let sig_bytes = self.turnkey_service.sign(message).await?;

        Ok(Signature::try_from(sig_bytes.as_slice()).map_err(|e| {
            SignerError::SigningError(format!("Failed to create signature from bytes: {}", e))
        })?)
    }
}

#[async_trait]
impl<T: TurnkeyServiceTrait> Signer for TurnkeySigner<T> {
    async fn address(&self) -> Result<Address, SignerError> {
        let raw_pubkey = hex::decode(&self.config.public_key)
            .map_err(|e| SignerError::KeyError(format!("Invalid public key: {}", e)))?;
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

// #[cfg(test)]
// mod tests {
//     use super::*;
//     use crate::{
//         models::{SecretString, SignerConfig, SolanaTransactionData, TurnkeySignerConfig},
//         services::{vault::VaultError, MockVaultServiceTrait},
//     };
//     use mockall::predicate::*;

//     fn create_test_signer_model() -> SignerRepoModel {
//         SignerRepoModel {
//             id: "test-vault-transit-signer".to_string(),
//             config: SignerConfig::VaultTransit(TurnkeySignerConfig {
//                 key_name: "transit-key".to_string(),
//                 address: "https://vault.example.com".to_string(),
//                 namespace: None,
//                 role_id: SecretString::new("role-123"),
//                 secret_id: SecretString::new("secret-456"),
//                 pubkey: "9zzYYGQM9prm/xXgn6Vwas/TVgteDaACCm1zW1ouKQs=".to_string(),
//                 mount_point: None,
//             }),
//         }
//     }

//     #[test]
//     fn test_new_with_service() {
//         let model = create_test_signer_model();
//         let mock_turnkey_service = MockVaultServiceTrait::new();

//         let signer = TurnkeySigner::new_with_service(&model, mock_turnkey_service);

//         assert_eq!(signer.key_name, "transit-key");
//         assert_eq!(
//             signer.pubkey,
//             "9zzYYGQM9prm/xXgn6Vwas/TVgteDaACCm1zW1ouKQs="
//         );
//     }

//     #[test]
//     fn test_new_for_testing() {
//         let mock_turnkey_service = MockVaultServiceTrait::new();

//         let signer = TurnkeySigner::new_for_testing(
//             "test-key".to_string(),
//             "test-pubkey".to_string(),
//             mock_turnkey_service,
//         );

//         assert_eq!(signer.key_name, "test-key");
//         assert_eq!(signer.pubkey, "test-pubkey");
//     }
//     #[tokio::test]
//     async fn test_sign_with_mock() {
//         let mut mock_turnkey_service = MockVaultServiceTrait::new();
//         let key_name = "test-key";
//         let test_message = b"hello world";

//         let mock_sig_bytes = [1u8; 64];
//         let mock_sig_base64 = base64::engine::general_purpose::STANDARD.encode(mock_sig_bytes);
//         let mock_vault_signature = format!("vault:v1:{}", mock_sig_base64);

//         mock_turnkey_service
//             .expect_sign()
//             .with(eq(key_name), eq(test_message.to_vec()))
//             .times(1)
//             .returning(move |_, _| {
//                 let mock_vault_signature = mock_vault_signature.clone();
//                 Box::pin(async move { Ok(mock_vault_signature) })
//             });

//         let signer = TurnkeySigner::new_for_testing(
//             key_name.to_string(),
//             "9zzYYGQM9prm/xXgn6Vwas/TVgteDaACCm1zW1ouKQs=".to_string(),
//             mock_turnkey_service,
//         );

//         let result = signer.sign(test_message).await;

//         assert!(result.is_ok());
//         let signature = result.unwrap();
//         assert_eq!(signature.as_ref(), &mock_sig_bytes);
//     }

//     #[tokio::test]
//     async fn test_sign_transaction_with_mock() {
//         let mock_turnkey_service = MockVaultServiceTrait::new();
//         let key_name = "test-key";

//         let signer = TurnkeySigner::new_for_testing(
//             key_name.to_string(),
//             "9zzYYGQM9prm/xXgn6Vwas/TVgteDaACCm1zW1ouKQs=".to_string(),
//             mock_turnkey_service,
//         );
//         let transaction_data = NetworkTransactionData::Solana(SolanaTransactionData {
//             fee_payer: "test".to_string(),
//             hash: None,
//             recent_blockhash: None,
//             instructions: vec![],
//         });

//         let result = signer.sign_transaction(transaction_data).await;

//         match result {
//             Err(SignerError::NotImplemented(msg)) => {
//                 assert_eq!(msg, "sign_transaction is not implemented".to_string());
//             }
//             _ => panic!("Expected SignerError::NotImplemented"),
//         }
//     }

//     #[tokio::test]
//     async fn test_pubkey_returns_correct_address() {
//         let mock_turnkey_service = MockVaultServiceTrait::new();
//         let base64_pubkey = "9zzYYGQM9prm/xXgn6Vwas/TVgteDaACCm1zW1ouKQs=";

//         let signer = TurnkeySigner::new_for_testing(
//             "test-key".to_string(),
//             base64_pubkey.to_string(),
//             mock_turnkey_service,
//         );

//         let result = signer.pubkey();
//         let result_address = signer.address().await;

//         // Assert
//         assert!(result.is_ok());
//         assert!(result_address.is_ok());
//         match result.unwrap() {
//             Address::Solana(pubkey) => {
//                 // The expected base58 encoded representation of the public key
//                 assert_eq!(pubkey, "He7WmJPCHfaJYHhMqK7QePfRT1JC5JC4UXxf3gnQhN3L");
//             }
//             _ => panic!("Expected Address::Solana variant"),
//         }
//         match result_address.unwrap() {
//             Address::Solana(pubkey) => {
//                 // The expected base58 encoded representation of the public key
//                 assert_eq!(pubkey, "He7WmJPCHfaJYHhMqK7QePfRT1JC5JC4UXxf3gnQhN3L");
//             }
//             _ => panic!("Expected Address::Solana variant"),
//         }
//     }
// }
