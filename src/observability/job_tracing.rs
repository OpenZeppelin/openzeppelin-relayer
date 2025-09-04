/// Macro to set up job tracing. Creates a span with job metadata,
/// sets the request ID, and maintains the span for the handler's duration.
///
/// # Example
/// ```
/// setup_job_tracing!(job, attempt);
/// ```
#[macro_export]
macro_rules! setup_job_tracing {
    ($job:expr, $attempt:expr) => {
        let _job_request_id = $job.request_id.clone().unwrap_or_else(|| $job.message_id.clone());
        let _job_span = tracing::info_span!(
            "job",
            request_id=%_job_request_id,
            job_message_id=%$job.message_id,
            job_type=%$job.job_type.to_string(),
            attempt=%$attempt.current()
        );
        let _job_guard = _job_span.enter();
        $crate::observability::request_id::set_request_id(_job_request_id);
    };
}
