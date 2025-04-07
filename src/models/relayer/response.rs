use crate::models::NetworkType;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::{
    RelayerNetworkPolicy, RelayerRepoModel, SolanaAllowedTokensPolicy, SolanaFeePaymentStrategy,
};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, ToSchema)]
pub struct RelayerResponse {
    pub id: String,
    pub name: String,
    pub network: String,
    pub network_type: NetworkType,
    pub paused: bool,
    pub policies: NetworkPolicyResponse,
    pub address: String,
    pub system_disabled: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, ToSchema)]
#[serde(untagged)]
pub enum NetworkPolicyResponse {
    Evm(EvmPolicyResponse),
    Solana(SolanaPolicyResponse),
    Stellar(StellarPolicyResponse),
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, ToSchema)]
pub struct EvmPolicyResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub gas_price_cap: Option<u128>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub whitelist_receivers: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub eip1559_pricing: Option<bool>,
    pub private_transactions: bool,
    pub min_balance: u128,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, ToSchema)]
pub struct SolanaPolicyResponse {
    fee_payment_strategy: SolanaFeePaymentStrategy,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fee_margin_percentage: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub allowed_tokens: Option<Vec<SolanaAllowedTokensPolicy>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub allowed_programs: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub allowed_accounts: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub disallowed_accounts: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub max_signatures: Option<u8>,
    pub max_tx_data_size: u16,
    pub min_balance: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub max_allowed_fee_lamports: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, ToSchema)]
pub struct StellarPolicyResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub max_fee: Option<u32>,
    pub min_balance: u64,
}

