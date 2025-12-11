//!  Integration tests for the OpenZeppelin Relayer.
//!
//! Contains tests for relayer functionality

#![cfg(feature = "integration-tests")]

mod integration {
    mod api;
    mod authorization;
    pub mod common;
    mod logging;
    mod networks;
}
