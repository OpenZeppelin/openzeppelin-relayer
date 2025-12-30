//! RPC Health Store
//!
//! This module provides a shared in-memory store for RPC health metadata.
//! Health state is shared across all relayers using the same RPC URL.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use chrono::{DateTime, Utc};
use once_cell::sync::Lazy;
use tracing::debug;

/// Metadata for tracking RPC endpoint health.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RpcConfigMetadata {
    /// Timestamps of recent failures. Only failures within the expiration window are kept.
    /// Limited to a reasonable size (threshold * 2) to prevent unbounded growth.
    pub failure_timestamps: Vec<DateTime<Utc>>,
    /// Timestamp until which this RPC endpoint is paused due to failures.
    /// If set and in the future, the endpoint is considered paused.
    pub paused_until: Option<DateTime<Utc>>,
}

/// Shared in-memory store for RPC health metadata.
///
/// This store is shared across all relayers, so health state for a given RPC URL
/// is consistent across all relayers using that URL.
pub struct RpcHealthStore {
    metadata: Arc<RwLock<HashMap<String, RpcConfigMetadata>>>,
}

static HEALTH_STORE: Lazy<RpcHealthStore> = Lazy::new(|| RpcHealthStore {
    metadata: Arc::new(RwLock::new(HashMap::new())),
});