impl From<RelayerRepoModel> for RelayerResponse {
    fn from(model: RelayerRepoModel) -> Self {
        let policies = match model.policies {
            RelayerNetworkPolicy::Evm(evm) => NetworkPolicyResponse::Evm(EvmPolicyResponse {
                gas_price_cap: evm.gas_price_cap,
                whitelist_receivers: evm.whitelist_receivers,
                eip1559_pricing: evm.eip1559_pricing,
                min_balance: evm.min_balance,
                private_transactions: evm.private_transactions,
            }),
            RelayerNetworkPolicy::Solana(solana) => {
                NetworkPolicyResponse::Solana(SolanaPolicyResponse {
                    fee_payment_strategy: solana.fee_payment_strategy,
                    fee_margin_percentage: solana.fee_margin_percentage,
                    min_balance: solana.min_balance,
                    allowed_tokens: solana.allowed_tokens,
                    allowed_programs: solana.allowed_programs,
                    allowed_accounts: solana.allowed_accounts,
                    disallowed_accounts: solana.disallowed_accounts,
                    max_signatures: solana.max_signatures,
                    max_tx_data_size: solana.max_tx_data_size,
                    max_allowed_fee_lamports: solana.max_allowed_fee_lamports,
                })
            }
            RelayerNetworkPolicy::Stellar(stellar) => {
                NetworkPolicyResponse::Stellar(StellarPolicyResponse {
                    max_fee: stellar.max_fee,
                    min_balance: stellar.min_balance,
                })
            }
        };

        Self {
            id: model.id,
            name: model.name,
            network: model.network,
            network_type: model.network_type,
            paused: model.paused,
            policies,
            address: model.address,
            system_disabled: model.system_disabled,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::models::{
        RelayerEvmPolicy, RelayerSolanaPolicy, RelayerStellarPolicy, SolanaFeePaymentStrategy,
    };

    use super::*;

    #[test]
    fn test_from_relayer_repo_model_evm() {
        let model = RelayerRepoModel {
            id: "test-id".to_string(),
            name: "Test Relayer".to_string(),
            network: "ethereum".to_string(),
            network_type: NetworkType::Evm,
            paused: false,
            policies: RelayerNetworkPolicy::Evm(RelayerEvmPolicy {
                gas_price_cap: Some(100),
                whitelist_receivers: Some(vec!["0x123".to_string()]),
                eip1559_pricing: Some(true),
                min_balance: 1000,
                private_transactions: true,
            }),
            address: "0xabc".to_string(),
            system_disabled: false,
            signer_id: "test-signer-id".to_string(),
            notification_id: Some("test-notification-id".to_string()),
            custom_rpc_urls: Some(vec!["https://test-rpc-url".to_string()]),
        };

        let response: RelayerResponse = model.clone().into();

        assert_eq!(response.id, model.id);
        assert_eq!(response.name, model.name);
        assert_eq!(response.network, model.network);
        assert_eq!(response.network_type, model.network_type);
        assert_eq!(response.paused, model.paused);
        assert_eq!(response.address, model.address);
        assert_eq!(response.system_disabled, model.system_disabled);

        if let NetworkPolicyResponse::Evm(evm) = response.policies {
            if let RelayerNetworkPolicy::Evm(expected) = model.policies {
                assert_eq!(evm.gas_price_cap, expected.gas_price_cap);
                assert_eq!(evm.whitelist_receivers, expected.whitelist_receivers);
                assert_eq!(evm.eip1559_pricing, expected.eip1559_pricing);
                assert_eq!(evm.min_balance, expected.min_balance);
                assert_eq!(evm.private_transactions, expected.private_transactions);
            } else {
                panic!("Expected EVM policy");
            }
        } else {
            panic!("Expected EVM policy response");
        }
    }

    #[test]
    fn test_from_relayer_repo_model_solana() {
        let model = RelayerRepoModel {
            id: "test-id".to_string(),
            name: "Test Relayer".to_string(),
            network: "solana".to_string(),
            network_type: NetworkType::Solana,
            paused: true,
            policies: RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
                fee_payment_strategy: SolanaFeePaymentStrategy::User,
                fee_margin_percentage: Some(0.5),
                min_balance: 5000,
                allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                    mint: "mint-address".to_string(),
                    decimals: Some(9),
                    symbol: Some("SOL".to_string()),
                    max_allowed_fee: Some(1000),
                    conversion_slippage_percentage: Some(100.0),
                }]),
                allowed_programs: Some(vec!["program1".to_string()]),
                allowed_accounts: Some(vec!["account1".to_string()]),
                disallowed_accounts: Some(vec!["bad-account".to_string()]),
                max_signatures: Some(10),
                max_tx_data_size: 1024,
                max_allowed_fee_lamports: Some(10000),
            }),
            address: "solana-address".to_string(),
            system_disabled: false,
            signer_id: "test-signer-id".to_string(),
            notification_id: Some("test-notification-id".to_string()),
            custom_rpc_urls: Some(vec!["https://test-rpc-url".to_string()]),
        };

        let response: RelayerResponse = model.clone().into();

        assert_eq!(response.id, model.id);
        assert_eq!(response.name, model.name);
        assert_eq!(response.network, model.network);
        assert_eq!(response.network_type, model.network_type);
        assert_eq!(response.paused, model.paused);
        assert_eq!(response.address, model.address);
        assert_eq!(response.system_disabled, model.system_disabled);

        if let NetworkPolicyResponse::Solana(solana) = response.policies {
            if let RelayerNetworkPolicy::Solana(expected) = model.policies {
                assert_eq!(solana.min_balance, expected.min_balance);
                assert_eq!(solana.allowed_tokens, expected.allowed_tokens);
                assert_eq!(solana.allowed_programs, expected.allowed_programs);
                assert_eq!(solana.allowed_accounts, expected.allowed_accounts);
                assert_eq!(solana.disallowed_accounts, expected.disallowed_accounts);
                assert_eq!(solana.max_signatures, expected.max_signatures);
                assert_eq!(solana.max_tx_data_size, expected.max_tx_data_size);
                assert_eq!(
                    solana.max_allowed_fee_lamports,
                    expected.max_allowed_fee_lamports
                );
            } else {
                panic!("Expected Solana policy");
            }
        } else {
            panic!("Expected Solana policy response");
        }
    }

    #[test]
    fn test_from_relayer_repo_model_stellar() {
        let model = RelayerRepoModel {
            id: "test-id".to_string(),
            name: "Test Relayer".to_string(),
            network: "stellar".to_string(),
            network_type: NetworkType::Stellar,
            paused: false,
            policies: RelayerNetworkPolicy::Stellar(RelayerStellarPolicy {
                max_fee: Some(200),
                min_balance: 2000,
                timeout_seconds: Some(100),
            }),
            address: "stellar-address".to_string(),
            system_disabled: true,
            signer_id: "test-signer-id".to_string(),
            notification_id: Some("test-notification-id".to_string()),
            custom_rpc_urls: Some(vec!["https://test-rpc-url".to_string()]),
        };

        let response: RelayerResponse = model.clone().into();

        assert_eq!(response.id, model.id);
        assert_eq!(response.name, model.name);
        assert_eq!(response.network, model.network);
        assert_eq!(response.network_type, model.network_type);
        assert_eq!(response.paused, model.paused);
        assert_eq!(response.address, model.address);
        assert_eq!(response.system_disabled, model.system_disabled);

        if let NetworkPolicyResponse::Stellar(stellar) = response.policies {
            if let RelayerNetworkPolicy::Stellar(expected) = model.policies {
                assert_eq!(stellar.max_fee, expected.max_fee);
                assert_eq!(stellar.min_balance, expected.min_balance);
            } else {
                panic!("Expected Stellar policy");
            }
        } else {
            panic!("Expected Stellar policy response");
        }
    }
}
