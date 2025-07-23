use crate::models::{
    Relayer, RelayerError, RelayerEvmPolicy, RelayerSolanaPolicy, RelayerStellarPolicy,
};
use serde::{Deserialize, Serialize};

use super::{RelayerNetworkPolicy, RelayerNetworkType, RpcConfig};

// Use the domain model RelayerNetworkType directly
pub type NetworkType = RelayerNetworkType;

/// Helper for safely updating relayer repository models from domain models
/// while preserving runtime fields like address and system_disabled
pub struct RelayerRepoUpdater {
    original: RelayerRepoModel,
}

impl RelayerRepoUpdater {
    /// Create an updater from an existing repository model
    pub fn from_existing(existing: RelayerRepoModel) -> Self {
        Self { original: existing }
    }

    /// Apply updates from a domain model while preserving runtime fields
    ///
    /// This method ensures that runtime fields (address, system_disabled) from the
    /// original repository model are preserved when converting from domain model,
    /// preventing data loss during updates.
    pub fn apply_domain_update(self, domain: Relayer) -> RelayerRepoModel {
        let mut updated = RelayerRepoModel::from(domain);
        // Preserve runtime fields from original
        updated.address = self.original.address;
        updated.system_disabled = self.original.system_disabled;
        updated
    }
}

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

impl From<RelayerRepoModel> for Relayer {
    fn from(repo_model: RelayerRepoModel) -> Self {
        Self {
            id: repo_model.id,
            name: repo_model.name,
            network: repo_model.network,
            paused: repo_model.paused,
            network_type: repo_model.network_type,
            policies: Some(repo_model.policies),
            signer_id: repo_model.signer_id,
            notification_id: repo_model.notification_id,
            custom_rpc_urls: repo_model.custom_rpc_urls,
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

    #[test]
    fn test_relayer_repo_updater_preserves_runtime_fields() {
        // Create an original relayer with runtime fields set
        let original = RelayerRepoModel {
            id: "test_relayer".to_string(),
            name: "Original Name".to_string(),
            address: "0x742d35Cc6634C0532925a3b8D8C2e48a73F6ba2E".to_string(), // Runtime field
            system_disabled: true,                                             // Runtime field
            paused: false,
            network: "mainnet".to_string(),
            network_type: NetworkType::Evm,
            signer_id: "test_signer".to_string(),
            policies: RelayerNetworkPolicy::Evm(RelayerEvmPolicy::default()),
            notification_id: None,
            custom_rpc_urls: None,
        };

        // Create a domain model with different business fields
        let domain_update = Relayer {
            id: "test_relayer".to_string(),
            name: "Updated Name".to_string(), // Changed
            paused: true,                     // Changed
            network: "mainnet".to_string(),
            network_type: RelayerNetworkType::Evm,
            signer_id: "test_signer".to_string(),
            policies: Some(RelayerNetworkPolicy::Evm(RelayerEvmPolicy::default())),
            notification_id: Some("new_notification".to_string()), // Changed
            custom_rpc_urls: None,
        };

        // Use updater to preserve runtime fields
        let updated =
            RelayerRepoUpdater::from_existing(original.clone()).apply_domain_update(domain_update);

        // Verify business fields were updated
        assert_eq!(updated.name, "Updated Name");
        assert!(updated.paused);
        assert_eq!(
            updated.notification_id,
            Some("new_notification".to_string())
        );

        // Verify runtime fields were preserved
        assert_eq!(
            updated.address,
            "0x742d35Cc6634C0532925a3b8D8C2e48a73F6ba2E"
        );
        assert!(updated.system_disabled);
    }
}
