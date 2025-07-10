use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;

use midnight_ledger_prototype::{
    coin_structure::coin::{Commitment, Nullifier, SecretKey},
    structure::ContractCall,
    transient_crypto::curve::Fr,
};

use midnight_node_ledger_helpers::{
    CoinInfo, CoinPublicKey, ContractAction, ContractAddress, ContractCalls, ContractDeploy,
    EncryptionPublicKey, HashOutput, Input, InputInfo, IntentInfo, NetworkId, Offer, OfferInfo,
    Output, OutputInfo, ProofPreimage, Proofish, SecretKeys, TokenType, Transaction,
    TransactionResult, Transcript, Transient, DB, NATIVE_TOKEN,
};

// Wrapper types for Midnight ZSwap types
#[derive(Debug, Clone)]
pub struct MidnightZSwapInput<P: Proofish<D>, D: DB> {
    pub inner: Input<P::LatestProof>,
    _phantom: PhantomData<D>,
}

#[derive(Debug, Clone)]
pub struct MidnightZSwapOutput<P: Proofish<D>, D: DB> {
    pub inner: Output<P::LatestProof>,
    _phantom: PhantomData<D>,
}

#[derive(Debug, Clone)]
pub struct MidnightZSwapOffer<P: Proofish<D>, D: DB> {
    pub inner: Offer<P::LatestProof>,
    _phantom: PhantomData<D>,
}

impl<P: Proofish<D>, D: DB> MidnightZSwapOffer<P, D> {
    pub fn new(
        inputs: Vec<Input<P::LatestProof>>,
        outputs: Vec<Output<P::LatestProof>>,
        transient: Vec<Transient<P::LatestProof>>,
        deltas: Vec<(TokenType, i128)>,
    ) -> Self {
        Self {
            inner: Offer::<P::LatestProof> {
                inputs,
                outputs,
                transient,
                deltas,
            },
            _phantom: PhantomData,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MidnightTransaction<P: Proofish<D>, D: DB> {
    pub inner: Transaction<P, D>,
    _phantom: PhantomData<D>,
}

impl<D: DB> MidnightTransaction<ProofPreimage, D> {
    pub fn new(
        guaranteed_offer: MidnightZSwapOffer<ProofPreimage, D>,
        fallible_offer: Option<MidnightZSwapOffer<ProofPreimage, D>>,
        contract_calls: Option<ContractCalls<ProofPreimage, D>>,
    ) -> Self {
        Self {
            inner: Transaction::new(
                guaranteed_offer.inner,
                fallible_offer.map(|o| o.inner),
                contract_calls,
            ),
            _phantom: PhantomData,
        }
    }
}

pub struct MidnightZSwapIntent<D: DB> {
    pub inner: IntentInfo<D>,
}

#[derive(Debug, Clone)]
pub struct MidnightZSwapTransient<P: Proofish<D>, D: DB> {
    pub inner: Transient<P>,
    _phantom: PhantomData<D>,
}

// Implement Deref for easy access to inner fields
impl<P: Proofish<D>, D: DB> std::ops::Deref for MidnightZSwapInput<P, D> {
    type Target = Input<P::LatestProof>;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<P: Proofish<D>, D: DB> std::ops::Deref for MidnightZSwapOutput<P, D> {
    type Target = Output<P::LatestProof>;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<P: Proofish<D>, D: DB> std::ops::Deref for MidnightZSwapOffer<P, D> {
    type Target = Offer<P::LatestProof>;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<D: DB> std::ops::Deref for MidnightZSwapIntent<D> {
    type Target = IntentInfo<D>;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<P: Proofish<D>, D: DB> std::ops::Deref for MidnightZSwapTransient<P, D> {
    type Target = Transient<P>;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<P: Proofish<D>, D: DB> std::ops::Deref for MidnightTransaction<P, D> {
    type Target = Transaction<P, D>;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

// Implement DerefMut for mutable access
impl<P: Proofish<D>, D: DB> std::ops::DerefMut for MidnightZSwapInput<P, D> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl<P: Proofish<D>, D: DB> std::ops::DerefMut for MidnightZSwapOutput<P, D> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl<P: Proofish<D>, D: DB> std::ops::DerefMut for MidnightZSwapOffer<P, D> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl<D: DB> std::ops::DerefMut for MidnightZSwapIntent<D> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl<P: Proofish<D>, D: DB> std::ops::DerefMut for MidnightZSwapTransient<P, D> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl<P: Proofish<D>, D: DB> std::ops::DerefMut for MidnightTransaction<P, D> {
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

// Copied from midnight-ledger-prototype since it's not exported
// <https://github.com/midnightntwrk/midnight-ledger-prototype/blob/b315c1d60d97c076e23fa3b6acf3329dde1aa4c4/onchain-runtime/src/context.rs#L197-L203>
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
pub const DUST_TOKEN_TYPE: TokenType = NATIVE_TOKEN;

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
