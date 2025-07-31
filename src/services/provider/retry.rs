//! # RPC Provider Retry Module
//!
//! This module implements retry mechanisms for RPC calls with exponential backoff,
//! jitter, and provider failover capabilities.
//!
//! ## Key Features
//!
//! - **Exponential Backoff**: Gradually increases retry delays to avoid overwhelming services
//! - **Randomized Jitter**: Prevents retry storms by randomizing delay times
//! - **Provider Failover**: Automatically switches to alternative providers when one fails
//! - **Configurable Behavior**: Customizable retry counts, delays, and failover strategies
//!
//! ## Main Components
//!
//! - [`RetryConfig`]: Configuration parameters for retry behavior
//! - [`retry_rpc_call`]: Core function that handles retry logic with provider failover
//! - [`calculate_retry_delay`]: Function that calculates delay with exponential backoff and jitter
//!
//! ## Usage
//!
//! The retry mechanism works with any RPC provider type and automatically handles
//! errors, maximizing the chances of successful operations.
use rand::Rng;
use std::future::Future;
use std::time::Duration;

use super::rpc_selector::RpcSelector;
use crate::config::ServerConfig;
use crate::constants::RETRY_JITTER_PERCENT;

/// Calculate the retry delay using exponential backoff with jitter
///
/// # Arguments
/// * `attempt` - The retry attempt number (0 = first attempt)
/// * `base_delay_ms` - Base delay in milliseconds
/// * `max_delay_ms` - Maximum delay in milliseconds
///
/// # Returns
/// Duration to wait before the next retry
pub fn calculate_retry_delay(attempt: u8, base_delay_ms: u64, max_delay_ms: u64) -> Duration {
    if base_delay_ms == 0 || max_delay_ms == 0 {
        return Duration::from_millis(0);
    }

    // Limit the max delay to 2^63 to avoid overflow. (u64::MAX is 2^64 - 1)
    let exp_backoff = if attempt > 63 {
        max_delay_ms
    } else {
        // 1u64 << attempt
        let multiplier = 1u64.checked_shl(attempt as u32).unwrap_or(u64::MAX);
        base_delay_ms.saturating_mul(multiplier)
    };

    let delay_ms = exp_backoff.min(max_delay_ms);

    apply_jitter(delay_ms)
}

/// Applies jitter to a delay value based on RETRY_JITTER_PERCENT
///
/// This creates a randomized delay within the range:
/// delay_ms × (1 ± RETRY_JITTER_PERCENT)
///
/// # Arguments
/// * `delay_ms` - The base delay in milliseconds to apply jitter to
///
/// # Returns
/// A Duration with jitter applied, guaranteed to be within
/// the range [delay_ms × (1-RETRY_JITTER_PERCENT), delay_ms × (1+RETRY_JITTER_PERCENT)]
fn apply_jitter(delay_ms: u64) -> Duration {
    if delay_ms == 0 {
        return Duration::from_millis(0);
    }

    // Calculate jitter range (how much we can add/subtract)
    let jitter_range = (delay_ms as f64 * RETRY_JITTER_PERCENT).floor() as u64;

    if jitter_range == 0 {
        return Duration::from_millis(delay_ms);
    }

    let mut rng = rand::rng();
    let jitter_value = rng.random_range(0..=jitter_range);

    let final_delay = if rng.random_bool(0.5) {
        delay_ms.saturating_add(jitter_value)
    } else {
        delay_ms.saturating_sub(jitter_value)
    };

    Duration::from_millis(final_delay)
}

/// Internal error type to distinguish specific retry outcomes
#[derive(Debug)]
enum InternalRetryError<E> {
    NonRetriable(E),
    RetriesExhausted(E),
}

/// Configuration for retry behavior
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts per provider
    pub max_retries: u8,
    /// Maximum number of provider failovers to attempt
    pub max_failovers: u8,
    /// Base delay in milliseconds for exponential backoff
    pub base_delay_ms: u64,
    /// Maximum delay in milliseconds for exponential backoff
    pub max_delay_ms: u64,
}

impl RetryConfig {
    /// Create a new RetryConfig with specified values
    ///
    /// # Arguments
    /// * `max_retries` - Maximum number of retry attempts per provider (0-255)
    /// * `max_failovers` - Maximum number of provider failovers (0-255)
    /// * `base_delay_ms` - Base delay in milliseconds for exponential backoff
    /// * `max_delay_ms` - Maximum delay in milliseconds (should be >= base_delay_ms)
    ///
    /// # Panics
    /// * If `max_delay_ms` < `base_delay_ms` when both are non-zero
    /// * If only one of the delay values is zero (both should be zero or both non-zero)
    pub fn new(max_retries: u8, max_failovers: u8, base_delay_ms: u64, max_delay_ms: u64) -> Self {
        // Validate delay consistency: both zero or both non-zero
        if (base_delay_ms == 0) != (max_delay_ms == 0) {
            panic!(
                "Delay values must be consistent: both zero (no delays) or both non-zero. Got base_delay_ms={}, max_delay_ms={}",
                base_delay_ms, max_delay_ms
            );
        }

        // Validate delay ordering when both are non-zero
        if base_delay_ms > 0 && max_delay_ms > 0 && max_delay_ms < base_delay_ms {
            panic!(
                "max_delay_ms ({}) must be >= base_delay_ms ({}) when both are non-zero",
                max_delay_ms, base_delay_ms
            );
        }

        Self {
            max_retries,
            max_failovers,
            base_delay_ms,
            max_delay_ms,
        }
    }

    /// Create a RetryConfig from environment variables
    pub fn from_env() -> Self {
        let config = ServerConfig::from_env();
        Self::new(
            config.provider_max_retries,
            config.provider_max_failovers,
            config.provider_retry_base_delay_ms,
            config.provider_retry_max_delay_ms,
        )
    }
}

