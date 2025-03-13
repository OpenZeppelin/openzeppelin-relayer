//! # Domain Module
//!
//! Core domain logic for the relayer service, implementing:
//!
//! * Transaction processing
//! * Relayer management
//! * Network-specific implementations

mod relayer;
pub use relayer::*;

pub mod transaction;
pub use transaction::*;
