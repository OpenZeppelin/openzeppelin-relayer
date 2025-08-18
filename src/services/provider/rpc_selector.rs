//! # RPC Provider Selector
//!
//! This module provides functionality for dynamically selecting RPC endpoints based on configured priorities,
//! health status, and selection strategies.
//!
//! ## Features
//!
//! - **Weighted selection**: Providers can be assigned different weights to control selection probability
//! - **Round-robin fallback**: If weighted selection fails or weights are equal, round-robin is used
//! - **Health tracking**: Failed providers are temporarily excluded from selection
//! - **Automatic recovery**: Failed providers are automatically recovered after a configurable period
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use eyre::Result;
use parking_lot::RwLock;
use rand::distr::weighted::WeightedIndex;
use rand::prelude::*;
use serde::Serialize;
use thiserror::Error;
use tokio::time::Instant;

use crate::models::RpcConfig;

#[derive(Error, Debug, Serialize)]
pub enum RpcSelectorError {
    #[error("No providers available")]
    NoProviders,
    #[error("Client initialization failed: {0}")]
    ClientInitializationError(String),
    #[error("Weighted index error: {0}")]
    WeightedIndexError(String),
    #[error("All available providers have failed")]
    AllProvidersFailed,
}

// Provider health tracking struct
#[derive(Debug)]
struct ProviderHealth {
    // Maps the index of each failed provider to the timestamp (Instant) when it will become available for use again.
    failed_provider_reset_times: std::collections::HashMap<usize, Instant>,
    // The amount of time a provider remains unavailable after being marked as failed.
    reset_duration: Duration,
}

impl ProviderHealth {
    // Create a new ProviderHealth tracker with a given reset duration
    fn new(reset_duration: Duration) -> Self {
        Self {
            failed_provider_reset_times: std::collections::HashMap::new(),
            reset_duration,
        }
    }

    // Mark a provider as failed
    fn mark_failed(&mut self, index: usize) {
        let reset_time = Instant::now() + self.reset_duration;
        self.failed_provider_reset_times.insert(index, reset_time);
    }

    // Check if a provider is marked as failed and handle auto-reset if needed
    fn is_failed(&mut self, index: usize) -> bool {
        if let Some(reset_time) = self.failed_provider_reset_times.get(&index) {
            if Instant::now() >= *reset_time {
                // Time has passed, remove from failed set (auto-reset for this provider)
                self.failed_provider_reset_times.remove(&index);
                return false;
            }
            return true;
        }
        false
    }

    // Reset all failed providers
    fn reset(&mut self) {
        self.failed_provider_reset_times.clear();
    }

    // Get the number of failed providers whose reset time has not yet passed.
    fn failed_count(&self) -> usize {
        self.failed_provider_reset_times
            .values()
            .filter(|&&reset_time| Instant::now() < reset_time)
            .count()
    }
}

/// Manages selection of RPC endpoints based on configuration.
#[derive(Debug)]
pub struct RpcSelector {
    /// RPC configurations
    configs: Vec<RpcConfig>,
    /// Pre-computed weighted distribution for faster provider selection
    weights_dist: Option<Arc<WeightedIndex<u8>>>,
    /// Counter for round-robin selection as a fallback or for equal weights
    next_index: Arc<AtomicUsize>,
    /// Health tracking for providers
    health: Arc<RwLock<ProviderHealth>>,
    /// Currently selected provider index
    current_index: Arc<AtomicUsize>,
    /// Flag indicating whether a current provider is valid
    has_current: Arc<AtomicBool>,
}

// Auto-reset duration for failed providers (5 minutes)
const DEFAULT_PROVIDER_RESET_DURATION: Duration = Duration::from_secs(300);

impl RpcSelector {
    /// Creates a new RpcSelector instance.
    ///
    /// # Arguments
    /// * `configs` - A vector of RPC configurations (URL and weight)
    ///
    /// # Returns
    /// * `Result<Self>` - A new selector instance or an error
    pub fn new(configs: Vec<RpcConfig>) -> Result<Self, RpcSelectorError> {
        if configs.is_empty() {
            return Err(RpcSelectorError::NoProviders);
        }

        // Create the weights distribution based on provided weights
        let weights_dist = Self::create_weights_distribution(&configs, &HashSet::new());

        // Initialize health tracker with default reset duration
        let health = ProviderHealth::new(DEFAULT_PROVIDER_RESET_DURATION);

        Ok(Self {
            configs,
            weights_dist,
            next_index: Arc::new(AtomicUsize::new(0)),
            health: Arc::new(RwLock::new(health)),
            current_index: Arc::new(AtomicUsize::new(0)),
            has_current: Arc::new(AtomicBool::new(false)), // Initially no current provider
        })
    }

