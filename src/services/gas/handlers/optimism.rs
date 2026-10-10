use crate::{
    constants::{
        DEFAULT_GAS_LIMIT, OPTIMISM_GAS_PRICE_ORACLE_ADDRESS, OPTIMISM_SCALAR_FEE_CACHE_TTL_SECS,
        OPTIMISM_VOLATILE_FEE_CACHE_TTL_SECS,
    },
    domain::evm::PriceParams,
    models::{EvmTransactionData, TransactionError, U256},
    services::provider::evm::EvmProviderTrait,
};
use alloy::{
    primitives::{Address, Bytes, TxKind},
    rpc::types::{TransactionInput, TransactionRequest},
};
use dashmap::DashMap;
use std::{
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};
use tokio::sync::RwLock;
use tracing::debug;

#[derive(Debug, Clone)]
pub struct OptimismFeeData {
    pub l1_base_fee: U256,
    pub base_fee: U256,
    pub decimals: U256,
    pub blob_base_fee: U256,
    pub base_fee_scalar: U256,
    pub blob_base_fee_scalar: U256,
}

/// The volatile half of an oracle read, stamped with when it was fetched.
#[derive(Debug, Clone, Copy)]
struct VolatileFees {
    l1_base_fee: U256,
    base_fee: U256,
    blob_base_fee: U256,
    fetched_at: Instant,
}

/// The quasi-constant half of an oracle read, stamped with when it was fetched.
#[derive(Debug, Clone, Copy)]
struct ScalarFees {
    decimals: U256,
    base_fee_scalar: U256,
    blob_base_fee_scalar: U256,
    fetched_at: Instant,
}

/// The two halves cached for one chain. Each expires on its own TTL, so a volatile miss never
/// forces a re-read of scalars that are still fresh.
#[derive(Debug, Default)]
struct OptimismFeeCacheEntry {
    volatile: Option<VolatileFees>,
    scalars: Option<ScalarFees>,
}

impl OptimismFeeCacheEntry {
    fn fresh_volatile(&self, ttl: Duration) -> Option<VolatileFees> {
        self.volatile.filter(|fees| fees.fetched_at.elapsed() < ttl)
    }

    fn fresh_scalars(&self, ttl: Duration) -> Option<ScalarFees> {
        self.scalars.filter(|fees| fees.fetched_at.elapsed() < ttl)
    }
}

/// Process-wide cache of the OP GasPriceOracle reads, keyed by chain id.
///
/// A handler is constructed per transaction job, so a cache living on the handler instance would
/// never be read twice — it has to outlive the handler. Mirrors the existing
/// [`GasPriceCache`](crate::services::gas::cache::GasPriceCache): a `OnceLock`-held global over a
/// `DashMap` keyed by `chain_id`, with `Instant`-stamped entries. There is no background refresh —
/// an entry is only ever filled by the caller that needed it.
#[derive(Debug)]
pub struct OptimismFeeCache {
    entries: DashMap<u64, Arc<RwLock<OptimismFeeCacheEntry>>>,
    volatile_ttl: Duration,
    scalar_ttl: Duration,
}

impl OptimismFeeCache {
    /// The single instance every handler in this process shares.
    pub fn global() -> &'static Arc<Self> {
        static GLOBAL_CACHE: OnceLock<Arc<OptimismFeeCache>> = OnceLock::new();
        GLOBAL_CACHE.get_or_init(|| {
            Arc::new(Self::new(
                Duration::from_secs(OPTIMISM_VOLATILE_FEE_CACHE_TTL_SECS),
                Duration::from_secs(OPTIMISM_SCALAR_FEE_CACHE_TTL_SECS),
            ))
        })
    }

    /// The TTLs are parameters rather than constants read inline so tests can drive expiry
    /// deterministically instead of sleeping, and so each test gets an isolated instance.
    fn new(volatile_ttl: Duration, scalar_ttl: Duration) -> Self {
        Self {
            entries: DashMap::new(),
            volatile_ttl,
            scalar_ttl,
        }
    }

    /// The per-chain entry, created empty on first use. The `DashMap` guard is dropped immediately —
    /// only the cloned `Arc` is held across the await in `fetch_fee_data_from`.
    fn entry(&self, chain_id: u64) -> Arc<RwLock<OptimismFeeCacheEntry>> {
        self.entries.entry(chain_id).or_default().clone()
    }
}

