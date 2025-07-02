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
    /// Returns true if all relevant indices have been processed and at least some data was processed.
    pub fn is_sync_complete(&self, highest_index: u64, highest_relevant_wallet_index: u64) -> bool {
        // Only consider sync complete if:
        // 1. We're at or past the highest relevant index
        // 2. We've actually processed data up to that point
        // 3. We've processed at least some data (transactions or Merkle updates)
        (self.start_index >= highest_relevant_wallet_index
            || highest_index >= highest_relevant_wallet_index)
            && self.last_processed_index >= highest_relevant_wallet_index
            && self.has_processed_data()
    }
}
