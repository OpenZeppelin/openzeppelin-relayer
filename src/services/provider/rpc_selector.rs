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

use eyre::Result;
use parking_lot::RwLock;
use rand::distr::weighted::WeightedIndex;
use rand::prelude::*;
use serde::Serialize;
use thiserror::Error;
use tracing::info;

use crate::models::RpcConfig;
use crate::services::provider::rpc_health_store::RpcHealthStore;

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

/// Manages selection of RPC endpoints based on configuration.
#[derive(Debug)]
pub struct RpcSelector {
    /// RPC configurations
    configs: Arc<RwLock<Vec<RpcConfig>>>,
    /// Pre-computed weighted distribution for faster provider selection
    weights_dist: Option<Arc<WeightedIndex<u8>>>,
    /// Counter for round-robin selection as a fallback or for equal weights
    next_index: Arc<AtomicUsize>,
    /// Currently selected provider index
    current_index: Arc<AtomicUsize>,
    /// Flag indicating whether a current provider is valid
    has_current: Arc<AtomicBool>,
    /// Number of consecutive failures before pausing a provider
    failure_threshold: u32,
    /// Duration in seconds to pause a provider after reaching failure threshold
    pause_duration_secs: u64,
    /// Duration in seconds after which failures are considered stale and reset
    failure_expiration_secs: u64,
}

impl RpcSelector {
    /// Creates a new RpcSelector instance.
    ///
    /// # Arguments
    /// * `configs` - RPC configurations
    /// * `failure_threshold` - Number of consecutive failures before pausing a provider.
    ///   Defaults to [`DEFAULT_PROVIDER_FAILURE_THRESHOLD`] if not provided via env var `PROVIDER_FAILURE_THRESHOLD`.
    /// * `pause_duration_secs` - Duration in seconds to pause a provider after reaching failure threshold.
    ///   Defaults to [`DEFAULT_PROVIDER_PAUSE_DURATION_SECS`] if not provided via env var `PROVIDER_PAUSE_DURATION_SECS`.
    /// * `failure_expiration_secs` - Duration in seconds after which failures are considered stale and reset.
    ///   Defaults to [`DEFAULT_PROVIDER_FAILURE_EXPIRATION_SECS`] (60 seconds).
    ///
    /// # Returns
    /// * `Result<Self>` - A new selector instance or an error
    ///
    /// # Note
    /// These values are typically loaded from `ServerConfig::from_env()` which reads from environment variables:
    /// - `PROVIDER_FAILURE_THRESHOLD` (default: 3, legacy: `RPC_FAILURE_THRESHOLD`)
    /// - `PROVIDER_PAUSE_DURATION_SECS` (default: 60, legacy: `RPC_PAUSE_DURATION_SECS`)
    pub fn new(
        configs: Vec<RpcConfig>,
        failure_threshold: u32,
        pause_duration_secs: u64,
        failure_expiration_secs: u64,
    ) -> Result<Self, RpcSelectorError> {
        if configs.is_empty() {
            return Err(RpcSelectorError::NoProviders);
        }

        // Create the weights distribution based on provided weights
        let weights_dist = Self::create_weights_distribution(&configs, &HashSet::new());

        let selector = Self {
            configs: Arc::new(RwLock::new(configs)),
            weights_dist,
            next_index: Arc::new(AtomicUsize::new(0)),
            current_index: Arc::new(AtomicUsize::new(0)),
            has_current: Arc::new(AtomicBool::new(false)), // Initially no current provider
            failure_threshold,
            pause_duration_secs,
            failure_expiration_secs,
        };

        // Randomize the starting index to avoid always starting with the same provider
        let mut rng = rand::rng();
        selector.next_index.store(
            rng.random_range(0..selector.configs.read().len()),
            Ordering::Relaxed,
        );

        Ok(selector)
    }

    /// Creates a new RpcSelector instance with default failure threshold and pause duration.
    ///
    /// This is a convenience method primarily for testing. In production code, use `new()` with
    /// values from `ServerConfig::from_env()`.
    ///
    /// # Arguments
    /// * `configs` - RPC configurations
    ///
    /// # Returns
    /// * `Result<Self>` - A new selector instance or an error
    pub fn new_with_defaults(configs: Vec<RpcConfig>) -> Result<Self, RpcSelectorError> {
        Self::new(
            configs,
            crate::config::ServerConfig::get_provider_failure_threshold(),
            crate::config::ServerConfig::get_provider_pause_duration_secs(),
            crate::config::ServerConfig::get_provider_failure_expiration_secs(),
        )
    }

    /// Gets the number of available providers
    ///
    /// # Returns
    /// * `usize` - The number of providers in the selector
    pub fn provider_count(&self) -> usize {
        self.configs.read().len()
    }

    /// Gets the number of available (non-paused) providers
    ///
    /// # Returns
    /// * `usize` - The number of non-paused providers
    pub fn available_provider_count(&self) -> usize {
        let health_store = RpcHealthStore::instance();
        let expiration = chrono::Duration::seconds(self.failure_expiration_secs as i64);
        self.configs
            .read()
            .iter()
            .filter(|c| !health_store.is_paused(&c.url, self.failure_threshold, expiration))
            .count()
    }

    /// Gets the current RPC configurations.
    ///
    /// # Returns
    /// * `Vec<RpcConfig>` - The current configurations
    pub fn get_configs(&self) -> Vec<RpcConfig> {
        self.configs.read().clone()
    }

