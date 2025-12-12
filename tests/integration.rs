//!  Integration tests for the OpenZeppelin Relayer.
//!
//! Contains tests for relayer functionality

#![cfg(feature = "integration-tests")]

mod integration {
    mod authorization;
    pub mod common;
    mod logging;
    mod networks;

    // Initialize logging before any tests run
    // This ensures our enhanced network selection logs are visible
    #[ctor::ctor]
    fn init() {
        common::logging::init_test_logging();
    }
}
