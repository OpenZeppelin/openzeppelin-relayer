//! Test context utilities for integration tests
//!
//! Provides reusable test context structs and utilities that bundle common setup logic,
//! reducing boilerplate in individual tests.
//!
//! ## Single-Network Tests
//!
//! Use [`EvmTestContext`] for simple tests that only need one EVM network:
//!
//! ```ignore
//! let ctx = EvmTestContext::new().expect("Failed to setup test");
//! ctx.client.create_relayer(...).await?;
//! ```
//!
//! ## Multi-Network Tests
//!
//! Use [`run_multi_network_test`] to run a test function across all eligible networks:
//!
//! ```ignore
//! run_multi_network_test("my_test", is_evm_network, my_test_fn).await;
//! ```

use super::{
    client::RelayerClient,
    logging::init_test_logging,
    network_selection::get_test_networks,
    registry::{NetworkConfig, TestRegistry},
};
use eyre::{ensure, Result};
use std::future::Future;
use tracing::{error, info, info_span};

/// Test context for EVM network integration tests
///
/// Bundles the common setup needed for EVM tests:
/// - Network selection and validation
/// - Registry loading
/// - Client initialization
///
/// # Example
///
/// ```ignore
/// let ctx = EvmTestContext::new().expect("Failed to setup test");
/// ctx.client.delete_all_relayers_by_network(&ctx.network).await?;
/// // ... test logic using ctx.client, ctx.network, ctx.network_config
/// ```
#[derive(Debug)]
pub struct EvmTestContext {
    /// HTTP client for the relayer API
    pub client: RelayerClient,
    /// Selected EVM network name (e.g., "sepolia")
    pub network: String,
    /// Network configuration from the registry
    pub network_config: NetworkConfig,
}

impl EvmTestContext {
    /// Creates a new EVM test context
    ///
    /// This performs the following setup:
    /// 1. Loads test networks from environment/config
    /// 2. Loads the test registry
    /// 3. Finds the first EVM network from the selection
    /// 4. Creates the relayer client
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - No networks are selected for testing
    /// - No EVM network is found in the selection
    /// - The registry cannot be loaded
    /// - The client cannot be initialized
    pub fn new() -> Result<Self> {
        let networks = get_test_networks()?;
        ensure!(!networks.is_empty(), "No networks selected for testing");

        let registry = TestRegistry::load()?;

        let network = networks
            .iter()
            .find(|n| {
                registry
                    .get_network(n)
                    .map(|c| c.network_type == "evm")
                    .unwrap_or(false)
            })
            .ok_or_else(|| eyre::eyre!("No EVM network found in selection"))?
            .clone();

        let network_config = registry.get_network(&network)?.clone();
        let client = RelayerClient::from_env()?;

        Ok(Self {
            client,
            network,
            network_config,
        })
    }

    /// Cleans up all relayers for the test network
    ///
    /// Returns the number of relayers deleted
    pub async fn cleanup_relayers(&self) -> Result<usize> {
        self.client
            .delete_all_relayers_by_network(&self.network)
            .await
    }
}

// =============================================================================
// Multi-Network Test Runner
// =============================================================================

/// Runs a test function across multiple networks with standard logging and failure handling.
///
/// This utility encapsulates the common boilerplate for multi-network integration tests:
/// - Initializes test logging
/// - Loads networks and registry
/// - Filters networks using the provided predicate
/// - Runs the test for each eligible network
/// - Collects and reports failures
/// - Panics if any test failed
///
/// # Arguments
///
/// * `test_name` - Name of the test for logging purposes
/// * `network_filter` - Predicate that determines if a network should be tested
/// * `test_fn` - Async function that runs the actual test for a single network
///
/// # Example
///
/// ```ignore
/// #[tokio::test]
/// #[serial]
/// async fn test_evm_basic_transfer() {
///     run_multi_network_test(
///         "basic_transfer",
///         is_evm_network,
///         run_basic_transfer_test,
///     ).await;
/// }
/// ```
pub async fn run_multi_network_test<F, Fut>(
    test_name: &str,
    network_filter: impl Fn(&str, &TestRegistry) -> bool,
    test_fn: F,
) where
    F: Fn(String) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    init_test_logging();
    let _span = info_span!("multi_network_test", name = %test_name).entered();

    info!("========================================");
    info!("Starting {} test", test_name);
    info!("========================================");

    let networks = get_test_networks().expect("Failed to get test networks");
    if networks.is_empty() {
        panic!("No networks selected for testing");
    }

    info!(
        count = networks.len(),
        networks = ?networks,
        "Enabled networks from registry.json"
    );

    let registry = TestRegistry::load().expect("Failed to load test registry");

    let eligible: Vec<String> = networks
        .clone()
        .into_iter()
        .filter(|n| network_filter(n, &registry))
        .collect();

    // Show which networks were filtered out
    let filtered_out: Vec<String> = networks
        .into_iter()
        .filter(|n| !eligible.contains(n))
        .collect();

    if !filtered_out.is_empty() {
        info!(
            count = filtered_out.len(),
            networks = ?filtered_out,
            "Networks filtered out (don't match test criteria)"
        );
    }

    if eligible.is_empty() {
        info!("No eligible networks for {}, skipping test", test_name);
        return;
    }

    info!("========================================");
    info!(
        count = eligible.len(),
        networks = ?eligible,
        "Running {} test on eligible networks",
        test_name
    );
    info!("========================================");

    let mut failures = Vec::new();
    for network in &eligible {
        match test_fn(network.clone()).await {
            Ok(()) => info!(network = %network, "PASS"),
            Err(e) => {
                error!(network = %network, error = %e, "FAIL");
                failures.push((network.clone(), e.to_string()));
            }
        }
    }

    // Report results
    info!("========================================");
    info!("{} Test Results", test_name);
    info!("========================================");
    info!(
        passed = eligible.len() - failures.len(),
        failed = failures.len(),
        total = eligible.len(),
        "Summary"
    );

    if !failures.is_empty() {
        error!("Failed networks:");
        for (network, error) in &failures {
            error!(network = %network, error = %error, "Test failed");
        }
        info!("========================================");
        panic!(
            "{} of {} {} tests failed",
            failures.len(),
            eligible.len(),
            test_name
        );
    }

    info!("All {} tests passed!", test_name);
    info!("========================================");
}

// =============================================================================
// Network Filter Predicates
// =============================================================================

/// Filter predicate for EVM networks.
///
/// Returns `true` if the network is configured as an EVM network.
///
/// # Example
///
/// ```ignore
/// run_multi_network_test("my_test", is_evm_network, my_test_fn).await;
/// ```
pub fn is_evm_network(network: &str, registry: &TestRegistry) -> bool {
    registry
        .get_network(network)
        .map(|c| c.network_type == "evm")
        .unwrap_or(false)
}

/// Creates a filter predicate for EVM networks with a specific contract deployed.
///
/// Returns a closure that checks if a network is EVM and has the specified contract.
///
/// # Example
///
/// ```ignore
/// run_multi_network_test(
///     "contract_test",
///     evm_with_contract("simple_storage"),
///     my_test_fn,
/// ).await;
/// ```
pub fn evm_with_contract(contract_name: &str) -> impl Fn(&str, &TestRegistry) -> bool + '_ {
    move |network, registry| {
        is_evm_network(network, registry)
            && registry
                .has_real_contract(network, contract_name)
                .unwrap_or(false)
    }
}
