use crate::models::evm::Speed;

/// Converts minutes to milliseconds
pub const fn minutes_ms(minutes: i64) -> i64 {
    minutes * 60 * 1000
}

/// Map of resubmit timeouts by speed
/// This defines how long to wait before resubmitting a transaction based on its speed
pub const RESUBMIT_TIMEOUT_BY_SPEED: [(Speed, i64); 4] = [
    (Speed::SafeLow, minutes_ms(10)),
    (Speed::Average, minutes_ms(5)),
    (Speed::Fast, minutes_ms(3)),
    (Speed::Fastest, minutes_ms(2)),
];

/// Gets the resubmit timeout for a given speed
/// Returns the timeout in milliseconds
/// If the speed is not found, returns the default timeout
pub fn get_resubmit_timeout_for_speed(speed: &Option<Speed>, default_timeout: i64) -> i64 {
    if let Some(speed_value) = speed {
        for (s, timeout) in &RESUBMIT_TIMEOUT_BY_SPEED {
            if s == speed_value {
                return *timeout;
            }
        }
    }
    default_timeout
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
    fn test_get_resubmit_timeout_for_speed() {
        // Test with existing speeds
        assert_eq!(
            get_resubmit_timeout_for_speed(&Some(Speed::SafeLow), 0),
            minutes_ms(10)
        );
        assert_eq!(
            get_resubmit_timeout_for_speed(&Some(Speed::Average), 0),
            minutes_ms(5)
        );
        assert_eq!(
            get_resubmit_timeout_for_speed(&Some(Speed::Fast), 0),
            minutes_ms(3)
        );
        assert_eq!(
            get_resubmit_timeout_for_speed(&Some(Speed::Fastest), 0),
            minutes_ms(2)
        );

        // Test with None speed (should return default)
        let default_timeout = minutes_ms(7);
        assert_eq!(
            get_resubmit_timeout_for_speed(&None, default_timeout),
            default_timeout
        );
    }
}
