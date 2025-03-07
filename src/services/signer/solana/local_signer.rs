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
    pub fn new(signer_model: &SignerRepoModel) -> Self {
        let config = signer_model
            .config
            .get_local()
            .expect("local config not found");
        let raw_key = config.raw_key.as_ref().expect("keystore not found");

        let keypair = Keypair::from_seed(raw_key).expect("invalid keypair");

        Self {
            local_signer_client: keypair,
        }
    }
}

impl SolanaSignTrait for LocalSigner {
    fn pubkey(&self) -> Result<Address, SignerError> {
        let address: Address = Address::Solana(self.local_signer_client.pubkey().to_string());
        Ok(address)
    }

    fn sign(&self, message: &[u8]) -> Result<Signature, SignerError> {
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
        // TODO: not implemented
        Ok(SignTransactionResponse::Solana(vec![]))
    }
}

#[cfg(test)]
mod tests {
    use crate::models::{LocalSignerConfig, SignerConfig, SignerType};

    use super::*;
    use solana_sdk::signature::Signer; // For Keypair::pubkey()

    // Returns a valid 32-byte seed (all bytes set to 1)
    fn valid_seed() -> Vec<u8> {
        vec![1u8; 32]
    }

    fn create_testing_signer() -> LocalSigner {
        let model = SignerRepoModel {
            id: "test".to_string(),
            config: SignerConfig::Local(LocalSignerConfig {
                raw_key: Some(valid_seed()),
            }),
        };
        LocalSigner::new(&model)
    }

    #[test]
    fn test_new_local_signer_success() {
        let local_signer = create_testing_signer();
        // Check that the keypair's public key is not the default (all zeros)
        let public_key = local_signer.local_signer_client.pubkey();
        assert_ne!(public_key.to_bytes(), [0u8; 32]);
    }

    #[test]
    #[should_panic(expected = "keystore not found")]
    fn test_new_local_signer_missing_keystore() {
        let model = SignerRepoModel {
            id: "test".to_string(),
            config: SignerConfig::Local(LocalSignerConfig { raw_key: None }),
        };
        LocalSigner::new(&model);
    }

    #[test]
    #[should_panic(expected = "invalid keypair")]
    fn test_new_local_signer_invalid_keypair() {
        let model = SignerRepoModel {
            id: "test".to_string(),
            config: SignerConfig::Local(LocalSignerConfig {
                raw_key: Some(vec![1u8; 10]),
            }),
        };
        let _ = LocalSigner::new(&model);
    }

    #[test]
    fn test_sign_returns_valid_signature() {
        let local_signer = create_testing_signer();
        let message = b"Test message";

        let result = local_signer.sign(message);
        assert!(result.is_ok());
    }

    #[test]
    fn test_sign_different_messages() {
        let local_signer = create_testing_signer();

        let msg1 = b"Test message 1";
        let msg2 = b"Test message 2";

        let sig1 = local_signer.sign(msg1).unwrap();
        let sig2 = local_signer.sign(msg2).unwrap();

        assert_ne!(sig1, sig2);
    }
}