/// Price parameter handler for Optimism-based networks
/// This calculates L1 data availability costs and adds them as extra fees
#[derive(Debug, Clone)]
pub struct OptimismPriceHandler<P> {
    provider: P,
    oracle_address: Address,
    /// Chain the `provider` is bound to, and therefore the key its oracle reads are cached under.
    chain_id: u64,
}

impl<P: EvmProviderTrait> OptimismPriceHandler<P> {
    pub fn new(provider: P, chain_id: u64) -> Self {
        Self {
            provider,
            oracle_address: OPTIMISM_GAS_PRICE_ORACLE_ADDRESS.parse().unwrap(),
            chain_id,
        }
    }

    // Function selectors for Optimism GasPriceOracle
    // bytes4(keccak256("l1BaseFee()"))
    const FN_SELECTOR_L1_BASE_FEE: [u8; 4] = [81, 155, 75, 211];
    // bytes4(keccak256("baseFee()"))
    const FN_SELECTOR_BASE_FEE: [u8; 4] = [110, 242, 92, 58];
    // bytes4(keccak256("decimals()"))
    const FN_SELECTOR_DECIMALS: [u8; 4] = [49, 60, 229, 103];
    // bytes4(keccak256("blobBaseFee()"))
    const FN_SELECTOR_BLOB_BASE_FEE: [u8; 4] = [248, 32, 97, 64];
    // bytes4(keccak256("baseFeeScalar()"))
    const FN_SELECTOR_BASE_FEE_SCALAR: [u8; 4] = [197, 152, 89, 24];
    // bytes4(keccak256("blobBaseFeeScalar()"))
    const FN_SELECTOR_BLOB_BASE_FEE_SCALAR: [u8; 4] = [104, 213, 220, 166];

    fn create_contract_call(&self, selector: [u8; 4]) -> TransactionRequest {
        let mut data = Vec::with_capacity(4);
        data.extend_from_slice(&selector);
        TransactionRequest {
            to: Some(TxKind::Call(self.oracle_address)),
            input: TransactionInput::from(Bytes::from(data)),
            ..Default::default()
        }
    }

    async fn read_u256(&self, selector: [u8; 4]) -> Result<U256, TransactionError> {
        let call = self.create_contract_call(selector);
        let bytes = self
            .provider
            .call_contract(&call)
            .await
            .map_err(|e| TransactionError::UnexpectedError(e.to_string()))?;
        Ok(U256::from_be_slice(bytes.as_ref()))
    }

    fn calculate_compressed_tx_size(tx: &EvmTransactionData) -> U256 {
        let data_bytes: Vec<u8> = tx
            .data
            .as_ref()
            .and_then(|hex_str| hex::decode(hex_str.trim_start_matches("0x")).ok())
            .unwrap_or_default();

        let zero_bytes = U256::from(data_bytes.iter().filter(|&b| *b == 0).count());
        let non_zero_bytes = U256::from(data_bytes.len()) - zero_bytes;

        (zero_bytes * U256::from(4)) + (non_zero_bytes * U256::from(16))
    }

    async fn read_volatile_fees(&self) -> Result<VolatileFees, TransactionError> {
        let (l1_base_fee, base_fee, blob_base_fee) = tokio::try_join!(
            self.read_u256(Self::FN_SELECTOR_L1_BASE_FEE),
            self.read_u256(Self::FN_SELECTOR_BASE_FEE),
            self.read_u256(Self::FN_SELECTOR_BLOB_BASE_FEE)
        )
        .map_err(|e| TransactionError::UnexpectedError(e.to_string()))?;

        debug!(
            chain_id = self.chain_id,
            "fetched volatile optimism gas oracle values"
        );
        Ok(VolatileFees {
            l1_base_fee,
            base_fee,
            blob_base_fee,
            fetched_at: Instant::now(),
        })
    }

