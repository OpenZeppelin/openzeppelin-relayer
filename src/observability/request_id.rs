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
            // Walk up the span tree: current → parent → grandparent → ...
            // The RequestId is stored on the root span by RequestIdMiddleware,
            // but child spans (e.g. from #[instrument]) don't inherit extensions.
            let mut current = reg.span(span_id);
            while let Some(span_ref) = current {
                if let Some(r) = span_ref.extensions().get::<RequestId>() {
                    out = Some(r.0.clone());
                    return;
                }
                current = span_ref.parent();
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

    #[test]
    fn get_request_id_traverses_parent_spans() {
        tracing::subscriber::with_default(
            tracing_subscriber::registry().with(fmt::layer()),
            || {
                let parent = info_span!("parent_span");
                let _parent_guard = parent.enter();

                set_request_id("parent-req-id");

                // Enter a child span (simulates #[instrument] on a downstream function)
                let child = info_span!("child_span");
                let _child_guard = child.enter();

                // Should find the RequestId on the parent span
                assert_eq!(get_request_id().as_deref(), Some("parent-req-id"));
            },
        );
    }

    #[test]
    fn get_request_id_prefers_closest_span() {
        tracing::subscriber::with_default(
            tracing_subscriber::registry().with(fmt::layer()),
            || {
                let parent = info_span!("parent_span");
                let _parent_guard = parent.enter();
                set_request_id("parent-id");

                let child = info_span!("child_span");
                let _child_guard = child.enter();
                set_request_id("child-id");

                // Should find the RequestId on the child span first
                assert_eq!(get_request_id().as_deref(), Some("child-id"));
            },
        );
    }

    #[test]
    fn get_request_id_traverses_multiple_levels() {
        tracing::subscriber::with_default(
            tracing_subscriber::registry().with(fmt::layer()),
            || {
                let root = info_span!("root_span");
                let _root_guard = root.enter();
                set_request_id("root-req-id");

                let mid = info_span!("mid_span");
                let _mid_guard = mid.enter();

                let leaf = info_span!("leaf_span");
                let _leaf_guard = leaf.enter();

                // Should traverse leaf → mid → root and find it on root
                assert_eq!(get_request_id().as_deref(), Some("root-req-id"));
            },
        );
    }
}
