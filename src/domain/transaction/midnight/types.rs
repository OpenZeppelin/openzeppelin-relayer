use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;

use midnight_coin_structure::coin::{Commitment, Nullifier, SecretKey};
use midnight_ledger::structure::ContractCall;
use midnight_node_ledger_helpers::{
    CoinInfo, CoinPublicKey, ContractAction, ContractAddress, ContractDeploy, DB,
    EncryptionPublicKey, Fr, HashOutput, Input, InputInfo, IntentInfo, NetworkId, Offer, OfferInfo,
    Output, OutputInfo, ProofKind, SecretKeys, ShieldedTokenType, SignatureKind, TokenType,
    Transaction, TransactionResult, Transcript, Transient,
};
use midnight_storage::Storable;

// Wrapper types for Midnight ZSwap types
#[derive(Debug, Clone)]
pub struct MidnightZSwapInput<P: ProofKind<D>, D: DB> {
    pub inner: Input<P::LatestProof, D>,
    _phantom: PhantomData<D>,
}

#[derive(Debug, Clone)]
pub struct MidnightZSwapOutput<P: ProofKind<D>, D: DB> {
    pub inner: Output<P::LatestProof, D>,
    _phantom: PhantomData<D>,
}

#[derive(Debug, Clone)]
pub struct MidnightZSwapOffer<P: ProofKind<D>, D: DB> {
    pub inner: Offer<P::LatestProof, D>,
    _phantom: PhantomData<D>,
}

