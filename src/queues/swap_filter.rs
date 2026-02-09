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
