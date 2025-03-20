use crate::constants::DEFAULT_TRANSACTION_SPEED;
use crate::models::evm::Speed;
use crate::utils::time::minutes_ms;

/// Gets the resubmit timeout for a given speed
/// Returns the timeout in milliseconds based on the speed:
/// - SafeLow: 10 minutes
/// - Average: 5 minutes
/// - Fast: 3 minutes
/// - Fastest: 2 minutes
///   If no speed is provided, uses the default transaction speed
pub fn get_resubmit_timeout_for_speed(speed: &Option<Speed>) -> i64 {
    let speed_value = speed.clone().unwrap_or(DEFAULT_TRANSACTION_SPEED);

    match speed_value {
        Speed::SafeLow => minutes_ms(10),
        Speed::Average => minutes_ms(5),
        Speed::Fast => minutes_ms(3),
        Speed::Fastest => minutes_ms(2),
    }
}

/// Calculates the resubmit age with exponential backoff
///
/// # Arguments
/// * `timeout` - The base timeout in milliseconds
/// * `attempts` - The number of attempts made so far
///
/// # Returns
/// The new timeout with exponential backoff applied: timeout * 2^(attempts-1)
pub fn get_resubmit_timeout_with_backoff(timeout: i64, attempts: usize) -> i64 {
    if attempts <= 1 {
        timeout
    } else {
        timeout * 2_i64.pow((attempts - 1) as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_resubmit_timeout_for_speed() {
        // Test with existing speeds
        assert_eq!(
            get_resubmit_timeout_for_speed(&Some(Speed::SafeLow)),
            minutes_ms(10)
        );
        assert_eq!(
            get_resubmit_timeout_for_speed(&Some(Speed::Average)),
            minutes_ms(5)
        );
        assert_eq!(
            get_resubmit_timeout_for_speed(&Some(Speed::Fast)),
            minutes_ms(3)
        );
        assert_eq!(
            get_resubmit_timeout_for_speed(&Some(Speed::Fastest)),
            minutes_ms(2)
        );

        // Test with None speed (should return default)
        assert_eq!(
            get_resubmit_timeout_for_speed(&None),
            minutes_ms(3) // DEFAULT_TRANSACTION_SPEED is Speed::Fast
        );
    }

    #[test]
    fn test_get_resubmit_timeout_with_backoff() {
        let base_timeout = 300000; // 5 minutes in ms

        // First attempt - no backoff
        assert_eq!(get_resubmit_timeout_with_backoff(base_timeout, 1), 300000);

        // Second attempt - 2x backoff
        assert_eq!(get_resubmit_timeout_with_backoff(base_timeout, 2), 600000);

        // Third attempt - 4x backoff
        assert_eq!(get_resubmit_timeout_with_backoff(base_timeout, 3), 1200000);

        // Fourth attempt - 8x backoff
        assert_eq!(get_resubmit_timeout_with_backoff(base_timeout, 4), 2400000);

        // Edge case - attempt 0 should be treated as attempt 1
        assert_eq!(get_resubmit_timeout_with_backoff(base_timeout, 0), 300000);
    }
}
