//! Constants for Stellar transaction processing.
//!
//! This module contains default values used throughout the Stellar transaction
//! handling logic, including fees, retry delays, and timeout thresholds.
//!
//! ## Gas Abstraction Contract Addresses
//!
//! This module also contains default contract addresses for Soroban gas abstraction:
//! - **Soroswap**: AMM DEX for token-to-XLM quotes and swaps
//! - **FeeForwarder**: Contract that enables fee payment in tokens instead of XLM
//!
//! These addresses can be overridden via environment variables if needed.
//! See `ServerConfig` for the corresponding env var names.

use chrono::Duration;

// =============================================================================
// Transaction Fees
// =============================================================================

pub const STELLAR_DEFAULT_TRANSACTION_FEE: u32 = 100;
/// Default maximum fee for fee-bump transactions (0.1 XLM = 1,000,000 stroops)
pub const STELLAR_DEFAULT_MAX_FEE: i64 = 1_000_000;
/// Maximum number of operations allowed in a Stellar transaction
pub const STELLAR_MAX_OPERATIONS: usize = 100;

/// Horizon API base URL for Stellar mainnet
pub const STELLAR_HORIZON_MAINNET_URL: &str = "https://horizon.stellar.org";
/// Horizon API base URL for Stellar testnet
pub const STELLAR_HORIZON_TESTNET_URL: &str = "https://horizon-testnet.stellar.org";

// Status check scheduling
/// Initial delay before first status check (in seconds)
/// Set to 2s for faster detection of transaction state changes
pub const STELLAR_STATUS_CHECK_INITIAL_DELAY_SECONDS: i64 = 2;

/// Minimum age before triggering Pending status recovery (in seconds)
/// Only schedule a recovery job if Pending transaction without hash exceeds this age
/// This prevents scheduling a job on every status check
pub const STELLAR_PENDING_RECOVERY_TRIGGER_SECONDS: i64 = 10;

// Transaction validity
/// Default transaction validity duration (in minutes) for sponsored transactions
/// Provides reasonable time for users to review and submit while ensuring transaction doesn't expire too quickly
pub const STELLAR_SPONSORED_TRANSACTION_VALIDITY_MINUTES: i64 = 2;

/// Get status check initial delay duration
pub fn get_stellar_status_check_initial_delay() -> Duration {
    Duration::seconds(STELLAR_STATUS_CHECK_INITIAL_DELAY_SECONDS)
}

/// Get sponsored transaction validity duration
pub fn get_stellar_sponsored_transaction_validity_duration() -> Duration {
    Duration::minutes(STELLAR_SPONSORED_TRANSACTION_VALIDITY_MINUTES)
}

// Recovery thresholds
/// Minimum time before re-queuing submit job for stuck Sent transactions (30 seconds)
/// Gives the original submit job time to complete before attempting recovery.
pub const STELLAR_RESEND_TIMEOUT_SECONDS: i64 = 30;

/// Maximum lifetime for stuck transactions (Sent, Pending, Submitted) before marking as Failed (15 minutes)
/// Safety net for transactions without time bounds - prevents infinite retries.
pub const STELLAR_MAX_STUCK_TRANSACTION_LIFETIME_MINUTES: i64 = 15;

/// Get resend timeout duration for stuck Sent transactions
pub fn get_stellar_resend_timeout() -> Duration {
    Duration::seconds(STELLAR_RESEND_TIMEOUT_SECONDS)
}

/// Get max lifetime duration for stuck transactions (Sent, Pending, Submitted)
pub fn get_stellar_max_stuck_transaction_lifetime() -> Duration {
    Duration::minutes(STELLAR_MAX_STUCK_TRANSACTION_LIFETIME_MINUTES)
}

// =============================================================================
// Soroswap DEX Contract Addresses
// =============================================================================
// Official Soroswap deployments from:
// - Mainnet: https://github.com/soroswap/core/blob/main/public/mainnet.contracts.json
// - Testnet: https://github.com/soroswap/core/blob/main/public/testnet.contracts.json

/// Soroswap router contract address on Stellar mainnet
pub const STELLAR_SOROSWAP_MAINNET_ROUTER: &str =
    "CAG5LRYQ5JVEUI5TEID72EYOVX44TTUJT5BQR2J6J77FH65PCCFAJDDH";

