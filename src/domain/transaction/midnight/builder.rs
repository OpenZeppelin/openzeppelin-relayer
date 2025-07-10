//! Midnight transaction builder
//!
//! This module provides a builder pattern for constructing Midnight transactions
//! following the midnight-node patterns.
//!
//! IMPORTANT: When creating transactions, always include a change output back to the sender
//! if there's any remaining value after accounting for the recipient amount and fees.
//! This ensures the indexer recognizes the transaction as relevant to the sender's wallet
//! during synchronization.

use midnight_node_ledger_helpers::{
    FromContext,
    IntentInfo,
    LedgerContext,
    OfferInfo,
    Proof,
    ProofProvider,
    // StandardTrasactionInfo has a typo and may be changed in the future
    StandardTrasactionInfo,
    Transaction,
    WellFormedStrictness,
    DB,
};
use std::sync::Arc;

use crate::models::TransactionError;

/// Builder for constructing Midnight transactions using midnight-node patterns
pub struct MidnightTransactionBuilder<D: DB> {
    /// The ledger context containing wallet and network information
    context: Option<Arc<LedgerContext<D>>>,
    /// The proof provider for generating ZK proofs
    proof_provider: Option<Box<dyn ProofProvider<D>>>,
    /// Random seed for transaction building
    rng_seed: Option<[u8; 32]>,
    /// The guaranteed offer to be added
    guaranteed_offer: Option<OfferInfo<D>>,
    /// Intent info containing all fallible offers with segment information preserved
    intent_info: Option<IntentInfo<D>>,
}

impl<D: DB> MidnightTransactionBuilder<D> {
    /// Creates a new transaction builder
    pub fn new() -> Self {
        Self {
            context: None,
            proof_provider: None,
            rng_seed: None,
            guaranteed_offer: None,
            intent_info: None,
        }
    }

    /// Sets the ledger context
    pub fn with_context(mut self, context: std::sync::Arc<LedgerContext<D>>) -> Self {
        self.context = Some(context);
        self
    }

    /// Sets the proof provider
    pub fn with_proof_provider(mut self, proof_provider: Box<dyn ProofProvider<D>>) -> Self {
        self.proof_provider = Some(proof_provider);
        self
    }

    /// Sets the RNG seed
    pub fn with_rng_seed(mut self, seed: [u8; 32]) -> Self {
        self.rng_seed = Some(seed);
        self
    }

    /// Sets the entire guaranteed offer
    pub fn with_guaranteed_offer(mut self, offer: OfferInfo<D>) -> Self {
        self.guaranteed_offer = Some(offer);
        self
    }

    /// Set the intent info
    pub fn with_intent_info(mut self, intent: IntentInfo<D>) -> Self {
        self.intent_info = Some(intent);
        self
    }

