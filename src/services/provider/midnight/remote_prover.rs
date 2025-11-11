//!
//! Remote proof server integration for Midnight zero-knowledge proofs.
//!
//! Provides a client for submitting transactions to a remote proof server and retrieving
//! zero-knowledge proofs required for transaction construction on the Midnight network.
//! Copied from <https://github.com/midnightntwrk/midnight-node/blob/node-0.12.0/util/toolkit/src/remote_prover.rs>

use async_trait::async_trait;
use backoff::{ExponentialBackoff, future::retry};
use midnight_node_ledger_helpers::{
    CostModel, DB, KeyLocation, NetworkId, PedersenRandomness, ProofMarker, ProofPreimageMarker,
    ProofProvider, Resolver, ResolverTrait, Signature, StdRng, Transaction, deserialize,
};

/// Remote proof server client for generating zero-knowledge proofs
pub struct RemoteProofServer {
    url: String,
    #[allow(dead_code)]
    network_id: NetworkId,
}

impl RemoteProofServer {
    /// Creates a new remote proof server client
    pub fn new(url: String, network_id: NetworkId) -> Self {
        Self { url, network_id }
    }

    /// Serializes a transaction and its circuit keys for the proof server
    pub async fn serialize_request_body<D: DB>(
        &self,
        tx: &Transaction<Signature, ProofPreimageMarker, PedersenRandomness, D>,
        resolver: &Resolver,
    ) -> Vec<u8> {
        let circuits_used = tx
            .calls()
            .map(|(_segment_id, call)| String::from_utf8_lossy(&call.entry_point).into_owned())
            .collect::<Vec<_>>();
        let mut keys = std::collections::HashMap::new();
        for k in circuits_used.into_iter() {
            let k = KeyLocation(std::borrow::Cow::Owned(k));
            let data = resolver
                .resolve_key(k.clone())
                .await
                .expect("failed to resolve key");
            if let Some(data) = data {
                keys.insert(k, data);
            }
        }
        let mut bytes = Vec::new();
        use midnight_node_ledger_helpers::mn_ledger_serialize;
        mn_ledger_serialize::tagged_serialize(tx, &mut bytes)
            .expect("failed to serialize transaction");
        mn_ledger_serialize::tagged_serialize(&keys, &mut bytes).expect("failed to serialize keys");
        bytes
    }
}

#[async_trait]
impl<D: DB + Clone> ProofProvider<D> for RemoteProofServer {
    async fn prove(
        &self,
        tx: Transaction<Signature, ProofPreimageMarker, PedersenRandomness, D>,
        _rng: StdRng,
        resolver: &Resolver,
        _cost_model: &CostModel,
    ) -> Transaction<Signature, ProofMarker, PedersenRandomness, D> {
        let url = reqwest::Url::parse(&self.url)
            .expect("failed to parse proof server URL")
            .join("prove-tx")
            .unwrap();

        let client = reqwest::ClientBuilder::new()
            .pool_idle_timeout(None)
            .build()
            .unwrap();
        let response_bytes = retry(ExponentialBackoff::default(), || async {
            let body = self.serialize_request_body(&tx, resolver).await;

            let resp = client
                .post(url.clone())
                .body(body)
                .send()
                .await
                .map_err(|e| {
                    println!("Proof Server Send Error: {e:?}");
                    backoff::Error::transient(e)
                })?;

            let resp_err = resp.error_for_status_ref().err();
            let resp_bytes = resp.bytes().await.map_err(|e| {
                println!("Proof Server to Bytes Error: {e:?}");
                backoff::Error::transient(e)
            })?;

            if let Some(e) = resp_err {
                println!("Proof Server Response Error: {e:?}. Bytes: {resp_bytes:?}");
                return Err(backoff::Error::transient(e));
            }

            Ok::<Vec<u8>, backoff::Error<reqwest::Error>>(resp_bytes.to_vec())
        })
        .await
        .expect("failed to send request");

        if response_bytes.is_empty() {
            panic!("Proof server returned empty response");
        }

        deserialize(&response_bytes[..]).expect("failed to deserialize transaction")
    }
}
