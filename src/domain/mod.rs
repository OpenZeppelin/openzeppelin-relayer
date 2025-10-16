//! # Domain Module
//!
//! Core domain logic for the relayer service, implementing:
//!
//! * Transaction processing
//! * Relayer management
//! * Network-specific implementations
//! * Shared network-specific domain logic

pub mod relayer;
pub use relayer::*;

pub mod transaction;
pub use transaction::*;

// Shared Solana domain logic (validation, utilities)
pub mod solana;
