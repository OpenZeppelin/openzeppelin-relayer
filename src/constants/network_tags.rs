/// Network tag constants for EVM networks
pub const NO_MEMPOOL_TAG: &str = "no-mempool";
pub const ARBITRUM_BASED_TAG: &str = "arbitrum-based";
pub const OPTIMISM_BASED_TAG: &str = "optimism-based";
/// @deprecated Use OPTIMISM_BASED_TAG instead. Will be removed in a future version.
pub const OPTIMISM_TAG: &str = "optimism";
pub const ROLLUP_TAG: &str = "rollup";
pub const LACKS_MEMPOOL_TAGS: [&str; 4] = [
    NO_MEMPOOL_TAG,
    ARBITRUM_BASED_TAG,
    OPTIMISM_BASED_TAG,
    OPTIMISM_TAG,
];
