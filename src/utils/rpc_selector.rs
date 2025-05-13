use std::sync::{atomic::AtomicUsize, Arc};

use eyre::Result;
use rand::distr::weighted::WeightedIndex;
use rand::prelude::*;
use serde::Serialize;
use thiserror::Error;

use crate::models::RpcConfig;

#[derive(Error, Debug, Serialize)]
pub enum RpcSelectorError {
    #[error("No providers available")]
    NoProviders,
    #[error("Client initialization failed: {0}")]
    ClientInitializationError(String),
}

/// Creates a weighted distribution for selecting RPC endpoints based on their weights.
///
/// # Arguments
/// * `configs` - A slice of RPC configurations with weights
///
/// # Returns
/// * `Option<Arc<WeightedIndex<u8>>>` - A weighted distribution if configs have different weights, None otherwise
pub fn create_weights_distribution(configs: &[RpcConfig]) -> Option<Arc<WeightedIndex<u8>>> {
    if configs.len() <= 1 {
        return None;
    }

    let weights: Vec<u8> = configs.iter().map(|config| config.get_weight()).collect();

    // Check if all weights are equal (in that case we'll use round-robin instead)
    if weights.iter().all(|&w| w == weights[0]) {
        None
    } else {
        match WeightedIndex::new(&weights) {
            Ok(dist) => Some(Arc::new(dist)),
            Err(_) => None,
        }
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
}

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

        // Use the common utility function to create the weighted distribution
        let weights_dist = create_weights_distribution(&configs);

        Ok(Self {
            configs,
            weights_dist,
            next_index: Arc::new(AtomicUsize::new(0)),
        })
    }

    /// Gets the URL of the next RPC endpoint based on the selection strategy.
    fn select_url(&self) -> Result<&str, RpcSelectorError> {
        if self.configs.is_empty() {
            return Err(RpcSelectorError::NoProviders);
        }

        // For a single provider, just return its URL
        if self.configs.len() == 1 {
            return Ok(&self.configs[0].url);
        }

        // Use weighted selection if available
        if let Some(dist) = &self.weights_dist {
            let mut rng = rand::rng();
            let index = dist.sample(&mut rng);
            return Ok(&self.configs[index].url);
        }

        // Fall back to round-robin for equal weights or if weighted distribution failed/not needed
        let len = self.configs.len();
        // Use Ordering::Relaxed as we only need atomicity, not synchronization
        let index = self
            .next_index
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            % len;
        Ok(&self.configs[index].url)
    }
}

// Special implementation for non-cloneable types (like RpcClient)
impl RpcSelector {
    /// Gets a client for the selected RPC endpoint.
    ///
    /// # Arguments
    /// * `initializer` - A function that takes a URL string and returns a Result<T>
    ///
    /// # Returns
    /// * `Result<T>` - The client instance or an error
    pub fn get_client<T>(
        &self,
        initializer: impl Fn(&str) -> Result<T>,
    ) -> Result<T, RpcSelectorError> {
        let url = self.select_url()?;

        // Always create a new client
        // TODO: This might be improved by caching the client
        initializer(url).map_err(|e| {
            RpcSelectorError::ClientInitializationError(format!(
                "Client initialization failed: {}",
                e
            ))
        })
    }
}

// Implement Clone for RpcSelector manually since the generic T doesn't require Clone
impl Clone for RpcSelector {
    fn clone(&self) -> Self {
        Self {
            configs: self.configs.clone(),
            weights_dist: self.weights_dist.clone(),
            next_index: self.next_index.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_weights_distribution_single_config() {
        let configs = vec![RpcConfig {
            url: "https://example.com/rpc".to_string(),
            weight: 1,
        }];

        let result = create_weights_distribution(&configs);
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

        let result = create_weights_distribution(&configs);
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

        let result = create_weights_distribution(&configs);
        assert!(result.is_some());
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
}