    async fn read_scalar_fees(&self) -> Result<ScalarFees, TransactionError> {
        let (decimals, base_fee_scalar, blob_base_fee_scalar) = tokio::try_join!(
            self.read_u256(Self::FN_SELECTOR_DECIMALS),
            self.read_u256(Self::FN_SELECTOR_BASE_FEE_SCALAR),
            self.read_u256(Self::FN_SELECTOR_BLOB_BASE_FEE_SCALAR)
        )
        .map_err(|e| TransactionError::UnexpectedError(e.to_string()))?;

        debug!(
            chain_id = self.chain_id,
            "fetched optimism gas oracle scalars"
        );
        Ok(ScalarFees {
            decimals,
            base_fee_scalar,
            blob_base_fee_scalar,
            fetched_at: Instant::now(),
        })
    }

    fn assemble_fee_data(volatile: VolatileFees, scalars: ScalarFees) -> OptimismFeeData {
        OptimismFeeData {
            l1_base_fee: volatile.l1_base_fee,
            base_fee: volatile.base_fee,
            decimals: scalars.decimals,
            blob_base_fee: volatile.blob_base_fee,
            base_fee_scalar: scalars.base_fee_scalar,
            blob_base_fee_scalar: scalars.blob_base_fee_scalar,
        }
    }

    /// Reads the GasPriceOracle values needed to price L1 data availability, serving them from the
    /// process-wide cache when they are still fresh.
    pub async fn fetch_fee_data(&self) -> Result<OptimismFeeData, TransactionError> {
        self.fetch_fee_data_from(OptimismFeeCache::global()).await
    }

    /// Cache-aware oracle read. Every one of the six values moves far more slowly than the
    /// transaction rate that reads them, so the warm path issues no `eth_call` at all.
    ///
    /// Single-flight: the per-chain write lock IS the gate. A burst of N cold jobs queues on it, the
    /// first performs the fetch, and the rest find the entry fresh on their re-check and return
    /// without an extra `eth_call`. The cost is that the lock is held across the RPC round trip, so
    /// on the SUCCESS path a burst's worst-case added latency is one refresh — the winner's full
    /// provider attempt cycle (the selector's per-endpoint retries and any failover), not a single
    /// HTTP timeout.
    ///
    /// On the FAILURE path there is no shared result: the loser of the race re-checks, still finds
    /// the entry empty, and runs its own attempt cycle, so a burst of N jobs against a failing
    /// oracle serialises rather than failing in parallel. That is deliberately not re-solved here —
    /// the provider layer already owns it, in the RPC selector's circuit breaker, which pauses a
    /// failing endpoint after a few consecutive failures and makes subsequent attempts fail fast.
    /// A second cooldown here would duplicate that mechanism at the wrong layer. The lock is per
    /// chain id, so a stalled read on one network cannot hold up another.
    async fn fetch_fee_data_from(
        &self,
        cache: &OptimismFeeCache,
    ) -> Result<OptimismFeeData, TransactionError> {
        let entry = cache.entry(self.chain_id);

        {
            let cached = entry.read().await;
            if let (Some(volatile), Some(scalars)) = (
                cached.fresh_volatile(cache.volatile_ttl),
                cached.fresh_scalars(cache.scalar_ttl),
            ) {
                return Ok(Self::assemble_fee_data(volatile, scalars));
            }
        }

        let mut cached = entry.write().await;

        // Each half is re-checked under the write lock (a concurrent job may have refreshed while
        // this one queued) and only what is actually missing is re-read — a volatile miss does not
        // re-read scalars that are still fresh.
        //
        // Scalars are resolved FIRST, and the volatile freshness check is deliberately made after
        // that await rather than alongside it: a scalar read goes through the provider's retry and
        // failover path, which can take longer than the volatile TTL, so a volatile entry that was
        // fresh when the lock was taken could have expired by the time it would be returned. Doing
        // the cheap, quasi-constant half first keeps the volatile read last and its value as young
        // as possible.
        //
        // A failed read propagates with `?` BEFORE anything is written back, so an RPC error can
        // neither poison the entry nor leave a value that was never fetched visible to the next
        // caller: the entry keeps whatever it already held (possibly nothing).
        let scalars = match cached.fresh_scalars(cache.scalar_ttl) {
            Some(scalars) => scalars,
            None => self.read_scalar_fees().await?,
        };
        let volatile = match cached.fresh_volatile(cache.volatile_ttl) {
            Some(volatile) => volatile,
            None => self.read_volatile_fees().await?,
        };

        cached.volatile = Some(volatile);
        cached.scalars = Some(scalars);

        Ok(Self::assemble_fee_data(volatile, scalars))
    }

