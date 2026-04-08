//! Remote proof server integration for Midnight ZK proof generation.
//!
//! Implements the `ProofProvider` trait using a remote proof server
//! (Docker: `midnightntwrk/proof-server` or public hosted endpoint).

use async_trait::async_trait;
use tracing::{info, warn};

use midnight_node_ledger_helpers::{
    CostModel, PedersenRandomness, ProofMarker, ProofPreimageMarker, ProofServerProvider, Resolver,
    Signature, StdRng, Transaction, DB,
};

use midnight_node_ledger_helpers::ProofProvider;

const PROOF_SERVER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Remote proof server that delegates ZK proof generation to an HTTP endpoint.
pub struct RemoteProofServer {
    url: String,
}

impl RemoteProofServer {
    pub fn new(url: String) -> Self {
        info!(url = %url, "Remote proof server configured");
        Self { url }
    }

    pub fn url(&self) -> &str {
        &self.url
    }
}

#[async_trait]
impl<D: DB + Clone> ProofProvider<D> for RemoteProofServer {
    async fn prove(
        &self,
        tx: Transaction<Signature, ProofPreimageMarker, PedersenRandomness, D>,
        _rng: StdRng,
        resolver: &Resolver,
        cost_model: &CostModel,
    ) -> Transaction<Signature, ProofMarker, PedersenRandomness, D> {
        let provider = ProofServerProvider {
            base_url: self.url.clone().into(),
            resolver,
        };

        tx.prove(provider, cost_model).await.unwrap_or_else(|e| {
            panic!("Proof server at {} failed: {e:?}", self.url);
        })
    }
}
