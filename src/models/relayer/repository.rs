use serde::{Deserialize, Serialize};
use strum::Display;

use crate::models::RelayerError;

#[derive(Debug, Clone, Serialize, PartialEq, Display, Deserialize)]
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

#[derive(Debug, Serialize, Clone)]
pub struct RelayerEvmPolicy {
    pub gas_price_cap: Option<u64>,
    pub whitelist_receivers: Option<Vec<String>>,
    pub eip1559_pricing: Option<bool>,
    pub private_transactions: Option<bool>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct RelayerSolanaPolicy {
    pub max_retries: Option<u32>,
    pub confirmation_blocks: Option<u64>,
    pub timeout_seconds: Option<u64>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct RelayerStellarPolicy {
    pub max_fee: Option<u32>,
    pub timeout_seconds: Option<u64>,
    pub min_account_balance: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RelayerRepoModel {
    pub id: String,
    pub name: String,
    pub network: String,
    pub paused: bool,
    pub network_type: NetworkType,
    pub signer_id: String,
    pub policies: Option<RelayerNetworkPolicy>,
    pub address: Option<String>,
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
            policies: None,
            signer_id: "test_signer".to_string(),
            address: None,
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
