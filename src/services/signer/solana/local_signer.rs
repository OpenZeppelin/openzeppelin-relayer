use async_trait::async_trait;
use log::info;
use solana_sdk::{
    instruction::Instruction,
    message::Message,
    signature::{Keypair, Signature},
    signer::{SeedDerivable, Signer as SolanaSigner},
    transaction::Transaction,
};

use crate::{
    domain::{
        SignDataRequest, SignDataResponse, SignDataResponseEvm, SignTransactionResponse,
        SignTypedDataRequest,
    },
    models::{Address, NetworkTransactionData, SignerError, SignerRepoModel, TransactionRepoModel},
    services::Signer,
};

use super::SolanaSignTrait;

pub struct LocalSigner {
    local_signer_client: Keypair,
}

impl LocalSigner {
    pub fn new(signer_model: &SignerRepoModel) -> Result<Self, SignerError> {
        let config = signer_model
            .config
            .get_local()
            .ok_or_else(|| SignerError::Configuration("Local config not found".to_string()))?;

        let keypair = Keypair::from_seed(&config.raw_key)
            .map_err(|e| SignerError::Configuration(format!("Failed to create signer: {}", e)))?;

        Ok(Self {
            local_signer_client: keypair,
        })
    }
}

#[async_trait]
impl SolanaSignTrait for LocalSigner {
    fn pubkey(&self) -> Result<Address, SignerError> {
        let address: Address = Address::Solana(self.local_signer_client.pubkey().to_string());
        Ok(address)
    }

    async fn sign(&self, message: &[u8]) -> Result<Signature, SignerError> {
        Ok(self.local_signer_client.sign_message(message))
    }
}

#[async_trait]
impl Signer for LocalSigner {
    async fn address(&self) -> Result<Address, SignerError> {
        let address: Address = Address::Solana(self.local_signer_client.pubkey().to_string());
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
    use crate::{
        models::{LocalSignerConfig, SignerConfig, SignerType, SolanaTransactionData},
        services::Signer,
    };

    use super::*;
    use solana_sdk::signature::Signer as SolanaSigner;

    // Returns a valid 32-byte seed (all bytes set to 1)
    fn valid_seed() -> Vec<u8> {
        vec![1u8; 32]
    }

    fn create_testing_signer() -> LocalSigner {
        let model = SignerRepoModel {
            id: "test".to_string(),
            config: SignerConfig::Local(LocalSignerConfig {
                raw_key: valid_seed(),
            }),
        };
        LocalSigner::new(&model).unwrap()
    }

    #[test]
    fn test_new_local_signer_success() {
        let local_signer = create_testing_signer();
        // Check that the keypair's public key is not the default (all zeros)
        let public_key = local_signer.local_signer_client.pubkey();
        assert_ne!(public_key.to_bytes(), [0u8; 32]);
    }

    #[test]
    fn test_new_local_signer_invalid_keypair() {
        let model = SignerRepoModel {
            id: "test".to_string(),
            config: SignerConfig::Local(LocalSignerConfig {
                raw_key: vec![1u8; 10],
            }),
        };
        let result = LocalSigner::new(&model);

        assert!(result.is_err());
        match result.err() {
            Some(SignerError::Configuration(msg)) => {
                assert!(msg.contains("Failed to create signer"));
            }
            _ => panic!("Expected SignerError::Configuration"),
        }
    }

    #[tokio::test]
    async fn test_sign_returns_valid_signature() {
        let local_signer = create_testing_signer();
        let message = b"Test message";

        let result = local_signer.sign(message).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_sign_different_messages() {
        let local_signer = create_testing_signer();

        let msg1 = b"Test message 1";
        let msg2 = b"Test message 2";

        let sig1 = local_signer.sign(msg1).await.unwrap();
        let sig2 = local_signer.sign(msg2).await.unwrap();

        assert_ne!(sig1, sig2);
    }

    #[test]
    fn test_pubkey_returns_correct_address() {
        let local_signer = create_testing_signer();

        let result = local_signer.pubkey();

        assert!(result.is_ok());

        let expected_pubkey = local_signer.local_signer_client.pubkey().to_string();

        match result.unwrap() {
            Address::Solana(pubkey) => {
                assert_eq!(pubkey, expected_pubkey);
            }
            _ => panic!("Expected Address::Solana variant"),
        }
    }

    #[test]
    fn test_pubkey_matches_keypair_pubkey() {
        let seed = valid_seed();
        let model = SignerRepoModel {
            id: "test".to_string(),
            config: SignerConfig::Local(LocalSignerConfig {
                raw_key: seed.clone(),
            }),
        };

        let local_signer = LocalSigner::new(&model).unwrap();

        let pubkey_result = local_signer.pubkey();
        assert!(pubkey_result.is_ok());

        let direct_keypair = Keypair::from_seed(&seed).expect("invalid keypair");
        let expected_pubkey = direct_keypair.pubkey().to_string();

        match pubkey_result.unwrap() {
            Address::Solana(pubkey) => {
                assert_eq!(pubkey, expected_pubkey);
            }
            _ => panic!("Expected Address::Solana variant"),
        }
    }

    #[tokio::test]
    async fn test_sign_transaction_not_implemented() {
        let local_signer = create_testing_signer();
        let transaction_data = NetworkTransactionData::Solana(SolanaTransactionData {
            fee_payer: "test".to_string(),
            hash: None,
            recent_blockhash: None,
            instructions: vec![],
        });

        let result = local_signer.sign_transaction(transaction_data).await;

        match result {
            Err(SignerError::NotImplemented(msg)) => {
                assert_eq!(msg, "sign_transaction is not implemented".to_string());
            }
            _ => panic!("Expected SignerError::NotImplemented"),
        }
    }
}