    pub fn calculate_fee(
        &self,
        fee_data: &OptimismFeeData,
        tx: &EvmTransactionData,
    ) -> Result<U256, TransactionError> {
        // Ecotone cost formula from code:
        // https://github.com/ethereum-optimism/op-geth/blob/0402d543c3d0cff3a3d344c0f4f83809edb44f10/core/types/rollup_cost.go#L188-L219
        //
        // Ecotone L1 cost function:
        //
        //   (calldataGas/16)*(l1BaseFee*16*l1BaseFeeScalar + l1BlobBaseFee*l1BlobBaseFeeScalar)/1e6
        //
        // We divide "calldataGas" by 16 to change from units of calldata gas to "estimated # of bytes when
        // compressed". Known as "compressedTxSize" in the spec.
        //
        // Function is actually computed as follows for better precision under integer arithmetic:
        //
        //   calldataGas*(l1BaseFee*16*l1BaseFeeScalar + l1BlobBaseFee*l1BlobBaseFeeScalar)/16e6

        let calldata_gas_used = Self::calculate_compressed_tx_size(tx);

        let ecotone_divisor = U256::from(1_000_000 * 16);
        let calldata_cost_per_byte = U256::from(fee_data.l1_base_fee)
            .saturating_mul(U256::from(16))
            .saturating_mul(U256::from(fee_data.base_fee_scalar));
        let blob_cost_per_byte = U256::from(fee_data.blob_base_fee)
            .saturating_mul(U256::from(fee_data.blob_base_fee_scalar));
        let fee = calldata_cost_per_byte
            .saturating_add(blob_cost_per_byte)
            .saturating_mul(U256::from(calldata_gas_used))
            .wrapping_div(ecotone_divisor);
        Ok(fee)
    }

