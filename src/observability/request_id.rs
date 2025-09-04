use tracing::Span;
use tracing_subscriber::{registry::LookupSpan, Registry};

#[derive(Clone, Debug)]
pub struct RequestId(pub String);

pub fn set_request_id(id: impl Into<String>) {
    let id = id.into();
    Span::current().with_subscriber(|(span_id, subscriber)| {
        if let Some(reg) = subscriber.downcast_ref::<Registry>() {
            if let Some(span_ref) = reg.span(span_id) {
                span_ref.extensions_mut().insert(RequestId(id));
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