/// Soroswap router contract address on Stellar testnet
pub const STELLAR_SOROSWAP_TESTNET_ROUTER: &str =
    "CCJUD55AG6W5HAI5LRVNKAE5WDP5XGZBUDS5WNTIVDU7O264UZZE7BRD";

/// Soroswap factory contract address on Stellar mainnet
pub const STELLAR_SOROSWAP_MAINNET_FACTORY: &str =
    "CA4HEQTL2WPEUYKYKCDOHCDNIV4QHNJ7EL4J4NQ6VADP7SYHVRYZ7AW2";

/// Soroswap factory contract address on Stellar testnet
pub const STELLAR_SOROSWAP_TESTNET_FACTORY: &str =
    "CDP3HMUH6SMS3S7NPGNDJLULCOXXEPSHY4JKUKMBNQMATHDHWXRRJTBY";

/// Native XLM wrapper token contract address on Stellar mainnet
/// This is the Soroban token contract that wraps native XLM for use in Soroswap
pub const STELLAR_SOROSWAP_MAINNET_NATIVE_WRAPPER: &str =
    "CAS3J7GYLGXMF6TDJBBYYSE3HQ6BBSMLNUQ34T6TZMYMW2EVH34XOWMA";

/// Native XLM wrapper token contract address on Stellar testnet
pub const STELLAR_SOROSWAP_TESTNET_NATIVE_WRAPPER: &str =
    "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC";

// =============================================================================
// FeeForwarder Contract Addresses
// =============================================================================
// The FeeForwarder contract enables gas abstraction by allowing users to pay
// transaction fees in tokens instead of native XLM.

/// FeeForwarder contract address on Stellar mainnet
/// Set to empty string until mainnet deployment is available
pub const STELLAR_FEE_FORWARDER_MAINNET: &str = "";

/// FeeForwarder contract address on Stellar testnet
pub const STELLAR_FEE_FORWARDER_TESTNET: &str =
    "CADRAMMYU62IUEQZEKG5MYVL7DYMH424ETG7QRZ5BVGP552DZNR4MH2Q";

// =============================================================================
// Helper Functions for Network-Aware Address Resolution
// =============================================================================

/// Get the default Soroswap router contract address for the given network
///
/// Returns the official Soroswap router deployment address.
/// Users can override this via the `STELLAR_SOROSWAP_ROUTER_ADDRESS` env var.
pub fn get_default_soroswap_router(is_testnet: bool) -> &'static str {
    if is_testnet {
        STELLAR_SOROSWAP_TESTNET_ROUTER
    } else {
        STELLAR_SOROSWAP_MAINNET_ROUTER
    }
}

/// Get the default Soroswap factory contract address for the given network
///
/// Returns the official Soroswap factory deployment address.
/// Users can override this via the `STELLAR_SOROSWAP_FACTORY_ADDRESS` env var.
pub fn get_default_soroswap_factory(is_testnet: bool) -> &'static str {
    if is_testnet {
        STELLAR_SOROSWAP_TESTNET_FACTORY
    } else {
        STELLAR_SOROSWAP_MAINNET_FACTORY
    }
}

/// Get the default native XLM wrapper token address for the given network
///
/// Returns the Soroban token contract that wraps native XLM for use in Soroswap.
/// Users can override this via the `STELLAR_SOROSWAP_NATIVE_WRAPPER_ADDRESS` env var.
pub fn get_default_soroswap_native_wrapper(is_testnet: bool) -> &'static str {
    if is_testnet {
        STELLAR_SOROSWAP_TESTNET_NATIVE_WRAPPER
    } else {
        STELLAR_SOROSWAP_MAINNET_NATIVE_WRAPPER
    }
}

/// Get the default FeeForwarder contract address for the given network
///
/// Returns the FeeForwarder deployment address, or None if not yet deployed.
/// Users can override this via the `STELLAR_FEE_FORWARDER_ADDRESS` env var.
pub fn get_default_fee_forwarder(is_testnet: bool) -> &'static str {
    if is_testnet {
        STELLAR_FEE_FORWARDER_TESTNET
    } else {
        STELLAR_FEE_FORWARDER_MAINNET
    }
}
