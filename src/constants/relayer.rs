//! Default constants for relayer configuration across different blockchain networks
//! These values are used to ensure relayers maintain sufficient funds and operate with safe defaults.

// === Network Minimum Balance Defaults ===
pub const DEFAULT_EVM_MIN_BALANCE: u128 = 1; // 0.001 ETH in wei
pub const DEFAULT_STELLAR_MIN_BALANCE: u64 = 1_000_000; // 1 XLM
pub const DEFAULT_SOLANA_MIN_BALANCE: u64 = 10_000_000; // 0.01 SOL in lamports

// === EVM Policy Defaults ===
/// Default gas price cap: 100 gwei in wei
pub const DEFAULT_EVM_GAS_PRICE_CAP: u128 = 100_000_000_000;
/// Default EIP-1559 pricing enabled
pub const DEFAULT_EVM_EIP1559_ENABLED: bool = true;
/// Default gas limit estimation enabled  
pub const DEFAULT_EVM_GAS_LIMIT_ESTIMATION: bool = true;
/// Default private transactions disabled
pub const DEFAULT_EVM_PRIVATE_TRANSACTIONS: bool = false;

// === Solana Policy Defaults ===
/// Default fee margin percentage for Solana transactions
pub const DEFAULT_SOLANA_FEE_MARGIN_PERCENTAGE: f32 = 5.0; // 5%
/// Default maximum transaction data size for Solana
pub const DEFAULT_SOLANA_MAX_TX_DATA_SIZE: u16 = 1232;
/// Default maximum signatures for Solana transactions
pub const DEFAULT_SOLANA_MAX_SIGNATURES: u8 = 8;
/// Default maximum allowed fee for Solana transactions (0.1 SOL)
pub const DEFAULT_SOLANA_MAX_ALLOWED_FEE: u64 = 100_000_000; // lamports

// === Stellar Policy Defaults ===
/// Default maximum fee for Stellar transactions (10 stroops)
pub const DEFAULT_STELLAR_MAX_FEE: u32 = 10;
/// Default timeout for Stellar transactions (30 seconds)
pub const DEFAULT_STELLAR_TIMEOUT_SECONDS: u64 = 30;

// === Token Swap Defaults ===
/// Default slippage percentage for token swaps
pub const DEFAULT_TOKEN_SWAP_SLIPPAGE: f32 = 1.0; // 1%

// === Legacy Constants ===
pub const MAX_SOLANA_TX_DATA_SIZE: u16 = 1232;
pub const EVM_SMALLEST_UNIT_NAME: &str = "wei";
pub const ZERO_ADDRESS: &str = "0x0000000000000000000000000000000000000000";
#[allow(dead_code)]
pub const STELLAR_SMALLEST_UNIT_NAME: &str = "stroop";
pub const SOLANA_SMALLEST_UNIT_NAME: &str = "lamport";

pub const DEFAULT_RPC_WEIGHT: u8 = 100;
