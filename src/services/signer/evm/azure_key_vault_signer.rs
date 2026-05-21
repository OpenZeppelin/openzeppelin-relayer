//! Azure Key Vault signer for EVM transactions and data.

use alloy::{
    consensus::{SignableTransaction, TxEip1559, TxLegacy},
    primitives::{utils::eip191_message, Signature},
};
use async_trait::async_trait;

use crate::{
    domain::{
        SignDataRequest, SignDataResponse, SignTransactionResponse, SignTransactionResponseEvm,
        SignTypedDataRequest,
    },
    models::{
        Address, EvmTransactionDataSignature, EvmTransactionDataTrait, NetworkTransactionData,
        SignerError,
    },
    services::{
        signer::{
            evm::{construct_eip712_message_hash, validate_and_format_signature},
            DataSignerTrait, Signer,
        },
        AzureKeyVaultEvmService, AzureKeyVaultService,
    },
};

pub struct AzureKeyVaultSigner {
    azure_key_vault_service: AzureKeyVaultService,
}

impl AzureKeyVaultSigner {
    pub fn new(azure_key_vault_service: AzureKeyVaultService) -> Self {
        Self {
            azure_key_vault_service,
        }
    }
}

#[async_trait]
impl Signer for AzureKeyVaultSigner {
    async fn address(&self) -> Result<Address, SignerError> {
        self.azure_key_vault_service
            .get_evm_address()
            .await
            .map_err(|e| SignerError::SigningError(e.to_string()))
    }

    async fn sign_transaction(
        &self,
        transaction: NetworkTransactionData,
    ) -> Result<SignTransactionResponse, SignerError> {
        let evm_data = transaction.get_evm_transaction_data()?;

        if evm_data.is_eip1559() {
            let unsigned_tx = TxEip1559::try_from(transaction)?;
            let payload = unsigned_tx.encoded_for_signing();
            let signed_bytes = self
                .azure_key_vault_service
                .sign_payload_evm(&payload)
                .await
                .map_err(|e| SignerError::SigningError(e.to_string()))?;

            if signed_bytes.len() != 65 {
                return Err(SignerError::SigningError(format!(
                    "Invalid signature length from Azure Key Vault: expected 65 bytes, got {}",
                    signed_bytes.len()
                )));
            }

            let signature = Signature::from_raw(&signed_bytes)
                .map_err(|e| SignerError::ConversionError(e.to_string()))?;

            let mut signature_bytes = signature.as_bytes();
            let signed_tx = unsigned_tx.into_signed(signature);

            if signature_bytes[64] == 27 {
                signature_bytes[64] = 0;
            } else if signature_bytes[64] == 28 {
                signature_bytes[64] = 1;
            }

            let mut raw = Vec::with_capacity(signed_tx.eip2718_encoded_length());
            signed_tx.eip2718_encode(&mut raw);

            Ok(SignTransactionResponse::Evm(SignTransactionResponseEvm {
                hash: signed_tx.hash().to_string(),
                signature: EvmTransactionDataSignature::from(&signature_bytes),
                raw,
            }))
        } else {
            let unsigned_tx = TxLegacy::try_from(transaction)?;
            let payload = unsigned_tx.encoded_for_signing();
            let signed_bytes = self
                .azure_key_vault_service
                .sign_payload_evm(&payload)
                .await
                .map_err(|e| SignerError::SigningError(e.to_string()))?;

            if signed_bytes.len() != 65 {
                return Err(SignerError::SigningError(format!(
                    "Invalid signature length from Azure Key Vault: expected 65 bytes, got {}",
                    signed_bytes.len()
                )));
            }

            let signature = Signature::from_raw(&signed_bytes)
                .map_err(|e| SignerError::ConversionError(e.to_string()))?;

            let signature_bytes = signature.as_bytes();
            let signed_tx = unsigned_tx.into_signed(signature);

            let mut raw = Vec::with_capacity(signed_tx.rlp_encoded_length());
            signed_tx.rlp_encode(&mut raw);

            Ok(SignTransactionResponse::Evm(SignTransactionResponseEvm {
                hash: signed_tx.hash().to_string(),
                signature: EvmTransactionDataSignature::from(&signature_bytes),
                raw,
            }))
        }
    }
}

#[async_trait]
impl DataSignerTrait for AzureKeyVaultSigner {
    async fn sign_data(&self, request: SignDataRequest) -> Result<SignDataResponse, SignerError> {
        let eip191_message = eip191_message(&request.message);
        let signature_bytes = self
            .azure_key_vault_service
            .sign_payload_evm(&eip191_message)
            .await
            .map_err(|e| SignerError::SigningError(e.to_string()))?;

        validate_and_format_signature(&signature_bytes, "Azure Key Vault")
    }

    async fn sign_typed_data(
        &self,
        request: SignTypedDataRequest,
    ) -> Result<SignDataResponse, SignerError> {
        let message_hash = construct_eip712_message_hash(&request)?;
        let signature_bytes = self
            .azure_key_vault_service
            .sign_hash_evm(&message_hash)
            .await
            .map_err(|e| SignerError::SigningError(e.to_string()))?;

        validate_and_format_signature(&signature_bytes, "Azure Key Vault")
    }
}