    /// Gets the number of available providers
    ///
    /// # Returns
    /// * `usize` - The number of providers in the selector
    pub fn provider_count(&self) -> usize {
        self.configs.len()
    }

    /// Gets the number of available (non-failed) providers
    ///
    /// # Returns
    /// * `usize` - The number of non-failed providers
    pub fn available_provider_count(&self) -> usize {
        let health = self.health.read();
        self.configs.len() - health.failed_count()
    }

    /// Marks the current endpoint as failed and forces selection of a different endpoint.
    ///
    /// This method is used when a provider consistently fails, and we want to try a different one.
    /// It adds the current provider to the failed providers set and will avoid selecting it again.
    pub fn mark_current_as_failed(&self) {
        // Only proceed if we have a current provider
        if self.has_current.load(Ordering::Relaxed) {
            let current = self.current_index.load(Ordering::Relaxed);

            // Mark this provider as failed
            let mut health = self.health.write();
            health.mark_failed(current);

            // Clear the current provider
            self.has_current.store(false, Ordering::Relaxed);

            // Move round-robin index forward to avoid selecting the same provider again
            if self.configs.len() > 1 {
                self.next_index.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Resets the failed providers set, making all providers available again.
    pub fn reset_failed_providers(&self) {
        let mut health = self.health.write();
        health.reset();
    }

    /// Creates a weighted distribution for selecting RPC endpoints based on their weights.
    ///
    /// # Arguments
    /// * `configs` - A slice of RPC configurations with weights
    /// * `excluded_indices` - A set of indices to exclude from the distribution
    ///
    /// # Returns
    /// * `Option<Arc<WeightedIndex<u8>>>` - A weighted distribution if configs have different weights, None otherwise
    fn create_weights_distribution(
        configs: &[RpcConfig],
        excluded_indices: &HashSet<usize>,
    ) -> Option<Arc<WeightedIndex<u8>>> {
        // Collect weights, using 0 for excluded providers
        let weights: Vec<u8> = configs
            .iter()
            .enumerate()
            .map(|(idx, config)| {
                if excluded_indices.contains(&idx) {
                    0
                } else {
                    config.get_weight()
                }
            })
            .collect();

        // Count available providers with non-zero weight
        let available_count = weights.iter().filter(|&&w| w > 0).count();
        if available_count == 0 {
            return None;
        }

        let first_non_zero_weight = weights.iter().find(|&&w| w > 0).copied();
        if let Some(first_weight) = first_non_zero_weight {
            // First check for the original equal weights case
            let all_equal = weights
                .iter()
                .filter(|&&w| w > 0)
                .all(|&w| w == first_weight);

            if all_equal {
                return None;
            }
        }

        // Create weighted distribution
        match WeightedIndex::new(&weights) {
            Ok(dist) => Some(Arc::new(dist)),
            Err(_) => None,
        }
    }

    /// Gets the URL of the next RPC endpoint based on the selection strategy.
    fn select_url(&self) -> Result<&str, RpcSelectorError> {
        if self.configs.is_empty() {
            return Err(RpcSelectorError::NoProviders);
        }

        // For a single provider, handle special case
        if self.configs.len() == 1 {
            let mut health = self.health.write();
            if health.is_failed(0) {
                // is_failed will attempt auto-reset for provider 0
                return Err(RpcSelectorError::AllProvidersFailed);
            }

            // Set as current
            self.current_index.store(0, Ordering::Relaxed);
            self.has_current.store(true, Ordering::Relaxed);
            return Ok(&self.configs[0].url);
        }

        // Try weighted selection first if available
        if let Some(dist) = &self.weights_dist {
            let mut rng = rand::rng();
            let mut health = self.health.write();

            // Try a limited number of times to find a non-failed provider with weighted selection
            const MAX_ATTEMPTS: usize = 5;
            for _ in 0..MAX_ATTEMPTS {
                let index = dist.sample(&mut rng);
                if !health.is_failed(index) {
                    self.current_index.store(index, Ordering::Relaxed);
                    self.has_current.store(true, Ordering::Relaxed);
                    return Ok(&self.configs[index].url);
                }
            }
            // If we couldn't find a provider after multiple attempts, fall back to round-robin
        }

        // Fall back to round-robin selection
        let len = self.configs.len();
        let start_index = self.next_index.load(Ordering::Relaxed) % len;

        // Find the next available (non-failed) provider
        for i in 0..len {
            let index = (start_index + i) % len;
            // Skip providers with zero weight
            if self.configs[index].get_weight() == 0 {
                continue;
            }

            let mut health = self.health.write();
            if !health.is_failed(index) {
                // Update the next_index atomically to point after this provider
                self.next_index.store((index + 1) % len, Ordering::Relaxed);

                // Set as current provider
                self.current_index.store(index, Ordering::Relaxed);
                self.has_current.store(true, Ordering::Relaxed);

                return Ok(&self.configs[index].url);
            }
        }

        // If we get here, all providers must have failed
        Err(RpcSelectorError::AllProvidersFailed)
    }

    /// Gets the URL of the currently selected RPC endpoint.
    ///
    /// # Returns
    /// * `Result<String, RpcSelectorError>` - The URL of the current provider, or an error
    pub fn get_current_url(&self) -> Result<String, RpcSelectorError> {
        self.select_url().map(|url| url.to_string())
    }

    /// Gets a client for the selected RPC endpoint.
    ///
    /// # Arguments
    /// * `initializer` - A function that takes a URL string and returns a `Result<T>`
    ///
    /// # Returns
    /// * `Result<T>` - The client instance or an error
    pub fn get_client<T>(
        &self,
        initializer: impl Fn(&str) -> Result<T>,
    ) -> Result<T, RpcSelectorError> {
        let url = self.select_url()?;

        initializer(url).map_err(|e| RpcSelectorError::ClientInitializationError(e.to_string()))
    }
}

// Implement Clone for RpcSelector manually since the generic T doesn't require Clone
impl Clone for RpcSelector {
    fn clone(&self) -> Self {
        Self {
            configs: self.configs.clone(),
            weights_dist: self.weights_dist.clone(),
            next_index: Arc::clone(&self.next_index),
            health: Arc::clone(&self.health),
            current_index: Arc::clone(&self.current_index),
            has_current: Arc::clone(&self.has_current),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_create_weights_distribution_single_config() {
        let configs = vec![RpcConfig {
            url: "https://example.com/rpc".to_string(),
            weight: 1,
        }];

        let excluded = HashSet::new();
        let result = RpcSelector::create_weights_distribution(&configs, &excluded);
        assert!(result.is_none());
    }

    #[test]
    fn test_create_weights_distribution_equal_weights() {
        let configs = vec![
            RpcConfig {
                url: "https://example1.com/rpc".to_string(),
                weight: 5,
            },
            RpcConfig {
                url: "https://example2.com/rpc".to_string(),
                weight: 5,
            },
            RpcConfig {
                url: "https://example3.com/rpc".to_string(),
                weight: 5,
            },
        ];

        let excluded = HashSet::new();
        let result = RpcSelector::create_weights_distribution(&configs, &excluded);
        assert!(result.is_none());
    }

    #[test]
    fn test_create_weights_distribution_different_weights() {
        let configs = vec![
            RpcConfig {
                url: "https://example1.com/rpc".to_string(),
                weight: 1,
            },
            RpcConfig {
                url: "https://example2.com/rpc".to_string(),
                weight: 2,
            },
            RpcConfig {
                url: "https://example3.com/rpc".to_string(),
                weight: 3,
            },
        ];

        let excluded = HashSet::new();
        let result = RpcSelector::create_weights_distribution(&configs, &excluded);
        assert!(result.is_some());
    }

    #[test]
    fn test_create_weights_distribution_with_excluded() {
        let configs = vec![
            RpcConfig {
                url: "https://example1.com/rpc".to_string(),
                weight: 1,
            },
            RpcConfig {
                url: "https://example2.com/rpc".to_string(),
                weight: 2,
            },
            RpcConfig {
                url: "https://example3.com/rpc".to_string(),
                weight: 3,
            },
        ];

        // Exclude the first provider
        let mut excluded = HashSet::new();
        excluded.insert(0);

        let result = RpcSelector::create_weights_distribution(&configs, &excluded);
        assert!(result.is_some());

        // Exclude two providers (with only one remaining, should return None)
        excluded.insert(1);
        let result = RpcSelector::create_weights_distribution(&configs, &excluded);
        assert!(result.is_none());
    }

    #[test]
    fn test_rpc_selector_new_empty_configs() {
        let configs: Vec<RpcConfig> = vec![];
        let result = RpcSelector::new(configs);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), RpcSelectorError::NoProviders));
    }

    #[test]
    fn test_rpc_selector_new_single_config() {
        let configs = vec![RpcConfig {
            url: "https://example.com/rpc".to_string(),
            weight: 1,
        }];

        let result = RpcSelector::new(configs);
        assert!(result.is_ok());
        let selector = result.unwrap();
        assert!(selector.weights_dist.is_none());
    }

    #[test]
    fn test_rpc_selector_new_multiple_equal_weights() {
        let configs = vec![
            RpcConfig {
                url: "https://example1.com/rpc".to_string(),
                weight: 5,
            },
            RpcConfig {
                url: "https://example2.com/rpc".to_string(),
                weight: 5,
            },
        ];

        let result = RpcSelector::new(configs);
        assert!(result.is_ok());
        let selector = result.unwrap();
        assert!(selector.weights_dist.is_none());
    }

    #[test]
    fn test_rpc_selector_new_multiple_different_weights() {
        let configs = vec![
            RpcConfig {
                url: "https://example1.com/rpc".to_string(),
                weight: 1,
            },
            RpcConfig {
                url: "https://example2.com/rpc".to_string(),
                weight: 3,
            },
        ];

        let result = RpcSelector::new(configs);
        assert!(result.is_ok());
        let selector = result.unwrap();
        assert!(selector.weights_dist.is_some());
    }

    #[test]
    fn test_rpc_selector_select_url_single_provider() {
        let configs = vec![RpcConfig {
            url: "https://example.com/rpc".to_string(),
            weight: 1,
        }];

        let selector = RpcSelector::new(configs).unwrap();
        let result = selector.select_url();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "https://example.com/rpc");
        assert!(selector.has_current.load(Ordering::Relaxed));
    }

    #[test]
    fn test_rpc_selector_select_url_round_robin() {
        let configs = vec![
            RpcConfig {
                url: "https://example1.com/rpc".to_string(),
                weight: 1,
            },
            RpcConfig {
                url: "https://example2.com/rpc".to_string(),
                weight: 1,
            },
        ];

        let selector = RpcSelector::new(configs).unwrap();

        // First call should return the first URL
        let first_url = selector.select_url().unwrap();
        // Second call should return the second URL due to round-robin
        let second_url = selector.select_url().unwrap();
        // Third call should return the first URL again
        let third_url = selector.select_url().unwrap();

        // We don't know which URL comes first, but the sequence should alternate
        assert_ne!(first_url, second_url);
        assert_eq!(first_url, third_url);
    }

    #[test]
    fn test_rpc_selector_get_client_success() {
        let configs = vec![RpcConfig {
            url: "https://example.com/rpc".to_string(),
            weight: 1,
        }];

        let selector = RpcSelector::new(configs).unwrap();

        // Create a simple initializer function that returns the URL as a string
        let initializer = |url: &str| -> Result<String> { Ok(url.to_string()) };

        let result = selector.get_client(initializer);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "https://example.com/rpc");
    }

