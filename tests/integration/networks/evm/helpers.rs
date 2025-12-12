//! Helper functions for EVM network tests

use crate::integration::common::{client::RelayerClient, registry::TestRegistry};
use eyre::Result;
use openzeppelin_relayer::models::relayer::RelayerResponse;
use tracing::{info, warn};

/// Get the pre-configured relayer for testing on a specific network
///
/// Uses the relayer defined in config.json, referenced by relayer_id in registry.json.
/// This avoids creating relayers programmatically since they're already configured.
pub async fn setup_test_relayer(
    client: &RelayerClient,
    registry: &TestRegistry,
    network: &str,
) -> Result<RelayerResponse> {
    let network_config = registry.get_network(network)?;
    let relayer_id = &network_config.relayer_id;

    let relayer = client.get_relayer(relayer_id).await?;

    info!(
        relayer_id = %relayer.id,
        address = ?relayer.address,
        "Relayer ready"
    );

    // Check for disabled status and warn
    if relayer.system_disabled == Some(true) {
        let reason = relayer
            .disabled_reason
            .as_ref()
            .map(|r| format!("{:?}", r))
            .unwrap_or_else(|| "unknown".to_string());
        warn!(
            relayer_id = %relayer.id,
            reason = %reason,
            "Relayer marked as disabled, attempting test anyway"
        );

        // Wait for health check, then verify status again
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;

        let relayer_status = client.get_relayer(&relayer.id).await?;
        if relayer_status.system_disabled == Some(true) {
            let reason = relayer_status
                .disabled_reason
                .as_ref()
                .map(|r| format!("{:?}", r))
                .unwrap_or_else(|| "unknown".to_string());
            warn!(
                relayer_id = %relayer.id,
                reason = %reason,
                "Relayer disabled after health check, attempting test anyway"
            );
        }
    }

    Ok(relayer)
}

/// Verify network readiness for testing
pub fn verify_network_ready(registry: &TestRegistry, network: &str) -> Result<()> {
    let network_config = registry.get_network(network)?;

    if network_config.network_type != "evm" {
        info!(network = %network, "Skipping - not an EVM network");
        return Ok(());
    }

    let readiness = registry.validate_readiness(network)?;
    if !readiness.ready {
        return Err(eyre::eyre!(
            "Network {} not ready: enabled={}, has_signer={}, has_contracts={}",
            network,
            readiness.enabled,
            readiness.has_signer,
            readiness.has_contracts
        ));
    }

    Ok(())
}
