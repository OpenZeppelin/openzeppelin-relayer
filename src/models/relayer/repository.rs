use serde::{Deserialize, Serialize};
use strum::Display;
use utoipa::ToSchema;

use crate::{
    constants::{
        DEFAULT_CONVERSION_SLIPPAGE_PERCENTAGE, DEFAULT_EVM_MIN_BALANCE,
        DEFAULT_SOLANA_MIN_BALANCE, DEFAULT_STELLAR_MIN_BALANCE, MAX_SOLANA_TX_DATA_SIZE,
    },
    models::RelayerError,
};

#[derive(Debug, Clone, Serialize, PartialEq, Display, Deserialize, Copy, ToSchema)]
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
    pub gas_price_cap: Option<u128>,
    pub whitelist_receivers: Option<Vec<String>>,
    pub eip1559_pricing: Option<bool>,
    pub private_transactions: bool,
    pub min_balance: u128,
}

impl Default for RelayerEvmPolicy {
    fn default() -> Self {
        Self {
            gas_price_cap: None,
            whitelist_receivers: None,
            eip1559_pricing: None,
            private_transactions: false,
            min_balance: DEFAULT_EVM_MIN_BALANCE,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, ToSchema)]
pub struct SolanaAllowedTokensPolicy {
    pub mint: String,
    #[schema(nullable = false)]
    pub decimals: Option<u8>,
    #[schema(nullable = false)]
    pub symbol: Option<String>,
    #[schema(nullable = false)]
    pub max_allowed_fee: Option<u64>,
    #[schema(nullable = false)]
    pub conversion_slippage_percentage: Option<f32>,
}

impl SolanaAllowedTokensPolicy {
    pub fn new(
        mint: String,
        decimals: Option<u8>,
        symbol: Option<String>,
        max_allowed_fee: Option<u64>,
        conversion_slippage_percentage: Option<f32>,
    ) -> Self {
        Self {
            mint,
            decimals,
            symbol,
            max_allowed_fee,
            conversion_slippage_percentage,
        }
    }

    // Create a new SolanaAllowedTokensPolicy with only the mint field
    // We are creating partial entry while processing config file and later
    // we will fill the rest of the fields
    pub fn new_partial(
        mint: String,
        max_allowed_fee: Option<u64>,
        conversion_slippage_percentage: Option<f32>,
    ) -> Self {
        Self {
            mint,
            decimals: None,
            symbol: None,
            max_allowed_fee,
            conversion_slippage_percentage,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum SolanaFeePaymentStrategy {
    User,
    Relayer,
}

#[derive(Debug, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct RelayerSolanaPolicy {
    pub fee_payment_strategy: SolanaFeePaymentStrategy,
    pub fee_margin_percentage: Option<f32>,
    pub min_balance: u64,
    pub allowed_tokens: Option<Vec<SolanaAllowedTokensPolicy>>,
    pub allowed_programs: Option<Vec<String>>,
    pub allowed_accounts: Option<Vec<String>>,
    pub disallowed_accounts: Option<Vec<String>>,
    pub max_signatures: Option<u8>,
    pub max_tx_data_size: u16,
    pub max_allowed_fee_lamports: Option<u64>,
}

impl RelayerSolanaPolicy {
    pub fn get_allowed_tokens(&self) -> Vec<SolanaAllowedTokensPolicy> {
        self.allowed_tokens.clone().unwrap_or_default()
    }

    pub fn get_allowed_token_entry(&self, mint: &str) -> Option<SolanaAllowedTokensPolicy> {
        self.allowed_tokens
            .clone()
            .unwrap_or_default()
            .into_iter()
            .find(|entry| entry.mint == mint)
    }

    pub fn get_allowed_token_decimals(&self, mint: &str) -> Option<u8> {
        self.get_allowed_token_entry(mint)
            .and_then(|entry| entry.decimals)
    }

    pub fn get_allowed_token_slippage(&self, mint: &str) -> f32 {
        self.get_allowed_token_entry(mint)
            .and_then(|entry| entry.conversion_slippage_percentage)
            .unwrap_or(DEFAULT_CONVERSION_SLIPPAGE_PERCENTAGE)
    }

    pub fn get_allowed_programs(&self) -> Vec<String> {
        self.allowed_programs.clone().unwrap_or_default()
    }

    pub fn get_allowed_accounts(&self) -> Vec<String> {
        self.allowed_accounts.clone().unwrap_or_default()
    }

    pub fn get_disallowed_accounts(&self) -> Vec<String> {
        self.disallowed_accounts.clone().unwrap_or_default()
    }

    pub fn get_max_signatures(&self) -> u8 {
        self.max_signatures.unwrap_or(1)
    }

    pub fn get_max_allowed_fee_lamports(&self) -> u64 {
        self.max_allowed_fee_lamports.unwrap_or(u64::MAX)
    }

    pub fn get_max_tx_data_size(&self) -> u16 {
        self.max_tx_data_size
    }

    pub fn get_fee_margin_percentage(&self) -> f32 {
        self.fee_margin_percentage.unwrap_or(0.0)
    }

    pub fn get_fee_payment_strategy(&self) -> SolanaFeePaymentStrategy {
        self.fee_payment_strategy.clone()
    }
}

impl Default for RelayerSolanaPolicy {
    fn default() -> Self {
        Self {
            fee_payment_strategy: SolanaFeePaymentStrategy::User,
            fee_margin_percentage: None,
            min_balance: DEFAULT_SOLANA_MIN_BALANCE,
            allowed_tokens: None,
            allowed_programs: None,
            allowed_accounts: None,
            disallowed_accounts: None,
            max_signatures: None,
            max_tx_data_size: MAX_SOLANA_TX_DATA_SIZE,
            max_allowed_fee_lamports: None,
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
    pub custom_rpc_urls: Option<Vec<String>>,
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

impl Default for RelayerRepoModel {
    fn default() -> Self {
        Self {
            id: "".to_string(),
            name: "".to_string(),
            network: "".to_string(),
            paused: false,
            network_type: NetworkType::Evm,
            signer_id: "".to_string(),
            policies: RelayerNetworkPolicy::Evm(RelayerEvmPolicy::default()),
            address: "0x".to_string(),
            notification_id: None,
            system_disabled: false,
            custom_rpc_urls: None,
        }
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
            custom_rpc_urls: None,
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