    #[test]
    fn test_rpc_selector_get_client_failure() {
        let configs = vec![RpcConfig {
            url: "https://example.com/rpc".to_string(),
            weight: 1,
        }];

        let selector = RpcSelector::new(configs).unwrap();

        // Create a failing initializer function
        let initializer =
            |_url: &str| -> Result<String> { Err(eyre::eyre!("Initialization error")) };

        let result = selector.get_client(initializer);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            RpcSelectorError::ClientInitializationError(_)
        ));
    }

    #[test]
    fn test_rpc_selector_clone() {
        let configs = vec![
            RpcConfig {
                url: "https://example1.com/rpc".to_string(),
                weight: 1,
            },
            RpcConfig {
                url: "https://example2.com/rpc".to_string(),
                weight: 3,
            },
        ];

        let selector = RpcSelector::new(configs).unwrap();
        let cloned = selector.clone();

        // Check that the cloned selector has the same configuration
        assert_eq!(selector.configs.len(), cloned.configs.len());
        assert_eq!(selector.configs[0].url, cloned.configs[0].url);
        assert_eq!(selector.configs[1].url, cloned.configs[1].url);

        // Check that weights distribution is also cloned
        assert_eq!(
            selector.weights_dist.is_some(),
            cloned.weights_dist.is_some()
        );
    }

    #[test]
    fn test_mark_current_as_failed_single_provider() {
        // With a single provider, marking as failed should cause an error when trying to select it again
        let configs = vec![RpcConfig {
            url: "https://example.com/rpc".to_string(),
            weight: 1,
        }];

        let selector = RpcSelector::new(configs).unwrap();
        let initial_url = selector.select_url().unwrap();

        // Mark as failed
        selector.mark_current_as_failed();

        // Next call should return an error
        let next_url = selector.select_url();
        assert!(next_url.is_err());
        assert!(matches!(
            next_url.unwrap_err(),
            RpcSelectorError::AllProvidersFailed
        ));

        // Reset failed providers
        selector.reset_failed_providers();

        // Now we should be able to select the provider again
        let after_reset = selector.select_url();
        assert!(after_reset.is_ok());
        assert_eq!(initial_url, after_reset.unwrap());
    }

    #[test]
    fn test_mark_current_as_failed_multiple_providers() {
        // With multiple providers, marking as failed should prevent that provider from being selected again
        let configs = vec![
            RpcConfig {
                url: "https://example1.com/rpc".to_string(),
                weight: 5,
            },
            RpcConfig {
                url: "https://example2.com/rpc".to_string(),
                weight: 5,
            },
            RpcConfig {
                url: "https://example3.com/rpc".to_string(),
                weight: 5,
            },
        ];

        let selector = RpcSelector::new(configs).unwrap();

        // Get the first URL
        let url1 = selector.select_url().unwrap().to_string();

        // Mark as failed to move to a different one
        selector.mark_current_as_failed();
        let url2 = selector.select_url().unwrap().to_string();

        // The URLs should be different
        assert_ne!(url1, url2);

        // Mark the second URL as failed too
        selector.mark_current_as_failed();
        let url3 = selector.select_url().unwrap().to_string();

        // Should get a third different URL
        assert_ne!(url1, url3);
        assert_ne!(url2, url3);

        // Mark the third URL as failed too
        selector.mark_current_as_failed();

        // Now all URLs should be marked as failed, so next call should return error
        let url4 = selector.select_url();
        assert!(url4.is_err());
        assert!(matches!(
            url4.unwrap_err(),
            RpcSelectorError::AllProvidersFailed
        ));
    }

    #[test]
    fn test_mark_current_as_failed_weighted() {
        // Test with weighted selection
        let configs = vec![
            RpcConfig {
                url: "https://example1.com/rpc".to_string(),
                weight: 1, // Low weight
            },
            RpcConfig {
                url: "https://example2.com/rpc".to_string(),
                weight: 10, // High weight
            },
        ];

        let selector = RpcSelector::new(configs).unwrap();
        assert!(selector.weights_dist.is_some()); // Confirm we're using weighted selection

        // Get a URL
        let url1 = selector.select_url().unwrap().to_string();

        // Mark it as failed
        selector.mark_current_as_failed();

        // Get another URL, it should be different
        let url2 = selector.select_url().unwrap().to_string();
        assert_ne!(url1, url2);

        // Mark this one as failed too
        selector.mark_current_as_failed();

        // With no more providers, next call should fail
        let url3 = selector.select_url();
        assert!(url3.is_err());

        // Reset and try again
        selector.reset_failed_providers();
        let url4 = selector.select_url();
        assert!(url4.is_ok());
    }

    #[test]
    fn test_auto_reset_mechanism() {
        // Create a selector with a very short reset duration
        let configs = vec![
            RpcConfig {
                url: "https://example1.com/rpc".to_string(),
                weight: 1,
            },
            RpcConfig {
                url: "https://example2.com/rpc".to_string(),
                weight: 1,
            },
        ];

        // Change the auto-reset duration for this test
        let selector = RpcSelector::new(configs).unwrap();
        {
            let mut health = selector.health.write();
            *health = ProviderHealth::new(Duration::from_millis(100)); // Very short duration for testing
        }

        // Select and mark both as failed
        selector.select_url().unwrap();
        selector.mark_current_as_failed();
        selector.select_url().unwrap();
        selector.mark_current_as_failed();

        // Immediately after, all providers should be failed
        let result = selector.select_url();
        assert!(result.is_err());

        // Sleep for longer than the reset duration
        thread::sleep(Duration::from_millis(150));

        // Force a check for auto-reset by directly calling is_failed()
        {
            let mut health = selector.health.write();
            // This should trigger auto-reset
            let _ = health.is_failed(0);
        }

        // After sleeping and checking, providers should be auto-reset
        let result = selector.select_url();
        assert!(
            result.is_ok(),
            "Providers should have been auto-reset after timeout"
        );
    }

    #[test]
    fn test_provider_count() {
        // Test with no providers
        let configs: Vec<RpcConfig> = vec![];
        let result = RpcSelector::new(configs);
        assert!(result.is_err());

        // Test with a single provider
        let configs = vec![RpcConfig {
            url: "https://example.com/rpc".to_string(),
            weight: 1,
        }];
        let selector = RpcSelector::new(configs).unwrap();
        assert_eq!(selector.provider_count(), 1);

        // Test with multiple providers
        let configs = vec![
            RpcConfig {
                url: "https://example1.com/rpc".to_string(),
                weight: 1,
            },
            RpcConfig {
                url: "https://example2.com/rpc".to_string(),
                weight: 2,
            },
            RpcConfig {
                url: "https://example3.com/rpc".to_string(),
                weight: 3,
            },
        ];
        let selector = RpcSelector::new(configs).unwrap();
        assert_eq!(selector.provider_count(), 3);
    }

    #[test]
    fn test_available_provider_count() {
        let configs = vec![
            RpcConfig {
                url: "https://example1.com/rpc".to_string(),
                weight: 1,
            },
            RpcConfig {
                url: "https://example2.com/rpc".to_string(),
                weight: 2,
            },
            RpcConfig {
                url: "https://example3.com/rpc".to_string(),
                weight: 3,
            },
        ];

        let selector = RpcSelector::new(configs).unwrap();
        assert_eq!(selector.provider_count(), 3);
        assert_eq!(selector.available_provider_count(), 3);

        // Mark one provider as failed
        selector.select_url().unwrap(); // Select a provider first
        selector.mark_current_as_failed();
        assert_eq!(selector.available_provider_count(), 2);

        // Mark another provider as failed
        selector.select_url().unwrap(); // Select another provider
        selector.mark_current_as_failed();
        assert_eq!(selector.available_provider_count(), 1);

        // Reset failed providers
        selector.reset_failed_providers();
        assert_eq!(selector.available_provider_count(), 3);
    }

    #[test]
    fn test_get_current_url() {
        let configs = vec![
            RpcConfig::new("https://example1.com/rpc".to_string()),
            RpcConfig::new("https://example2.com/rpc".to_string()),
        ];

        let selector = RpcSelector::new(configs).unwrap();

        // Should return a valid URL
        let url = selector.get_current_url();
        assert!(url.is_ok());
        let url_str = url.unwrap();
        assert!(
            url_str == "https://example1.com/rpc" || url_str == "https://example2.com/rpc",
            "Unexpected URL: {}",
            url_str
        );
    }

    #[test]
    fn test_concurrent_usage() {
        // Test RpcSelector with concurrent access from multiple threads
        let configs = vec![
            RpcConfig::new("https://example1.com/rpc".to_string()),
            RpcConfig::new("https://example2.com/rpc".to_string()),
            RpcConfig::new("https://example3.com/rpc".to_string()),
        ];

        let selector = RpcSelector::new(configs).unwrap();
        let selector_arc = Arc::new(selector);

        let mut handles = Vec::with_capacity(10);

        // Launch 10 threads that select and mark providers
        for _ in 0..10 {
            let selector_clone = Arc::clone(&selector_arc);
            let handle = thread::spawn(move || {
                let url = selector_clone.select_url().unwrap().to_string();
                if url.contains("example1") {
                    // Only mark example1 as failed
                    selector_clone.mark_current_as_failed();
                }
                url
            });
            handles.push(handle);
        }

        // Collect results
        let mut urls = Vec::new();
        for handle in handles {
            urls.push(handle.join().unwrap());
        }

        // Check that at least some threads got different URLs
        let unique_urls: std::collections::HashSet<String> = urls.into_iter().collect();
        assert!(unique_urls.len() > 1, "Expected multiple unique URLs");

        // After all threads, example1 should be marked as failed
        let mut found_non_example1 = false;
        for _ in 0..10 {
            let url = selector_arc.select_url().unwrap().to_string();
            if !url.contains("example1") {
                found_non_example1 = true;
            }
        }

        assert!(found_non_example1, "Should avoid selecting failed provider");
    }

    #[test]
    fn test_provider_health_methods() {
        let duration = Duration::from_secs(10);
        let mut health = ProviderHealth::new(duration);

        // Initially no failed providers
        assert_eq!(health.failed_count(), 0);
        assert!(!health.is_failed(0));

        // Mark as failed and verify
        health.mark_failed(0);
        assert_eq!(health.failed_count(), 1);
        assert!(health.is_failed(0));

        // Reset and verify
        health.reset();
        assert_eq!(health.failed_count(), 0);
        assert!(!health.is_failed(0));
    }

    #[test]
    fn test_consecutive_mark_as_failed() {
        let configs = vec![
            RpcConfig::new("https://example1.com/rpc".to_string()),
            RpcConfig::new("https://example2.com/rpc".to_string()),
        ];

        let selector = RpcSelector::new(configs).unwrap();

        // First call to select a provider
        selector.select_url().unwrap();

        // Mark as failed twice consecutively without selecting in between
        selector.mark_current_as_failed();
        selector.mark_current_as_failed(); // This should be a no-op since has_current is now 0

        // We should still be able to select a provider (since only one was marked failed)
        let result = selector.select_url();
        assert!(result.is_ok());
    }

    #[test]
    fn test_partial_auto_reset() {
        let configs = vec![
            RpcConfig::new("https://example1.com/rpc".to_string()),
            RpcConfig::new("https://example2.com/rpc".to_string()),
            RpcConfig::new("https://example3.com/rpc".to_string()),
        ];

        let selector = RpcSelector::new(configs).unwrap();

        // Override the reset durations to be different
        {
            let mut health = selector.health.write();
            *health = ProviderHealth::new(Duration::from_millis(50)); // Very short duration
        }

        // Select and mark all providers as failed
        for _ in 0..3 {
            selector.select_url().unwrap();
            selector.mark_current_as_failed();
        }

        // All providers should now be marked as failed
        assert!(selector.select_url().is_err());

        // Mark provider 0 with a longer timeout manually
        {
            let mut health = selector.health.write();
            health
                .failed_provider_reset_times
                .insert(0, Instant::now() + Duration::from_millis(200));
        }

        // Sleep for enough time to auto-reset providers 1 and 2, but not 0
        thread::sleep(Duration::from_millis(100));

        // Now provider 0 should still be failed, but 1 and 2 should be available
        let url = selector.select_url();
        assert!(url.is_ok());

        // The selected URL should not be provider 0
        assert!(!url.unwrap().contains("example1"));
    }

    #[test]
    fn test_weighted_to_round_robin_fallback() {
        let configs = vec![
            RpcConfig {
                url: "https://example1.com/rpc".to_string(),
                weight: 10, // High weight
            },
            RpcConfig {
                url: "https://example2.com/rpc".to_string(),
                weight: 1, // Low weight
            },
            RpcConfig {
                url: "https://example3.com/rpc".to_string(),
                weight: 1, // Low weight
            },
        ];

        let selector = RpcSelector::new(configs).unwrap();
        assert!(selector.weights_dist.is_some()); // Using weighted selection

        // Mock a situation where weighted selection would fail multiple times
        // by marking the high-weight provider as failed
        let mut selected_first = false;

        // Try multiple times - the first provider should be selected more often due to weight
        for _ in 0..10 {
            let url = selector.select_url().unwrap();
            if url.contains("example1") {
                selected_first = true;
                // Mark the high-weight provider as failed
                selector.mark_current_as_failed();
                break;
            }
        }

        assert!(
            selected_first,
            "High-weight provider should have been selected"
        );

        // After marking it failed, the other providers should be selected
        let mut seen_urls = HashSet::new();
        for _ in 0..10 {
            let url = selector.select_url().unwrap().to_string();
            seen_urls.insert(url);
        }

        // Should have seen at least example2 and example3
        assert!(seen_urls.len() >= 2);
        assert!(
            !seen_urls.iter().any(|url| url.contains("example1")),
            "Failed provider should not be selected"
        );
    }

    #[test]
    fn test_zero_weight_providers() {
        let configs = vec![
            RpcConfig {
                url: "https://example1.com/rpc".to_string(),
                weight: 0, // Zero weight
            },
            RpcConfig {
                url: "https://example2.com/rpc".to_string(),
                weight: 5, // Normal weight
            },
        ];

        let selector = RpcSelector::new(configs).unwrap();

        // With weighted selection, should never select the zero-weight provider
        let mut seen_urls = HashSet::new();
        for _ in 0..10 {
            let url = selector.select_url().unwrap().to_string();
            seen_urls.insert(url);
        }

        assert_eq!(seen_urls.len(), 1);
        assert!(
            seen_urls.iter().next().unwrap().contains("example2"),
            "Only the non-zero weight provider should be selected"
        );
    }

    #[test]
    fn test_extreme_weight_differences() {
        let configs = vec![
            RpcConfig {
                url: "https://example1.com/rpc".to_string(),
                weight: 100, // Very high weight
            },
            RpcConfig {
                url: "https://example2.com/rpc".to_string(),
                weight: 1, // Very low weight
            },
        ];

        let selector = RpcSelector::new(configs).unwrap();

        // High weight provider should be selected much more frequently
        let mut count_high = 0;

        for _ in 0..100 {
            let url = selector.select_url().unwrap().to_string();
            if url.contains("example1") {
                count_high += 1;
            }

            // Reset to clear current selection
            selector.has_current.store(false, Ordering::Relaxed);
        }

        // High-weight provider should be selected at least 90% of the time
        assert!(
            count_high > 90,
            "High-weight provider selected only {}/{} times",
            count_high,
            100
        );
    }

    #[test]
    fn test_mark_unselected_as_failed() {
        let configs = vec![
            RpcConfig::new("https://example1.com/rpc".to_string()),
            RpcConfig::new("https://example2.com/rpc".to_string()),
        ];

        let selector = RpcSelector::new(configs).unwrap();

        // Without selecting, mark as failed (should be a no-op)
        selector.mark_current_as_failed();

        // Should still be able to select both providers
        let mut seen_urls = HashSet::new();
        for _ in 0..10 {
            let url = selector.select_url().unwrap().to_string();
            seen_urls.insert(url);

            // Reset for next iteration
            selector.has_current.store(false, Ordering::Relaxed);
        }

        assert_eq!(
            seen_urls.len(),
            2,
            "Both providers should still be available"
        );
    }

    #[test]
    fn test_rpc_selector_error_serialization() {
        let error = RpcSelectorError::NoProviders;
        let json = serde_json::to_string(&error).unwrap();
        assert!(json.contains("NoProviders"));

        let error = RpcSelectorError::ClientInitializationError("test error".to_string());
        let json = serde_json::to_string(&error).unwrap();
        assert!(json.contains("ClientInitializationError"));
        assert!(json.contains("test error"));

        let error = RpcSelectorError::WeightedIndexError("index error".to_string());
        let json = serde_json::to_string(&error).unwrap();
        assert!(json.contains("WeightedIndexError"));
        assert!(json.contains("index error"));

        let error = RpcSelectorError::AllProvidersFailed;
        let json = serde_json::to_string(&error).unwrap();
        assert!(json.contains("AllProvidersFailed"));
    }
}
