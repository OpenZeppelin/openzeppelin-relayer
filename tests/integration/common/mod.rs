//! Common utilities and helpers for integration tests

pub mod client;
pub mod confirmation;
pub mod context;
pub mod evm_helpers;
pub mod logging;
pub mod network_selection;
pub mod registry;

/// Returns whether strict E2E mode is enabled via the `STRICT_E2E` env var.
///
/// When `true` (the default in CI via `STRICT_E2E=true`), missing env vars and
/// unreachable services cause panics/failures instead of silent skips.
/// Defaults to `false` so local runs without the relayer are permissive.
pub fn strict_e2e_enabled() -> bool {
    std::env::var("STRICT_E2E")
        .map(|v| !matches!(v.to_lowercase().as_str(), "0" | "false" | "no" | "off"))
        .unwrap_or(false)
}
