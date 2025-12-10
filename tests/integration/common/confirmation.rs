//! Receipt confirmation utilities for integration tests
//!
//! This module provides utilities for waiting on transaction receipts
//! with network-aware timing based on block times and confirmation requirements.

use super::client::RelayerClient;
use eyre::{Context, Result};
use openzeppelin_relayer::models::TransactionStatus;
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};
use tracing::warn;

/// Configuration for receipt polling based on network characteristics
#[derive(Debug, Clone)]
pub struct ReceiptConfig {
    /// How often to poll for transaction status (in milliseconds)
    pub poll_interval_ms: u64,
    /// Maximum time to wait for receipt (in milliseconds)
    pub max_wait_ms: u64,
    /// Network name for logging
    pub network: String,
}

/// Main config.json structure for loading networks path
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Config {
    networks: String,
}

/// Network configuration file structure
#[derive(Debug, Clone, Serialize, Deserialize)]
struct NetworkConfigFile {
    networks: Vec<NetworkEntry>,
}

/// Individual network entry from config files
#[derive(Debug, Clone, Serialize, Deserialize)]
struct NetworkEntry {
    network: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    average_blocktime_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    required_confirmations: Option<u32>,
}

impl ReceiptConfig {
    /// Create a ReceiptConfig from network configuration
    ///
    /// Calculates appropriate poll interval and max wait time based on:
    /// - `average_blocktime_ms` from network config (or default 12000ms)
    /// - `required_confirmations` from network config (or default 6)
    ///
    /// Poll interval = block_time / 2 (poll twice per expected block)
    /// Max wait = block_time * confirmations * 3 (safety margin)
    pub fn from_network(network: &str) -> Result<Self> {
        let (block_time_ms, confirmations) = load_network_timing(network)?;

        // Poll at half the block time (minimum 1 second)
        let poll_interval_ms = (block_time_ms / 2).max(1000);

        // Wait for confirmations with 3x safety margin (minimum 30 seconds)
        let max_wait_ms = (block_time_ms * confirmations as u64 * 3).max(30_000);

        Ok(Self {
            poll_interval_ms,
            max_wait_ms,
            network: network.to_string(),
        })
    }

    /// Create with custom timing values
    pub fn custom(network: &str, poll_interval_ms: u64, max_wait_ms: u64) -> Self {
        Self {
            poll_interval_ms,
            max_wait_ms,
            network: network.to_string(),
        }
    }

    /// Get poll interval as Duration
    pub fn poll_interval(&self) -> Duration {
        Duration::from_millis(self.poll_interval_ms)
    }

    /// Get max wait as Duration
    pub fn max_wait(&self) -> Duration {
        Duration::from_millis(self.max_wait_ms)
    }
}

/// Load network timing configuration from config/networks/*.json
fn load_network_timing(network: &str) -> Result<(u64, u32)> {
    // Read config.json to get networks path
    let config_path_str =
        env::var("CONFIG_PATH").unwrap_or_else(|_| "config/config.json".to_string());
    let config_path = Path::new(&config_path_str);

    if !config_path.exists() {
        // Return defaults if config not found
        return Ok((12_000, 6));
    }

    let config_contents = fs::read_to_string(config_path)
        .wrap_err_with(|| format!("Failed to read config: {}", config_path.display()))?;

    let config: Config =
        serde_json::from_str(&config_contents).wrap_err("Failed to parse config.json")?;

    // Get networks directory from config
    let networks_dir = Path::new(&config.networks);

    if !networks_dir.exists() {
        return Ok((12_000, 6));
    }

    // Search through all network config files
    for entry in fs::read_dir(networks_dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }

        let contents = fs::read_to_string(&path)?;
        let network_config: NetworkConfigFile = match serde_json::from_str(&contents) {
            Ok(config) => config,
            Err(_) => continue,
        };

        // Find the matching network
        for net in network_config.networks {
            if net.network == network {
                let block_time = net.average_blocktime_ms.unwrap_or(12_000);
                let confirmations = net.required_confirmations.unwrap_or(6);
                return Ok((block_time, confirmations));
            }
        }
    }

    // Network not found, return defaults
    Ok((12_000, 6))
}

/// Parse a status string from API response to TransactionStatus
fn parse_status(s: &str) -> Option<TransactionStatus> {
    match s.to_lowercase().as_str() {
        "canceled" => Some(TransactionStatus::Canceled),
        "pending" => Some(TransactionStatus::Pending),
        "sent" => Some(TransactionStatus::Sent),
        "submitted" => Some(TransactionStatus::Submitted),
        "mined" => Some(TransactionStatus::Mined),
        "confirmed" => Some(TransactionStatus::Confirmed),
        "failed" => Some(TransactionStatus::Failed),
        "expired" => Some(TransactionStatus::Expired),
        _ => None,
    }
}

