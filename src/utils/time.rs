use std::future::Future;
use std::time::Duration;

use chrono::Utc;
use color_eyre::Result;
use tracing::{debug, warn};

/// Converts minutes to milliseconds
pub const fn minutes_ms(minutes: i64) -> i64 {
    minutes * 60 * 1000
}

/// Polls until a condition is met or timeout is reached.
///
/// This helper provides a reusable abstraction for waiting on async conditions
/// with configurable timeout and polling interval.
///
/// # Arguments
/// * `check` - Closure that returns `Ok(true)` when condition is met, `Ok(false)` to continue polling
/// * `max_wait` - Maximum time to wait before giving up
/// * `poll_interval` - Time to sleep between polls
/// * `operation_name` - Name of the operation for logging
///
/// # Returns
/// * `Ok(true)` - Condition was met within timeout
/// * `Ok(false)` - Timeout reached without condition being met (errors are logged and polling continues)
pub async fn poll_until<F, Fut>(
    check: F,
    max_wait: Duration,
    poll_interval: Duration,
    operation_name: &str,
) -> Result<bool>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<bool>>,
{
    let start = std::time::Instant::now();

    loop {
        match check().await {
            Ok(true) => {
                debug!("{} completed", operation_name);
                return Ok(true);
            }
            Ok(false) => {}
            Err(e) => {
                warn!(error = %e, "Error checking {} status while waiting", operation_name);
            }
        }

        if start.elapsed() > max_wait {
            warn!(
                "Timed out waiting for {} to complete, proceeding anyway",
                operation_name
            );
            return Ok(false);
        }

        tokio::time::sleep(poll_interval).await;
    }
}

/// Calculates a future timestamp by adding delay seconds to the current time
///
/// # Arguments
/// * `delay_seconds` - Number of seconds to add to the current timestamp
///
/// # Returns
/// Unix timestamp (seconds since epoch) representing the scheduled time
pub fn calculate_scheduled_timestamp(delay_seconds: i64) -> i64 {
    Utc::now().timestamp() + delay_seconds
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    #[test]
    fn test_minutes_ms() {
        assert_eq!(minutes_ms(1), 60_000);
        assert_eq!(minutes_ms(5), 300_000);
        assert_eq!(minutes_ms(10), 600_000);
    }

    #[test]
    fn test_calculate_scheduled_timestamp() {
        let before = Utc::now().timestamp();
        let delay_seconds = 30;
        let scheduled = calculate_scheduled_timestamp(delay_seconds);
        let after = Utc::now().timestamp();

        // The scheduled time should be approximately delay_seconds in the future
        assert!(scheduled >= before + delay_seconds);
        assert!(scheduled <= after + delay_seconds + 1); // Allow 1 second for test execution
    }

    #[tokio::test]
    async fn test_poll_until_condition_met_immediately() {
        let result = poll_until(
            || async { Ok(true) },
            Duration::from_millis(100),
            Duration::from_millis(10),
            "immediate_test",
        )
        .await;

        assert!(result.is_ok());
        assert!(result.unwrap());
    }

    #[tokio::test]
    async fn test_poll_until_condition_met_after_multiple_polls() {
        let poll_count = Arc::new(AtomicU32::new(0));
        let poll_count_clone = Arc::clone(&poll_count);

        let result = poll_until(
            move || {
                let count = poll_count_clone.fetch_add(1, Ordering::SeqCst);
                async move {
                    // Return true on the 3rd poll (count == 2)
                    Ok(count >= 2)
                }
            },
            Duration::from_secs(1),
            Duration::from_millis(10),
            "delayed_condition_test",
        )
        .await;

        assert!(result.is_ok());
        assert!(result.unwrap());
        assert!(poll_count.load(Ordering::SeqCst) >= 3);
    }

    #[tokio::test]
    async fn test_poll_until_timeout_reached() {
        let result = poll_until(
            || async { Ok(false) },
            Duration::from_millis(50),
            Duration::from_millis(10),
            "timeout_test",
        )
        .await;

        assert!(result.is_ok());
        assert!(!result.unwrap());
    }

    #[tokio::test]
    async fn test_poll_until_continues_polling_after_errors() {
        let poll_count = Arc::new(AtomicU32::new(0));
        let poll_count_clone = Arc::clone(&poll_count);

        let result = poll_until(
            move || {
                let count = poll_count_clone.fetch_add(1, Ordering::SeqCst);
                async move {
                    if count < 2 {
                        // Return error on first two polls
                        Err(color_eyre::eyre::eyre!("temporary error"))
                    } else {
                        // Return success on 3rd poll
                        Ok(true)
                    }
                }
            },
            Duration::from_secs(1),
            Duration::from_millis(10),
            "error_recovery_test",
        )
        .await;

        assert!(result.is_ok());
        assert!(result.unwrap());
        assert!(poll_count.load(Ordering::SeqCst) >= 3);
    }

    #[tokio::test]
    async fn test_poll_until_timeout_after_persistent_errors() {
        let poll_count = Arc::new(AtomicU32::new(0));
        let poll_count_clone = Arc::clone(&poll_count);

        let result = poll_until(
            move || {
                poll_count_clone.fetch_add(1, Ordering::SeqCst);
                async { Err(color_eyre::eyre::eyre!("persistent error")) }
            },
            Duration::from_millis(50),
            Duration::from_millis(10),
            "persistent_error_test",
        )
        .await;

        // Should timeout (return Ok(false)) since errors don't stop polling
        assert!(result.is_ok());
        assert!(!result.unwrap());
        // Should have polled multiple times
        assert!(poll_count.load(Ordering::SeqCst) >= 2);
    }

    #[tokio::test]
    async fn test_poll_until_respects_poll_interval() {
        let start = std::time::Instant::now();
        let poll_count = Arc::new(AtomicU32::new(0));
        let poll_count_clone = Arc::clone(&poll_count);

        let result = poll_until(
            move || {
                let count = poll_count_clone.fetch_add(1, Ordering::SeqCst);
                async move { Ok(count >= 3) }
            },
            Duration::from_secs(1),
            Duration::from_millis(50),
            "interval_test",
        )
        .await;

        let elapsed = start.elapsed();

        assert!(result.is_ok());
        assert!(result.unwrap());
        // With 50ms interval and 4 polls (0, 1, 2, 3), we expect at least 150ms
        // (3 sleeps between 4 polls)
        assert!(elapsed >= Duration::from_millis(100));
    }
}
