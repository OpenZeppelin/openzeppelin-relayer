//! Blockchain Transaction Service Library
//!
//! This library provides functionality for sending/broadcasting transactions to blockchain networks
//! and triggering notifications based on transaction events. It includes:
//!
//! - Configuration management through JSON files
//! - Blockchain network transaction broadcasting
//! - Customizable webhook notifications
//! - Extensible repository and service architecture
//!
//! # Module Structure
//!
//! - `config`: Configuration management
//! - `logging`: Logging and tracing
//! - `models`: Data structures for configuration and blockchain data
//! - `repositories`: Configuration storage and management
//! - `services`: Core business logic and blockchain interaction
//! - `utils`: Common utilities and helper functions

pub mod logging;
