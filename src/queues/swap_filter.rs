use crate::models::{RelayerNetworkPolicy, RelayerRepoModel};
use tracing::debug;

/// Filters relayers to find those eligible for swap workers (Solana or Stellar).
pub fn filter_relayers_for_swap(relayers: Vec<RelayerRepoModel>) -> Vec<RelayerRepoModel> {
    relayers
        .into_iter()
        .filter(|relayer| match &relayer.policies {
            RelayerNetworkPolicy::Solana(policy) => {
                let swap_config = match policy.get_swap_config() {
                    Some(config) => config,
                    None => {
                        debug!(relayer_id = %relayer.id, "No Solana swap configuration specified; skipping");
                        return false;
                    }
                };

                if swap_config.cron_schedule.is_none() {
                    debug!(relayer_id = %relayer.id, "No cron schedule specified; skipping");
                    return false;
                }
                true
            }
            RelayerNetworkPolicy::Stellar(policy) => {
                let swap_config = match policy.get_swap_config() {
                    Some(config) => config,
                    None => {
                        debug!(relayer_id = %relayer.id, "No Stellar swap configuration specified; skipping");
                        return false;
                    }
                };

                if swap_config.cron_schedule.is_none() {
                    debug!(relayer_id = %relayer.id, "No cron schedule specified; skipping");
                    return false;
                }
                true
            }
            _ => {
                debug!(relayer_id = %relayer.id, "Network type does not support swap; skipping");
                false
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        RelayerSolanaPolicy, RelayerSolanaSwapConfig, RelayerStellarPolicy,
        RelayerStellarSwapConfig,
    };

    fn solana_relayer_with_swap(id: &str, cron: Option<&str>) -> RelayerRepoModel {
        RelayerRepoModel {
            id: id.to_string(),
            policies: RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
                swap_config: Some(RelayerSolanaSwapConfig {
                    strategy: None,
                    cron_schedule: cron.map(String::from),
                    min_balance_threshold: None,
                    jupiter_swap_options: None,
                }),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn solana_relayer_no_swap(id: &str) -> RelayerRepoModel {
        RelayerRepoModel {
            id: id.to_string(),
            policies: RelayerNetworkPolicy::Solana(RelayerSolanaPolicy::default()),
            ..Default::default()
        }
    }

    fn stellar_relayer_with_swap(id: &str, cron: Option<&str>) -> RelayerRepoModel {
        RelayerRepoModel {
            id: id.to_string(),
            policies: RelayerNetworkPolicy::Stellar(RelayerStellarPolicy {
                swap_config: Some(RelayerStellarSwapConfig {
                    strategies: vec![],
                    cron_schedule: cron.map(String::from),
                    min_balance_threshold: None,
                }),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn stellar_relayer_no_swap(id: &str) -> RelayerRepoModel {
        RelayerRepoModel {
            id: id.to_string(),
            policies: RelayerNetworkPolicy::Stellar(RelayerStellarPolicy::default()),
            ..Default::default()
        }
    }

    fn evm_relayer(id: &str) -> RelayerRepoModel {
        RelayerRepoModel {
            id: id.to_string(),
            ..Default::default() // default is EVM
        }
    }

    #[test]
    fn test_empty_list_returns_empty() {
        assert!(filter_relayers_for_swap(vec![]).is_empty());
    }

    #[test]
    fn test_solana_with_swap_and_cron_included() {
        let relayers = vec![solana_relayer_with_swap("sol-1", Some("0 0 * * * *"))];
        let result = filter_relayers_for_swap(relayers);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, "sol-1");
    }

    #[test]
    fn test_solana_with_swap_but_no_cron_excluded() {
        let relayers = vec![solana_relayer_with_swap("sol-1", None)];
        assert!(filter_relayers_for_swap(relayers).is_empty());
    }

    #[test]
    fn test_solana_without_swap_config_excluded() {
        let relayers = vec![solana_relayer_no_swap("sol-1")];
        assert!(filter_relayers_for_swap(relayers).is_empty());
    }

    #[test]
    fn test_stellar_with_swap_and_cron_included() {
        let relayers = vec![stellar_relayer_with_swap("xlm-1", Some("0 */5 * * * *"))];
        let result = filter_relayers_for_swap(relayers);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, "xlm-1");
    }

    #[test]
    fn test_stellar_with_swap_but_no_cron_excluded() {
        let relayers = vec![stellar_relayer_with_swap("xlm-1", None)];
        assert!(filter_relayers_for_swap(relayers).is_empty());
    }

    #[test]
    fn test_stellar_without_swap_config_excluded() {
        let relayers = vec![stellar_relayer_no_swap("xlm-1")];
        assert!(filter_relayers_for_swap(relayers).is_empty());
    }

    #[test]
    fn test_evm_relayer_excluded() {
        let relayers = vec![evm_relayer("evm-1")];
        assert!(filter_relayers_for_swap(relayers).is_empty());
    }

    #[test]
    fn test_mixed_list_filters_correctly() {
        let relayers = vec![
            solana_relayer_with_swap("sol-ok", Some("0 0 * * * *")),
            solana_relayer_with_swap("sol-no-cron", None),
            solana_relayer_no_swap("sol-no-swap"),
            stellar_relayer_with_swap("xlm-ok", Some("0 0 * * * *")),
            stellar_relayer_no_swap("xlm-no-swap"),
            evm_relayer("evm-1"),
        ];
        let result = filter_relayers_for_swap(relayers);
        let ids: Vec<&str> = result.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["sol-ok", "xlm-ok"]);
    }
}
