//! Remote proof server integration for Midnight ZK proof generation.
//!
//! Implements the `ProofProvider` trait using a remote proof server
//! (Docker: `midnightntwrk/proof-server` or public hosted endpoint).

use async_trait::async_trait;
use tracing::{error, info};

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

        // The `ProofProvider::prove` trait signature is infallible — we have
        // no way to return an `Err` to the library's `tx_info.prove()`
        // wrapper here. A proof-server HTTP failure / timeout therefore has
        // to panic. The call site in `midnight_transaction.rs` wraps
        // `tx_info.prove()` in `AssertUnwindSafe(...).catch_unwind()` so the
        // panic becomes a recoverable job error rather than crashing the
        // worker. We log structured detail here so operators still see the
        // prover URL and the underlying error in logs regardless of whether
        // the panic is caught downstream.
        tx.prove(provider, cost_model).await.unwrap_or_else(|e| {
            error!(
                prover_url = %self.url,
                error = ?e,
                "Remote proof server failed"
            );
            panic!("Proof server at {} failed: {e:?}", self.url);
        })
    }
}
