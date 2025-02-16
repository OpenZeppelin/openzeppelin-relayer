use serde::{Deserialize, Serialize};
use strum::Display;

use crate::{
    constants::{
        DEFAULT_EVM_MIN_BALANCE, DEFAULT_SOLANA_MIN_BALANCE, DEFAULT_STELLAR_MIN_BALANCE,
        MAX_SOLANA_TX_DATA_SIZE,
    },
    models::RelayerError,
};

#[derive(Debug, Clone, Serialize, PartialEq, Display, Deserialize, Copy)]
#[serde(rename_all = "lowercase")]
pub enum NetworkType {
    Evm,
    Stellar,
    Solana,
}

#[derive(Debug, Serialize, Clone)]
pub enum RelayerNetworkPolicy {
    Evm(RelayerEvmPolicy),
    Solana(RelayerSolanaPolicy),
    Stellar(RelayerStellarPolicy),
}

impl RelayerNetworkPolicy {
    pub fn get_evm_policy(&self) -> RelayerEvmPolicy {
        match self {
            Self::Evm(policy) => policy.clone(),
            _ => RelayerEvmPolicy::default(),
        }
    }

    pub fn get_solana_policy(&self) -> RelayerSolanaPolicy {
        match self {
            Self::Solana(policy) => policy.clone(),
            _ => RelayerSolanaPolicy::default(),
        }
    }

    pub fn get_stellar_policy(&self) -> RelayerStellarPolicy {
        match self {
            Self::Stellar(policy) => policy.clone(),
            _ => RelayerStellarPolicy::default(),
        }
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct RelayerEvmPolicy {
    pub gas_price_cap: Option<u64>,
    pub whitelist_receivers: Option<Vec<String>>,
    pub eip1559_pricing: bool,
    pub private_transactions: bool,
    pub min_balance: u128,
}

impl Default for RelayerEvmPolicy {
    fn default() -> Self {
        Self {
            gas_price_cap: None,
            whitelist_receivers: None,
            eip1559_pricing: false,
            private_transactions: false,
            min_balance: DEFAULT_EVM_MIN_BALANCE,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct SolanaAllowedTokensPolicy {
    pub mint: String,
    pub decimals: Option<u8>,
    pub symbol: Option<String>,
}

impl SolanaAllowedTokensPolicy {
    pub fn new(mint: String, decimals: Option<u8>, symbol: Option<String>) -> Self {
        Self {
            mint,
            decimals,
            symbol,
        }
    }

    // Create a new SolanaAllowedTokensPolicy with only the mint field
    // We are creating partial entry while processing config file and later
    // we will fill the rest of the fields
    pub fn new_partial(mint: String) -> Self {
        Self {
            mint,
            decimals: None,
            symbol: None,
        }
    }
}

#[derive(Debug, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct RelayerSolanaPolicy {
    pub min_balance: u64,
    pub allowed_tokens: Option<Vec<SolanaAllowedTokensPolicy>>,
    pub allowed_programs: Option<Vec<String>>,
    pub allowed_accounts: Option<Vec<String>>,
    pub disallowed_accounts: Option<Vec<String>>,
    pub max_supported_token_fee: Option<u64>,
    pub max_signatures: Option<u8>,
    pub max_tx_data_size: u16,
}

impl Default for RelayerSolanaPolicy {
    fn default() -> Self {
        Self {
            min_balance: DEFAULT_SOLANA_MIN_BALANCE,
            allowed_tokens: None,
            allowed_programs: None,
            allowed_accounts: None,
            disallowed_accounts: None,
            max_supported_token_fee: None,
            max_signatures: None,
            max_tx_data_size: MAX_SOLANA_TX_DATA_SIZE,
        }
    }
}

#[derive(Debug, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct RelayerStellarPolicy {
    pub max_fee: Option<u32>,
    pub timeout_seconds: Option<u64>,
    pub min_balance: u64,
}

impl Default for RelayerStellarPolicy {
    fn default() -> Self {
        Self {
            max_fee: None,
            timeout_seconds: None,
            min_balance: DEFAULT_STELLAR_MIN_BALANCE,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RelayerRepoModel {
    pub id: String,
    pub name: String,
    pub network: String,
    pub paused: bool,
    pub network_type: NetworkType,
    pub signer_id: String,
    pub policies: RelayerNetworkPolicy,
    pub address: String,
    pub notification_id: Option<String>,
    pub system_disabled: bool,
}

impl RelayerRepoModel {
    pub fn validate_active_state(&self) -> Result<(), RelayerError> {
        if self.paused {
            return Err(RelayerError::RelayerPaused);
        }

        if self.system_disabled {
            return Err(RelayerError::RelayerDisabled);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_relayer(paused: bool, system_disabled: bool) -> RelayerRepoModel {
        RelayerRepoModel {
            id: "test_relayer".to_string(),
            name: "Test Relayer".to_string(),
            paused,
            system_disabled,
            network: "test_network".to_string(),
            network_type: NetworkType::Evm,
            policies: RelayerNetworkPolicy::Evm(RelayerEvmPolicy::default()),
            signer_id: "test_signer".to_string(),
            address: "0x".to_string(),
            notification_id: None,
        }
    }

    #[test]
    fn test_validate_active_state_active() {
        let relayer = create_test_relayer(false, false);
        assert!(relayer.validate_active_state().is_ok());
    }

    #[test]
    fn test_validate_active_state_paused() {
        let relayer = create_test_relayer(true, false);
        let result = relayer.validate_active_state();
        assert!(matches!(result, Err(RelayerError::RelayerPaused)));
    }

    #[test]
    fn test_validate_active_state_disabled() {
        let relayer = create_test_relayer(false, true);
        let result = relayer.validate_active_state();
        assert!(matches!(result, Err(RelayerError::RelayerDisabled)));
    }

    #[test]
    fn test_validate_active_state_paused_and_disabled() {
        let relayer = create_test_relayer(true, true);
        let result = relayer.validate_active_state();
        assert!(matches!(result, Err(RelayerError::RelayerPaused)));
    }
}
