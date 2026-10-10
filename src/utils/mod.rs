mod serde;

pub use serde::*;

mod key;
pub use key::*;

mod auth;
pub use auth::*;

mod polling;
pub use polling::*;

mod time;
pub use time::*;

mod transaction;
pub use transaction::*;

mod base64;
pub use base64::*;

mod address_derivation;
pub use address_derivation::*;

#[cfg(fuzzing)]
pub mod der;
#[cfg(not(fuzzing))]
mod der;
pub use der::*;

mod secp256k;
pub use secp256k::*;

mod ed25519;
pub use ed25519::*;

mod redis;
pub use redis::*;

mod service_info_log;
pub use service_info_log::*;

mod uuid;
pub use uuid::*;

mod encryption;
pub use encryption::*;

mod encryption_context;
pub use encryption_context::*;

mod json_rpc_error;
pub use json_rpc_error::*;

mod url_security;
pub use url_security::*;
mod error_sanitization;
pub use error_sanitization::*;

pub mod aws_error;
// `classify_sdk_error` returns a stable kind tag and is safe to embed in
// returned errors, so we re-export it at `utils::`. `DisplayErrorContext`
// is intentionally NOT re-exported here — it can leak source-chain details
// and is log-only; callers must import it via the fully qualified
// `utils::aws_error::DisplayErrorContext` path to keep the misuse risk visible.
pub use aws_error::classify_sdk_error;

mod url;
pub use url::*;

#[cfg(test)]
pub mod mocks;
