use crate::models::{
    Relayer, RelayerError, RelayerEvmPolicy, RelayerSolanaPolicy, RelayerStellarPolicy,
};
use serde::{Deserialize, Serialize};

use super::{RelayerNetworkPolicy, RelayerNetworkType, RpcConfig};

// Use the domain model RelayerNetworkType directly
pub type NetworkType = RelayerNetworkType;

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    pub custom_rpc_urls: Option<Vec<RpcConfig>>,
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

impl From<Relayer> for RelayerRepoModel {
    fn from(relayer: Relayer) -> Self {
        Self {
            id: relayer.id,
            name: relayer.name,
            network: relayer.network,
            paused: relayer.paused,
            network_type: relayer.network_type,
            signer_id: relayer.signer_id,
            policies: relayer.policies.unwrap_or_else(|| {
                // Default policy based on network type
                match relayer.network_type {
                    RelayerNetworkType::Evm => {
                        RelayerNetworkPolicy::Evm(RelayerEvmPolicy::default())
                    }
                    RelayerNetworkType::Solana => {
                        RelayerNetworkPolicy::Solana(RelayerSolanaPolicy::default())
                    }
                    RelayerNetworkType::Stellar => {
                        RelayerNetworkPolicy::Stellar(RelayerStellarPolicy::default())
                    }
                }
            }),
            address: "".to_string(), // Will be filled in later by process_relayers
            notification_id: relayer.notification_id,
            system_disabled: false,
            custom_rpc_urls: relayer.custom_rpc_urls,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::models::RelayerEvmPolicy;

    use super::*;

    fn create_test_relayer(paused: bool, system_disabled: bool) -> RelayerRepoModel {
        RelayerRepoModel {
            id: "test_relayer".to_string(),
            name: "Test Relayer".to_string(),
            paused,
            system_disabled,
            network: "test_network".to_string(),
            network_type: NetworkType::Evm,
            signer_id: "test_signer".to_string(),
            policies: RelayerNetworkPolicy::Evm(RelayerEvmPolicy::default()),
            address: "0xtest".to_string(),
            notification_id: None,
            custom_rpc_urls: None,
        }
    }

    #[test]
    fn test_validate_active_state_success() {
        let relayer = create_test_relayer(false, false);
        assert!(relayer.validate_active_state().is_ok());
    }

    #[test]
    fn test_validate_active_state_paused() {
        let relayer = create_test_relayer(true, false);
        let result = relayer.validate_active_state();
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), RelayerError::RelayerPaused));
    }

    #[test]
    fn test_validate_active_state_disabled() {
        let relayer = create_test_relayer(false, true);
        let result = relayer.validate_active_state();
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), RelayerError::RelayerDisabled));
    }
}