/// Generic RPC call retry function that handles retrying operations with exponential backoff
/// and provider failover.
///
/// This function will:
/// 1. Get a provider using the provider_initializer
/// 2. Try the operation up to provider_max_retries times with that provider
///    (retrying only on retriable errors)
/// 3. If all retries fail or a non-retriable error occurs, mark the provider as failed and get a new provider
/// 4. Continue up to provider_max_failovers times (capped by total available providers)
///
/// # Type Parameters
/// * `P` - The provider type
/// * `T` - The result type of the operation
/// * `E` - The error type that implements `From<String>`
/// * `F` - The function type that takes a provider and returns a future
/// * `Fut` - The future type returned by the operation
/// * `I` - The provider initializer function type
///
/// # Arguments
/// * `selector` - RPC selector for managing and selecting providers
/// * `operation_name` - Name of the operation for logging
/// * `is_retriable_error` - Function that determines if an error is retriable
/// * `should_mark_provider_failed` - Function that determines if an error should mark the provider as failed
/// * `provider_initializer` - Function that initializes a provider from a URL
/// * `operation` - A future-returning closure that makes the RPC call
/// * `config` - Optional configuration parameters for retry behavior
///
/// # Returns
/// * The result of the operation if successful, or an error
pub async fn retry_rpc_call<P, T, E, F, Fut, I>(
    selector: &RpcSelector,
    operation_name: &str,
    is_retriable_error: impl Fn(&E) -> bool,
    should_mark_provider_failed: impl Fn(&E) -> bool,
    provider_initializer: I,
    operation: F,
    config: Option<RetryConfig>,
) -> Result<T, E>
where
    P: Clone,
    E: std::fmt::Display + From<String>,
    F: Fn(P) -> Fut,
    Fut: Future<Output = Result<T, E>>,
    I: Fn(&str) -> Result<P, E>,
{
    let config = config.unwrap_or_else(RetryConfig::from_env);
    let total_providers = selector.provider_count();
    let max_failovers = std::cmp::min(config.max_failovers as usize, total_providers - 1);
    let mut failover_count = 0;
    let mut total_attempts = 0;
    let mut last_error = None;

    log::debug!(
        "Starting RPC call '{}' with max_retries={}, max_failovers={}, available_providers={}",
        operation_name,
        config.max_retries,
        max_failovers,
        total_providers
    );

    while failover_count <= max_failovers && selector.available_provider_count() > 0 {
        // Try to get and initialize a provider
        let (provider, provider_url) =
            match get_provider(selector, operation_name, &provider_initializer) {
                Ok((provider, url)) => (provider, url),
                Err(e) => {
                    last_error = Some(e);
                    failover_count += 1;

                    // If we've exhausted all providers or reached max failovers, stop
                    if failover_count > max_failovers || selector.available_provider_count() == 0 {
                        break;
                    }

                    // Mark current as failed to get a different one next time
                    selector.mark_current_as_failed();
                    continue;
                }
            };

        log::debug!(
            "Selected provider: {} for operation '{}'",
            provider_url,
            operation_name
        );

        // Try the operation with this provider with retries
        match try_with_retries(
            &provider,
            &provider_url,
            operation_name,
            &operation,
            &is_retriable_error,
            &config,
            &mut total_attempts,
        )
        .await
        {
            Ok(result) => {
                log::debug!(
                    "RPC call '{}' succeeded with provider '{}' (total attempts: {})",
                    operation_name,
                    provider_url,
                    total_attempts
                );
                return Ok(result);
            }
            Err(internal_err) => {
                match internal_err {
                    InternalRetryError::NonRetriable(original_err) => {
                        // Check if this non-retriable error should mark the provider as failed
                        if should_mark_provider_failed(&original_err)
                            && selector.available_provider_count() > 1
                        {
                            log::warn!(
                                "Non-retriable error '{}' for provider '{}' on operation '{}' should mark provider as failed. Marking as failed and switching to next provider...",
                                original_err,
                                provider_url,
                                operation_name
                            );
                            selector.mark_current_as_failed();
                        }
                        return Err(original_err);
                    }
                    InternalRetryError::RetriesExhausted(original_err) => {
                        last_error = Some(original_err);

                        // If retries are exhausted, we always intend to mark the provider as failed,
                        // unless it's the last available one.
                        if selector.available_provider_count() > 1 {
                            log::warn!(
                                "All {} retry attempts failed for provider '{}' on operation '{}'. Error: {}. Marking as failed and switching to next provider (failover {}/{})...",
                                config.max_retries,
                                provider_url,
                                operation_name,
                                last_error.as_ref().unwrap(),
                                failover_count + 1,
                                max_failovers
                            );
                            selector.mark_current_as_failed();
                            failover_count += 1;
                        } else {
                            log::warn!(
                                "All {} retry attempts failed for provider '{}' on operation '{}'. Error: {}. This is the last available provider, not marking as failed.",
                                config.max_retries,
                                provider_url,
                                operation_name,
                                last_error.as_ref().unwrap()
                            );
                            break;
                        }
                    }
                }
            }
        }
    }

    let error_message = match &last_error {
        Some(e) => format!(
            "RPC call '{}' failed after {} total attempts across {} providers: {}",
            operation_name,
            total_attempts,
            failover_count,
            e
        ),
        None => format!(
            "RPC call '{}' failed after {} total attempts across {} providers with no error details",
            operation_name,
            total_attempts,
            failover_count
        )
    };

    log::error!("{}", error_message);

    // If we're here, all retries with all attempted providers failed
    Err(last_error.unwrap_or_else(|| E::from(error_message)))
}

/// Helper function to get and initialize a provider
fn get_provider<P, E, I>(
    selector: &RpcSelector,
    operation_name: &str,
    provider_initializer: &I,
) -> Result<(P, String), E>
where
    E: std::fmt::Display + From<String>,
    I: Fn(&str) -> Result<P, E>,
{
    // Get the next provider URL from the selector
    let provider_url = selector
        .get_client(|url| Ok::<_, eyre::Report>(url.to_string()))
        .map_err(|e| {
            let err_msg = format!("Failed to get provider URL for {}: {}", operation_name, e);
            log::warn!("{}", err_msg);
            E::from(err_msg)
        })?;

    // Initialize the provider
    let provider = provider_initializer(&provider_url).map_err(|e| {
        log::warn!(
            "Failed to initialize provider '{}' for operation '{}': {}",
            provider_url,
            operation_name,
            e
        );
        e
    })?;

    Ok((provider, provider_url))
}

