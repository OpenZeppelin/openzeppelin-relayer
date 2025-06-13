//! RPC error codes module
//!
//! This module contains error code constants for JSON-RPC implementations:
//! - Common JSON-RPC 2.0 standard error codes
//! - OpenZeppelin-specific error codes for extended functionality
//! - Network-specific error codes (Solana, EVM, Stellar)

pub mod common_codes;
pub mod openzeppelin_codes;

pub use common_codes::*;
pub use openzeppelin_codes::*;
