mod local_signer;

use async_trait::async_trait;
use local_signer::LocalSigner;

use crate::{
    domain::SignTransactionResponse,
    models::{
        Address, NetworkTransactionData, Signer as SignerDomainModel, SignerConfig, SignerError,
    },
    services::signer::{Signer, SignerFactoryError},
};

pub enum MidnightSigner {
    Local(LocalSigner),
}

impl MidnightSigner {
    pub fn viewing_key(&self) -> crate::services::sync::midnight::indexer::ViewingKeyFormat {
        match self {
            Self::Local(s) => s.viewing_key(),
        }
    }

    pub fn shielded_address(&self) -> &str {
        match self {
            Self::Local(s) => s.shielded_address(),
        }
    }

    pub fn dust_address(&self) -> &str {
        match self {
            Self::Local(s) => s.dust_address(),
        }
    }
}

#[async_trait]
impl Signer for MidnightSigner {
    async fn address(&self) -> Result<Address, SignerError> {
        match self {
            Self::Local(s) => s.address().await,
        }
    }

    async fn sign_transaction(
        &self,
        transaction: NetworkTransactionData,
    ) -> Result<SignTransactionResponse, SignerError> {
        match self {
            Self::Local(s) => s.sign_transaction(transaction).await,
        }
    }
}

pub struct MidnightSignerFactory;

impl MidnightSignerFactory {
    pub fn create_midnight_signer(
        signer_model: &SignerDomainModel,
        network_name: &str,
    ) -> Result<MidnightSigner, SignerFactoryError> {
        match &signer_model.config {
            SignerConfig::Local(_) => {
                let local_signer = LocalSigner::new(signer_model, network_name)?;
                Ok(MidnightSigner::Local(local_signer))
            }
            _ => Err(SignerFactoryError::UnsupportedType(
                "Midnight currently only supports Local signers".into(),
            )),
        }
    }
}
