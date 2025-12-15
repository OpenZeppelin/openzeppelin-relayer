//! Test context utilities for integration tests
//!
//! Provides utilities for running test functions across all eligible networks.
//!
//! ## Multi-Network Tests
//!
//! Use [`run_multi_network_test`] to run a test function across all eligible networks:
//!
//! ```ignore
//! run_multi_network_test("my_test", is_evm_network, my_test_fn).await;
//! ```

use super::{
    logging::init_test_logging,
    network_selection::get_test_networks,
    registry::{RelayerDiscovery, RelayerInfo, TestRegistry},
};
use eyre::Result;
use std::future::Future;
use tracing::{error, info, info_span, warn};

// =============================================================================
// Multi-Network Test Runner
// =============================================================================

/// Runs a test function across multiple networks with standard logging and failure handling.
///
/// This utility encapsulates the common boilerplate for multi-network integration tests:
/// - Initializes test logging
/// - Loads networks and registry
/// - Filters networks using the provided predicate
/// - Discovers ALL active relayers for each eligible network
/// - Runs the test for each network × relayer combination
/// - Collects and reports failures
/// - Panics if any test failed
///
/// # Arguments
///
/// * `test_name` - Name of the test for logging purposes
/// * `network_filter` - Predicate that determines if a network should be tested
/// * `test_fn` - Async function that runs the actual test for a network and relayer
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
    F: Fn(String, RelayerInfo) -> Fut,
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
    let mut total_tests = 0;

    for network in &eligible {
        // Discover ALL active relayers for this network
        let relayers = match RelayerDiscovery::find_relayers_for_network(network) {
            Ok(relayers) => relayers,
            Err(e) => {
                error!(network = %network, error = %e, "Failed to discover relayers");
                failures.push((network.clone(), "N/A".to_string(), e.to_string()));
                continue;
            }
        };

        if relayers.is_empty() {
            warn!(network = %network, "No active relayers found, skipping");
            continue;
        }

        info!(
            network = %network,
            count = relayers.len(),
            relayers = ?relayers.iter().map(|r| &r.id).collect::<Vec<_>>(),
            "Testing with ALL active relayers"
        );

        // Test each relayer sequentially
        for relayer in relayers {
            total_tests += 1;
            match test_fn(network.clone(), relayer.clone()).await {
                Ok(()) => {
                    info!(
                        network = %network,
                        relayer = %relayer.id,
                        "PASS"
                    );
                }
                Err(e) => {
                    error!(
                        network = %network,
                        relayer = %relayer.id,
                        error = %e,
                        "FAIL"
                    );
                    failures.push((network.clone(), relayer.id.clone(), e.to_string()));
                }
            }
        }
    }

    // Report results
    info!("========================================");
    info!("{} Test Results", test_name);
    info!("========================================");
    info!(
        passed = total_tests - failures.len(),
        failed = failures.len(),
        total = total_tests,
        "Summary"
    );

    if !failures.is_empty() {
        error!("Failed tests:");
        for (network, relayer_id, error) in &failures {
            error!(
                network = %network,
                relayer = %relayer_id,
                error = %error,
                "Test failed"
            );
        }
        info!("========================================");
        panic!(
            "{} of {} {} tests failed",
            failures.len(),
            total_tests,
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
