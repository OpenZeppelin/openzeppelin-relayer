use std::str::FromStr;

use async_trait::async_trait;
use base64::Engine;
use log::{info, warn};
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
    models::{Address, NetworkTransactionData, SignerError, SignerRepoModel, TransactionRepoModel},
    services::Signer,
    utils::{base64_decode, base64_encode},
};

use super::SolanaSignTrait;

pub struct VaultTransitSigner {
    vault_signer_client: VaultClient,
    pubkey: String,
    key_name: String,
}

impl VaultTransitSigner {
    pub fn new(signer_model: &SignerRepoModel) -> Self {
        let config = signer_model
            .config
            .get_vault_transit()
            .expect("vault transit config not found");

        let settings = VaultClientSettingsBuilder::default()
            .address(config.address.clone())
            .token(config.token.clone())
            // .namespace("your-namespace")
            .build().expect("failed to build vault client settings");

        let client = VaultClient::new(settings).expect("failed to create vault client");

        Self {
            vault_signer_client: client,
            pubkey: config.pubkey.clone(),
            key_name: config.key_name.clone(),
        }
    }
}

#[async_trait]
impl SolanaSignTrait for VaultTransitSigner {
    fn pubkey(&self) -> Result<Address, SignerError> {
        let raw_pubkey =
            base64_decode(&self.pubkey).map_err(|e| SignerError::KeyError(e.to_string()))?;
        let pubkey = bs58::encode(&raw_pubkey).into_string();
        let address: Address = Address::Solana(pubkey);

        Ok(address)
    }

    async fn sign(&self, message: &[u8]) -> Result<Signature, SignerError> {
        let vault_signature = transit::data::sign(
            &self.vault_signer_client,
            "transit",
            &self.key_name,
            &base64_encode(message),
            None,
        )
        .await
        .map_err(|e| SignerError::SigningError(format!("Failed to sign with Vault: {}", e)))?;

        let vault_signature_str = &vault_signature.signature;
        let base64_sig = vault_signature_str
            .strip_prefix("vault:v1:")
            .unwrap_or(vault_signature_str);

        let sig_bytes = base64_decode(base64_sig)
            .map_err(|e| SignerError::SigningError(format!("Failed to decode signature: {}", e)))?;

        Ok(Signature::try_from(sig_bytes.as_slice()).map_err(|e| {
            SignerError::SigningError(format!("Failed to create signature from bytes: {}", e))
        })?)
    }
}

#[async_trait]
impl Signer for VaultTransitSigner {
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
        // TODO: not implemented
        Ok(SignTransactionResponse::Solana(vec![]))
    }
}

/// tests
///

#[cfg(test)]
mod tests {}
