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
//! - `api`: API routes and handlers
//! - `bootstrap`: Bootstrap and initialization
//! - `config`: Configuration management
//! - `constants`: Constants and environment variables
//! - `domain`: Domain-specific logic
//! - `jobs`: Job scheduling and execution
//! - `logging`: Logging and tracing
//! - `metrics`: Metrics and monitoring
//! - `models`: Data structures for configuration and blockchain data
//! - `repositories`: Configuration storage and management
//! - `services`: Core business logic and blockchain interaction
//! - `utils`: Common utilities and helper functions

#[cfg(all(feature = "redis-tls-native", feature = "redis-tls-rustls"))]
compile_error!(
    "Features `redis-tls-native` and `redis-tls-rustls` are mutually exclusive. \
     Please enable only one."
);

pub mod api;
pub mod bootstrap;
pub mod config;
pub mod constants;
pub mod domain;
pub mod jobs;
pub mod logging;
pub mod metrics;
pub mod models;
pub mod observability;
pub mod openapi;
pub mod queues;
pub mod repositories;
pub mod services;
pub mod utils;