    /// Marks the current endpoint as failed and forces selection of a different endpoint.
    ///
    /// This method is used when a provider consistently fails, and we want to try a different one.
    /// It adds the current provider to the failed providers set and will avoid selecting it again.
    pub fn mark_current_as_failed(&self) {
        info!("Marking current provider as failed");
        // Only proceed if we have a current provider
        if self.has_current.load(Ordering::Relaxed) {
            let current = self.current_index.load(Ordering::Relaxed);
            let configs = self.configs.read();
            let config = &configs[current];

            // Mark this provider as failed in the health store
            let health_store = RpcHealthStore::instance();
            use chrono::Duration;
            health_store.mark_failed(
                &config.url,
                self.failure_threshold,
                Duration::seconds(self.pause_duration_secs as i64),
                Duration::seconds(self.failure_expiration_secs as i64),
            );

            // Clear the current provider
            self.has_current.store(false, Ordering::Relaxed);

            // Move round-robin index forward to avoid selecting the same provider again
            if configs.len() > 1 {
                self.next_index.fetch_add(1, Ordering::Relaxed);
            }
        }
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

    /// Attempts weighted selection of a provider.
    ///
    /// # Arguments
    /// * `configs` - RPC configurations
    /// * `excluded_urls` - URLs of providers that have already been tried
    /// * `allow_paused` - If true, allows selection of paused providers
    /// * `health_store` - Health store instance
    /// * `expiration` - Duration after which failures expire
    ///
    /// # Returns
    /// * `Option<(usize, String)>` - Some(index, url) if a provider was selected, None otherwise
    fn try_weighted_selection(
        &self,
        configs: &[RpcConfig],
        excluded_urls: &std::collections::HashSet<String>,
        allow_paused: bool,
        health_store: &RpcHealthStore,
        expiration: chrono::Duration,
    ) -> Option<(usize, String)> {
        let dist = self.weights_dist.as_ref()?;
        let mut rng = rand::rng();

        const MAX_ATTEMPTS: usize = 10;
        for _ in 0..MAX_ATTEMPTS {
            let index = dist.sample(&mut rng);
            // Skip providers with zero weight
            if configs[index].get_weight() == 0 {
                continue;
            }
            // Skip providers already tried in this failover cycle
            if excluded_urls.contains(&configs[index].url) {
                continue;
            }
            // Check health status unless paused providers are allowed
            if !allow_paused
                && health_store.is_paused(&configs[index].url, self.failure_threshold, expiration)
            {
                continue;
            }
            // Found a suitable provider
            self.current_index.store(index, Ordering::Relaxed);
            self.has_current.store(true, Ordering::Relaxed);
            return Some((index, configs[index].url.clone()));
        }
        None
    }

    /// Attempts round-robin selection of a provider.
    ///
    /// # Arguments
    /// * `configs` - RPC configurations
    /// * `excluded_urls` - URLs of providers that have already been tried
    /// * `allow_paused` - If true, allows selection of paused providers
    /// * `health_store` - Health store instance
    /// * `expiration` - Duration after which failures expire
    /// * `start_index` - Starting index for round-robin iteration
    ///
    /// # Returns
    /// * `Option<(usize, String)>` - Some(index, url) if a provider was selected, None otherwise
    fn try_round_robin_selection(
        &self,
        configs: &[RpcConfig],
        excluded_urls: &std::collections::HashSet<String>,
        allow_paused: bool,
        health_store: &RpcHealthStore,
        expiration: chrono::Duration,
        start_index: usize,
    ) -> Option<(usize, String)> {
        let len = configs.len();
        for i in 0..len {
            let index = (start_index + i) % len;
            // Skip providers with zero weight
            if configs[index].get_weight() == 0 {
                continue;
            }
            // Skip providers already tried in this failover cycle
            if excluded_urls.contains(&configs[index].url) {
                continue;
            }
            // Check health status unless paused providers are allowed
            if !allow_paused
                && health_store.is_paused(&configs[index].url, self.failure_threshold, expiration)
            {
                continue;
            }
            // Found a suitable provider
            // Update the next_index atomically to point after this provider
            self.next_index.store((index + 1) % len, Ordering::Relaxed);
            self.current_index.store(index, Ordering::Relaxed);
            self.has_current.store(true, Ordering::Relaxed);
            return Some((index, configs[index].url.clone()));
        }
        None
    }

    /// Gets the URL of the next RPC endpoint based on the selection strategy.
    ///
    /// This method first tries to select non-paused providers. If no non-paused providers
    /// are available, it falls back to paused providers as a last resort, since they might
    /// have recovered.
    ///
    /// # Arguments
    /// * `excluded_urls` - URLs of providers that have already been tried in the current failover cycle
    fn select_url_internal(
        &self,
        excluded_urls: &std::collections::HashSet<String>,
    ) -> Result<String, RpcSelectorError> {
        let configs = self.configs.read();
        if configs.is_empty() {
            return Err(RpcSelectorError::NoProviders);
        }

        let health_store = RpcHealthStore::instance();
        let expiration = chrono::Duration::seconds(self.failure_expiration_secs as i64);

        // For a single provider, handle special case
        if configs.len() == 1 {
            // Skip providers with zero weight
            if configs[0].get_weight() == 0 {
                return Err(RpcSelectorError::AllProvidersFailed);
            }
            // Skip if already tried
            if excluded_urls.contains(&configs[0].url) {
                return Err(RpcSelectorError::AllProvidersFailed);
            }
            // Even if paused, try it as last resort
            self.current_index.store(0, Ordering::Relaxed);
            self.has_current.store(true, Ordering::Relaxed);
            return Ok(configs[0].url.clone());
        }

        // First, try to find a non-paused provider
        // Try weighted selection first if available
        if let Some((_, url)) = self.try_weighted_selection(
            &configs,
            excluded_urls,
            false, // allow_paused = false
            &health_store,
            expiration,
        ) {
            return Ok(url);
        }

        // Fall back to round-robin selection for non-paused providers
        let start_index = self.next_index.load(Ordering::Relaxed) % configs.len();
        if let Some((_, url)) = self.try_round_robin_selection(
            &configs,
            excluded_urls,
            false, // allow_paused = false
            &health_store,
            expiration,
            start_index,
        ) {
            return Ok(url);
        }

        // If we get here, no non-paused providers are available
        // Fall back to paused providers as a last resort
        tracing::warn!(
            "No non-paused providers available, falling back to paused providers as last resort"
        );

        // Try weighted selection for paused providers
        if let Some((_, url)) = self.try_weighted_selection(
            &configs,
            excluded_urls,
            true, // allow_paused = true
            &health_store,
            expiration,
        ) {
            return Ok(url);
        }

        // Fall back to round-robin for paused providers
        if let Some((_, url)) = self.try_round_robin_selection(
            &configs,
            excluded_urls,
            true, // allow_paused = true
            &health_store,
            expiration,
            start_index,
        ) {
            return Ok(url);
        }

        // If we get here, all providers have zero weight (shouldn't happen in practice)
        Err(RpcSelectorError::AllProvidersFailed)
    }

    /// Gets the URL of the currently selected RPC endpoint.
    ///
    /// # Returns
    /// * `Result<String, RpcSelectorError>` - The URL of the current provider, or an error
    pub fn get_current_url(&self) -> Result<String, RpcSelectorError> {
        self.select_url_internal(&std::collections::HashSet::new())
    }

    /// Gets the URL of the next RPC endpoint, excluding providers that have already been tried.
    ///
    /// # Arguments
    /// * `excluded_urls` - URLs of providers that have already been tried in the current failover cycle
    ///
    /// # Returns
    /// * `Result<String, RpcSelectorError>` - The URL of the next provider, or an error
    pub fn get_next_url(
        &self,
        excluded_urls: &std::collections::HashSet<String>,
    ) -> Result<String, RpcSelectorError> {
        self.select_url_internal(excluded_urls)
    }

    /// Gets the URL of the next RPC endpoint (for backward compatibility in tests).
    /// This method doesn't exclude any providers - use `get_next_url()` with excluded URLs in production code.
    #[cfg(test)]
    pub fn select_url(&self) -> Result<String, RpcSelectorError> {
        self.select_url_internal(&std::collections::HashSet::new())
    }

    /// Gets a client for the selected RPC endpoint.
    ///
    /// # Arguments
    /// * `initializer` - A function that takes a URL string and returns a `Result<T>`
    /// * `excluded_urls` - URLs of providers that have already been tried in the current failover cycle
    ///
    /// # Returns
    /// * `Result<T>` - The client instance or an error
    pub fn get_client<T>(
        &self,
        initializer: impl Fn(&str) -> Result<T>,
        excluded_urls: &std::collections::HashSet<String>,
    ) -> Result<T, RpcSelectorError> {
        let url = self.select_url_internal(excluded_urls)?;

        initializer(&url).map_err(|e| RpcSelectorError::ClientInitializationError(e.to_string()))
    }
}

// Implement Clone for RpcSelector manually since the generic T doesn't require Clone
impl Clone for RpcSelector {
    fn clone(&self) -> Self {
        Self {
            configs: Arc::new(RwLock::new(self.configs.read().clone())),
            weights_dist: self.weights_dist.clone(),
            next_index: Arc::clone(&self.next_index),
            current_index: Arc::clone(&self.current_index),
            has_current: Arc::clone(&self.has_current),
            failure_threshold: self.failure_threshold,
            pause_duration_secs: self.pause_duration_secs,
            failure_expiration_secs: self.failure_expiration_secs,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::provider::rpc_health_store::RpcHealthStore;
    use serial_test::serial;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn test_create_weights_distribution_single_config() {
        let configs = vec![RpcConfig {
            url: "https://example.com/rpc".to_string(),
            weight: 1,
            ..Default::default()
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
                ..Default::default()
            },
            RpcConfig {
                url: "https://example2.com/rpc".to_string(),
                weight: 5,
                ..Default::default()
            },
            RpcConfig {
                url: "https://example3.com/rpc".to_string(),
                weight: 5,
                ..Default::default()
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
                ..Default::default()
            },
            RpcConfig {
                url: "https://example2.com/rpc".to_string(),
                weight: 2,
                ..Default::default()
            },
            RpcConfig {
                url: "https://example3.com/rpc".to_string(),
                weight: 3,
                ..Default::default()
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
                ..Default::default()
            },
            RpcConfig {
                url: "https://example2.com/rpc".to_string(),
                weight: 2,
                ..Default::default()
            },
            RpcConfig {
                url: "https://example3.com/rpc".to_string(),
                weight: 3,
                ..Default::default()
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
        let result = RpcSelector::new_with_defaults(configs);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), RpcSelectorError::NoProviders));
    }

    #[test]
    fn test_rpc_selector_new_single_config() {
        let configs = vec![RpcConfig {
            url: "https://example.com/rpc".to_string(),
            weight: 1,
            ..Default::default()
        }];

        let result = RpcSelector::new_with_defaults(configs);
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
                ..Default::default()
            },
            RpcConfig {
                url: "https://example2.com/rpc".to_string(),
                weight: 5,
                ..Default::default()
            },
        ];

