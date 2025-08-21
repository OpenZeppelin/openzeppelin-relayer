//! Indexer integration module for Midnight blockchain
//!
//! This module provides the client and types for interacting with the Midnight GraphQL indexer.
//! The indexer tracks blockchain state and provides APIs for querying transactions, blocks,
//! and wallet-specific data using viewing keys.

/// GraphQL client for interacting with the Midnight indexer
mod client;
/// Type definitions for indexer data structures
mod types;

pub use client::MidnightIndexerClient;
pub use types::*;