/// Helper function to try an operation with retries
async fn try_with_retries<P, T, E, F, Fut>(
    provider: &P,
    provider_url: &str,
    operation_name: &str,
    operation: &F,
    is_retriable_error: &impl Fn(&E) -> bool,
    config: &RetryConfig,
    total_attempts: &mut usize,
) -> Result<T, InternalRetryError<E>>
where
    P: Clone,
    E: std::fmt::Display + From<String>,
    F: Fn(P) -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    // For max_retries of 0 or 1, we don't retry - just attempt once
    if config.max_retries <= 1 {
        *total_attempts += 1;
        return operation(provider.clone())
            .await
            .map_err(InternalRetryError::NonRetriable);
    }

    for current_attempt_idx in 0..config.max_retries {
        *total_attempts += 1;

        match operation(provider.clone()).await {
            Ok(result) => {
                log::debug!(
                    "RPC call '{}' succeeded with provider '{}' (attempt {}/{}, total attempts: {})",
                    operation_name,
                    provider_url,
                    current_attempt_idx + 1,
                    config.max_retries,
                    *total_attempts
                );
                return Ok(result);
            }
            Err(e) => {
                let is_retriable = is_retriable_error(&e);
                let is_last_attempt = current_attempt_idx + 1 >= config.max_retries;

                log::warn!(
                    "RPC call '{}' failed with provider '{}' (attempt {}/{}): {} [{}]",
                    operation_name,
                    provider_url,
                    current_attempt_idx + 1,
                    config.max_retries,
                    e,
                    if is_retriable {
                        "retriable"
                    } else {
                        "non-retriable"
                    }
                );

                if !is_retriable {
                    return Err(InternalRetryError::NonRetriable(e));
                }

                if is_last_attempt {
                    log::warn!(
                        "All {} retries exhausted for RPC call '{}' with provider '{}'. Last error: {}",
                        config.max_retries, operation_name, provider_url, e
                    );
                    return Err(InternalRetryError::RetriesExhausted(e));
                }

                // Calculate and apply delay before next retry
                let delay = calculate_retry_delay(
                    current_attempt_idx + 1,
                    config.base_delay_ms,
                    config.max_delay_ms,
                );

                log::debug!(
                    "Retrying RPC call '{}' with provider '{}' after {:?} delay (attempt {}/{})",
                    operation_name,
                    provider_url,
                    delay,
                    current_attempt_idx + 2,
                    config.max_retries
                );
                tokio::time::sleep(delay).await;
            }
        }
    }

    unreachable!(
        "Loop should have returned if max_retries > 1; max_retries=0 or 1 case is handled above."
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::RpcConfig;
    use lazy_static::lazy_static;
    use std::cmp::Ordering;
    use std::env;
    use std::sync::atomic::{AtomicU8, Ordering as AtomicOrdering};
    use std::sync::Arc;
    use std::sync::Mutex;

    // Use a mutex to ensure tests don't run in parallel when modifying env vars
    lazy_static! {
        static ref RETRY_TEST_ENV_MUTEX: Mutex<()> = Mutex::new(());
    }

    // Define a simple error type for testing
    #[derive(Debug, Clone)]
    struct TestError(String);

    impl std::fmt::Display for TestError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "TestError: {}", self.0)
        }
    }

    impl From<String> for TestError {
        fn from(msg: String) -> Self {
            TestError(msg)
        }
    }

    // Helper struct to ensure environment variables are reset after tests
    struct EnvGuard {
        keys: Vec<String>,
        old_values: Vec<Option<String>>,
    }

    impl EnvGuard {
        fn new() -> Self {
            Self {
                keys: Vec::new(),
                old_values: Vec::new(),
            }
        }

        fn set(&mut self, key: &str, value: &str) {
            let old_value = env::var(key).ok();
            self.keys.push(key.to_string());
            self.old_values.push(old_value);
            env::set_var(key, value);
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for i in 0..self.keys.len() {
                match &self.old_values[i] {
                    Some(value) => env::set_var(&self.keys[i], value),
                    None => env::remove_var(&self.keys[i]),
                }
            }
        }
    }

    // Set up test environment variables
    fn setup_test_env() -> EnvGuard {
        let mut guard = EnvGuard::new();
        guard.set("API_KEY", "fake-api-key-for-tests-01234567890123456789");
        guard.set("PROVIDER_MAX_RETRIES", "2");
        guard.set("PROVIDER_MAX_FAILOVERS", "1");
        guard.set("PROVIDER_RETRY_BASE_DELAY_MS", "1");
        guard.set("PROVIDER_RETRY_MAX_DELAY_MS", "5");
        guard.set("REDIS_URL", "redis://localhost:6379");
        guard.set(
            "RELAYER_PRIVATE_KEY",
            "0x1234567890123456789012345678901234567890123456789012345678901234",
        );
        guard
    }

    #[test]
    fn test_calculate_retry_delay() {
        // Test exponential backoff pattern
        let base_delay_ms = 10;
        let max_delay_ms = 10000;

        let expected_backoffs = [
            10,  // 10 * 2^0
            20,  // 10 * 2^1
            40,  // 10 * 2^2
            80,  // 10 * 2^3
            160, // 10 * 2^4
            320, // 10 * 2^5
        ];

        for (i, expected) in expected_backoffs.iter().enumerate() {
            let attempt = i as u8;
            let delay = calculate_retry_delay(attempt, base_delay_ms, max_delay_ms);

            let min_expected = (*expected as f64 * (1.0 - RETRY_JITTER_PERCENT)).floor() as u128;
            let max_expected = (*expected as f64 * (1.0 + RETRY_JITTER_PERCENT)).ceil() as u128;

            assert!(
                (min_expected..=max_expected).contains(&delay.as_millis()),
                "Delay {} outside expected range {}..={}",
                delay.as_millis(),
                min_expected,
                max_expected
            );
        }

        // Test max delay capping
        let base_delay_ms = 100;
        let max_delay_ms = 1000;
        let delay = calculate_retry_delay(4, base_delay_ms, max_delay_ms);
        let min_expected = (max_delay_ms as f64 * (1.0 - RETRY_JITTER_PERCENT)).floor() as u128;
        let max_expected = (max_delay_ms as f64 * (1.0 + RETRY_JITTER_PERCENT)).ceil() as u128;
        assert!(
            (min_expected..=max_expected).contains(&delay.as_millis()),
            "Delay {} outside expected range {}..={}",
            delay.as_millis(),
            min_expected,
            max_expected
        );

        // Test edge cases
        assert_eq!(calculate_retry_delay(5, 0, 1000).as_millis(), 0);
        assert_eq!(calculate_retry_delay(5, 100, 0).as_millis(), 0);
        assert_eq!(calculate_retry_delay(5, 0, 0).as_millis(), 0);

        // Test with max attempt (u8::MAX)
        let max_delay_ms = 10_000;
        let delay = calculate_retry_delay(u8::MAX, 1, max_delay_ms);
        assert!(
            delay.as_millis()
                <= (max_delay_ms as f64 * (1.0 + RETRY_JITTER_PERCENT)).ceil() as u128
        );
    }

    #[test]
    fn test_apply_jitter() {
        let base_delay = 1000;
        let jittered = apply_jitter(base_delay);

        let min_expected = (base_delay as f64 * (1.0 - RETRY_JITTER_PERCENT)).floor() as u64;
        let max_expected = (base_delay as f64 * (1.0 + RETRY_JITTER_PERCENT)).ceil() as u64;

        assert!(
            (min_expected as u128..=max_expected as u128).contains(&jittered.as_millis()),
            "Jittered value {} outside expected range {}..={}",
            jittered.as_millis(),
            min_expected,
            max_expected
        );

        // Test edge cases
        assert_eq!(apply_jitter(0).as_millis(), 0);

        // Test small values where jitter might be 0
        for delay in 1..5 {
            let jittered = apply_jitter(delay);
            let jitter_range = (delay as f64 * RETRY_JITTER_PERCENT).floor() as u64;

            if jitter_range == 0 {
                assert_eq!(jittered.as_millis(), delay as u128);
            } else {
                let min_expected = delay.saturating_sub(jitter_range);
                let max_expected = delay.saturating_add(jitter_range);
                assert!(
                    (min_expected as u128..=max_expected as u128).contains(&jittered.as_millis()),
                    "Jittered value {} outside expected range {}..={}",
                    jittered.as_millis(),
                    min_expected,
                    max_expected
                );
            }
        }

        let base_delay = 10000;
        let iterations = 200;
        let mut additions = 0;
        let mut subtractions = 0;

        for _ in 0..iterations {
            let jittered = apply_jitter(base_delay);
            let j_millis = jittered.as_millis();
            let b_delay = base_delay as u128;

            match j_millis.cmp(&b_delay) {
                Ordering::Greater => {
                    additions += 1;
                }
                Ordering::Less => {
                    subtractions += 1;
                }
                Ordering::Equal => {}
            }
        }

        assert!(additions > 0, "No additions were observed");
        assert!(subtractions > 0, "No subtractions were observed");
    }

    #[test]
    fn test_retry_config() {
        let config = RetryConfig::new(5, 2, 100, 5000);
        assert_eq!(config.max_retries, 5);
        assert_eq!(config.max_failovers, 2);
        assert_eq!(config.base_delay_ms, 100);
        assert_eq!(config.max_delay_ms, 5000);
    }

    #[test]
    fn test_retry_config_from_env() {
        let _lock = RETRY_TEST_ENV_MUTEX
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut guard = setup_test_env();
        // Add missing environment variables that ServerConfig requires
        guard.set("REDIS_URL", "redis://localhost:6379");
        guard.set(
            "RELAYER_PRIVATE_KEY",
            "0x1234567890123456789012345678901234567890123456789012345678901234",
        );

        let config = RetryConfig::from_env();
        assert_eq!(config.max_retries, 2);
        assert_eq!(config.max_failovers, 1);
        assert_eq!(config.base_delay_ms, 1);
        assert_eq!(config.max_delay_ms, 5);
    }

    #[test]
    fn test_calculate_retry_delay_edge_cases() {
        // Test attempt = 0 (should be base_delay * 2^0 = base_delay)
        let delay = calculate_retry_delay(0, 100, 1000);
        let min_expected = (100.0 * (1.0 - RETRY_JITTER_PERCENT)).floor() as u128;
        let max_expected = (100.0 * (1.0 + RETRY_JITTER_PERCENT)).ceil() as u128;
        assert!(
            (min_expected..=max_expected).contains(&delay.as_millis()),
            "Delay {} outside expected range {}..={}",
            delay.as_millis(),
            min_expected,
            max_expected
        );

        // Test equal base and max delays
        let delay = calculate_retry_delay(5, 100, 100);
        let min_expected = (100.0 * (1.0 - RETRY_JITTER_PERCENT)).floor() as u128;
        let max_expected = (100.0 * (1.0 + RETRY_JITTER_PERCENT)).ceil() as u128;
        assert!(
            (min_expected..=max_expected).contains(&delay.as_millis()),
            "Delay {} outside expected range {}..={}",
            delay.as_millis(),
            min_expected,
            max_expected
        );

        // Test very large delays (near overflow protection)
        let delay = calculate_retry_delay(60, 1000, u64::MAX);
        assert!(delay.as_millis() > 0);

        // Test minimum values
        let delay = calculate_retry_delay(1, 1, 1);
        assert_eq!(delay.as_millis(), 1);
    }

    #[test]
    fn test_retry_config_validation() {
        // Valid configurations should work
        let _config = RetryConfig::new(3, 1, 100, 1000);
        let _config = RetryConfig::new(3, 1, 0, 0); // Both zero is valid
        let _config = RetryConfig::new(3, 1, 100, 100); // Equal values are valid
        let _config = RetryConfig::new(0, 0, 1, 1); // Minimum non-zero values
        let _config = RetryConfig::new(255, 255, 1, 1000); // Maximum u8 values
    }

    #[test]
    #[should_panic(
        expected = "max_delay_ms (50) must be >= base_delay_ms (100) when both are non-zero"
    )]
    fn test_retry_config_validation_panic_delay_ordering() {
        // This should panic because max_delay_ms < base_delay_ms
        let _config = RetryConfig::new(3, 1, 100, 50);
    }

    #[test]
    #[should_panic(
        expected = "Delay values must be consistent: both zero (no delays) or both non-zero"
    )]
    fn test_retry_config_validation_panic_inconsistent_delays_base_zero() {
        // This should panic because only base_delay_ms is zero
        let _config = RetryConfig::new(3, 1, 0, 1000);
    }

    #[test]
    #[should_panic(
        expected = "Delay values must be consistent: both zero (no delays) or both non-zero"
    )]
    fn test_retry_config_validation_panic_inconsistent_delays_max_zero() {
        // This should panic because only max_delay_ms is zero
        let _config = RetryConfig::new(3, 1, 100, 0);
    }

    #[test]
    fn test_get_provider() {
        let _guard = setup_test_env();

        let configs = vec![
            RpcConfig::new("http://localhost:8545".to_string()),
            RpcConfig::new("http://localhost:8546".to_string()),
        ];
        let selector = RpcSelector::new(configs).expect("Failed to create selector");

        let initializer =
            |url: &str| -> Result<String, TestError> { Ok(format!("provider-{}", url)) };

        let result = get_provider(&selector, "test_operation", &initializer);
        assert!(result.is_ok());
        let (provider, url) = result.unwrap();
        assert_eq!(url, "http://localhost:8545");
        assert_eq!(provider, "provider-http://localhost:8545");

        let initializer = |_: &str| -> Result<String, TestError> {
            Err(TestError("Failed to initialize".to_string()))
        };

        let result = get_provider(&selector, "test_operation", &initializer);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(format!("{}", err).contains("Failed to initialize"));
    }

    #[tokio::test]
    async fn test_try_with_retries() {
        let provider = "test_provider".to_string();
        let provider_url = "http://localhost:8545";
        let mut total_attempts = 0;
        let config = RetryConfig::new(3, 1, 5, 10);

        let operation = |p: String| async move {
            assert_eq!(p, "test_provider");
            Ok::<_, TestError>(42)
        };

        let result = try_with_retries(
            &provider,
            provider_url,
            "test_operation",
            &operation,
            &|_| false,
            &config,
            &mut total_attempts,
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42);
        assert_eq!(total_attempts, 1);

        let attempts = Arc::new(AtomicU8::new(0));
        let attempts_clone = attempts.clone();
        let operation = move |_: String| {
            let attempts = attempts_clone.clone();
            async move {
                let current = attempts.fetch_add(1, AtomicOrdering::SeqCst);
                if current < 2 {
                    Err(TestError("Retriable error".to_string()))
                } else {
                    Ok(42)
                }
            }
        };

        let mut total_attempts = 0;
        let result = try_with_retries(
            &provider,
            provider_url,
            "test_operation",
            &operation,
            &|_| true,
            &config,
            &mut total_attempts,
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42);
        assert_eq!(total_attempts, 3);

        // Test non-retriable error
        let operation = |_: String| async { Err(TestError("Non-retriable error".to_string())) };

        let mut total_attempts = 0;
        let result: Result<i32, InternalRetryError<TestError>> = try_with_retries(
            &provider,
            provider_url,
            "test_operation",
            &operation,
            &|_| false,
            &config,
            &mut total_attempts,
        )
        .await;

        assert!(result.is_err());
        assert_eq!(total_attempts, 1);
        let err = result.unwrap_err();
        assert!(matches!(err, InternalRetryError::NonRetriable(_)));

        // Test exhausting all retries
        let operation = |_: String| async { Err(TestError("Always fails".to_string())) };

        let mut total_attempts = 0;
        let result: Result<i32, InternalRetryError<TestError>> = try_with_retries(
            &provider,
            provider_url,
            "test_operation",
            &operation,
            &|_| true,
            &config,
            &mut total_attempts,
        )
        .await;

        assert!(result.is_err());
        assert_eq!(total_attempts, 3); // Should try 3 times (max_retries)
        let error = result.unwrap_err();
        assert!(matches!(error, InternalRetryError::RetriesExhausted(_)));
    }

    #[tokio::test]
    async fn test_try_with_retries_max_retries_zero() {
        let provider = "test_provider".to_string();
        let provider_url = "http://localhost:8545";
        let mut total_attempts = 0;
        let config = RetryConfig::new(0, 1, 5, 10);

        // Test successful operation with max_retries = 0
        let operation = |_p: String| async move { Ok::<_, TestError>(42) };

        let result = try_with_retries(
            &provider,
            provider_url,
            "test_operation",
            &operation,
            &|_| false,
            &config,
            &mut total_attempts,
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42);

        // Test failing operation with max_retries = 0
        let operation = |_: String| async { Err(TestError("Always fails".to_string())) };

        let mut total_attempts = 0;
        let result: Result<i32, InternalRetryError<TestError>> = try_with_retries(
            &provider,
            provider_url,
            "test_operation",
            &operation,
            &|_| true,
            &config,
            &mut total_attempts,
        )
        .await;

        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(matches!(error, InternalRetryError::NonRetriable(_))); // Should be NonRetriable due to max_retries <= 1
    }

    #[tokio::test]
    async fn test_try_with_retries_max_retries_one() {
        let provider = "test_provider".to_string();
        let provider_url = "http://localhost:8545";
        let mut total_attempts = 0;
        let config = RetryConfig::new(1, 1, 5, 10);

        // Test successful operation with max_retries = 1
        let operation = |p: String| async move {
            assert_eq!(p, "test_provider");
            Ok::<_, TestError>(42)
        };

        let result = try_with_retries(
            &provider,
            provider_url,
            "test_operation",
            &operation,
            &|_| false,
            &config,
            &mut total_attempts,
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42);

        // Test failing operation with max_retries = 1
        let operation = |_: String| async { Err(TestError("Always fails".to_string())) };

        let mut total_attempts = 0;
        let result: Result<i32, InternalRetryError<TestError>> = try_with_retries(
            &provider,
            provider_url,
            "test_operation",
            &operation,
            &|_| true,
            &config,
            &mut total_attempts,
        )
        .await;

        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(matches!(error, InternalRetryError::NonRetriable(_))); // Should be NonRetriable due to max_retries <= 1
    }

    #[tokio::test]
    async fn test_non_retriable_error_does_not_mark_provider_failed() {
        let _guard = setup_test_env();

        let configs = vec![
            RpcConfig::new("http://localhost:8545".to_string()),
            RpcConfig::new("http://localhost:8546".to_string()),
        ];
        let selector = RpcSelector::new(configs).expect("Failed to create selector");

        let provider_initializer = |url: &str| -> Result<String, TestError> { Ok(url.to_string()) };

        // Operation that always fails with a non-retriable error
        let operation =
            |_provider: String| async move { Err(TestError("Non-retriable error".to_string())) };

        let config = RetryConfig::new(3, 1, 0, 0);

        // Get initial provider count
        let initial_available_count = selector.available_provider_count();

        let result: Result<i32, TestError> = retry_rpc_call(
            &selector,
            "test_operation",
            |_| false, // Error is NOT retriable
            |_| false, // Error is NOT retriable
            provider_initializer,
            operation,
            Some(config),
        )
        .await;

        assert!(result.is_err());

        // Provider should NOT be marked as failed for non-retriable errors
        let final_available_count = selector.available_provider_count();
        assert_eq!(
            initial_available_count, final_available_count,
            "Provider count should remain the same for non-retriable errors"
        );
    }

    #[tokio::test]
    async fn test_retriable_error_marks_provider_failed_after_retries_exhausted() {
        let _guard = setup_test_env();

        let configs = vec![
            RpcConfig::new("http://localhost:8545".to_string()),
            RpcConfig::new("http://localhost:8546".to_string()),
        ];
        let selector = RpcSelector::new(configs).expect("Failed to create selector");

        let provider_initializer = |url: &str| -> Result<String, TestError> { Ok(url.to_string()) };

        // Operation that always fails with a retriable error
        let operation = |_provider: String| async { Err(TestError("Retriable error".to_string())) };

        let config = RetryConfig::new(2, 1, 0, 0); // 2 retries, 1 failover

        // Get initial provider count
        let initial_available_count = selector.available_provider_count();

        let result: Result<i32, TestError> = retry_rpc_call(
            &selector,
            "test_operation",
            |_| true, // Error IS retriable
            |_| true, // Error SHOULD mark provider as failed
            provider_initializer,
            operation,
            Some(config),
        )
        .await;

        assert!(result.is_err());

        // At least one provider should be marked as failed after retries are exhausted
        let final_available_count = selector.available_provider_count();
        assert!(final_available_count < initial_available_count,
            "At least one provider should be marked as failed after retriable errors exhaust retries");
    }

    #[tokio::test]
    async fn test_retry_rpc_call_success() {
        let _guard = setup_test_env();

        let configs = vec![
            RpcConfig::new("http://localhost:8545".to_string()),
            RpcConfig::new("http://localhost:8546".to_string()),
        ];
        let selector = RpcSelector::new(configs).expect("Failed to create selector");

        let attempts = Arc::new(AtomicU8::new(0));
        let attempts_clone = attempts.clone();

        let provider_initializer =
            |_url: &str| -> Result<String, TestError> { Ok("mock_provider".to_string()) };

        let operation = move |_provider: String| {
            let attempts = attempts_clone.clone();
            async move {
                attempts.fetch_add(1, AtomicOrdering::SeqCst);
                Ok::<_, TestError>(42)
            }
        };

        let config = RetryConfig::new(1, 1, 0, 0);

        let result = retry_rpc_call(
            &selector,
            "test_operation",
            |_| false, // No errors are retriable
            |_| false, // No errors are retriable
            provider_initializer,
            operation,
            Some(config),
        )
        .await;

        assert!(result.is_ok(), "Expected OK result but got: {:?}", result);
        assert_eq!(result.unwrap(), 42);
        assert_eq!(attempts.load(AtomicOrdering::SeqCst), 1); // Should be called once
    }

    #[tokio::test]
    async fn test_retry_rpc_call_with_provider_failover() {
        let _guard = setup_test_env();

        let configs = vec![
            RpcConfig::new("http://localhost:8545".to_string()),
            RpcConfig::new("http://localhost:8546".to_string()),
        ];
        let selector = RpcSelector::new(configs).expect("Failed to create selector");

        let current_provider = Arc::new(Mutex::new(String::new()));
        let current_provider_clone = current_provider.clone();

        let provider_initializer = move |url: &str| -> Result<String, TestError> {
            let mut provider = current_provider_clone.lock().unwrap();
            *provider = url.to_string();
            Ok(url.to_string())
        };

        let operation = move |provider: String| async move {
            if provider.contains("8545") {
                Err(TestError("First provider error".to_string()))
            } else {
                Ok(42)
            }
        };

        let config = RetryConfig::new(2, 1, 0, 0); // Set max_retries to 2 to enable retry exhaustion

        let result = retry_rpc_call(
            &selector,
            "test_operation",
            |_| true, // Errors are retriable to trigger RetriesExhausted and failover
            |_| true, // Errors SHOULD mark provider as failed to enable failover
            provider_initializer,
            operation,
            Some(config),
        )
        .await;

        assert!(result.is_ok(), "Expected OK result but got: {:?}", result);
        assert_eq!(result.unwrap(), 42);

        // Final provider should be the second one
        let final_provider = current_provider.lock().unwrap().clone();
        assert!(
            final_provider.contains("8546"),
            "Wrong provider selected: {}",
            final_provider
        );
    }

    #[tokio::test]
    async fn test_retry_rpc_call_all_providers_fail() {
        let _guard = setup_test_env();

        let configs = vec![
            RpcConfig::new("http://localhost:8545".to_string()),
            RpcConfig::new("http://localhost:8546".to_string()),
        ];
        let selector = RpcSelector::new(configs).expect("Failed to create selector");

        let provider_initializer =
            |_: &str| -> Result<String, TestError> { Ok("mock_provider".to_string()) };

        let operation = |_: String| async { Err(TestError("Always fails".to_string())) };

        let config = RetryConfig::new(2, 1, 0, 0); // Set max_retries to 2 to enable retry exhaustion

        let result: Result<i32, TestError> = retry_rpc_call(
            &selector,
            "test_operation",
            |_| true,  // Errors are retriable to trigger RetriesExhausted and failover
            |_| false, // Errors are NOT retriable to prevent marking
            provider_initializer,
            operation,
            Some(config),
        )
        .await;

        assert!(result.is_err(), "Expected an error but got: {:?}", result);
    }

    #[tokio::test]
    async fn test_retry_rpc_call_with_default_config() {
        let (_guard, selector) = {
            let _lock = RETRY_TEST_ENV_MUTEX
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let guard = setup_test_env();

            let configs = vec![RpcConfig::new("http://localhost:8545".to_string())];
            let selector = RpcSelector::new(configs).expect("Failed to create selector");
            (guard, selector)
        };

        let provider_initializer =
            |_url: &str| -> Result<String, TestError> { Ok("mock_provider".to_string()) };

        let operation = |_provider: String| async move { Ok::<_, TestError>(42) };

        // Test with None config (should use default from env)
        let result = retry_rpc_call(
            &selector,
            "test_operation",
            |_| false,
            |_| false,
            provider_initializer,
            operation,
            None, // Use default config
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_retry_rpc_call_provider_initialization_failures() {
        let _guard = setup_test_env();

        let configs = vec![
            RpcConfig::new("http://localhost:8545".to_string()),
            RpcConfig::new("http://localhost:8546".to_string()),
        ];
        let selector = RpcSelector::new(configs).expect("Failed to create selector");

        let attempt_count = Arc::new(AtomicU8::new(0));
        let attempt_count_clone = attempt_count.clone();

        let provider_initializer = move |url: &str| -> Result<String, TestError> {
            let count = attempt_count_clone.fetch_add(1, AtomicOrdering::SeqCst);
            if count == 0 && url.contains("8545") {
                Err(TestError("First provider init failed".to_string()))
            } else {
                Ok(url.to_string())
            }
        };

        let operation = |_provider: String| async move { Ok::<_, TestError>(42) };

        let config = RetryConfig::new(2, 1, 0, 0);

        let result = retry_rpc_call(
            &selector,
            "test_operation",
            |_| true,
            |_| false,
            provider_initializer,
            operation,
            Some(config),
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42);
        assert!(attempt_count.load(AtomicOrdering::SeqCst) >= 2); // Should have tried multiple providers
    }

    #[test]
    fn test_get_provider_selector_errors() {
        let _guard = setup_test_env();

        // Create selector with a single provider, select it, then mark it as failed
        let configs = vec![RpcConfig::new("http://localhost:8545".to_string())];
        let selector = RpcSelector::new(configs).expect("Failed to create selector");

        // First select the provider to make it current, then mark it as failed
        let _ = selector.get_current_url().unwrap(); // This selects the provider
        selector.mark_current_as_failed(); // Now mark it as failed

        let provider_initializer =
            |url: &str| -> Result<String, TestError> { Ok(format!("provider-{}", url)) };

        // Now get_provider should fail because the only provider is marked as failed
        let result = get_provider(&selector, "test_operation", &provider_initializer);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_last_provider_never_marked_as_failed() {
        let _guard = setup_test_env();

        // Test with a single provider
        let configs = vec![RpcConfig::new("http://localhost:8545".to_string())];
        let selector = RpcSelector::new(configs).expect("Failed to create selector");

        let provider_initializer = |url: &str| -> Result<String, TestError> { Ok(url.to_string()) };

        // Operation that always fails with a retriable error
        let operation = |_provider: String| async { Err(TestError("Always fails".to_string())) };

        let config = RetryConfig::new(2, 1, 0, 0); // 2 retries, 1 failover

        // Get initial provider count
        let initial_available_count = selector.available_provider_count();
        assert_eq!(initial_available_count, 1);

        let result: Result<i32, TestError> = retry_rpc_call(
            &selector,
            "test_operation",
            |_| true, // Error IS retriable
            |_| true, // Error SHOULD mark provider as failed, but last provider should be preserved
            provider_initializer,
            operation,
            Some(config),
        )
        .await;

        assert!(result.is_err());

        // The last provider should NOT be marked as failed
        let final_available_count = selector.available_provider_count();
        assert_eq!(
            final_available_count, initial_available_count,
            "Last provider should never be marked as failed"
        );
        assert_eq!(
            final_available_count, 1,
            "Should still have 1 provider available"
        );
    }

    #[tokio::test]
    async fn test_last_provider_behavior_with_multiple_providers() {
        let _guard = setup_test_env();

        // Test with multiple providers, but mark all but one as failed
        let configs = vec![
            RpcConfig::new("http://localhost:8545".to_string()),
            RpcConfig::new("http://localhost:8546".to_string()),
            RpcConfig::new("http://localhost:8547".to_string()),
        ];
        let selector = RpcSelector::new(configs).expect("Failed to create selector");

        let provider_initializer = |url: &str| -> Result<String, TestError> { Ok(url.to_string()) };

        // Operation that always fails with a retriable error
        let operation = |_provider: String| async { Err(TestError("Always fails".to_string())) };

        let config = RetryConfig::new(2, 2, 0, 0); // 2 retries, 2 failovers

        // Get initial provider count
        let initial_available_count = selector.available_provider_count();
        assert_eq!(initial_available_count, 3);

        let result: Result<i32, TestError> = retry_rpc_call(
            &selector,
            "test_operation",
            |_| true, // Error IS retriable
            |_| true, // Error SHOULD mark provider as failed, but last provider should be preserved
            provider_initializer,
            operation,
            Some(config),
        )
        .await;

        assert!(result.is_err());

        // Should have marked 2 providers as failed, but kept the last one
        let final_available_count = selector.available_provider_count();
        assert_eq!(
            final_available_count, 1,
            "Should have exactly 1 provider left (the last one should not be marked as failed)"
        );
    }

    #[tokio::test]
    async fn test_non_retriable_error_should_mark_provider_failed() {
        let _guard = setup_test_env();

        let configs = vec![
            RpcConfig::new("http://localhost:8545".to_string()),
            RpcConfig::new("http://localhost:8546".to_string()),
        ];
        let selector = RpcSelector::new(configs).expect("Failed to create selector");

        let provider_initializer = |url: &str| -> Result<String, TestError> { Ok(url.to_string()) };

        // Operation that fails with a non-retriable error that SHOULD mark provider as failed
        let operation = |_provider: String| async move {
            Err(TestError("Critical non-retriable error".to_string()))
        };

        let config = RetryConfig::new(3, 1, 0, 0);

        // Get initial provider count
        let initial_available_count = selector.available_provider_count();
        assert_eq!(initial_available_count, 2);

        let result: Result<i32, TestError> = retry_rpc_call(
            &selector,
            "test_operation",
            |_| false,                    // Error is NOT retriable
            |e| e.0.contains("Critical"), // Error SHOULD mark provider as failed if it contains "Critical"
            provider_initializer,
            operation,
            Some(config),
        )
        .await;

        assert!(result.is_err());

        // Provider should be marked as failed because should_mark_provider_failed returned true
        let final_available_count = selector.available_provider_count();
        assert_eq!(final_available_count, 1,
            "Provider should be marked as failed when should_mark_provider_failed returns true for non-retriable error");
    }

    #[tokio::test]
    async fn test_non_retriable_error_should_not_mark_provider_failed() {
        let _guard = setup_test_env();

        let configs = vec![
            RpcConfig::new("http://localhost:8545".to_string()),
            RpcConfig::new("http://localhost:8546".to_string()),
        ];
        let selector = RpcSelector::new(configs).expect("Failed to create selector");

        let provider_initializer = |url: &str| -> Result<String, TestError> { Ok(url.to_string()) };

        // Operation that fails with a non-retriable error that should NOT mark provider as failed
        let operation = |_provider: String| async move {
            Err(TestError("Minor non-retriable error".to_string()))
        };

        let config = RetryConfig::new(3, 1, 0, 0);

        // Get initial provider count
        let initial_available_count = selector.available_provider_count();
        assert_eq!(initial_available_count, 2);

        let result: Result<i32, TestError> = retry_rpc_call(
            &selector,
            "test_operation",
            |_| false,                    // Error is NOT retriable
            |e| e.0.contains("Critical"), // Error should NOT mark provider as failed (doesn't contain "Critical")
            provider_initializer,
            operation,
            Some(config),
        )
        .await;

        assert!(result.is_err());

        // Provider should NOT be marked as failed because should_mark_provider_failed returned false
        let final_available_count = selector.available_provider_count();
        assert_eq!(final_available_count, initial_available_count,
            "Provider should NOT be marked as failed when should_mark_provider_failed returns false for non-retriable error");
    }

    #[tokio::test]
    async fn test_retriable_error_ignores_should_mark_provider_failed() {
        let _guard = setup_test_env();

        let configs = vec![
            RpcConfig::new("http://localhost:8545".to_string()),
            RpcConfig::new("http://localhost:8546".to_string()),
        ];
        let selector = RpcSelector::new(configs).expect("Failed to create selector");

        let provider_initializer = |url: &str| -> Result<String, TestError> { Ok(url.to_string()) };

        // Operation that always fails with a retriable error
        let operation =
            |_provider: String| async { Err(TestError("Retriable network error".to_string())) };

        let config = RetryConfig::new(2, 1, 0, 0); // 2 retries, 1 failover

        // Get initial provider count
        let initial_available_count = selector.available_provider_count();
        assert_eq!(initial_available_count, 2);

        let result: Result<i32, TestError> = retry_rpc_call(
            &selector,
            "test_operation",
            |_| true,  // Error IS retriable
            |_| false, // should_mark_provider_failed returns false, but should be IGNORED for retriable errors
            provider_initializer,
            operation,
            Some(config),
        )
        .await;

        assert!(result.is_err());

        // Provider should be marked as failed despite should_mark_provider_failed returning false,
        // because retriable errors that exhaust retries always mark the provider as failed
        let final_available_count = selector.available_provider_count();
        assert!(final_available_count < initial_available_count,
            "Provider should be marked as failed when retriable errors exhaust retries, regardless of should_mark_provider_failed");
    }

    #[tokio::test]
    async fn test_mixed_error_scenarios_with_different_marking_behavior() {
        let _guard = setup_test_env();

        // Test scenario 1: Non-retriable error that should mark provider as failed
        let configs = vec![
            RpcConfig::new("http://localhost:8545".to_string()),
            RpcConfig::new("http://localhost:8546".to_string()),
        ];
        let selector = RpcSelector::new(configs).expect("Failed to create selector");

        let provider_initializer = |url: &str| -> Result<String, TestError> { Ok(url.to_string()) };

        let operation =
            |_provider: String| async move { Err(TestError("Critical network error".to_string())) };

        let config = RetryConfig::new(1, 1, 0, 0);
        let initial_count = selector.available_provider_count();

        let result: Result<i32, TestError> = retry_rpc_call(
            &selector,
            "test_operation",
            |_| false,                    // Non-retriable
            |e| e.0.contains("Critical"), // Should mark as failed
            provider_initializer,
            operation,
            Some(config.clone()),
        )
        .await;

        assert!(result.is_err());
        let after_critical_count = selector.available_provider_count();
        assert_eq!(
            after_critical_count,
            initial_count - 1,
            "Critical error should mark provider as failed"
        );

        // Test scenario 2: Non-retriable error that should NOT mark provider as failed
        let operation =
            |_provider: String| async move { Err(TestError("Minor validation error".to_string())) };

        let result: Result<i32, TestError> = retry_rpc_call(
            &selector,
            "test_operation",
            |_| false,                    // Non-retriable
            |e| e.0.contains("Critical"), // Should NOT mark as failed (doesn't contain "Critical")
            provider_initializer,
            operation,
            Some(config),
        )
        .await;

        assert!(result.is_err());
        let final_count = selector.available_provider_count();
        assert_eq!(
            final_count, after_critical_count,
            "Minor error should NOT mark provider as failed"
        );
    }

    #[tokio::test]
    async fn test_should_mark_provider_failed_respects_last_provider_protection() {
        let _guard = setup_test_env();

        // Test with a single provider (last provider protection)
        let configs = vec![RpcConfig::new("http://localhost:8545".to_string())];
        let selector = RpcSelector::new(configs).expect("Failed to create selector");

        let provider_initializer = |url: &str| -> Result<String, TestError> { Ok(url.to_string()) };

        // Operation that fails with a non-retriable error that SHOULD mark provider as failed
        let operation =
            |_provider: String| async move { Err(TestError("Critical network error".to_string())) };

        let config = RetryConfig::new(1, 1, 0, 0);

        // Get initial provider count
        let initial_available_count = selector.available_provider_count();
        assert_eq!(initial_available_count, 1);

        let result: Result<i32, TestError> = retry_rpc_call(
            &selector,
            "test_operation",
            |_| false,                    // Error is NOT retriable
            |e| e.0.contains("Critical"), // Error SHOULD mark provider as failed
            provider_initializer,
            operation,
            Some(config),
        )
        .await;

        assert!(result.is_err());

        // Last provider should NEVER be marked as failed, even if should_mark_provider_failed returns true
        let final_available_count = selector.available_provider_count();
        assert_eq!(final_available_count, initial_available_count,
            "Last provider should never be marked as failed, regardless of should_mark_provider_failed");
        assert_eq!(
            final_available_count, 1,
            "Should still have 1 provider available"
        );
    }

    #[tokio::test]
    async fn test_should_mark_provider_failed_with_multiple_providers_last_protection() {
        let _guard = setup_test_env();

        // Test with multiple providers, but ensure last one is protected
        let configs = vec![
            RpcConfig::new("http://localhost:8545".to_string()),
            RpcConfig::new("http://localhost:8546".to_string()),
        ];
        let selector = RpcSelector::new(configs).expect("Failed to create selector");

        let attempt_count = Arc::new(AtomicU8::new(0));
        let attempt_count_clone = attempt_count.clone();

        let provider_initializer = |url: &str| -> Result<String, TestError> { Ok(url.to_string()) };

        // Operation that always fails with errors that should mark provider as failed
        let operation = move |_provider: String| {
            let attempt_count = attempt_count_clone.clone();
            async move {
                let count = attempt_count.fetch_add(1, AtomicOrdering::SeqCst);
                Err(TestError(format!("Critical error #{}", count)))
            }
        };

        let config = RetryConfig::new(1, 1, 0, 0); // 1 retry, 1 failover

        // Get initial provider count
        let initial_available_count = selector.available_provider_count();
        assert_eq!(initial_available_count, 2);

        let result: Result<i32, TestError> = retry_rpc_call(
            &selector,
            "test_operation",
            |_| false,                    // All errors are non-retriable
            |e| e.0.contains("Critical"), // All errors should mark provider as failed
            provider_initializer,
            operation,
            Some(config),
        )
        .await;

        assert!(result.is_err());

        // First provider should be marked as failed, but last provider should be protected
        let final_available_count = selector.available_provider_count();
        assert_eq!(
            final_available_count, 1,
            "First provider should be marked as failed, but last provider should be protected"
        );
    }
}
