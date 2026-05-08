//! Helpers for diagnosing AWS SDK errors.
//!
//! `SdkError`'s `Display` impl collapses everything below the SDK
//! (DNS, TCP, TLS, connector pool, credential providers) into a single
//! short string like `"dispatch failure"`, which makes prod logs nearly
//! useless for distinguishing root causes.
//!
//! This module provides two utilities meant to be paired at every AWS SDK
//! call-site:
//!
//! * [`classify_sdk_error`] — returns a stable, low-cardinality `&'static str`
//!   suitable for a `tracing` field or metric label, distinguishing the
//!   actionable subcategories of `DispatchFailure` (timeout / io / user /
//!   other) from `TimeoutError`, `ServiceError`, etc.
//! * [`DisplayErrorContext`] — re-export of the SDK's own helper that walks
//!   the full `std::error::Error::source()` chain so the underlying cause
//!   (e.g., `connect timed out`, `dns error: failed to lookup address`)
//!   appears in the log instead of just the top-level wrapper.
//!
//! Typical usage:
//!
//! ```ignore
//! tracing::error!(
//!     error.kind = classify_sdk_error(&err),
//!     error.detail = %DisplayErrorContext(&err),
//!     "AWS call failed"
//! );
//! ```

pub use aws_smithy_types::error::display::DisplayErrorContext;

use aws_smithy_runtime_api::client::result::SdkError;

/// Classify an [`SdkError`] into a stable, low-cardinality kind tag.
///
/// `DispatchFailure` is split by its underlying [`ConnectorError`] kind so
/// log aggregators can distinguish a `dispatch_timeout` (likely runtime
/// starvation or a slow upstream) from a `dispatch_io` (connection reset,
/// pool exhaustion) without parsing free-form strings.
///
/// [`ConnectorError`]: aws_smithy_runtime_api::client::result::ConnectorError
pub fn classify_sdk_error<E, R>(err: &SdkError<E, R>) -> &'static str {
    match err {
        SdkError::ConstructionFailure(_) => "construction",
        SdkError::TimeoutError(_) => "timeout",
        SdkError::DispatchFailure(inner) => match inner.as_connector_error() {
            Some(ce) if ce.is_timeout() => "dispatch_timeout",
            Some(ce) if ce.is_io() => "dispatch_io",
            Some(ce) if ce.is_user() => "dispatch_user",
            Some(_) => "dispatch_other",
            None => "dispatch_unknown",
        },
        SdkError::ResponseError(_) => "response_parse",
        SdkError::ServiceError(_) => "service",
        // SdkError is `#[non_exhaustive]`; future variants get a stable label.
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_smithy_runtime_api::client::orchestrator::HttpResponse;
    use aws_smithy_runtime_api::client::result::ConnectorError;
    use std::convert::Infallible;
    use std::io;

    // SdkError<E, R> — E is the operation error type, R is the response type.
    // ConstructionFailure / TimeoutError / DispatchFailure don't actually carry
    // an E, so Infallible is the cheapest stand-in. R is supplied explicitly
    // because aws-smithy-runtime-api 1.11+ no longer defaults it.
    type TestErr = SdkError<Infallible, HttpResponse>;

    fn boxed(msg: &str) -> Box<dyn std::error::Error + Send + Sync> {
        Box::new(io::Error::other(msg.to_string()))
    }

    #[test]
    fn classifies_construction_failure() {
        let err: TestErr = SdkError::construction_failure(boxed("could not build request"));
        assert_eq!(classify_sdk_error(&err), "construction");
    }

    #[test]
    fn classifies_timeout_error() {
        let err: TestErr = SdkError::timeout_error(boxed("operation timed out"));
        assert_eq!(classify_sdk_error(&err), "timeout");
    }

    #[test]
    fn classifies_dispatch_failure_timeout() {
        // Connector-level timeout is the most likely shape under runtime
        // saturation; this kind tag is what should drive the "we're starving
        // the AWS SDK connector futures" diagnosis.
        let err: TestErr =
            SdkError::dispatch_failure(ConnectorError::timeout(boxed("connect timed out")));
        assert_eq!(classify_sdk_error(&err), "dispatch_timeout");
    }

    #[test]
    fn classifies_dispatch_failure_io() {
        let err: TestErr =
            SdkError::dispatch_failure(ConnectorError::io(boxed("connection reset by peer")));
        assert_eq!(classify_sdk_error(&err), "dispatch_io");
    }

    #[test]
    fn classifies_dispatch_failure_user() {
        let err: TestErr =
            SdkError::dispatch_failure(ConnectorError::user(boxed("invalid endpoint URL")));
        assert_eq!(classify_sdk_error(&err), "dispatch_user");
    }

    #[test]
    fn classifies_dispatch_failure_other() {
        let err: TestErr =
            SdkError::dispatch_failure(ConnectorError::other(boxed("unexpected"), None));
        assert_eq!(classify_sdk_error(&err), "dispatch_other");
    }

    #[test]
    fn display_error_context_surfaces_underlying_cause() {
        // Re-pins the behaviour the helper relies on: DisplayErrorContext must
        // walk source() chains, otherwise the prod logs would still collapse to
        // "dispatch failure" and we'd be back where we started.
        let inner = io::Error::new(io::ErrorKind::TimedOut, "tcp connect timed out at layer 4");
        let err: TestErr = SdkError::dispatch_failure(ConnectorError::timeout(Box::new(inner)));
        let rendered = format!("{}", DisplayErrorContext(&err));
        assert!(
            rendered.contains("tcp connect timed out at layer 4"),
            "DisplayErrorContext should expose the inner cause; got: {rendered}"
        );
    }
}
