// openzeppelin-relayer/src/services/signer/stellar/mod.rs
//! Stellar signer implementation (local keystore)

mod local_signer;
use async_trait::async_trait;
use local_signer::*;

use crate::{
    domain::{SignDataRequest, SignDataResponse, SignTransactionResponse, SignTypedDataRequest},
    models::{Address, NetworkTransactionData, SignerConfig, SignerRepoModel},
    services::signer::{SignerError, SignerFactoryError},
    services::Signer,
};

use super::DataSignerTrait;

pub enum StellarSigner {
    Local(LocalSigner),
    Vault(LocalSigner),
    VaultCloud(LocalSigner),
}

#[async_trait]
impl Signer for StellarSigner {
    async fn address(&self) -> Result<Address, SignerError> {
        match self {
            Self::Local(s) | Self::Vault(s) | Self::VaultCloud(s) => s.address().await,
        }
    }

    async fn sign_transaction(
        &self,
        tx: NetworkTransactionData,
    ) -> Result<SignTransactionResponse, SignerError> {
        match self {
            Self::Local(s) | Self::Vault(s) | Self::VaultCloud(s) => s.sign_transaction(tx).await,
        }
    }
}

pub struct StellarSignerFactory;

impl StellarSignerFactory {
    pub fn create_stellar_signer(m: &SignerRepoModel) -> Result<StellarSigner, SignerFactoryError> {
        let signer = match m.config {
            SignerConfig::Local(_)
            | SignerConfig::Test(_)
            | SignerConfig::Vault(_)
            | SignerConfig::VaultCloud(_) => StellarSigner::Local(LocalSigner::new(m)?),
            SignerConfig::AwsKms(_) => {
                return Err(SignerFactoryError::UnsupportedType("AWS KMS".into()))
            }
            SignerConfig::VaultTransit(_) => {
                return Err(SignerFactoryError::UnsupportedType("Vault Transit".into()))
            }
            SignerConfig::Turnkey(_) => {
                return Err(SignerFactoryError::UnsupportedType("Turnkey".into()))
            }
        };
        Ok(signer)
    }
}
