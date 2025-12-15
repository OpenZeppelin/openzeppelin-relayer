//!  Integration tests for the OpenZeppelin Relayer.
//!
//! Contains tests for relayer functionality

#![cfg(feature = "integration-tests")]

mod integration {
    pub mod common;
    mod tests;

    // Initialize logging before any tests run
    // This ensures our enhanced network selection logs are visible
    #[ctor::ctor]
    fn init() {
        common::logging::init_test_logging();
    }
}
