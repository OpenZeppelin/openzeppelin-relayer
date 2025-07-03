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
