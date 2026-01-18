//! Initialization routines for the relayer system
//!
//! This module contains functions and utilities for initializing various
//! components of the relayer system, including relayers, configuration,
//! application state, workers, and plugins.
//!
//! # Submodules
//!
//! - `initialize_relayers`: Functions for initializing relayers
//! - `config_processor`: Functions for processing configuration files
//! - `initialize_app_state`: Functions for initializing application state
//! - `initialize_workers`: Functions for initializing background workers
//! - `initialize_plugins`: Functions for initializing the plugin worker pool
mod initialize_relayers;
pub use initialize_relayers::*;

mod config_processor;
pub use config_processor::*;

mod initialize_app_state;
pub use initialize_app_state::*;

mod initialize_workers;
pub use initialize_workers::*;

mod initialize_plugins;
pub use initialize_plugins::*;
