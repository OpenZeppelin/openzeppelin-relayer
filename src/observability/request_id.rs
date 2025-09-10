use tracing::Span;
use tracing_subscriber::{registry::LookupSpan, Registry};

#[derive(Clone, Debug)]
pub struct RequestId(pub String);

pub fn set_request_id(id: impl Into<String>) {
    let id = id.into();
    Span::current().with_subscriber(|(span_id, subscriber)| {
        if let Some(reg) = subscriber.downcast_ref::<Registry>() {
            if let Some(span_ref) = reg.span(span_id) {
                span_ref.extensions_mut().replace(RequestId(id));
            }
        }
    });
}

pub fn get_request_id() -> Option<String> {
    let mut out = None;
    Span::current().with_subscriber(|(span_id, subscriber)| {
        if let Some(reg) = subscriber.downcast_ref::<Registry>() {
            if let Some(span_ref) = reg.span(span_id) {
                if let Some(r) = span_ref.extensions().get::<RequestId>() {
                    out = Some(r.0.clone());
                }
            }
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing::info_span;
    use tracing_subscriber::{fmt, prelude::*};

    #[test]
    fn set_and_get_request_id_within_span() {
        tracing::subscriber::with_default(
            tracing_subscriber::registry().with(fmt::layer()),
            || {
                let span = info_span!("test_span");
                let _guard = span.enter();

                set_request_id("abc-123");
                assert_eq!(get_request_id().as_deref(), Some("abc-123"));
            },
        );
    }

    #[test]
    fn overwrite_request_id_replaces_value() {
        tracing::subscriber::with_default(
            tracing_subscriber::registry().with(fmt::layer()),
            || {
                let span = info_span!("test_span");
                let _guard = span.enter();

                set_request_id("first");
                assert_eq!(get_request_id().as_deref(), Some("first"));

                set_request_id("second");
                assert_eq!(get_request_id().as_deref(), Some("second"));
            },
        );
    }
}
