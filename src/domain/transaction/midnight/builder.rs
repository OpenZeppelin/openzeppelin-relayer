//! Midnight transaction builder
//!
//! This module provides a builder pattern for constructing Midnight transactions.

use super::types::*;
use crate::models::TransactionError;

use midnight_node_ledger_helpers::{ProofPreimage, DB};
use std::collections::HashMap;

/// Builder for constructing Midnight transactions
pub struct MidnightTransactionBuilder<D: DB> {
    intents: HashMap<SegmentId, MidnightZSwapIntent<D>>,
    guaranteed_offer: Option<MidnightZSwapOffer<ProofPreimage, D>>,
    fallible_offers: HashMap<SegmentId, MidnightZSwapOffer<ProofPreimage, D>>,
    current_segment: SegmentId,
}

impl<D: DB> MidnightTransactionBuilder<D> {
    /// Creates a new transaction builder
    pub fn new() -> Self {
        Self {
            intents: HashMap::new(),
            guaranteed_offer: None,
            fallible_offers: HashMap::new(),
            current_segment: 1, // Start with segment 1 (0 is reserved for guaranteed)
        }
    }

    /// Sets the current segment ID for subsequent operations
    pub fn with_segment(mut self, segment_id: SegmentId) -> Self {
        if segment_id != SEGMENT_GUARANTEED {
            self.current_segment = segment_id;
        }
        self
    }

    /// Adds an intent to the current segment
    pub fn add_intent(mut self, intent: MidnightZSwapIntent<D>) -> Self {
        self.intents.insert(self.current_segment, intent);
        self
    }

    /// Sets the guaranteed Zswap offer
    pub fn with_guaranteed_offer(mut self, offer: MidnightZSwapOffer<ProofPreimage, D>) -> Self {
        self.guaranteed_offer = Some(offer);
        self
    }

    /// Adds a fallible Zswap offer to the current segment
    pub fn add_fallible_offer(mut self, offer: MidnightZSwapOffer<ProofPreimage, D>) -> Self {
        self.fallible_offers.insert(self.current_segment, offer);
        self
    }

    /// Adds a Zswap input to the guaranteed offer
    pub fn add_guaranteed_input(mut self, input: MidnightZSwapInput<ProofPreimage, D>) -> Self {
        if let Some(ref mut offer) = self.guaranteed_offer {
            offer.inputs.push(input.inner);
        } else {
            let offer = MidnightZSwapOffer::<ProofPreimage, D>::new(
                vec![input.inner],
                vec![],
                vec![],
                vec![],
            );
            self.guaranteed_offer = Some(offer);
        }
        self
    }

    /// Adds a Zswap output to the guaranteed offer
    pub fn add_guaranteed_output(mut self, output: MidnightZSwapOutput<ProofPreimage, D>) -> Self {
        if let Some(ref mut offer) = self.guaranteed_offer {
            offer.outputs.push(output.inner);
        } else {
            let offer = MidnightZSwapOffer::<ProofPreimage, D>::new(
                vec![],
                vec![output.inner],
                vec![],
                vec![],
            );
            self.guaranteed_offer = Some(offer);
        }
        self
    }

    /// Builds the final transaction
    pub fn build(self) -> Result<MidnightTransaction<ProofPreimage, D>, TransactionError> {
        // TODO: Add validation
        // 1. Check segment sequencing rules
        // 2. Validate intent structure
        // 3. Check balance requirements

        Ok(MidnightTransaction::new(
            self.guaranteed_offer
                .unwrap_or_else(|| MidnightZSwapOffer::new(vec![], vec![], vec![], vec![])),
            None, // TODO: Handle fallible offers
            None, // TODO: Handle contract calls
        ))
    }

    /// Creates a simple transfer transaction
    pub fn simple_transfer(
        _from_coin: MidnightCoinInfo,
        _to_address: MidnightCoinPublicKey,
        _amount: u128,
    ) -> Result<MidnightTransaction<ProofPreimage, D>, TransactionError> {
        // TODO: Implement simple transfer construction
        // This would create a basic transaction with one input and one output

        let builder = Self::new();
        builder.build()
    }
}

impl<D: DB> Default for MidnightTransactionBuilder<D> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use midnight_node_ledger_helpers::{DefaultDB, Transaction};

    #[test]
    fn test_builder_new() {
        let builder = MidnightTransactionBuilder::<DefaultDB>::new();
        assert_eq!(builder.current_segment, 1);
        assert!(builder.intents.is_empty());
        assert!(builder.guaranteed_offer.is_none());
        assert!(builder.fallible_offers.is_empty());
    }

    #[test]
    fn test_builder_with_segment() {
        let builder = MidnightTransactionBuilder::<DefaultDB>::new().with_segment(5);
        assert_eq!(builder.current_segment, 5);

        // Should not allow setting to guaranteed segment
        let builder = builder.with_segment(SEGMENT_GUARANTEED);
        assert_eq!(builder.current_segment, 5);
    }

    #[test]
    fn test_build_empty_transaction() {
        let builder = MidnightTransactionBuilder::<DefaultDB>::new();
        let tx = builder.build();
        assert!(tx.is_ok());

        let tx = tx.unwrap();
        match tx.inner {
            Transaction::Standard(stx) => {
                assert!(stx.guaranteed_coins.inputs.is_empty());
                assert!(stx.guaranteed_coins.outputs.is_empty());
                assert!(stx.fallible_coins.is_none());
                assert!(stx.contract_calls.is_none());
            }
            _ => panic!("Expected Standard transaction"),
        }
    }
}