/// Wait for a transaction to be confirmed
///
/// Polls the transaction status until it's confirmed or times out.
///
/// # Arguments
///
/// * `client` - RelayerClient to use for status checks
/// * `relayer_id` - ID of the relayer
/// * `tx_id` - Transaction ID to wait for
/// * `config` - Receipt configuration with timing parameters
///
/// # Returns
///
/// * `Ok(())` - Transaction was confirmed
/// * `Err` - Timeout, transaction failed, or API error
pub async fn wait_for_receipt(
    client: &RelayerClient,
    relayer_id: &str,
    tx_id: &str,
    config: &ReceiptConfig,
) -> Result<()> {
    let start = Instant::now();
    let max_wait = config.max_wait();
    let poll_interval = config.poll_interval();

    loop {
        // Check if we've exceeded max wait time
        if start.elapsed() > max_wait {
            return Err(eyre::eyre!(
                "Timeout waiting for transaction {} on {} after {:?}",
                tx_id,
                config.network,
                start.elapsed()
            ));
        }

        // Get transaction status
        let tx_response = client.get_transaction(relayer_id, tx_id).await?;

        // Extract status from response
        let status = parse_status(tx_response.status.as_str());

        match status {
            Some(TransactionStatus::Confirmed) | Some(TransactionStatus::Mined) => {
                return Ok(());
            }
            Some(TransactionStatus::Failed) => {
                let error = tx_response.error.as_deref().unwrap_or("Unknown error");
                return Err(eyre::eyre!(
                    "Transaction {} failed on {}: {}",
                    tx_id,
                    config.network,
                    error
                ));
            }
            Some(TransactionStatus::Canceled) => {
                return Err(eyre::eyre!(
                    "Transaction {} was canceled on {}",
                    tx_id,
                    config.network
                ));
            }
            Some(TransactionStatus::Expired) => {
                return Err(eyre::eyre!(
                    "Transaction {} expired on {}",
                    tx_id,
                    config.network
                ));
            }
            Some(TransactionStatus::Pending)
            | Some(TransactionStatus::Sent)
            | Some(TransactionStatus::Submitted) => {
                // Still processing, wait and poll again
                tokio::time::sleep(poll_interval).await;
            }
            None => {
                // Log unknown status but continue polling
                warn!(
                    status = %tx_response.status,
                    tx_id = %tx_id,
                    network = %config.network,
                    "Unknown transaction status, continuing to poll"
                );
                tokio::time::sleep(poll_interval).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_receipt_config_from_network_defaults() {
        // When network is not found, should use defaults
        let config = ReceiptConfig::from_network("non-existent-network").unwrap();

        // Default block time: 12000ms, confirmations: 6
        // poll_interval = 12000 / 2 = 6000ms
        // max_wait = 12000 * 6 * 3 = 216000ms
        assert_eq!(config.poll_interval_ms, 6000);
        assert_eq!(config.max_wait_ms, 216_000);
    }

    #[test]
    fn test_receipt_config_custom() {
        let config = ReceiptConfig::custom("test-network", 2000, 60000);

        assert_eq!(config.poll_interval_ms, 2000);
        assert_eq!(config.max_wait_ms, 60000);
        assert_eq!(config.network, "test-network");
    }

    #[test]
    fn test_receipt_config_durations() {
        let config = ReceiptConfig::custom("test", 1000, 30000);

        assert_eq!(config.poll_interval(), Duration::from_millis(1000));
        assert_eq!(config.max_wait(), Duration::from_millis(30000));
    }

    #[test]
    fn test_parse_status() {
        assert_eq!(parse_status("pending"), Some(TransactionStatus::Pending));
        assert_eq!(
            parse_status("submitted"),
            Some(TransactionStatus::Submitted)
        );
        assert_eq!(
            parse_status("confirmed"),
            Some(TransactionStatus::Confirmed)
        );
        assert_eq!(parse_status("mined"), Some(TransactionStatus::Mined));
        assert_eq!(parse_status("failed"), Some(TransactionStatus::Failed));
        assert_eq!(parse_status("sent"), Some(TransactionStatus::Sent));
        assert_eq!(parse_status("canceled"), Some(TransactionStatus::Canceled));
        assert_eq!(parse_status("expired"), Some(TransactionStatus::Expired));
        assert_eq!(parse_status("PENDING"), Some(TransactionStatus::Pending)); // Case insensitive
        assert_eq!(parse_status("other"), None); // Unknown status
    }

    #[test]
    fn test_load_network_timing_with_config() {
        // This test will use real config if available
        let result = load_network_timing("sepolia");
        assert!(result.is_ok());

        let (block_time, confirmations) = result.unwrap();
        // Should have valid values (either from config or defaults)
        assert!(block_time > 0);
        assert!(confirmations > 0);
    }

    #[test]
    fn test_receipt_config_minimum_values() {
        // Test that minimums are enforced
        // For a very fast network (100ms blocks, 1 confirmation)
        // poll_interval should be at least 1000ms
        // max_wait should be at least 30000ms

        let config = ReceiptConfig::custom("fast-network", 50, 100);
        // Custom doesn't enforce minimums, but from_network does

        assert_eq!(config.poll_interval_ms, 50); // Custom allows any value

        // from_network enforces minimums
        let config2 = ReceiptConfig::from_network("non-existent").unwrap();
        assert!(config2.poll_interval_ms >= 1000);
        assert!(config2.max_wait_ms >= 30000);
    }
}