        let result = RpcSelector::new_with_defaults(configs);
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
                ..Default::default()
            },
            RpcConfig {
                url: "https://example2.com/rpc".to_string(),
                weight: 3,
                ..Default::default()
            },
        ];

        let result = RpcSelector::new_with_defaults(configs);
        assert!(result.is_ok());
        let selector = result.unwrap();
        assert!(selector.weights_dist.is_some());
    }

    #[test]
    fn test_rpc_selector_select_url_single_provider() {
        let configs = vec![RpcConfig {
            url: "https://example.com/rpc".to_string(),
            weight: 1,
            ..Default::default()
        }];

        let selector = RpcSelector::new_with_defaults(configs).unwrap();
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
                ..Default::default()
            },
            RpcConfig {
                url: "https://example2.com/rpc".to_string(),
                weight: 1,
                ..Default::default()
            },
        ];

        let selector = RpcSelector::new_with_defaults(configs).unwrap();

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
            ..Default::default()
        }];

        let selector = RpcSelector::new_with_defaults(configs).unwrap();

        // Create a simple initializer function that returns the URL as a string
        let initializer = |url: &str| -> Result<String> { Ok(url.to_string()) };

        let result = selector.get_client(initializer, &std::collections::HashSet::new());
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "https://example.com/rpc");
    }

    #[test]
    fn test_rpc_selector_get_client_failure() {
        let configs = vec![RpcConfig {
            url: "https://example.com/rpc".to_string(),
            weight: 1,
            ..Default::default()
        }];

        let selector = RpcSelector::new_with_defaults(configs).unwrap();

        // Create a failing initializer function
        let initializer =
            |_url: &str| -> Result<String> { Err(eyre::eyre!("Initialization error")) };

        let result = selector.get_client(initializer, &std::collections::HashSet::new());
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
                ..Default::default()
            },
            RpcConfig {
                url: "https://example2.com/rpc".to_string(),
                weight: 3,
                ..Default::default()
            },
        ];

        let selector = RpcSelector::new_with_defaults(configs).unwrap();
        let cloned = selector.clone();

        // Check that the cloned selector has the same configuration
        assert_eq!(selector.configs.read().len(), cloned.configs.read().len());
        assert_eq!(selector.configs.read()[0].url, cloned.configs.read()[0].url);
        assert_eq!(selector.configs.read()[1].url, cloned.configs.read()[1].url);

        // Check that weights distribution is also cloned
        assert_eq!(
            selector.weights_dist.is_some(),
            cloned.weights_dist.is_some()
        );
    }

    #[test]
    #[serial]
    fn test_mark_current_as_failed_single_provider() {
        // Clear health store to ensure clean state
        RpcHealthStore::instance().clear_all();

        // With a single provider, marking as failed multiple times (to reach threshold) will pause it,
        // but it can still be selected as a last resort
        let configs = vec![RpcConfig {
            url: "https://test-single-provider.example.com/rpc".to_string(),
            weight: 1,
            ..Default::default()
        }];

        // Create selector with threshold=1 for testing
        let selector = RpcSelector::new(configs, 1, 60, 60).unwrap();
        let _initial_url = selector.select_url().unwrap();

        // Mark as failed once (with threshold=1, this will pause it)
        selector.mark_current_as_failed();

        // With threshold=1, available_provider_count should be 0
        assert_eq!(selector.available_provider_count(), 0);

        // But select_url should still work (selecting paused provider as last resort)
        let next_url = selector.select_url();
        assert!(next_url.is_ok());
        assert_eq!(
            next_url.unwrap(),
            "https://test-single-provider.example.com/rpc"
        );
    }

    #[test]
    #[serial]
    fn test_mark_current_as_failed_multiple_providers() {
        // Clear health store to ensure clean state
        RpcHealthStore::instance().clear_all();

        // With multiple providers, marking as failed (with threshold=1) will pause them,
        // but they can still be selected as a last resort
        let configs = vec![
            RpcConfig {
                url: "https://test-multi1.example.com/rpc".to_string(),
                weight: 5,
                ..Default::default()
            },
            RpcConfig {
                url: "https://test-multi2.example.com/rpc".to_string(),
                weight: 5,
                ..Default::default()
            },
            RpcConfig {
                url: "https://test-multi3.example.com/rpc".to_string(),
                weight: 5,
                ..Default::default()
            },
        ];

        // Create selector with threshold=1 for testing
        let selector = RpcSelector::new(configs, 1, 60, 60).unwrap();

        // Get the first URL
        let url1 = selector.select_url().unwrap().to_string();

        // Mark as failed (with threshold=1, this pauses it)
        selector.mark_current_as_failed();
        // Available count should decrease
        assert_eq!(selector.available_provider_count(), 2);

        // Next selection should prefer non-paused providers
        let url2 = selector.select_url().unwrap().to_string();
        // Should be different from the paused one
        assert_ne!(url1, url2);

        // Mark the second URL as failed too
        selector.mark_current_as_failed();
        assert_eq!(selector.available_provider_count(), 1);

        let url3 = selector.select_url().unwrap().to_string();
        // Should get the third URL (non-paused)
        assert_ne!(url1, url3);
        assert_ne!(url2, url3);

        // Mark the third URL as failed too
        selector.mark_current_as_failed();
        assert_eq!(selector.available_provider_count(), 0);

        // Now all URLs are paused, but select_url should still work (selecting paused providers as last resort)
        let url4 = selector.select_url();
        assert!(url4.is_ok());
        // Should return one of the paused providers
        let url4_str = url4.unwrap();
        assert!(
            url4_str == "https://test-multi1.example.com/rpc"
                || url4_str == "https://test-multi2.example.com/rpc"
                || url4_str == "https://test-multi3.example.com/rpc"
        );
    }

    #[test]
    #[serial]
    fn test_mark_current_as_failed_weighted() {
        // Clear health store to ensure clean state
        RpcHealthStore::instance().clear_all();

        // Test with weighted selection
        let configs = vec![
            RpcConfig {
                url: "https://test-weighted1.example.com/rpc".to_string(),
                weight: 1, // Low weight
                ..Default::default()
            },
            RpcConfig {
                url: "https://test-weighted2.example.com/rpc".to_string(),
                weight: 10, // High weight
                ..Default::default()
            },
        ];

        // Create selector with threshold=1 for testing
        let selector = RpcSelector::new(configs, 1, 60, 60).unwrap();
        assert!(selector.weights_dist.is_some()); // Confirm we're using weighted selection

        // Get a URL
        let url1 = selector.select_url().unwrap().to_string();

        // Mark it as failed (with threshold=1, this pauses it)
        selector.mark_current_as_failed();
        assert_eq!(selector.available_provider_count(), 1);

        // Get another URL, it should prefer the non-paused one
        let url2 = selector.select_url().unwrap().to_string();
        assert_ne!(url1, url2);

        // Mark this one as failed too
        selector.mark_current_as_failed();
        assert_eq!(selector.available_provider_count(), 0);

        // With all providers paused, select_url should still work (selecting paused providers as last resort)
        let url3 = selector.select_url();
        assert!(url3.is_ok());
        let url3_str = url3.unwrap();
        assert!(
            url3_str == "https://test-weighted1.example.com/rpc"
                || url3_str == "https://test-weighted2.example.com/rpc"
        );
    }

    #[test]
    fn test_provider_count() {
        // Test with no providers
        let configs: Vec<RpcConfig> = vec![];
        let result = RpcSelector::new_with_defaults(configs);
        assert!(result.is_err());

        // Test with a single provider
        let configs = vec![RpcConfig {
            url: "https://example.com/rpc".to_string(),
            weight: 1,
            ..Default::default()
        }];
        let selector = RpcSelector::new_with_defaults(configs).unwrap();
        assert_eq!(selector.provider_count(), 1);

        // Test with multiple providers
        let configs = vec![
            RpcConfig {
                url: "https://example1.com/rpc".to_string(),
                weight: 1,
                ..Default::default()
            },
            RpcConfig {
                url: "https://example2.com/rpc".to_string(),
                weight: 2,
                ..Default::default()
            },
            RpcConfig {
                url: "https://example3.com/rpc".to_string(),
                weight: 3,
                ..Default::default()
            },
        ];
        let selector = RpcSelector::new_with_defaults(configs).unwrap();
        assert_eq!(selector.provider_count(), 3);
    }

    #[test]
    #[serial]
    fn test_available_provider_count() {
        // Clear health store to ensure clean state
        RpcHealthStore::instance().clear_all();

        let configs = vec![
            RpcConfig {
                url: "https://test-available1.example.com/rpc".to_string(),
                weight: 1,
                ..Default::default()
            },
            RpcConfig {
                url: "https://test-available2.example.com/rpc".to_string(),
                weight: 2,
                ..Default::default()
            },
            RpcConfig {
                url: "https://test-available3.example.com/rpc".to_string(),
                weight: 3,
                ..Default::default()
            },
        ];

        // Create selector with threshold=1 for testing
        let selector = RpcSelector::new(configs, 1, 60, 60).unwrap();
        assert_eq!(selector.provider_count(), 3);
        assert_eq!(selector.available_provider_count(), 3);

        // Mark one provider as failed (with threshold=1, this pauses it)
        selector.select_url().unwrap(); // Select a provider first
        selector.mark_current_as_failed();
        // Available count should decrease (only non-paused providers)
        assert_eq!(selector.available_provider_count(), 2);

        // Mark another provider as failed
        selector.select_url().unwrap(); // Select another provider
        selector.mark_current_as_failed();
        assert_eq!(selector.available_provider_count(), 1);
    }

    #[test]
    fn test_get_current_url() {
        let configs = vec![
            RpcConfig::new("https://example1.com/rpc".to_string()),
            RpcConfig::new("https://example2.com/rpc".to_string()),
        ];

        let selector = RpcSelector::new_with_defaults(configs).unwrap();

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
    #[serial]
    fn test_concurrent_usage() {
        // Clear health store to ensure clean state
        RpcHealthStore::instance().clear_all();

        // Test RpcSelector with concurrent access from multiple threads
        let configs = vec![
            RpcConfig::new("https://test-concurrent1.example.com/rpc".to_string()),
            RpcConfig::new("https://test-concurrent2.example.com/rpc".to_string()),
            RpcConfig::new("https://test-concurrent3.example.com/rpc".to_string()),
        ];

        // Create selector with threshold=1 for testing
        let selector = RpcSelector::new(configs, 1, 60, 60).unwrap();
        let selector_arc = Arc::new(selector);

        let mut handles = Vec::with_capacity(10);

        // Launch 10 threads that select and mark providers
        for _ in 0..10 {
            let selector_clone = Arc::clone(&selector_arc);
            let handle = thread::spawn(move || {
                let url = selector_clone.select_url().unwrap().to_string();
                if url.contains("test-concurrent1") {
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

        // After all threads, example1 should be marked as failed (paused)
        // Selections should prefer non-paused providers
        let mut found_non_example1 = false;
        for _ in 0..10 {
            let url = selector_arc.select_url().unwrap().to_string();
            if !url.contains("test-concurrent1") {
                found_non_example1 = true;
            }
        }

        assert!(found_non_example1, "Should prefer non-paused providers");
    }

    #[test]
    fn test_consecutive_mark_as_failed() {
        let configs = vec![
            RpcConfig::new("https://example1.com/rpc".to_string()),
            RpcConfig::new("https://example2.com/rpc".to_string()),
        ];

        let selector = RpcSelector::new_with_defaults(configs).unwrap();

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
    #[serial]
    fn test_weighted_to_round_robin_fallback() {
        // Clear health store to ensure clean state
        RpcHealthStore::instance().clear_all();

        let configs = vec![
            RpcConfig {
                url: "https://test-wrr1.example.com/rpc".to_string(),
                weight: 10, // High weight
                ..Default::default()
            },
            RpcConfig {
                url: "https://test-wrr2.example.com/rpc".to_string(),
                weight: 1, // Low weight
                ..Default::default()
            },
            RpcConfig {
                url: "https://test-wrr3.example.com/rpc".to_string(),
                weight: 1, // Low weight
                ..Default::default()
            },
        ];

        // Create selector with threshold=1 for testing
        let selector = RpcSelector::new(configs, 1, 60, 60).unwrap();
        assert!(selector.weights_dist.is_some()); // Using weighted selection

        // Mock a situation where weighted selection would fail multiple times
        // by marking the high-weight provider as failed
        let mut selected_first = false;

        // Try multiple times - the first provider should be selected more often due to weight
        for _ in 0..10 {
            let url = selector.select_url().unwrap();
            if url.contains("test-wrr1") {
                selected_first = true;
                // Mark the high-weight provider as failed (pauses it)
                selector.mark_current_as_failed();
                break;
            }
        }

        assert!(
            selected_first,
            "High-weight provider should have been selected"
        );

        // After marking it failed (paused), selections should prefer the other providers (non-paused)
        let mut seen_urls = HashSet::new();
        for _ in 0..10 {
            let url = selector.select_url().unwrap().to_string();
            seen_urls.insert(url);
        }

        // Should have seen at least example2 and example3 (non-paused providers)
        assert!(seen_urls.len() >= 2);
        assert!(
            !seen_urls.iter().any(|url| url.contains("test-wrr1")),
            "Paused provider should not be selected (prefer non-paused)"
        );
    }

    #[test]
    fn test_zero_weight_providers() {
        let configs = vec![
            RpcConfig {
                url: "https://example1.com/rpc".to_string(),
                weight: 0, // Zero weight
                ..Default::default()
            },
            RpcConfig {
                url: "https://example2.com/rpc".to_string(),
                weight: 5, // Normal weight
                ..Default::default()
            },
        ];

        let selector = RpcSelector::new_with_defaults(configs).unwrap();

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
    #[serial]
    fn test_extreme_weight_differences() {
        let configs = vec![
            RpcConfig {
                url: "https://example1.com/rpc".to_string(),
                weight: 100, // Very high weight
                ..Default::default()
            },
            RpcConfig {
                url: "https://example2.com/rpc".to_string(),
                weight: 1, // Very low weight
                ..Default::default()
            },
        ];

        let selector = RpcSelector::new_with_defaults(configs).unwrap();

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

        let selector = RpcSelector::new_with_defaults(configs).unwrap();

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

    #[cfg(test)]
    mod rate_limiting_tests {
        use super::*;
        use crate::services::provider::rpc_health_store::RpcHealthStore;

        /// Test that RpcSelector switches to the second RPC when the first one is rate-limited.
        ///
        /// This test simulates a scenario where:
        /// 1. Two RPC configs are set up with equal weights
        /// 2. The first RPC starts returning rate limit errors (429)
        /// 3. The selector should switch to the second RPC
        /// 4. The first RPC should be marked as failed and excluded from selection
        #[test]
        #[serial]
        fn test_rpc_selector_switches_on_rate_limit() {
            RpcHealthStore::instance().clear_all();
            let configs = vec![
                RpcConfig {
                    url: "https://test-rate-limit1.example.com".to_string(),
                    weight: 100,
                    ..Default::default()
                },
                RpcConfig {
                    url: "https://test-rate-limit2.example.com".to_string(),
                    weight: 100,
                    ..Default::default()
                },
            ];

            // Create selector with threshold=1 for testing
            let selector = RpcSelector::new(configs, 1, 60, 60).unwrap();

            // Initially, both providers should be available
            assert_eq!(selector.available_provider_count(), 2);

            // Select the first provider
            let first_url = selector.select_url().unwrap();

            // Verify we got a valid URL
            assert!(
                first_url == "https://test-rate-limit1.example.com"
                    || first_url == "https://test-rate-limit2.example.com"
            );

            // Simulate rate limiting: mark the current provider as failed
            // This simulates what happens when a provider returns HTTP 429 after retries are exhausted
            selector.mark_current_as_failed();

            // Now only one provider should be available (non-paused)
            assert_eq!(selector.available_provider_count(), 1);

            // The next selection should prefer the non-paused provider
            let second_url = selector.select_url().unwrap();

            // Verify we got a different URL (the non-paused one)
            assert_ne!(first_url, second_url);

            // Verify the failed provider is not selected again (prefer non-paused)
            let third_url = selector.select_url().unwrap();
            assert_eq!(second_url, third_url); // Should keep using the working provider

            // Verify the failed provider is excluded from preferred selection
            let mut selected_urls = std::collections::HashSet::new();
            for _ in 0..10 {
                let url = selector.select_url().unwrap();
                selected_urls.insert(url.to_string());
            }

            // Should only select from the non-failed provider (preferred)
            assert_eq!(selected_urls.len(), 1);
            assert!(!selected_urls.contains(&first_url.to_string()));
            assert!(selected_urls.contains(&second_url.to_string()));
        }

        /// Test that RpcSelector handles rate limiting with weighted selection.
        ///
        /// This test verifies that even with weighted selection, a rate-limited provider
        /// is excluded and the selector falls back to the other provider.
        #[test]
        #[serial]
        fn test_rpc_selector_rate_limit_with_weighted_selection() {
            RpcHealthStore::instance().clear_all();
            let configs = vec![
                RpcConfig {
                    url: "https://test-weighted-rl1.example.com".to_string(),
                    weight: 80, // Higher weight, should be preferred
                    ..Default::default()
                },
                RpcConfig {
                    url: "https://test-weighted-rl2.example.com".to_string(),
                    weight: 20, // Lower weight
                    ..Default::default()
                },
            ];

            // Create selector with threshold=1 for testing
            let selector = RpcSelector::new(configs, 1, 60, 60).unwrap();

            // Select multiple times - with weighted selection, rpc1 should be selected more often
            let mut rpc1_count = 0;
            let mut rpc2_count = 0;

            for _ in 0..20 {
                let url = selector.select_url().unwrap();
                if url == "https://test-weighted-rl1.example.com" {
                    rpc1_count += 1;
                } else {
                    rpc2_count += 1;
                }
            }

            // With weighted selection, rpc1 should be selected more often
            assert!(rpc1_count > rpc2_count);

            // Now simulate rate limiting on rpc1
            // First, select rpc1
            let mut selected_rpc1 = false;
            for _ in 0..10 {
                let url = selector.select_url().unwrap();
                if url == "https://test-weighted-rl1.example.com" {
                    selector.mark_current_as_failed();
                    selected_rpc1 = true;
                    break;
                }
            }
            assert!(selected_rpc1, "Should have selected rpc1 at least once");

            // After marking rpc1 as failed (paused), selections should prefer rpc2 (non-paused)
            for _ in 0..20 {
                let url = selector.select_url().unwrap();
                assert_eq!(url, "https://test-weighted-rl2.example.com");
            }

            // Verify only one provider is available (non-paused)
            assert_eq!(selector.available_provider_count(), 1);
        }

        /// Test that a rate-limited provider stays failed.
        ///
        /// This test verifies that failed providers remain failed,
        /// which simulates persistence of health state.
        #[test]
        #[serial]
        fn test_rpc_selector_rate_limit_recovery() {
            RpcHealthStore::instance().clear_all();
            let configs = vec![
                RpcConfig {
                    url: "https://test-recovery1.example.com".to_string(),
                    weight: 100,
                    ..Default::default()
                },
                RpcConfig {
                    url: "https://test-recovery2.example.com".to_string(),
                    weight: 100,
                    ..Default::default()
                },
            ];

            // Create selector with threshold=1 for testing
            let selector = RpcSelector::new(configs, 1, 60, 60).unwrap();

            // Select first provider
            let first_url = selector.select_url().unwrap();

            // Mark it as failed (simulating rate limit, with threshold=1 this pauses it)
            selector.mark_current_as_failed();
            assert_eq!(selector.available_provider_count(), 1);

            // Next selection should prefer the other provider (non-paused)
            let second_url = selector.select_url().unwrap();
            assert_ne!(first_url, second_url);

            // Verify only the working provider is selected (prefer non-paused)
            for _ in 0..10 {
                let url = selector.select_url().unwrap();
                assert_eq!(url, second_url);
            }

            // Since we persist health, the failed provider stays failed (paused)
            assert_eq!(selector.available_provider_count(), 1);
        }

        /// Test that when both providers are rate-limited, the selector handles it gracefully.
        #[test]
        #[serial]
        fn test_rpc_selector_both_providers_rate_limited() {
            RpcHealthStore::instance().clear_all();
            let configs = vec![
                RpcConfig {
                    url: "https://test-both-rl1.example.com".to_string(),
                    weight: 100,
                    ..Default::default()
                },
                RpcConfig {
                    url: "https://test-both-rl2.example.com".to_string(),
                    weight: 100,
                    ..Default::default()
                },
            ];

            // Create selector with threshold=1 for testing
            let selector = RpcSelector::new(configs, 1, 60, 60).unwrap();

            // Select and mark first provider as failed (pauses it)
            selector.select_url().unwrap();
            selector.mark_current_as_failed();
            assert_eq!(selector.available_provider_count(), 1);

            // Select and mark second provider as failed (pauses it)
            selector.select_url().unwrap();
            selector.mark_current_as_failed();
            assert_eq!(selector.available_provider_count(), 0);

            // Now all providers are paused, but select_url should still work (selecting paused providers as last resort)
            let result = selector.select_url();
            assert!(result.is_ok());
            let url = result.unwrap();
            assert!(
                url == "https://test-both-rl1.example.com"
                    || url == "https://test-both-rl2.example.com"
            );
        }

        /// Test that rate limiting works correctly with round-robin fallback.
        ///
        /// This test verifies that when weighted selection fails due to rate limiting,
        /// the selector correctly falls back to round-robin selection.
        #[test]
        #[serial]
        fn test_rpc_selector_rate_limit_round_robin_fallback() {
            RpcHealthStore::instance().clear_all();
            let configs = vec![
                RpcConfig {
                    url: "https://test-rr-fallback1.example.com".to_string(),
                    weight: 100,
                    ..Default::default()
                },
                RpcConfig {
                    url: "https://test-rr-fallback2.example.com".to_string(),
                    weight: 100,
                    ..Default::default()
                },
                RpcConfig {
                    url: "https://test-rr-fallback3.example.com".to_string(),
                    weight: 100,
                    ..Default::default()
                },
            ];

            // Create selector with threshold=1 for testing
            let selector = RpcSelector::new(configs, 1, 60, 60).unwrap();

            // Mark rpc1 as failed (simulating rate limit)
            selector.select_url().unwrap();
            let first_url = selector.get_current_url().unwrap();

            // If we got rpc1, mark it as failed
            if first_url == "https://test-rr-fallback1.example.com" {
                selector.mark_current_as_failed();
            } else {
                // Otherwise, select until we get rpc1, then mark it as failed
                loop {
                    let url = selector.select_url().unwrap();
                    if url == "https://test-rr-fallback1.example.com" {
                        selector.mark_current_as_failed();
                        break;
                    }
                }
            }

            // Now rpc1 should be paused, and selections should prefer rpc2 and rpc3 (non-paused)
            let mut selected_urls = std::collections::HashSet::new();
            for _ in 0..20 {
                let url = selector.select_url().unwrap();
                selected_urls.insert(url.to_string());
                // rpc1 should not be selected (prefer non-paused)
                assert_ne!(url, "https://test-rr-fallback1.example.com");
            }

            // Should have selected from both rpc2 and rpc3
            assert!(selected_urls.contains("https://test-rr-fallback2.example.com"));
            assert!(selected_urls.contains("https://test-rr-fallback3.example.com"));
            assert_eq!(selected_urls.len(), 2);
        }

        #[test]
        #[serial]
        fn test_select_url_excludes_tried_providers() {
            RpcHealthStore::instance().clear_all();
            let configs = vec![
                RpcConfig {
                    url: "https://provider1.com".to_string(),
                    weight: 1,
                    ..Default::default()
                },
                RpcConfig {
                    url: "https://provider2.com".to_string(),
                    weight: 1,
                    ..Default::default()
                },
                RpcConfig {
                    url: "https://provider3.com".to_string(),
                    weight: 1,
                    ..Default::default()
                },
            ];

            let selector = RpcSelector::new_with_defaults(configs).unwrap();

            // Exclude provider1
            let mut excluded = std::collections::HashSet::new();
            excluded.insert("https://provider1.com".to_string());

            // Should select provider2 or provider3, not provider1
            for _ in 0..10 {
                let url = selector.get_next_url(&excluded).unwrap();
                assert_ne!(url, "https://provider1.com");
            }
        }

        #[test]
        #[serial]
        fn test_select_url_fallback_to_paused_providers() {
            RpcHealthStore::instance().clear_all();
            let configs = vec![
                RpcConfig {
                    url: "https://provider1.com".to_string(),
                    weight: 1,
                    ..Default::default()
                },
                RpcConfig {
                    url: "https://provider2.com".to_string(),
                    weight: 1,
                    ..Default::default()
                },
            ];

            let selector = RpcSelector::new_with_defaults(configs).unwrap();
            let health_store = RpcHealthStore::instance();
            let expiration = chrono::Duration::seconds(60);

            // Pause both providers
            health_store.mark_failed(
                "https://provider1.com",
                3,
                chrono::Duration::seconds(60),
                expiration,
            );
            health_store.mark_failed(
                "https://provider1.com",
                3,
                chrono::Duration::seconds(60),
                expiration,
            );
            health_store.mark_failed(
                "https://provider1.com",
                3,
                chrono::Duration::seconds(60),
                expiration,
            );

            health_store.mark_failed(
                "https://provider2.com",
                3,
                chrono::Duration::seconds(60),
                expiration,
            );
            health_store.mark_failed(
                "https://provider2.com",
                3,
                chrono::Duration::seconds(60),
                expiration,
            );
            health_store.mark_failed(
                "https://provider2.com",
                3,
                chrono::Duration::seconds(60),
                expiration,
            );

            // Both should be paused
            assert!(health_store.is_paused("https://provider1.com", 3, expiration));
            assert!(health_store.is_paused("https://provider2.com", 3, expiration));

            // Should still be able to select (fallback to paused providers)
            let url = selector
                .get_next_url(&std::collections::HashSet::new())
                .unwrap();
            assert!(url == "https://provider1.com" || url == "https://provider2.com");
        }

        #[test]
        #[serial]
        fn test_select_url_single_provider_excluded() {
            RpcHealthStore::instance().clear_all();
            let configs = vec![RpcConfig {
                url: "https://single-provider.com".to_string(),
                weight: 1,
                ..Default::default()
            }];

            let selector = RpcSelector::new_with_defaults(configs).unwrap();

            // Exclude the only provider
            let mut excluded = std::collections::HashSet::new();
            excluded.insert("https://single-provider.com".to_string());

            // Should return error
            let result = selector.get_next_url(&excluded);
            assert!(result.is_err());
            assert!(matches!(
                result.unwrap_err(),
                RpcSelectorError::AllProvidersFailed
            ));
        }

        #[test]
        #[serial]
        fn test_select_url_all_providers_excluded() {
            RpcHealthStore::instance().clear_all();
            let configs = vec![
                RpcConfig {
                    url: "https://provider1.com".to_string(),
                    weight: 1,
                    ..Default::default()
                },
                RpcConfig {
                    url: "https://provider2.com".to_string(),
                    weight: 1,
                    ..Default::default()
                },
            ];

            let selector = RpcSelector::new_with_defaults(configs).unwrap();

            // Exclude all providers
            let mut excluded = std::collections::HashSet::new();
            excluded.insert("https://provider1.com".to_string());
            excluded.insert("https://provider2.com".to_string());

            // Should return error
            let result = selector.get_next_url(&excluded);
            assert!(result.is_err());
            assert!(matches!(
                result.unwrap_err(),
                RpcSelectorError::AllProvidersFailed
            ));
        }

        #[test]
        #[serial]
        fn test_select_url_excluded_providers_with_weighted_selection() {
            RpcHealthStore::instance().clear_all();
            let configs = vec![
                RpcConfig {
                    url: "https://provider1.com".to_string(),
                    weight: 10,
                    ..Default::default()
                },
                RpcConfig {
                    url: "https://provider2.com".to_string(),
                    weight: 1,
                    ..Default::default()
                },
                RpcConfig {
                    url: "https://provider3.com".to_string(),
                    weight: 1,
                    ..Default::default()
                },
            ];

            let selector = RpcSelector::new_with_defaults(configs).unwrap();

            // Exclude provider1 (highest weight)
            let mut excluded = std::collections::HashSet::new();
            excluded.insert("https://provider1.com".to_string());

            // Should select from provider2 or provider3, never provider1
            for _ in 0..20 {
                let url = selector.get_next_url(&excluded).unwrap();
                assert_ne!(url, "https://provider1.com");
            }
        }
    }
}
