// openzeppelin-relayer/src/services/signer/midnight/mod.rs
//! Midnight signer implementation (local keystore)

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

/// Trait for Midnight-specific signer functionality
pub trait MidnightSignerTrait: Signer {
    /// Get a reference to the wallet seed
    fn wallet_seed(&self) -> &midnight_node_ledger_helpers::WalletSeed;
}

pub enum MidnightSigner {
    Local(LocalSigner),
    Vault(LocalSigner),
    VaultCloud(LocalSigner),
}

#[async_trait]
impl Signer for MidnightSigner {
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

impl MidnightSignerTrait for MidnightSigner {
    fn wallet_seed(&self) -> &midnight_node_ledger_helpers::WalletSeed {
        match self {
            Self::Local(s) | Self::Vault(s) | Self::VaultCloud(s) => s.wallet_seed(),
        }
    }
}

pub struct MidnightSignerFactory;

impl MidnightSignerFactory {
    pub fn create_midnight_signer(
        m: &SignerRepoModel,
    ) -> Result<MidnightSigner, SignerFactoryError> {
        let signer = match m.config {
            SignerConfig::Local(_)
            | SignerConfig::Test(_)
            | SignerConfig::Vault(_)
            | SignerConfig::VaultCloud(_) => MidnightSigner::Local(LocalSigner::new(m)?),
            SignerConfig::AwsKms(_) => {
                return Err(SignerFactoryError::UnsupportedType("AWS KMS".into()))
            }
            SignerConfig::VaultTransit(_) => {
                return Err(SignerFactoryError::UnsupportedType("Vault Transit".into()))
            }
            SignerConfig::Turnkey(_) => {
                return Err(SignerFactoryError::UnsupportedType("Turnkey".into()))
            }
            SignerConfig::GoogleCloudKms(_) => {
                return Err(SignerFactoryError::UnsupportedType(
                    "Google Cloud KMS".into(),
                ))
            }
        };
        Ok(signer)
    }
}
