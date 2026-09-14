//! Proves the EVM status poll cadence env overrides reach the resolved config.
//!
//! This is a SEPARATE test binary because the cadence getters cache their
//! resolved values in a `OnceLock`: the environment has to be set before
//! anything else in the process reads them. Keeping a single test in this
//! binary makes that ordering guaranteed rather than incidental.

use openzeppelin_relayer::config::ServerConfig;
use openzeppelin_relayer::constants::get_evm_status_check_initial_delay;
use openzeppelin_relayer::models::NetworkType;
use openzeppelin_relayer::queues::retry_config::{status_backoff_config, STATUS_EVM_BACKOFF};
use std::env;

#[test]
fn evm_status_cadence_env_overrides_reach_resolved_config() {
    env::set_var("EVM_STATUS_CHECK_INITIAL_DELAY_SECONDS", "1");
    env::set_var("EVM_STATUS_RETRY_INITIAL_MS", "1000");
    env::set_var("EVM_STATUS_RETRY_MAX_MS", "4000");

    assert_eq!(
        ServerConfig::get_evm_status_check_initial_delay_seconds(),
        1
    );
    assert_eq!(get_evm_status_check_initial_delay().num_seconds(), 1);

    let backoff = status_backoff_config(Some(NetworkType::Evm));
    assert_eq!(backoff.initial_ms, 1000);
    assert_eq!(backoff.max_ms, 4000);
    assert_eq!(backoff.jitter, STATUS_EVM_BACKOFF.jitter);
}