    /// Builds the final transaction
    pub async fn build(self) -> Result<Transaction<Proof, D>, TransactionError> {
        let context_arc = self
            .context
            .ok_or_else(|| TransactionError::ValidationError("Context not provided".to_string()))?;
        let proof_provider = self.proof_provider.ok_or_else(|| {
            TransactionError::ValidationError("Proof provider not provided".to_string())
        })?;
        let rng_seed = self.rng_seed.ok_or_else(|| {
            TransactionError::ValidationError("RNG seed not provided".to_string())
        })?;

        // Create StandardTransactionInfo with the context
        let mut tx_info = StandardTrasactionInfo::new_from_context(
            context_arc.clone(),
            proof_provider.into(),
            Some(rng_seed),
        );

        // Set the guaranteed offer if present
        if let Some(offer) = self.guaranteed_offer {
            tx_info.set_guaranteed_coins(offer);
        }

        // Set the intent info if present to preserve segment information
        if let Some(intent) = self.intent_info {
            tx_info.set_intents(vec![intent]);
        }

        // Build transaction and generate proofs
        let proven_tx = tx_info.prove().await;

        // Get the ledger state from the context
        let ledger_state_guard = context_arc.ledger_state.lock().map_err(|e| {
            TransactionError::UnexpectedError(format!("Failed to acquire ledger state lock: {}", e))
        })?;

        let ref_state = &*ledger_state_guard;

        // Perform well_formed validation
        proven_tx
            .well_formed(ref_state, WellFormedStrictness::default())
            .map_err(|e| {
                TransactionError::ValidationError(format!(
                    "Transaction failed well_formed validation: {:?}",
                    e
                ))
            })?;

        Ok(proven_tx)
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
    use midnight_node_ledger_helpers::{DefaultDB, LedgerContext, WalletSeed};
    use std::sync::Arc;

    fn create_test_context() -> Arc<LedgerContext<DefaultDB>> {
        // Set required environment variable for Midnight tests
        std::env::set_var(
            "MIDNIGHT_LEDGER_TEST_STATIC_DIR",
            "/tmp/midnight-test-static",
        );

        let wallet_seed = WalletSeed::from([1u8; 32]);
        Arc::new(LedgerContext::new_from_wallet_seeds(&[wallet_seed]))
    }

    #[test]
    fn test_builder_new() {
        let builder = MidnightTransactionBuilder::<DefaultDB>::new();
        assert!(builder.context.is_none());
        assert!(builder.proof_provider.is_none());
        assert!(builder.rng_seed.is_none());
        assert!(builder.guaranteed_offer.is_none());
        assert!(builder.intent_info.is_none());
    }

    #[test]
    fn test_builder_default() {
        let builder = MidnightTransactionBuilder::<DefaultDB>::default();
        assert!(builder.context.is_none());
        assert!(builder.proof_provider.is_none());
        assert!(builder.rng_seed.is_none());
        assert!(builder.guaranteed_offer.is_none());
        assert!(builder.intent_info.is_none());
    }

    #[test]
    fn test_builder_with_context() {
        let builder = MidnightTransactionBuilder::<DefaultDB>::new();
        let context = create_test_context();
        let builder_with_context = builder.with_context(context.clone());
        assert!(builder_with_context.context.is_some());
    }

    #[test]
    fn test_builder_with_rng_seed() {
        let builder = MidnightTransactionBuilder::<DefaultDB>::new();
        let seed = [42u8; 32];
        let builder_with_seed = builder.with_rng_seed(seed);
        assert!(builder_with_seed.rng_seed.is_some());
        assert_eq!(builder_with_seed.rng_seed.unwrap(), seed);
    }

    #[tokio::test]
    async fn test_builder_build_without_context_fails() {
        let builder = MidnightTransactionBuilder::<DefaultDB>::new();
        let result = builder.build().await;
        assert!(result.is_err());
        match result.err().unwrap() {
            TransactionError::ValidationError(msg) => {
                assert!(msg.contains("Context not provided"));
            }
            _ => panic!("Expected ValidationError for missing context"),
        }
    }

    #[tokio::test]
    async fn test_builder_build_without_proof_provider_fails() {
        let builder = MidnightTransactionBuilder::<DefaultDB>::new();
        let context = create_test_context();
        let builder_with_context = builder.with_context(context);
        let result = builder_with_context.build().await;
        assert!(result.is_err());
        match result.err().unwrap() {
            TransactionError::ValidationError(msg) => {
                assert!(msg.contains("Proof provider not provided"));
            }
            _ => panic!("Expected ValidationError for missing proof provider"),
        }
    }

    #[test]
    fn test_builder_chain_methods() {
        let context = create_test_context();
        let seed = [42u8; 32];

        let builder = MidnightTransactionBuilder::<DefaultDB>::new()
            .with_context(context)
            .with_rng_seed(seed);

        assert!(builder.context.is_some());
        assert!(builder.rng_seed.is_some());
        assert_eq!(builder.rng_seed.unwrap(), seed);
    }
}