    pub async fn handle_price_params(
        &self,
        tx: &EvmTransactionData,
        mut original_params: PriceParams,
    ) -> Result<PriceParams, TransactionError> {
        // Fetch Optimism fee data and calculate L1 data cost
        let fee_data = self.fetch_fee_data().await?;
        let l1_data_cost = self.calculate_fee(&fee_data, tx)?;

        // Add the L1 data cost as extra fee
        original_params.extra_fee = Some(l1_data_cost);

        // Recalculate total cost with the extra fee
        let gas_limit = tx.gas_limit.unwrap_or(DEFAULT_GAS_LIMIT);
        let value = tx.value;
        let is_eip1559 = original_params.max_fee_per_gas.is_some();

        original_params.total_cost =
            original_params.calculate_total_cost(is_eip1559, gas_limit, value);

        Ok(original_params)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::provider::evm::MockEvmProviderTrait;
    use std::sync::atomic::{AtomicUsize, Ordering};

    type TestHandler = OptimismPriceHandler<MockEvmProviderTrait>;

    const BASE_CHAIN_ID: u64 = 8453;
    const OPTIMISM_CHAIN_ID: u64 = 10;

    fn volatile_ttl() -> Duration {
        Duration::from_secs(OPTIMISM_VOLATILE_FEE_CACHE_TTL_SECS)
    }

    fn scalar_ttl() -> Duration {
        Duration::from_secs(OPTIMISM_SCALAR_FEE_CACHE_TTL_SECS)
    }

    /// The six oracle selectors in the order the fields appear on `OptimismFeeData`. A read of the
    /// selector at index `i` answers `i + 1`, so a test can assert both how many reads reached the
    /// RPC and that each one lands in the right field.
    const ORACLE_SELECTORS: [[u8; 4]; 6] = [
        TestHandler::FN_SELECTOR_L1_BASE_FEE,
        TestHandler::FN_SELECTOR_BASE_FEE,
        TestHandler::FN_SELECTOR_DECIMALS,
        TestHandler::FN_SELECTOR_BLOB_BASE_FEE,
        TestHandler::FN_SELECTOR_BASE_FEE_SCALAR,
        TestHandler::FN_SELECTOR_BLOB_BASE_FEE_SCALAR,
    ];

    /// A provider that answers every oracle selector and counts the calls. `yield_now` forces each
    /// read to return `Pending` once, so concurrent callers actually interleave and the single-flight
    /// gate is exercised rather than accidentally serialised by an instantly-ready future.
    fn oracle_provider(calls: Arc<AtomicUsize>) -> MockEvmProviderTrait {
        let mut provider = MockEvmProviderTrait::new();
        provider.expect_call_contract().returning(move |tx| {
            calls.fetch_add(1, Ordering::SeqCst);
            let input = tx.input.input.clone().unwrap_or_default();
            let position = ORACLE_SELECTORS
                .iter()
                .position(|selector| input.starts_with(&selector[..]))
                .expect("call to an unexpected oracle selector");
            let mut word = [0u8; 32];
            word[31] = (position + 1) as u8;
            Box::pin(async move {
                tokio::task::yield_now().await;
                Ok(Bytes::from(word.to_vec()))
            })
        });
        provider
    }

    /// Answers the three scalar selectors normally and rate-limits the three volatile ones, so the
    /// write-lock body takes its mixed path: the half it reads first succeeds, the second fails.
    fn volatile_failing_provider(calls: Arc<AtomicUsize>) -> MockEvmProviderTrait {
        const VOLATILE_SELECTORS: [[u8; 4]; 3] = [
            TestHandler::FN_SELECTOR_L1_BASE_FEE,
            TestHandler::FN_SELECTOR_BASE_FEE,
            TestHandler::FN_SELECTOR_BLOB_BASE_FEE,
        ];
        let mut provider = MockEvmProviderTrait::new();
        provider.expect_call_contract().returning(move |tx| {
            calls.fetch_add(1, Ordering::SeqCst);
            let input = tx.input.input.clone().unwrap_or_default();
            let is_volatile = VOLATILE_SELECTORS
                .iter()
                .any(|selector| input.starts_with(&selector[..]));
            let position = ORACLE_SELECTORS
                .iter()
                .position(|selector| input.starts_with(&selector[..]))
                .expect("call to an unexpected oracle selector");
            let mut word = [0u8; 32];
            word[31] = (position + 1) as u8;
            Box::pin(async move {
                tokio::task::yield_now().await;
                if is_volatile {
                    return Err(crate::services::provider::ProviderError::RateLimited);
                }
                Ok(Bytes::from(word.to_vec()))
            })
        });
        provider
    }

    /// Records the selector of every read in call order, so a test can assert which half of the
    /// oracle is read first.
    fn recording_provider(order: Arc<std::sync::Mutex<Vec<[u8; 4]>>>) -> MockEvmProviderTrait {
        let mut provider = MockEvmProviderTrait::new();
        provider.expect_call_contract().returning(move |tx| {
            let input = tx.input.input.clone().unwrap_or_default();
            let position = ORACLE_SELECTORS
                .iter()
                .position(|selector| input.starts_with(&selector[..]))
                .expect("call to an unexpected oracle selector");
            order
                .lock()
                .expect("order mutex should not be poisoned")
                .push(ORACLE_SELECTORS[position]);
            let mut word = [0u8; 32];
            word[31] = (position + 1) as u8;
            Box::pin(async move {
                tokio::task::yield_now().await;
                Ok(Bytes::from(word.to_vec()))
            })
        });
        provider
    }

    fn failing_provider() -> MockEvmProviderTrait {
        let mut provider = MockEvmProviderTrait::new();
        provider.expect_call_contract().returning(|_| {
            Box::pin(async { Err(crate::services::provider::ProviderError::RateLimited) })
        });
        provider
    }

    /// `OptimismFeeData` is not `PartialEq`, so the mapping is asserted field by field — which is
    /// the point of the test: each value must land in the field its selector answers for.
    fn assert_matches_oracle(fee_data: &OptimismFeeData) {
        assert_eq!(fee_data.l1_base_fee, U256::from(1u64));
        assert_eq!(fee_data.base_fee, U256::from(2u64));
        assert_eq!(fee_data.decimals, U256::from(3u64));
        assert_eq!(fee_data.blob_base_fee, U256::from(4u64));
        assert_eq!(fee_data.base_fee_scalar, U256::from(5u64));
        assert_eq!(fee_data.blob_base_fee_scalar, U256::from(6u64));
    }

    #[tokio::test]
    async fn test_fee_cache_cold_miss_reads_all_six_oracle_values() {
        let calls = Arc::new(AtomicUsize::new(0));
        let handler = OptimismPriceHandler::new(oracle_provider(calls.clone()), BASE_CHAIN_ID);
        let cache = OptimismFeeCache::new(volatile_ttl(), scalar_ttl());

        let fee_data = handler
            .fetch_fee_data_from(&cache)
            .await
            .expect("cold fetch should succeed");

        assert_eq!(calls.load(Ordering::SeqCst), 6);
        assert_matches_oracle(&fee_data);
    }

    #[tokio::test]
    async fn test_fee_cache_warm_hit_reads_nothing() {
        let calls = Arc::new(AtomicUsize::new(0));
        let handler = OptimismPriceHandler::new(oracle_provider(calls.clone()), BASE_CHAIN_ID);
        let cache = OptimismFeeCache::new(volatile_ttl(), scalar_ttl());

        handler
            .fetch_fee_data_from(&cache)
            .await
            .expect("cold fetch should succeed");
        let fee_data = handler
            .fetch_fee_data_from(&cache)
            .await
            .expect("warm fetch should succeed");

        assert_eq!(calls.load(Ordering::SeqCst), 6);
        assert_matches_oracle(&fee_data);
    }

    #[tokio::test]
    async fn test_fee_cache_expired_volatile_does_not_re_read_fresh_scalars() {
        let calls = Arc::new(AtomicUsize::new(0));
        let handler = OptimismPriceHandler::new(oracle_provider(calls.clone()), BASE_CHAIN_ID);
        // A zero volatile TTL expires the trio the instant it is written; the scalars stay fresh.
        let cache = OptimismFeeCache::new(Duration::ZERO, scalar_ttl());

        handler
            .fetch_fee_data_from(&cache)
            .await
            .expect("cold fetch should succeed");
        let fee_data = handler
            .fetch_fee_data_from(&cache)
            .await
            .expect("volatile refresh should succeed");

        assert_eq!(calls.load(Ordering::SeqCst), 9);
        assert_matches_oracle(&fee_data);
    }

    #[tokio::test]
    async fn test_fee_cache_concurrent_cold_callers_fetch_once() {
        let calls = Arc::new(AtomicUsize::new(0));
        let handler = OptimismPriceHandler::new(oracle_provider(calls.clone()), BASE_CHAIN_ID);
        let cache = OptimismFeeCache::new(volatile_ttl(), scalar_ttl());

        let (first, second, third) = tokio::join!(
            handler.fetch_fee_data_from(&cache),
            handler.fetch_fee_data_from(&cache),
            handler.fetch_fee_data_from(&cache)
        );

        assert_eq!(calls.load(Ordering::SeqCst), 6);
        assert_matches_oracle(&first.expect("first concurrent fetch should succeed"));
        assert_matches_oracle(&second.expect("second concurrent fetch should succeed"));
        assert_matches_oracle(&third.expect("third concurrent fetch should succeed"));
    }

    #[tokio::test]
    async fn test_fee_cache_is_not_populated_by_a_failed_read() {
        let handler = OptimismPriceHandler::new(failing_provider(), BASE_CHAIN_ID);
        let cache = OptimismFeeCache::new(volatile_ttl(), scalar_ttl());

        let result = handler.fetch_fee_data_from(&cache).await;
        assert!(result.is_err(), "a failing oracle read must propagate");

        let entry = cache.entry(BASE_CHAIN_ID);
        let cached = entry.read().await;
        assert!(
            cached.volatile.is_none(),
            "failure must not cache volatile values"
        );
        assert!(cached.scalars.is_none(), "failure must not cache scalars");
    }

    /// The write-back is deliberately placed after BOTH halves have been fetched, so a partial
    /// outage leaves the entry completely empty rather than half-populated. Without this test,
    /// moving `cached.scalars = Some(scalars)` up to the scalar match arm — a plausible "write each
    /// half as soon as we have it" refactor — still passes every other test, because the
    /// all-failing provider never gets past the first read.
    #[tokio::test]
    async fn test_fee_cache_is_not_half_populated_when_only_the_volatile_reads_fail() {
        let calls = Arc::new(AtomicUsize::new(0));
        let handler =
            OptimismPriceHandler::new(volatile_failing_provider(calls.clone()), BASE_CHAIN_ID);
        let cache = OptimismFeeCache::new(volatile_ttl(), scalar_ttl());

        let result = handler.fetch_fee_data_from(&cache).await;
        assert!(
            result.is_err(),
            "a failing volatile read must propagate even though the scalar reads succeeded"
        );

        let entry = cache.entry(BASE_CHAIN_ID);
        let cached = entry.read().await;
        assert!(
            cached.scalars.is_none(),
            "a scalar read that succeeded must NOT be written back when the volatile reads failed"
        );
        assert!(
            cached.volatile.is_none(),
            "failure must not cache volatile values"
        );
    }

    /// Pins the read order. The scalar half is resolved first precisely because its read can outlast
    /// the volatile TTL (provider retries and failover), which would otherwise let a volatile value
    /// that was fresh when the lock was taken be returned after expiring.
    #[tokio::test]
    async fn test_fee_cache_reads_the_scalars_before_the_volatile_values() {
        let order = Arc::new(std::sync::Mutex::new(Vec::new()));
        let handler = OptimismPriceHandler::new(recording_provider(order.clone()), BASE_CHAIN_ID);
        let cache = OptimismFeeCache::new(volatile_ttl(), scalar_ttl());

        handler
            .fetch_fee_data_from(&cache)
            .await
            .expect("cold fetch should succeed");

        let order = order.lock().expect("order mutex should not be poisoned");
        let scalars = [
            TestHandler::FN_SELECTOR_DECIMALS,
            TestHandler::FN_SELECTOR_BASE_FEE_SCALAR,
            TestHandler::FN_SELECTOR_BLOB_BASE_FEE_SCALAR,
        ];
        let last_scalar = order
            .iter()
            .rposition(|selector| scalars.contains(selector))
            .expect("the scalars must have been read");
        let first_volatile = order
            .iter()
            .position(|selector| !scalars.contains(selector))
            .expect("the volatile values must have been read");
        assert_eq!(order.len(), 6);
        assert!(
            last_scalar < first_volatile,
            "every scalar read must precede the first volatile read, got {order:?}"
        );
    }

    /// Pins the cache key: one chain serving another chain's oracle values is the worst failure
    /// this cache could have, and without this test every other one uses a single chain id.
    #[tokio::test]
    async fn test_fee_cache_is_keyed_by_chain_id() {
        let calls = Arc::new(AtomicUsize::new(0));
        let base_handler = OptimismPriceHandler::new(oracle_provider(calls.clone()), BASE_CHAIN_ID);
        let optimism_handler =
            OptimismPriceHandler::new(oracle_provider(calls.clone()), OPTIMISM_CHAIN_ID);
        let cache = OptimismFeeCache::new(volatile_ttl(), scalar_ttl());

        base_handler
            .fetch_fee_data_from(&cache)
            .await
            .expect("base cold fetch should succeed");
        assert_eq!(calls.load(Ordering::SeqCst), 6);

        // A different chain is a cold miss of its own, not a hit on the first chain's entry.
        optimism_handler
            .fetch_fee_data_from(&cache)
            .await
            .expect("optimism cold fetch should succeed");
        assert_eq!(calls.load(Ordering::SeqCst), 12);

        // ...and filling the second chain did not evict or disturb the first.
        base_handler
            .fetch_fee_data_from(&cache)
            .await
            .expect("base warm fetch should succeed");
        assert_eq!(calls.load(Ordering::SeqCst), 12);
    }

    #[tokio::test]
    async fn test_optimism_price_handler() {
        let mut mock_provider = MockEvmProviderTrait::new();

        // Mock all the contract calls for Optimism oracle
        mock_provider.expect_call_contract().returning(|_| {
            // Return mock data for oracle calls
            Box::pin(async { Ok(vec![0u8; 32].into()) })
        });

        let handler = OptimismPriceHandler::new(mock_provider, OPTIMISM_CHAIN_ID);

        let tx = EvmTransactionData {
            from: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            to: Some("0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string()),
            value: U256::from(1_000_000_000_000_000_000u128),
            data: Some("0x1234567890abcdef".to_string()),
            gas_limit: Some(21000),
            gas_price: Some(20_000_000_000),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            speed: None,
            nonce: None,
            chain_id: 10, // Optimism chain ID
            hash: None,
            signature: None,
            raw: None,
        };

        let original_params = PriceParams {
            gas_price: Some(20_000_000_000),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            is_min_bumped: None,
            extra_fee: None,
            total_cost: U256::ZERO,
        };

        let result = handler.handle_price_params(&tx, original_params).await;

        assert!(result.is_ok());
        let handled_params = result.unwrap();

        // Gas price should remain unchanged for Optimism (only extra fee is added)
        assert_eq!(handled_params.gas_price, Some(20_000_000_000));

        // Extra fee should be added
        assert!(handled_params.extra_fee.is_some());

        // Total cost should be recalculated
        assert!(handled_params.total_cost > U256::ZERO);
    }

    #[test]
    fn test_calculate_compressed_tx_size() {
        // Test with empty data
        let empty_tx = EvmTransactionData {
            from: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            to: Some("0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string()),
            value: U256::from(1_000_000_000_000_000_000u128),
            data: None,
            gas_limit: Some(21000),
            gas_price: Some(20_000_000_000),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            speed: None,
            nonce: None,
            chain_id: 10,
            hash: None,
            signature: None,
            raw: None,
        };

        let size =
            OptimismPriceHandler::<MockEvmProviderTrait>::calculate_compressed_tx_size(&empty_tx);
        assert_eq!(size, U256::ZERO);

        // Test with data containing zeros and non-zeros
        let data_tx = EvmTransactionData {
            data: Some("0x00001234".to_string()), // 2 zero bytes, 2 non-zero bytes
            ..empty_tx
        };

        let size =
            OptimismPriceHandler::<MockEvmProviderTrait>::calculate_compressed_tx_size(&data_tx);
        // Expected: ((2 * 4) + (2 * 16)) / 16 = (8 + 32) / 16 = 40 / 16 = 2.5 -> 2 (integer division)
        let expected = U256::from(2) * U256::from(4) + U256::from(2) * U256::from(16);
        assert_eq!(size, expected);
    }

    #[test]
    fn test_calculate_fee_with_specific_data_and_fee_data() {
        let mock_provider = MockEvmProviderTrait::new();
        let handler = OptimismPriceHandler::new(mock_provider, OPTIMISM_CHAIN_ID);

        let fee_data = OptimismFeeData {
            l1_base_fee: U256::from(422079632u64),
            base_fee: U256::from(138u64),
            decimals: U256::from(6u64),
            blob_base_fee: U256::from(2u64),
            base_fee_scalar: U256::from(5227u64),
            blob_base_fee_scalar: U256::from(1014213u64),
        };

        let tx = EvmTransactionData {
            from: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            to: Some("0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string()),
            value: U256::ZERO,
            data: Some("0xaf524e5852ba824bfabc2bcfcdf7f0edbb486ebb05e1836c90e78047efeb949990f72e5f00000000000000000000000000000000000000000000000000000000000000600f0b4ec422bb6297b5ded2971c583488bc1a714a2b11201bb32988080dec689b0000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000000000000002000000000000000000000000000000000000000000000000000000000000000653636313431353161306633613839373337393633303932650000000000000000363831306565633032393766616339373337303865316531000000000000000000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000de0b6b3a76400000000000000000000000000000000000000000000000000000000000000000014cab1a2f55e1c3f8905f46c0f9f73746b7fc160c5000000000000000000000000".to_string()),
            gas_limit: Some(21000),
            gas_price: Some(20_000_000_000),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            speed: None,
            nonce: None,
            chain_id: 10,
            hash: None,
            signature: None,
            raw: None,
        };

        let result = handler.calculate_fee(&fee_data, &tx);
        assert!(result.is_ok());
        let fee = result.unwrap();
        assert_eq!(fee, U256::from(7342268088u64));
    }
}
