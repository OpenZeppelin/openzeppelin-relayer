use alloy::{
    consensus::{SignableTransaction, TxLegacy},
    network::{EthereumWallet, TransactionBuilder, TxSigner},
    rpc::types::Transaction,
    signers::{
        k256::ecdsa::SigningKey, local::LocalSigner as AlloyLocalSignerClient,
        Signer as AlloySigner, SignerSync,
    },
};

use alloy::primitives::{address, Address as AlloyAddress, FixedBytes, U256};

use async_trait::async_trait;

use crate::{
    domain::{
        SignDataRequest, SignDataResponse, SignDataResponseEvm, SignTransactionResponse,
        SignTransactionResponseEvm, SignTypedDataRequest,
    },
    models::{
        Address, EvmTransactionDataSignature, SignerError, SignerRepoModel, TransactionRepoModel,
    },
    services::Signer,
};

use super::DataSignerTrait;

use alloy::rpc::types::TransactionRequest;

pub struct LocalSigner {
    local_signer_client: AlloyLocalSignerClient<SigningKey>,
}

impl LocalSigner {
    pub fn new(signer_model: &SignerRepoModel) -> Self {
        let raw_key = signer_model.raw_key.as_ref().expect("keystore not found");

        // transforms the key into alloy wallet
        let key_bytes = FixedBytes::from_slice(raw_key);
        let local_signer_client =
            AlloyLocalSignerClient::from_bytes(&key_bytes).expect("failed to create signer");

        Self {
            local_signer_client,
        }
    }
}

impl From<AlloyAddress> for Address {
    fn from(addr: AlloyAddress) -> Self {
        Address::Evm(addr.into_array())
    }
}

#[async_trait]
impl Signer for LocalSigner {
    async fn address(&self) -> Result<Address, SignerError> {
        let address: Address = self.local_signer_client.address().into();
        Ok(address)
    }

    async fn sign_transaction(
        &self,
        transaction: TransactionRepoModel,
    ) -> Result<SignTransactionResponse, SignerError> {
        let mut unsigned_tx = TxLegacy::try_from(transaction)?;

        let signature = self
            .local_signer_client
            .sign_transaction(&mut unsigned_tx)
            .await
            .map_err(|e| SignerError::SigningError(format!("Failed to sign transaction: {e}")))?;

        let signed_tx = unsigned_tx.into_signed(signature);
        let signature_bytes = signature.as_bytes();

        let mut raw = Vec::new();
        signed_tx.rlp_encode(&mut raw);

        Ok(SignTransactionResponse::Evm(SignTransactionResponseEvm {
            hash: signed_tx.hash().to_string(),
            signature: EvmTransactionDataSignature::from(&signature_bytes),
            raw,
        }))
    }
}

#[async_trait]
impl DataSignerTrait for LocalSigner {
    async fn sign_data(&self, request: SignDataRequest) -> Result<SignDataResponse, SignerError> {
        let message = request.message.as_bytes();

        let signature = self
            .local_signer_client
            .sign_message(message)
            .await
            .map_err(|e| SignerError::SigningError(format!("Failed to sign message: {}", e)))?;

        let ste = signature.as_bytes();

        Ok(SignDataResponse::Evm(SignDataResponseEvm {
            r: hex::encode(&ste[0..32]),
            s: hex::encode(&ste[32..64]),
            v: ste[64],
            sig: hex::encode(ste),
        }))
    }

    async fn sign_typed_data(
        &self,
        _typed_data: SignTypedDataRequest,
    ) -> Result<SignDataResponse, SignerError> {
        todo!()
    }
}