impl<P: ProofKind<D>, D: DB> MidnightZSwapOffer<P, D> {
    pub fn new(
        inputs: Vec<Input<P::LatestProof, D>>,
        outputs: Vec<Output<P::LatestProof, D>>,
        transient: Vec<Transient<P::LatestProof, D>>,
        deltas: Vec<(ShieldedTokenType, i128)>,
    ) -> Self {
        use midnight_node_ledger_helpers::Delta;
        // Convert Vec<(ShieldedTokenType, i128)> to Vec<Delta>
        let delta_vec: Vec<Delta> = deltas
            .into_iter()
            .map(|(token_type, value)| Delta { token_type, value })
            .collect();
        Self {
            inner: Offer::<P::LatestProof, D> {
                inputs: inputs.into(),
                outputs: outputs.into(),
                transient: transient.into(),
                deltas: (&delta_vec).into(),
            },
            _phantom: PhantomData,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MidnightTransaction<P: ProofKind<D>, S: SignatureKind<D>, B: Storable<D>, D: DB> {
    pub inner: Transaction<S, P, B, D>,
    _phantom: PhantomData<D>,
}

pub struct MidnightZSwapIntent<D: DB + Clone> {
    pub inner: IntentInfo<D>,
}

#[derive(Debug, Clone)]
pub struct MidnightZSwapTransient<P: ProofKind<D>, D: DB> {
    pub inner: Transient<P::LatestProof, D>,
    _phantom: PhantomData<D>,
}

// Implement Deref for easy access to inner fields
impl<P: ProofKind<D>, D: DB> std::ops::Deref for MidnightZSwapInput<P, D> {
    type Target = Input<P::LatestProof, D>;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<P: ProofKind<D>, D: DB> std::ops::Deref for MidnightZSwapOutput<P, D> {
    type Target = Output<P::LatestProof, D>;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<P: ProofKind<D>, D: DB> std::ops::Deref for MidnightZSwapOffer<P, D> {
    type Target = Offer<P::LatestProof, D>;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<D: DB + Clone> std::ops::Deref for MidnightZSwapIntent<D> {
    type Target = IntentInfo<D>;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<P: ProofKind<D>, D: DB> std::ops::Deref for MidnightZSwapTransient<P, D> {
    type Target = Transient<P::LatestProof, D>;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<P: ProofKind<D>, S: SignatureKind<D>, B: Storable<D>, D: DB> std::ops::Deref
    for MidnightTransaction<P, S, B, D>
{
    type Target = Transaction<S, P, B, D>;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

// Implement DerefMut for mutable access
impl<P: ProofKind<D>, D: DB> std::ops::DerefMut for MidnightZSwapInput<P, D> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl<P: ProofKind<D>, D: DB> std::ops::DerefMut for MidnightZSwapOutput<P, D> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl<P: ProofKind<D>, D: DB> std::ops::DerefMut for MidnightZSwapOffer<P, D> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl<D: DB + Clone> std::ops::DerefMut for MidnightZSwapIntent<D> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl<P: ProofKind<D>, D: DB> std::ops::DerefMut for MidnightZSwapTransient<P, D> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl<P: ProofKind<D>, S: SignatureKind<D>, B: Storable<D>, D: DB> std::ops::DerefMut
    for MidnightTransaction<P, S, B, D>
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

pub type MidnightContractAction<P, D> = ContractAction<P, D>;
pub type MidnightTranscript<D> = Transcript<D>;
pub type MidnightContractDeploy<D> = ContractDeploy<D>;
pub type MidnightContractCall<P, D> = ContractCall<P, D>;
pub type MidnightTransactionResult<D> = TransactionResult<D>;
pub type MidnightSecretKeys = SecretKeys;
pub type MidnightCoinPublicKey = CoinPublicKey;
pub type MidnightEncryptionPublicKey = EncryptionPublicKey;
pub type MidnightNullifier = Nullifier;
pub type MidnightCommitment = Commitment;
pub type MidnightTokenType = TokenType;
pub type MidnightCoinInfo = CoinInfo;
pub type MidnightInputInfo<D> = InputInfo<D>;
pub type MidnightOutputInfo<D> = OutputInfo<D>;
pub type MidnightOfferInfo<D> = OfferInfo<D>;

// Segment ID type
pub type SegmentId = u16;

// Copied from midnight-ledger since it's not exported
// <https://github.com/midnightntwrk/midnight-ledger/blob/ledger-6.1.0-alpha.3/onchain-runtime/src/context.rs#L343-L355>
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Effects {
    pub claimed_nullifiers: HashSet<Nullifier>,
    pub claimed_receives: HashSet<Commitment>,
    pub claimed_spends: HashSet<Commitment>,
    pub claimed_contract_calls: HashSet<(u64, ContractAddress, HashOutput, Fr)>,
    pub mints: HashMap<HashOutput, u64>,
}

// Either type for representing choices
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Either<L, R> {
    Left(L),
    Right(R),
}

// Proof request types for prover server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MidnightProofRequest {
    InputProof {
        segment: SegmentId,
        coin: CoinInfo,
        secret_key: Either<SecretKey, String>, // Either<SecretKey, ContractAddress>
        merkle_path: Vec<[u8; 32]>,
        randomness: Fr, // Fr field element
    },
    OutputProof {
        segment: SegmentId,
        coin: CoinInfo,
        public_key: Either<CoinPublicKey, String>, // Either<PublicKey, ContractAddress>
        randomness: Fr,                            // Fr field element
    },
    BindingProof {
        intent_hash: [u8; 32],
        commitment: Commitment, // Pedersen commitment
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MidnightProofResponse {
    pub proof: Vec<u8>,
    pub public_inputs: Vec<String>,
}

// Constants
pub const SEGMENT_GUARANTEED: SegmentId = 0;
pub const DUST_TOKEN_TYPE: TokenType = TokenType::Dust;

pub fn to_midnight_network_id(network: &str) -> NetworkId {
    match network.to_lowercase().as_str() {
        "mainnet" => NetworkId::MainNet,
        "devnet" => NetworkId::DevNet,
        "testnet" => NetworkId::TestNet,
        _ => NetworkId::Undeployed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use midnight_node_ledger_helpers::NetworkId;

    #[test]
    fn test_to_midnight_network_id() {
        assert_eq!(to_midnight_network_id("mainnet"), NetworkId::MainNet);
        assert_eq!(to_midnight_network_id("MAINNET"), NetworkId::MainNet);
        assert_eq!(to_midnight_network_id("testnet"), NetworkId::TestNet);
        assert_eq!(to_midnight_network_id("TESTNET"), NetworkId::TestNet);
        assert_eq!(to_midnight_network_id("devnet"), NetworkId::DevNet);
        assert_eq!(to_midnight_network_id("DEVNET"), NetworkId::DevNet);
        assert_eq!(to_midnight_network_id("localnet"), NetworkId::Undeployed);
        assert_eq!(to_midnight_network_id("random"), NetworkId::Undeployed);
    }

    #[test]
    fn test_either_enum() {
        let left: Either<i32, String> = Either::Left(42);
        let right: Either<i32, String> = Either::Right("hello".to_string());

        match left {
            Either::Left(val) => assert_eq!(val, 42),
            Either::Right(_) => panic!("Expected Left variant"),
        }

        match right {
            Either::Left(_) => panic!("Expected Right variant"),
            Either::Right(val) => assert_eq!(val, "hello"),
        }
    }

    #[test]
    fn test_either_serialization() {
        let left: Either<i32, String> = Either::Left(42);
        let json = serde_json::to_string(&left).unwrap();
        let deserialized: Either<i32, String> = serde_json::from_str(&json).unwrap();

        match deserialized {
            Either::Left(val) => assert_eq!(val, 42),
            Either::Right(_) => panic!("Expected Left variant after deserialization"),
        }
    }
}
