//! Midnight transaction domain implementation
//!
//! This module provides the core transaction handling logic for the Midnight network,
//! including transaction building, signing, and submission.

pub mod builder;
pub mod midnight_transaction;
pub mod types;

pub use builder::MidnightTransactionBuilder;
pub use midnight_transaction::MidnightTransaction;
pub use types::*;
