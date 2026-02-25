use chrono::Utc;

/// Converts minutes to milliseconds
pub const fn minutes_ms(minutes: i64) -> i64 {
    minutes * 60 * 1000
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
}
