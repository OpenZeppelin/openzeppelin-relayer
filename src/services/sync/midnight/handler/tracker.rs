use std::collections::HashSet;

/// Service for tracking synchronization progress
///
/// The progress tracker records which blockchain indices have been processed, counts transactions
/// and Merkle updates, and provides statistics and validation methods for sync completeness.
#[derive(Debug, Clone)]
pub struct ProgressTracker {
    /// Starting index for this sync session
    start_index: u64,
    /// The highest blockchain index we've processed
    last_processed_index: u64,
    /// Track all blockchain indices we've processed
    processed_indices: HashSet<u64>,
    /// Total transactions processed
    total_transactions_processed: usize,
    /// Total Merkle updates processed
    total_merkle_updates_processed: usize,
}

impl ProgressTracker {
    /// Create a new progress tracker starting from the given index.
    pub fn new(start_index: u64) -> Self {
        Self {
            start_index,
            last_processed_index: start_index,
            processed_indices: HashSet::new(),
            total_transactions_processed: 0,
            total_merkle_updates_processed: 0,
        }
    }

    /// Record that we processed data at a specific index
    pub fn record_processed(&mut self, index: u64) {
        self.last_processed_index = self.last_processed_index.max(index);
        self.processed_indices.insert(index);
    }

    /// Record a processed transaction at the given index
    pub fn record_transaction(&mut self, index: u64) {
        self.record_processed(index);
        self.total_transactions_processed += 1;
    }

    /// Record a processed Merkle update at the given index
    pub fn record_merkle_update(&mut self, index: u64) {
        self.record_processed(index);
        self.total_merkle_updates_processed += 1;
    }

    pub fn has_processed_data(&self) -> bool {
        self.total_transactions_processed > 0 || self.total_merkle_updates_processed > 0
    }

    /// Check if sync is complete based on progress updates
    ///
    /// Returns true if we're already at or past the highest relevant index.
    pub fn is_sync_complete(&self, highest_index: u64, highest_relevant_wallet_index: u64) -> bool {
        // Consider sync complete if:
        // 1. We started at or past the highest relevant wallet index (nothing new to sync)
        // 2. OR we've reached the highest relevant index and processed data
        if self.start_index >= highest_relevant_wallet_index {
            // Already caught up - nothing new to sync
            return true;
        }

        // Otherwise, we need to have processed data up to the highest relevant index
        (highest_index >= highest_relevant_wallet_index)
            && self.last_processed_index >= highest_relevant_wallet_index
            && self.has_processed_data()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_progress_tracker_new() {
        let tracker = ProgressTracker::new(100);
        assert_eq!(tracker.start_index, 100);
        assert_eq!(tracker.last_processed_index, 100);
        assert!(tracker.processed_indices.is_empty());
        assert_eq!(tracker.total_transactions_processed, 0);
        assert_eq!(tracker.total_merkle_updates_processed, 0);
        assert!(!tracker.has_processed_data());
    }

    #[test]
    fn test_record_processed() {
        let mut tracker = ProgressTracker::new(100);

        tracker.record_processed(105);
        assert_eq!(tracker.last_processed_index, 105);
        assert!(tracker.processed_indices.contains(&105));

        tracker.record_processed(103);
        assert_eq!(tracker.last_processed_index, 105); // Should remain at max
        assert!(tracker.processed_indices.contains(&103));
        assert!(tracker.processed_indices.contains(&105));

        tracker.record_processed(110);
        assert_eq!(tracker.last_processed_index, 110);
        assert_eq!(tracker.processed_indices.len(), 3);
    }

    #[test]
    fn test_record_transaction() {
        let mut tracker = ProgressTracker::new(100);

        tracker.record_transaction(105);
        assert_eq!(tracker.last_processed_index, 105);
        assert!(tracker.processed_indices.contains(&105));
        assert_eq!(tracker.total_transactions_processed, 1);
        assert!(tracker.has_processed_data());

        tracker.record_transaction(106);
        assert_eq!(tracker.total_transactions_processed, 2);
    }

    #[test]
    fn test_record_merkle_update() {
        let mut tracker = ProgressTracker::new(100);

        tracker.record_merkle_update(105);
        assert_eq!(tracker.last_processed_index, 105);
        assert!(tracker.processed_indices.contains(&105));
        assert_eq!(tracker.total_merkle_updates_processed, 1);
        assert!(tracker.has_processed_data());

        tracker.record_merkle_update(106);
        assert_eq!(tracker.total_merkle_updates_processed, 2);
    }

    #[test]
    fn test_has_processed_data() {
        let mut tracker = ProgressTracker::new(100);
        assert!(!tracker.has_processed_data());

        tracker.record_transaction(105);
        assert!(tracker.has_processed_data());

        let mut tracker2 = ProgressTracker::new(100);
        tracker2.record_merkle_update(105);
        assert!(tracker2.has_processed_data());

        let mut tracker3 = ProgressTracker::new(100);
        tracker3.record_processed(105); // Just recording index, no data
        assert!(!tracker3.has_processed_data());
    }

    #[test]
    fn test_is_sync_complete_already_caught_up() {
        let tracker = ProgressTracker::new(200);

        // Already at or past highest relevant wallet index
        assert!(tracker.is_sync_complete(250, 150));
        assert!(tracker.is_sync_complete(250, 200));
    }

    #[test]
    fn test_is_sync_complete_needs_processing() {
        let mut tracker = ProgressTracker::new(100);

        // Not complete - haven't processed anything
        assert!(!tracker.is_sync_complete(250, 200));

        // Process some data but not enough
        tracker.record_transaction(150);
        assert!(!tracker.is_sync_complete(250, 200));

        // Process up to highest relevant index
        tracker.record_transaction(200);
        assert!(tracker.is_sync_complete(250, 200));
    }

    #[test]
    fn test_is_sync_complete_edge_cases() {
        let mut tracker = ProgressTracker::new(100);

        // Highest index is less than highest relevant wallet index
        tracker.record_transaction(150);
        assert!(!tracker.is_sync_complete(150, 200));

        // Processed index equals highest relevant wallet index
        tracker.record_merkle_update(200);
        assert!(tracker.is_sync_complete(200, 200));
    }

    #[test]
    fn test_mixed_processing() {
        let mut tracker = ProgressTracker::new(100);

        // Mix of transactions and merkle updates
        tracker.record_transaction(105);
        tracker.record_merkle_update(106);
        tracker.record_transaction(107);
        tracker.record_processed(108); // Just an index
        tracker.record_merkle_update(109);

        assert_eq!(tracker.last_processed_index, 109);
        assert_eq!(tracker.processed_indices.len(), 5);
        assert_eq!(tracker.total_transactions_processed, 2);
        assert_eq!(tracker.total_merkle_updates_processed, 2);
        assert!(tracker.has_processed_data());
    }
}
