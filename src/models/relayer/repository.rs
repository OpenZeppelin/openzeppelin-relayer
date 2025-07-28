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
    use crate::models::{
        RelayerEvmPolicy, RelayerSolanaPolicy, RelayerStellarPolicy, SolanaAllowedTokensPolicy,
        SolanaFeePaymentStrategy,
    };

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

    fn create_test_relayer_solana(paused: bool, system_disabled: bool) -> RelayerRepoModel {
        RelayerRepoModel {
            id: "test_solana_relayer".to_string(),
            name: "Test Solana Relayer".to_string(),
            paused,
            system_disabled,
            network: "mainnet".to_string(),
            network_type: NetworkType::Solana,
            signer_id: "test_signer".to_string(),
            policies: RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
                fee_payment_strategy: Some(SolanaFeePaymentStrategy::Relayer),
                min_balance: Some(1000000),
                max_signatures: Some(5),
                allowed_tokens: None,
                allowed_programs: None,
                allowed_accounts: None,
                disallowed_accounts: None,
                max_tx_data_size: None,
                max_allowed_fee_lamports: None,
                swap_config: None,
                fee_margin_percentage: None,
            }),
            address: "SolanaAddress123".to_string(),
            notification_id: None,
            custom_rpc_urls: None,
        }
    }

    fn create_test_relayer_stellar(paused: bool, system_disabled: bool) -> RelayerRepoModel {
        RelayerRepoModel {
            id: "test_stellar_relayer".to_string(),
            name: "Test Stellar Relayer".to_string(),
            paused,
            system_disabled,
            network: "mainnet".to_string(),
            network_type: NetworkType::Stellar,
            signer_id: "test_signer".to_string(),
            policies: RelayerNetworkPolicy::Stellar(RelayerStellarPolicy {
                min_balance: Some(20000000),
                max_fee: Some(100000),
                timeout_seconds: Some(30),
            }),
            address: "GXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX".to_string(),
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
    fn test_validate_active_state_success_solana() {
        let relayer = create_test_relayer_solana(false, false);
        assert!(relayer.validate_active_state().is_ok());
    }

    #[test]
    fn test_validate_active_state_success_stellar() {
        let relayer = create_test_relayer_stellar(false, false);
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
    fn test_validate_active_state_paused_solana() {
        let relayer = create_test_relayer_solana(true, false);
        let result = relayer.validate_active_state();
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), RelayerError::RelayerPaused));
    }

    #[test]
    fn test_validate_active_state_paused_stellar() {
        let relayer = create_test_relayer_stellar(true, false);
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
    fn test_validate_active_state_disabled_solana() {
        let relayer = create_test_relayer_solana(false, true);
        let result = relayer.validate_active_state();
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), RelayerError::RelayerDisabled));
    }

    #[test]
    fn test_validate_active_state_disabled_stellar() {
        let relayer = create_test_relayer_stellar(false, true);
        let result = relayer.validate_active_state();
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), RelayerError::RelayerDisabled));
    }

    #[test]
    fn test_validate_active_state_both_paused_and_disabled() {
        // When both are true, should return paused error (checked first)
        let relayer = create_test_relayer(true, true);
        let result = relayer.validate_active_state();
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), RelayerError::RelayerPaused));
    }

    #[test]
    fn test_conversion_from_repo_model_to_domain_evm() {
        let repo_model = create_test_relayer(false, false);
        let domain_relayer = Relayer::from(repo_model.clone());

        assert_eq!(domain_relayer.id, repo_model.id);
        assert_eq!(domain_relayer.name, repo_model.name);
        assert_eq!(domain_relayer.network, repo_model.network);
        assert_eq!(domain_relayer.paused, repo_model.paused);
        assert_eq!(domain_relayer.network_type, repo_model.network_type);
        assert_eq!(domain_relayer.signer_id, repo_model.signer_id);
        assert_eq!(domain_relayer.notification_id, repo_model.notification_id);
        assert_eq!(domain_relayer.custom_rpc_urls, repo_model.custom_rpc_urls);

        // Policies should be converted correctly
        assert!(domain_relayer.policies.is_some());
        if let Some(RelayerNetworkPolicy::Evm(_)) = domain_relayer.policies {
            // Success - correct policy type
        } else {
            panic!("Expected EVM policy");
        }
    }

    #[test]
    fn test_conversion_from_repo_model_to_domain_solana() {
        let repo_model = create_test_relayer_solana(false, false);
        let domain_relayer = Relayer::from(repo_model.clone());

        assert_eq!(domain_relayer.id, repo_model.id);
        assert_eq!(domain_relayer.network_type, RelayerNetworkType::Solana);

        // Policies should be converted correctly
        assert!(domain_relayer.policies.is_some());
        if let Some(RelayerNetworkPolicy::Solana(solana_policy)) = domain_relayer.policies {
            assert_eq!(solana_policy.min_balance, Some(1000000));
            assert_eq!(solana_policy.max_signatures, Some(5));
            assert_eq!(
                solana_policy.fee_payment_strategy,
                Some(SolanaFeePaymentStrategy::Relayer)
            );
        } else {
            panic!("Expected Solana policy");
        }
    }

    #[test]
    fn test_conversion_from_repo_model_to_domain_stellar() {
        let repo_model = create_test_relayer_stellar(false, false);
        let domain_relayer = Relayer::from(repo_model.clone());

        assert_eq!(domain_relayer.id, repo_model.id);
        assert_eq!(domain_relayer.network_type, RelayerNetworkType::Stellar);

        // Policies should be converted correctly
        assert!(domain_relayer.policies.is_some());
        if let Some(RelayerNetworkPolicy::Stellar(stellar_policy)) = domain_relayer.policies {
            assert_eq!(stellar_policy.min_balance, Some(20000000));
            assert_eq!(stellar_policy.max_fee, Some(100000));
            assert_eq!(stellar_policy.timeout_seconds, Some(30));
        } else {
            panic!("Expected Stellar policy");
        }
    }

    #[test]
    fn test_conversion_from_domain_to_repo_model_evm() {
        let domain_relayer = Relayer {
            id: "test_evm_relayer".to_string(),
            name: "Test EVM Relayer".to_string(),
            network: "mainnet".to_string(),
            paused: false,
            network_type: RelayerNetworkType::Evm,
            policies: Some(RelayerNetworkPolicy::Evm(RelayerEvmPolicy {
                gas_price_cap: Some(100_000_000_000),
                eip1559_pricing: Some(true),
                min_balance: None,
                gas_limit_estimation: None,
                whitelist_receivers: None,
                private_transactions: None,
            })),
            signer_id: "test_signer".to_string(),
            notification_id: Some("notification_123".to_string()),
            custom_rpc_urls: None,
        };

        let repo_model = RelayerRepoModel::from(domain_relayer.clone());

        assert_eq!(repo_model.id, domain_relayer.id);
        assert_eq!(repo_model.name, domain_relayer.name);
        assert_eq!(repo_model.network, domain_relayer.network);
        assert_eq!(repo_model.paused, domain_relayer.paused);
        assert_eq!(repo_model.network_type, domain_relayer.network_type);
        assert_eq!(repo_model.signer_id, domain_relayer.signer_id);
        assert_eq!(repo_model.notification_id, domain_relayer.notification_id);
        assert_eq!(repo_model.custom_rpc_urls, domain_relayer.custom_rpc_urls);

        // Runtime fields should have default values
        assert_eq!(repo_model.address, "");
        assert!(!repo_model.system_disabled);

        // Policies should be converted correctly
        if let RelayerNetworkPolicy::Evm(evm_policy) = repo_model.policies {
            assert_eq!(evm_policy.gas_price_cap, Some(100_000_000_000));
            assert_eq!(evm_policy.eip1559_pricing, Some(true));
        } else {
            panic!("Expected EVM policy");
        }
    }

    #[test]
    fn test_conversion_from_domain_to_repo_model_solana() {
        let domain_relayer = Relayer {
            id: "test_solana_relayer".to_string(),
            name: "Test Solana Relayer".to_string(),
            network: "mainnet".to_string(),
            paused: false,
            network_type: RelayerNetworkType::Solana,
            policies: Some(RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
                fee_payment_strategy: Some(SolanaFeePaymentStrategy::User),
                min_balance: Some(5000000),
                max_signatures: Some(8),
                allowed_tokens: Some(vec![SolanaAllowedTokensPolicy::new(
                    "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
                    Some(100000),
                    None,
                )]),
                allowed_programs: None,
                allowed_accounts: None,
                disallowed_accounts: None,
                max_tx_data_size: None,
                max_allowed_fee_lamports: None,
                swap_config: None,
                fee_margin_percentage: None,
            })),
            signer_id: "test_signer".to_string(),
            notification_id: None,
            custom_rpc_urls: None,
        };

        let repo_model = RelayerRepoModel::from(domain_relayer.clone());

        assert_eq!(repo_model.network_type, RelayerNetworkType::Solana);

        // Policies should be converted correctly
        if let RelayerNetworkPolicy::Solana(solana_policy) = repo_model.policies {
            assert_eq!(
                solana_policy.fee_payment_strategy,
                Some(SolanaFeePaymentStrategy::User)
            );
            assert_eq!(solana_policy.min_balance, Some(5000000));
            assert_eq!(solana_policy.max_signatures, Some(8));
            assert!(solana_policy.allowed_tokens.is_some());
        } else {
            panic!("Expected Solana policy");
        }
    }

    #[test]
    fn test_conversion_from_domain_to_repo_model_stellar() {
        let domain_relayer = Relayer {
            id: "test_stellar_relayer".to_string(),
            name: "Test Stellar Relayer".to_string(),
            network: "mainnet".to_string(),
            paused: false,
            network_type: RelayerNetworkType::Stellar,
            policies: Some(RelayerNetworkPolicy::Stellar(RelayerStellarPolicy {
                min_balance: Some(30000000),
                max_fee: Some(150000),
                timeout_seconds: Some(60),
            })),
            signer_id: "test_signer".to_string(),
            notification_id: None,
            custom_rpc_urls: None,
        };

        let repo_model = RelayerRepoModel::from(domain_relayer.clone());

        assert_eq!(repo_model.network_type, RelayerNetworkType::Stellar);

        // Policies should be converted correctly
        if let RelayerNetworkPolicy::Stellar(stellar_policy) = repo_model.policies {
            assert_eq!(stellar_policy.min_balance, Some(30000000));
            assert_eq!(stellar_policy.max_fee, Some(150000));
            assert_eq!(stellar_policy.timeout_seconds, Some(60));
        } else {
            panic!("Expected Stellar policy");
        }
    }

    #[test]
    fn test_conversion_from_domain_with_no_policies_evm() {
        let domain_relayer = Relayer {
            id: "test_evm_relayer".to_string(),
            name: "Test EVM Relayer".to_string(),
            network: "mainnet".to_string(),
            paused: false,
            network_type: RelayerNetworkType::Evm,
            policies: None, // No policies provided
            signer_id: "test_signer".to_string(),
            notification_id: None,
            custom_rpc_urls: None,
        };

        let repo_model = RelayerRepoModel::from(domain_relayer);

        // Should create default EVM policy
        if let RelayerNetworkPolicy::Evm(evm_policy) = repo_model.policies {
            // Default EVM policy should have all None values
            assert_eq!(evm_policy.gas_price_cap, None);
            assert_eq!(evm_policy.eip1559_pricing, None);
            assert_eq!(evm_policy.min_balance, None);
            assert_eq!(evm_policy.gas_limit_estimation, None);
            assert_eq!(evm_policy.whitelist_receivers, None);
            assert_eq!(evm_policy.private_transactions, None);
        } else {
            panic!("Expected default EVM policy");
        }
    }

    #[test]
    fn test_conversion_from_domain_with_no_policies_solana() {
        let domain_relayer = Relayer {
            id: "test_solana_relayer".to_string(),
            name: "Test Solana Relayer".to_string(),
            network: "mainnet".to_string(),
            paused: false,
            network_type: RelayerNetworkType::Solana,
            policies: None, // No policies provided
            signer_id: "test_signer".to_string(),
            notification_id: None,
            custom_rpc_urls: None,
        };

        let repo_model = RelayerRepoModel::from(domain_relayer);

        // Should create default Solana policy
        if let RelayerNetworkPolicy::Solana(solana_policy) = repo_model.policies {
            // Default Solana policy should have all None values
            assert_eq!(solana_policy.fee_payment_strategy, None);
            assert_eq!(solana_policy.min_balance, None);
            assert_eq!(solana_policy.max_signatures, None);
            assert_eq!(solana_policy.allowed_tokens, None);
            assert_eq!(solana_policy.allowed_programs, None);
            assert_eq!(solana_policy.allowed_accounts, None);
            assert_eq!(solana_policy.disallowed_accounts, None);
            assert_eq!(solana_policy.max_tx_data_size, None);
            assert_eq!(solana_policy.max_allowed_fee_lamports, None);
            assert_eq!(solana_policy.swap_config, None);
            assert_eq!(solana_policy.fee_margin_percentage, None);
        } else {
            panic!("Expected default Solana policy");
        }
    }

    #[test]
    fn test_conversion_from_domain_with_no_policies_stellar() {
        let domain_relayer = Relayer {
            id: "test_stellar_relayer".to_string(),
            name: "Test Stellar Relayer".to_string(),
            network: "mainnet".to_string(),
            paused: false,
            network_type: RelayerNetworkType::Stellar,
            policies: None, // No policies provided
            signer_id: "test_signer".to_string(),
            notification_id: None,
            custom_rpc_urls: None,
        };

        let repo_model = RelayerRepoModel::from(domain_relayer);

        // Should create default Stellar policy
        if let RelayerNetworkPolicy::Stellar(stellar_policy) = repo_model.policies {
            // Default Stellar policy should have all None values
            assert_eq!(stellar_policy.min_balance, None);
            assert_eq!(stellar_policy.max_fee, None);
            assert_eq!(stellar_policy.timeout_seconds, None);
        } else {
            panic!("Expected default Stellar policy");
        }
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

    #[test]
    fn test_relayer_repo_updater_preserves_runtime_fields_solana() {
        // Create an original Solana relayer with runtime fields set
        let original = RelayerRepoModel {
            id: "test_solana_relayer".to_string(),
            name: "Original Solana Name".to_string(),
            address: "SolanaOriginalAddress123".to_string(), // Runtime field
            system_disabled: true,                           // Runtime field
            paused: false,
            network: "mainnet".to_string(),
            network_type: NetworkType::Solana,
            signer_id: "test_signer".to_string(),
            policies: RelayerNetworkPolicy::Solana(RelayerSolanaPolicy::default()),
            notification_id: None,
            custom_rpc_urls: None,
        };

        // Create a domain model with different business fields
        let domain_update = Relayer {
            id: "test_solana_relayer".to_string(),
            name: "Updated Solana Name".to_string(), // Changed
            paused: true,                            // Changed
            network: "mainnet".to_string(),
            network_type: RelayerNetworkType::Solana,
            signer_id: "test_signer".to_string(),
            policies: Some(RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
                min_balance: Some(2000000), // Changed
                ..RelayerSolanaPolicy::default()
            })),
            notification_id: Some("solana_notification".to_string()), // Changed
            custom_rpc_urls: None,
        };

        // Use updater to preserve runtime fields
        let updated =
            RelayerRepoUpdater::from_existing(original.clone()).apply_domain_update(domain_update);

        // Verify business fields were updated
        assert_eq!(updated.name, "Updated Solana Name");
        assert!(updated.paused);
        assert_eq!(
            updated.notification_id,
            Some("solana_notification".to_string())
        );

        // Verify runtime fields were preserved
        assert_eq!(updated.address, "SolanaOriginalAddress123");
        assert!(updated.system_disabled);

        // Verify policies were updated
        if let RelayerNetworkPolicy::Solana(solana_policy) = updated.policies {
            assert_eq!(solana_policy.min_balance, Some(2000000));
        } else {
            panic!("Expected Solana policy");
        }
    }

    #[test]
    fn test_relayer_repo_updater_preserves_runtime_fields_stellar() {
        // Create an original Stellar relayer with runtime fields set
        let original = RelayerRepoModel {
            id: "test_stellar_relayer".to_string(),
            name: "Original Stellar Name".to_string(),
            address: "GORIGINALXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX".to_string(), // Runtime field
            system_disabled: false, // Runtime field
            paused: true,
            network: "mainnet".to_string(),
            network_type: NetworkType::Stellar,
            signer_id: "test_signer".to_string(),
            policies: RelayerNetworkPolicy::Stellar(RelayerStellarPolicy::default()),
            notification_id: Some("original_notification".to_string()),
            custom_rpc_urls: None,
        };

        // Create a domain model with different business fields
        let domain_update = Relayer {
            id: "test_stellar_relayer".to_string(),
            name: "Updated Stellar Name".to_string(), // Changed
            paused: false,                            // Changed
            network: "mainnet".to_string(),
            network_type: RelayerNetworkType::Stellar,
            signer_id: "test_signer".to_string(),
            policies: Some(RelayerNetworkPolicy::Stellar(RelayerStellarPolicy {
                min_balance: Some(40000000), // Changed
                max_fee: Some(200000),       // Changed
                timeout_seconds: Some(120),  // Changed
            })),
            notification_id: None, // Changed
            custom_rpc_urls: None,
        };

        // Use updater to preserve runtime fields
        let updated =
            RelayerRepoUpdater::from_existing(original.clone()).apply_domain_update(domain_update);

        // Verify business fields were updated
        assert_eq!(updated.name, "Updated Stellar Name");
        assert!(!updated.paused);
        assert_eq!(updated.notification_id, None);

        // Verify runtime fields were preserved
        assert_eq!(
            updated.address,
            "GORIGINALXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX"
        );
        assert!(!updated.system_disabled);

        // Verify policies were updated
        if let RelayerNetworkPolicy::Stellar(stellar_policy) = updated.policies {
            assert_eq!(stellar_policy.min_balance, Some(40000000));
            assert_eq!(stellar_policy.max_fee, Some(200000));
            assert_eq!(stellar_policy.timeout_seconds, Some(120));
        } else {
            panic!("Expected Stellar policy");
        }
    }

    #[test]
    fn test_repo_model_serialization_deserialization_evm() {
        let original = create_test_relayer(false, false);

        // Serialize to JSON
        let serialized = serde_json::to_string(&original).unwrap();
        assert!(!serialized.is_empty());

        // Deserialize back
        let deserialized: RelayerRepoModel = serde_json::from_str(&serialized).unwrap();

        // Verify all fields match
        assert_eq!(original.id, deserialized.id);
        assert_eq!(original.name, deserialized.name);
        assert_eq!(original.network, deserialized.network);
        assert_eq!(original.paused, deserialized.paused);
        assert_eq!(original.network_type, deserialized.network_type);
        assert_eq!(original.signer_id, deserialized.signer_id);
        assert_eq!(original.address, deserialized.address);
        assert_eq!(original.notification_id, deserialized.notification_id);
        assert_eq!(original.system_disabled, deserialized.system_disabled);
        assert_eq!(original.custom_rpc_urls, deserialized.custom_rpc_urls);

        // Verify policies match
        match (&original.policies, &deserialized.policies) {
            (RelayerNetworkPolicy::Evm(_), RelayerNetworkPolicy::Evm(_)) => {
                // Success - both are EVM policies
            }
            _ => panic!("Policy types don't match after serialization/deserialization"),
        }
    }

    #[test]
    fn test_repo_model_serialization_deserialization_solana() {
        let original = create_test_relayer_solana(true, false);

        // Serialize to JSON
        let serialized = serde_json::to_string(&original).unwrap();
        assert!(!serialized.is_empty());

        // Deserialize back
        let deserialized: RelayerRepoModel = serde_json::from_str(&serialized).unwrap();

        // Verify key fields match
        assert_eq!(original.id, deserialized.id);
        assert_eq!(original.network_type, RelayerNetworkType::Solana);
        assert_eq!(deserialized.network_type, RelayerNetworkType::Solana);
        assert_eq!(original.paused, deserialized.paused);

        // Verify policies match
        match (&original.policies, &deserialized.policies) {
            (RelayerNetworkPolicy::Solana(orig), RelayerNetworkPolicy::Solana(deser)) => {
                assert_eq!(orig.fee_payment_strategy, deser.fee_payment_strategy);
                assert_eq!(orig.min_balance, deser.min_balance);
                assert_eq!(orig.max_signatures, deser.max_signatures);
            }
            _ => panic!("Policy types don't match after serialization/deserialization"),
        }
    }

    #[test]
    fn test_repo_model_serialization_deserialization_stellar() {
        let original = create_test_relayer_stellar(false, true);

        // Serialize to JSON
        let serialized = serde_json::to_string(&original).unwrap();
        assert!(!serialized.is_empty());

        // Deserialize back
        let deserialized: RelayerRepoModel = serde_json::from_str(&serialized).unwrap();

        // Verify key fields match
        assert_eq!(original.id, deserialized.id);
        assert_eq!(original.network_type, RelayerNetworkType::Stellar);
        assert_eq!(deserialized.network_type, RelayerNetworkType::Stellar);
        assert_eq!(original.system_disabled, deserialized.system_disabled);

        // Verify policies match
        match (&original.policies, &deserialized.policies) {
            (RelayerNetworkPolicy::Stellar(orig), RelayerNetworkPolicy::Stellar(deser)) => {
                assert_eq!(orig.min_balance, deser.min_balance);
                assert_eq!(orig.max_fee, deser.max_fee);
                assert_eq!(orig.timeout_seconds, deser.timeout_seconds);
            }
            _ => panic!("Policy types don't match after serialization/deserialization"),
        }
    }

    #[test]
    fn test_repo_model_default() {
        let default_model = RelayerRepoModel::default();

        assert_eq!(default_model.id, "");
        assert_eq!(default_model.name, "");
        assert_eq!(default_model.network, "");
        assert!(!default_model.paused);
        assert_eq!(default_model.network_type, NetworkType::Evm);
        assert_eq!(default_model.signer_id, "");
        assert_eq!(default_model.address, "0x");
        assert_eq!(default_model.notification_id, None);
        assert!(!default_model.system_disabled);
        assert_eq!(default_model.custom_rpc_urls, None);

        // Default should have EVM policy
        if let RelayerNetworkPolicy::Evm(_) = default_model.policies {
            // Success
        } else {
            panic!("Default should have EVM policy");
        }
    }

    #[test]
    fn test_round_trip_conversion_all_network_types() {
        // Test round-trip conversion: Domain -> Repo -> Domain for all network types

        // EVM
        let original_evm = Relayer {
            id: "evm_relayer".to_string(),
            name: "EVM Relayer".to_string(),
            network: "mainnet".to_string(),
            paused: false,
            network_type: RelayerNetworkType::Evm,
            policies: Some(RelayerNetworkPolicy::Evm(RelayerEvmPolicy {
                gas_price_cap: Some(50_000_000_000),
                eip1559_pricing: Some(true),
                min_balance: None,
                gas_limit_estimation: None,
                whitelist_receivers: None,
                private_transactions: None,
            })),
            signer_id: "evm_signer".to_string(),
            notification_id: Some("evm_notification".to_string()),
            custom_rpc_urls: None,
        };

        let repo_evm = RelayerRepoModel::from(original_evm.clone());
        let recovered_evm = Relayer::from(repo_evm);

        assert_eq!(original_evm.id, recovered_evm.id);
        assert_eq!(original_evm.network_type, recovered_evm.network_type);
        assert_eq!(original_evm.notification_id, recovered_evm.notification_id);

        // Solana
        let original_solana = Relayer {
            id: "solana_relayer".to_string(),
            name: "Solana Relayer".to_string(),
            network: "mainnet".to_string(),
            paused: true,
            network_type: RelayerNetworkType::Solana,
            policies: Some(RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
                fee_payment_strategy: Some(SolanaFeePaymentStrategy::User),
                min_balance: Some(3000000),
                max_signatures: None,
                allowed_tokens: None,
                allowed_programs: None,
                allowed_accounts: None,
                disallowed_accounts: None,
                max_tx_data_size: None,
                max_allowed_fee_lamports: None,
                swap_config: None,
                fee_margin_percentage: None,
            })),
            signer_id: "solana_signer".to_string(),
            notification_id: None,
            custom_rpc_urls: None,
        };

        let repo_solana = RelayerRepoModel::from(original_solana.clone());
        let recovered_solana = Relayer::from(repo_solana);

        assert_eq!(original_solana.id, recovered_solana.id);
        assert_eq!(original_solana.network_type, recovered_solana.network_type);
        assert_eq!(original_solana.paused, recovered_solana.paused);

        // Stellar
        let original_stellar = Relayer {
            id: "stellar_relayer".to_string(),
            name: "Stellar Relayer".to_string(),
            network: "mainnet".to_string(),
            paused: false,
            network_type: RelayerNetworkType::Stellar,
            policies: Some(RelayerNetworkPolicy::Stellar(RelayerStellarPolicy {
                min_balance: Some(50000000),
                max_fee: Some(250000),
                timeout_seconds: Some(180),
            })),
            signer_id: "stellar_signer".to_string(),
            notification_id: Some("stellar_notification".to_string()),
            custom_rpc_urls: None,
        };

        let repo_stellar = RelayerRepoModel::from(original_stellar.clone());
        let recovered_stellar = Relayer::from(repo_stellar);

        assert_eq!(original_stellar.id, recovered_stellar.id);
        assert_eq!(
            original_stellar.network_type,
            recovered_stellar.network_type
        );
        assert_eq!(
            original_stellar.notification_id,
            recovered_stellar.notification_id
        );
    }
}