impl RpcHealthStore {
    /// Gets the singleton instance of the health store.
    pub fn instance() -> &'static RpcHealthStore {
        &HEALTH_STORE
    }

    /// Gets metadata for a given RPC URL.
    ///
    /// Returns default (empty) metadata if the URL is not in the store.
    ///
    /// # Arguments
    /// * `url` - The RPC endpoint URL
    ///
    /// # Returns
    /// * `RpcConfigMetadata` - The metadata for the URL, or default if not found
    pub fn get_metadata(&self, url: &str) -> RpcConfigMetadata {
        let metadata = self.metadata.read().unwrap();
        metadata.get(url).cloned().unwrap_or_default()
    }

    /// Updates metadata for a given RPC URL.
    ///
    /// # Arguments
    /// * `url` - The RPC endpoint URL
    /// * `metadata` - The metadata to store
    pub fn update_metadata(&self, url: &str, metadata: RpcConfigMetadata) {
        let mut store = self.metadata.write().unwrap();
        store.insert(url.to_string(), metadata);
    }

    /// Marks an RPC endpoint as failed, adding a failure timestamp.
    /// If the number of recent failures (within expiration window) reaches the threshold, pauses the endpoint.
    /// Stale failures (older than failure_expiration) are automatically removed.
    ///
    /// # Arguments
    /// * `url` - The RPC endpoint URL
    /// * `threshold` - The number of failures before pausing
    /// * `pause_duration` - The duration to pause for
    /// * `failure_expiration` - Duration after which failures are considered stale and removed
    pub fn mark_failed(
        &self,
        url: &str,
        threshold: u32,
        pause_duration: chrono::Duration,
        failure_expiration: chrono::Duration,
    ) {
        let mut store = self.metadata.write().unwrap();
        let mut metadata = store.get(url).cloned().unwrap_or_default();

        let now = Utc::now();

        // Remove stale failures (older than expiration window)
        metadata
            .failure_timestamps
            .retain(|&ts| now - ts <= failure_expiration);

        // Add current failure timestamp
        metadata.failure_timestamps.push(now);

        // Limit size to prevent unbounded growth (keep slightly more than threshold for safety)
        let max_size = (threshold * 2) as usize;
        if metadata.failure_timestamps.len() > max_size {
            // Keep only the most recent failures (they're already in chronological order)
            // Remove the oldest ones
            let remove_count = metadata.failure_timestamps.len() - max_size;
            metadata.failure_timestamps.drain(0..remove_count);
        }

        // Check if we've reached the threshold
        let recent_failures = metadata.failure_timestamps.len() as u32;
        let was_paused = metadata.paused_until.is_some();

        if recent_failures >= threshold {
            let paused_until = now + pause_duration;
            metadata.paused_until = Some(paused_until);

            if !was_paused {
                // Provider just got paused
                debug!(
                    provider_url = %url,
                    failure_count = %recent_failures,
                    threshold = %threshold,
                    paused_until = %paused_until,
                    pause_duration_secs = %pause_duration.num_seconds(),
                    "RPC provider paused due to failures"
                );
            } else {
                // Provider was already paused, but pause duration extended
                debug!(
                    provider_url = %url,
                    failure_count = %recent_failures,
                    threshold = %threshold,
                    paused_until = %paused_until,
                    pause_duration_secs = %pause_duration.num_seconds(),
                    "RPC provider pause extended due to additional failures"
                );
            }
        }

        store.insert(url.to_string(), metadata);
    }

    /// Resets the failure count and unpauses the endpoint.
    ///
    /// # Arguments
    /// * `url` - The RPC endpoint URL
    pub fn reset_failures(&self, url: &str) {
        let mut store = self.metadata.write().unwrap();
        store.remove(url);
    }

    /// Resets the failure count only if the provider has failures recorded.
    /// This is more efficient than `reset_failures` as it avoids write lock
    /// acquisition when there are no failures.
    ///
    /// # Arguments
    /// * `url` - The RPC endpoint URL
    ///
    /// # Returns
    /// * `bool` - True if failures were reset, false if no failures existed
    pub fn reset_failures_if_exists(&self, url: &str) -> bool {
        // Fast path: check if entry exists with read lock first
        let has_failures = {
            let store = self.metadata.read().unwrap();
            store.contains_key(url)
        };

        if has_failures {
            let mut store = self.metadata.write().unwrap();
            store.remove(url);
            true
        } else {
            false
        }
    }

    /// Checks if an RPC endpoint is currently paused.
    ///
    /// An endpoint is considered paused if:
    /// - It has reached the failure threshold (within expiration window) AND
    /// - It has a paused_until timestamp that is in the future
    ///
    /// Stale failures (older than failure_expiration) are automatically removed
    /// to allow the provider to be retried.
    ///
    /// # Arguments
    /// * `url` - The RPC endpoint URL
    /// * `threshold` - The failure threshold to check against
    /// * `failure_expiration` - Duration after which failures are considered stale and removed
    ///
    /// # Returns
    /// * `bool` - True if the endpoint is paused, false otherwise
    pub fn is_paused(
        &self,
        url: &str,
        threshold: u32,
        failure_expiration: chrono::Duration,
    ) -> bool {
        let mut metadata_guard = self.metadata.write().unwrap();
        if let Some(meta) = metadata_guard.get_mut(url) {
            let now = Utc::now();

            // Remove stale failures (older than expiration window)
            meta.failure_timestamps
                .retain(|&ts| now - ts <= failure_expiration);

            // If pause has expired, clear it
            if let Some(paused_until) = meta.paused_until {
                if now >= paused_until {
                    // Pause expired - clear pause (but keep failure timestamps for tracking)
                    debug!(
                        provider_url = %url,
                        paused_until = %paused_until,
                        current_time = %now,
                        remaining_failures = %meta.failure_timestamps.len(),
                        "RPC provider pause expired, provider available again"
                    );
                    meta.paused_until = None;
                    // If no recent failures remain, clear everything
                    if meta.failure_timestamps.is_empty() {
                        metadata_guard.remove(url);
                    }
                    return false;
                }
            }

            // Check if paused: must have reached threshold AND be within pause window
            let recent_failures = meta.failure_timestamps.len() as u32;
            if recent_failures >= threshold {
                if let Some(paused_until) = meta.paused_until {
                    return now < paused_until;
                }
                // If we've reached threshold but no pause_until is set, not paused
                return false;
            }

            // If no recent failures remain, remove the entry
            if meta.failure_timestamps.is_empty() && meta.paused_until.is_none() {
                metadata_guard.remove(url);
            }
        }
        false
    }

    /// Clears all metadata from the store.
    /// Primarily useful for testing.
    #[cfg(test)]
    pub fn clear_all(&self) {
        let mut store = self.metadata.write().unwrap();
        store.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_metadata_returns_default_when_not_found() {
        let store = RpcHealthStore::instance();
        store.clear_all();

        // Use a unique URL to avoid interference from other tests
        let url = "https://test-get-metadata.example.com";
        let metadata = store.get_metadata(url);
        assert_eq!(metadata, RpcConfigMetadata::default());
        assert_eq!(metadata.failure_timestamps.len(), 0);
        assert_eq!(metadata.paused_until, None);
    }

    #[test]
    fn test_update_and_get_metadata() {
        let store = RpcHealthStore::instance();
        store.clear_all();

        let url = "https://test-update-metadata.example.com";
        let mut metadata = RpcConfigMetadata::default();
        metadata.failure_timestamps.push(Utc::now());
        metadata.failure_timestamps.push(Utc::now());
        metadata.failure_timestamps.push(Utc::now());

        store.update_metadata(url, metadata.clone());

        let retrieved = store.get_metadata(url);
        assert_eq!(
            retrieved.failure_timestamps.len(),
            metadata.failure_timestamps.len()
        );
    }

    #[test]
    fn test_mark_failed_increments_count() {
        let store = RpcHealthStore::instance();
        store.clear_all();

        // Use a unique URL to avoid interference
        let url = "https://test-increment-count.example.com";
        let expiration = chrono::Duration::seconds(60);
        let threshold = 3;

        // First failure
        store.mark_failed(url, threshold, chrono::Duration::seconds(60), expiration);
        let metadata = store.get_metadata(url);
        assert_eq!(
            metadata.failure_timestamps.len(),
            1,
            "Should have 1 failure after first mark"
        );
        assert!(metadata.paused_until.is_none(), "Should not be paused yet");
        // Check pause status after verifying metadata
        assert!(
            !store.is_paused(url, threshold, expiration),
            "Should not be paused with 1 failure"
        );
        // Verify metadata still exists after is_paused call
        let metadata_after = store.get_metadata(url);
        assert_eq!(
            metadata_after.failure_timestamps.len(),
            1,
            "Should still have 1 failure"
        );

        // Second failure
        store.mark_failed(url, threshold, chrono::Duration::seconds(60), expiration);
        let metadata = store.get_metadata(url);
        assert_eq!(
            metadata.failure_timestamps.len(),
            2,
            "Should have 2 failures after second mark"
        );
        assert!(metadata.paused_until.is_none(), "Should not be paused yet");
        assert!(
            !store.is_paused(url, threshold, expiration),
            "Should not be paused with 2 failures"
        );
        let metadata_after = store.get_metadata(url);
        assert_eq!(
            metadata_after.failure_timestamps.len(),
            2,
            "Should still have 2 failures"
        );

        // Third failure - should pause
        store.mark_failed(url, threshold, chrono::Duration::seconds(60), expiration);
        let metadata = store.get_metadata(url);
        assert_eq!(
            metadata.failure_timestamps.len(),
            3,
            "Should have 3 failures after third mark"
        );
        assert!(
            metadata.paused_until.is_some(),
            "Should be paused after reaching threshold"
        );
        assert!(
            store.is_paused(url, threshold, expiration),
            "Should be paused"
        );
        let metadata_after = store.get_metadata(url);
        assert_eq!(
            metadata_after.failure_timestamps.len(),
            3,
            "Should still have 3 failures"
        );
        assert!(
            metadata_after.paused_until.is_some(),
            "Should still be paused"
        );
    }

    #[test]
    fn test_reset_failures() {
        let store = RpcHealthStore::instance();
        store.clear_all();

        let url = "https://test-reset-failures.example.com";
        let expiration = chrono::Duration::seconds(60);
        let threshold = 3;

        // Mark failed 3 times to trigger pause
        store.mark_failed(url, threshold, chrono::Duration::seconds(60), expiration);
        store.mark_failed(url, threshold, chrono::Duration::seconds(60), expiration);
        store.mark_failed(url, threshold, chrono::Duration::seconds(60), expiration);
        assert!(store.is_paused(url, threshold, expiration));

        store.reset_failures(url);
        assert!(!store.is_paused(url, threshold, expiration));
        let metadata = store.get_metadata(url);
        assert_eq!(metadata, RpcConfigMetadata::default());
    }

    #[test]
    fn test_is_paused_with_failure_count_below_threshold() {
        let store = RpcHealthStore::instance();
        store.clear_all();

        let url = "https://test-below-threshold.example.com";
        let expiration = chrono::Duration::seconds(60);
        let mut metadata = RpcConfigMetadata::default();
        metadata.failure_timestamps.push(Utc::now());
        store.update_metadata(url, metadata);

        // Should not be paused if below threshold
        assert!(!store.is_paused(url, 3, expiration));
    }

    #[test]
    fn test_is_paused_with_time_based_pause() {
        let store = RpcHealthStore::instance();
        store.clear_all();

        let url = "https://test-time-based-pause.example.com";
        let expiration = chrono::Duration::seconds(60);
        let mut metadata = RpcConfigMetadata::default();
        // Add 3 failures to reach threshold
        metadata.failure_timestamps.push(Utc::now());
        metadata.failure_timestamps.push(Utc::now());
        metadata.failure_timestamps.push(Utc::now());
        metadata.paused_until = Some(Utc::now() + chrono::Duration::seconds(60));
        store.update_metadata(url, metadata);

        assert!(store.is_paused(url, 3, expiration));
    }

    #[test]
    fn test_is_paused_expires_after_time() {
        let store = RpcHealthStore::instance();
        store.clear_all();

        let url = "https://test-expires-after-time.example.com";
        let expiration = chrono::Duration::seconds(60);
        let mut metadata = RpcConfigMetadata::default();
        // Set pause to expire in the past
        metadata.paused_until = Some(Utc::now() - chrono::Duration::seconds(60));
        store.update_metadata(url, metadata);

        // Should not be paused if pause time has expired
        assert!(!store.is_paused(url, 3, expiration));
    }

    #[test]
    fn test_shared_state_across_instances() {
        let store1 = RpcHealthStore::instance();
        let store2 = RpcHealthStore::instance();
        store1.clear_all();

        let url = "https://test-shared-state.example.com";
        let expiration = chrono::Duration::seconds(60);
        let threshold = 3;

        // Mark failed 3 times to trigger pause
        store1.mark_failed(url, threshold, chrono::Duration::seconds(60), expiration);
        store1.mark_failed(url, threshold, chrono::Duration::seconds(60), expiration);
        store1.mark_failed(url, threshold, chrono::Duration::seconds(60), expiration);

        // Both instances should see the same state
        assert!(store1.is_paused(url, threshold, expiration));
        assert!(store2.is_paused(url, threshold, expiration));

        let metadata1 = store1.get_metadata(url);
        let metadata2 = store2.get_metadata(url);
        assert_eq!(
            metadata1.failure_timestamps.len(),
            metadata2.failure_timestamps.len()
        );
    }

    #[test]
    fn test_stale_failures_are_expired() {
        let store = RpcHealthStore::instance();
        store.clear_all();

        let url = "https://test-stale-failures.example.com";
        let expiration = chrono::Duration::seconds(60);

        // Add failures that are old (outside expiration window)
        let mut metadata = RpcConfigMetadata::default();
        metadata
            .failure_timestamps
            .push(Utc::now() - chrono::Duration::seconds(120)); // 2 minutes ago
        metadata
            .failure_timestamps
            .push(Utc::now() - chrono::Duration::seconds(90)); // 1.5 minutes ago
        store.update_metadata(url, metadata);

        // Old failures should be expired when checking pause status
        assert!(!store.is_paused(url, 3, expiration));

        // Metadata should be cleaned up (no recent failures)
        let metadata = store.get_metadata(url);
        assert_eq!(metadata.failure_timestamps.len(), 0);
    }

    #[test]
    fn test_failure_timestamps_size_limit() {
        let store = RpcHealthStore::instance();
        store.clear_all();

        let url = "https://test-size-limit.example.com";
        let expiration = chrono::Duration::seconds(60);
        let threshold = 3;

        // Add many failures quickly (within expiration window)
        for _ in 0..10 {
            store.mark_failed(url, threshold, chrono::Duration::seconds(60), expiration);
        }

        let metadata = store.get_metadata(url);
        // Should be limited to threshold * 2 = 6 entries
        assert!(metadata.failure_timestamps.len() <= (threshold * 2) as usize);
    }

    #[test]
    fn test_mixed_stale_and_recent_failures() {
        let store = RpcHealthStore::instance();
        store.clear_all();

        let url = "https://test-mixed-failures.example.com";
        let expiration = chrono::Duration::seconds(60);
        let threshold = 3;

        // Add old failures manually
        let mut metadata = RpcConfigMetadata::default();
        metadata
            .failure_timestamps
            .push(Utc::now() - chrono::Duration::seconds(120)); // Stale
        metadata
            .failure_timestamps
            .push(Utc::now() - chrono::Duration::seconds(90)); // Stale
        store.update_metadata(url, metadata);

        // Add recent failures - mark_failed will remove stale ones first
        store.mark_failed(url, threshold, chrono::Duration::seconds(60), expiration);
        store.mark_failed(url, threshold, chrono::Duration::seconds(60), expiration);

        let metadata = store.get_metadata(url);
        // Should only have 2 recent failures (stale ones removed during mark_failed)
        assert_eq!(metadata.failure_timestamps.len(), 2);
        assert!(!store.is_paused(url, threshold, expiration)); // Below threshold
    }

    #[test]
    fn test_pause_extension_when_already_paused() {
        let store = RpcHealthStore::instance();
        store.clear_all();

        // Use a unique URL to avoid interference
        let url = "https://test-pause-extension.example.com";
        let expiration = chrono::Duration::seconds(60);
        let pause_duration = chrono::Duration::seconds(60);
        let threshold = 3;

        // Mark failed 3 times to trigger pause
        store.mark_failed(url, threshold, pause_duration, expiration);
        store.mark_failed(url, threshold, pause_duration, expiration);
        store.mark_failed(url, threshold, pause_duration, expiration);

        // Verify pause was set (check metadata before calling is_paused)
        let metadata1 = store.get_metadata(url);
        assert_eq!(
            metadata1.failure_timestamps.len(),
            3,
            "Should have 3 failures"
        );
        assert!(
            metadata1.paused_until.is_some(),
            "Should be paused after 3 failures"
        );
        let initial_paused_until = metadata1.paused_until.unwrap();

        // Verify it's actually paused
        assert!(
            store.is_paused(url, threshold, expiration),
            "Should be paused"
        );
        // Verify metadata still exists after is_paused
        let metadata1_after = store.get_metadata(url);
        assert_eq!(
            metadata1_after.failure_timestamps.len(),
            3,
            "Should still have 3 failures"
        );

        // Wait a bit (simulate time passing)
        std::thread::sleep(std::time::Duration::from_millis(10));

        // Mark failed again while already paused - should extend pause
        store.mark_failed(url, threshold, pause_duration, expiration);

        let metadata2 = store.get_metadata(url);
        assert_eq!(
            metadata2.failure_timestamps.len(),
            4,
            "Should have 4 failures now"
        );
        assert!(
            metadata2.paused_until.is_some(),
            "Should still be paused after 4th failure"
        );
        let new_paused_until = metadata2.paused_until.unwrap();

        // Pause should be extended (new paused_until should be later)
        assert!(
            new_paused_until > initial_paused_until,
            "Pause should be extended"
        );
        assert!(
            store.is_paused(url, threshold, expiration),
            "Should still be paused"
        );
    }

    #[test]
    fn test_stale_failures_removed_during_mark_failed() {
        let store = RpcHealthStore::instance();
        store.clear_all();

        let url = "https://test-stale-removed.example.com";
        let expiration = chrono::Duration::seconds(60);

        // Add old failures manually
        let mut metadata = RpcConfigMetadata::default();
        metadata
            .failure_timestamps
            .push(Utc::now() - chrono::Duration::seconds(120)); // Stale
        metadata
            .failure_timestamps
            .push(Utc::now() - chrono::Duration::seconds(90)); // Stale
        store.update_metadata(url, metadata);

        // Mark failed - should remove stale failures and add new one
        store.mark_failed(url, 3, chrono::Duration::seconds(60), expiration);

        let metadata = store.get_metadata(url);
        // Should only have 1 failure (stale ones removed, new one added)
        assert_eq!(metadata.failure_timestamps.len(), 1);
        // Verify the remaining failure is recent
        let remaining_failure = metadata.failure_timestamps[0];
        let age = Utc::now() - remaining_failure;
        assert!(age.num_seconds() < 5); // Should be very recent
    }

    #[test]
    fn test_pause_expiration_cleans_up_metadata() {
        let store = RpcHealthStore::instance();
        store.clear_all();

        let url = "https://test-pause-expiration-cleanup.example.com";
        let expiration = chrono::Duration::seconds(60);

        // Create metadata with expired pause but no recent failures
        let mut metadata = RpcConfigMetadata::default();
        metadata.paused_until = Some(Utc::now() - chrono::Duration::seconds(10)); // Expired
        store.update_metadata(url, metadata);

        // Check pause status - should expire and clean up
        assert!(!store.is_paused(url, 3, expiration));

        // Metadata should be removed since pause expired and no failures remain
        let metadata_after = store.get_metadata(url);
        assert_eq!(metadata_after, RpcConfigMetadata::default());
    }

    #[test]
    fn test_pause_expiration_keeps_recent_failures() {
        let store = RpcHealthStore::instance();
        store.clear_all();

        // Use a unique URL to avoid interference
        let url = "https://test-pause-expiration.example.com";
        let expiration = chrono::Duration::seconds(60);
        let threshold = 3;

        // Create metadata with expired pause but recent failures
        // We need at least threshold failures to have been paused, but pause is now expired
        let mut metadata = RpcConfigMetadata::default();
        // Add threshold failures (but they're recent, not stale)
        metadata
            .failure_timestamps
            .push(Utc::now() - chrono::Duration::seconds(30)); // Recent
        metadata
            .failure_timestamps
            .push(Utc::now() - chrono::Duration::seconds(25)); // Recent
        metadata
            .failure_timestamps
            .push(Utc::now() - chrono::Duration::seconds(20)); // Recent
        metadata.paused_until = Some(Utc::now() - chrono::Duration::seconds(10)); // Expired pause
        store.update_metadata(url, metadata);

        // Check pause status - should expire pause but keep failures
        // Note: is_paused will modify the metadata (remove expired pause)
        // Since we have threshold failures but pause is expired, should return false
        assert!(
            !store.is_paused(url, threshold, expiration),
            "Should not be paused when pause expired"
        );

        // Metadata should still exist with failures but no pause
        let metadata_after = store.get_metadata(url);
        assert_eq!(
            metadata_after.failure_timestamps.len(),
            3,
            "Should keep all recent failures"
        );
        assert!(
            metadata_after.paused_until.is_none(),
            "Pause should be cleared"
        );
    }
}
