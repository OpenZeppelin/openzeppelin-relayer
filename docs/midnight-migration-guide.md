# Midnight Support — Migration Guide

> Reference document for rebuilding Midnight support from a fresh branch,
> using learnings from `plat-6527-midnight-support` (June–November 2025).
>
> **Branch dependency:** `midnight-node` tag `node-0.12.0`
> **Current latest:** `node-0.22.3` stable, `node-1.0.0-rc.1` (mainnet RC)
> **Midnight mainnet:** Live since March 30, 2026 (Kukolu phase, federated PoA)

---

## 1. Architecture Overview

The branch adds Midnight as a fourth network alongside EVM, Solana, and Stellar.
It follows the same layered pattern used by all networks:

```
Config (JSON) → ConfigFile models → RepoModels → Domain models → Services
```

Each layer uses **tagged enums** for polymorphic dispatch (no trait objects):

```
NetworkFileConfig::Midnight(MidnightNetworkConfig)
NetworkConfigData::Midnight(MidnightNetworkConfig)
NetworkRelayer::Midnight(DefaultMidnightRelayer)
NetworkTransaction::Midnight(DefaultMidnightTransaction)
NetworkTransactionRequest::Midnight(MidnightTransactionRequest)
NetworkSigner::Midnight(MidnightSigner)
```

This is a proven pattern — replicate it exactly.

---

## 2. Module-by-Module Assessment

### Legend
- **Reuse** — Copy with minor adjustments (no midnight-node dependency)
- **Adapt** — Good structure, needs updating for new API
- **Rewrite** — Heavily coupled to old midnight-node types

---

### 2.1 Config & Models (Reuse)

These modules have **no dependency on midnight-node crates** and can be carried over
with minimal changes.

| File | What it does | Notes |
|------|-------------|-------|
| `src/config/config_file/network/midnight.rs` | `MidnightNetworkConfig` — network, indexer_urls, prover_url, commitment_tree_ttl | Validate network_id: was testnet-only, now needs mainnet support |
| `src/models/network/midnight/network.rs` | `MidnightNetwork` runtime model | Add any new fields from 1.0 |
| `src/models/relayer/config.rs` | `ConfigFileRelayerMidnightPolicy` (only `min_balance`) | May need gas/fee fields if 1.0 introduces fees |
| `src/models/relayer/mod.rs` | `RelayerMidnightPolicy`, `RelayerNetworkType::Midnight` | Straightforward |
| `src/models/transaction/request/midnight.rs` | `MidnightTransactionRequest` — guaranteed_offer, intents, fallible_offers, ttl | Verify offer/intent model matches 1.0 transaction structure |
| `src/models/transaction/response.rs` | Response types with Midnight variants | Straightforward |
| `config/networks/midnight.json` | Default network config | Update RPC/indexer URLs for mainnet |

**Integration points** (enum variants to add in each file):
- `src/config/config_file/network/mod.rs` — `NetworkFileConfig::Midnight`
- `src/models/network/mod.rs` / `repository.rs` — `NetworkConfigData::Midnight`
- `src/models/transaction/request/mod.rs` — `NetworkTransactionRequest::Midnight`
- `src/models/relayer/mod.rs` — `RelayerNetworkPolicy::Midnight`

### 2.2 Address Model (Adapt)

| File | Status | Notes |
|------|--------|-------|
| `src/models/address/midnight/address.rs` | **Adapt** | Uses bech32m encoding with HRP `mn_shield-addr_<network>`. Depends on `midnight_node_ledger_helpers` for `Wallet`, `WalletSeed`, `NetworkId`. Address format itself (bech32m) may be stable, but wallet derivation API likely changed. |

**Key pattern:** Address = bech32m(coin_public_key ++ encryption_public_key).
Verify this derivation path still holds in 1.0.

### 2.3 Provider (Rewrite)

| File | Status | Notes |
|------|--------|-------|
| `src/services/provider/midnight/mod.rs` | **Rewrite** | Core Subxt interaction. Uses `OnlineClient<PolkadotConfig>`, `mn_meta::tx().midnight().send_mn_transaction()`. Subxt metadata is auto-generated from runtime — guaranteed to break between 0.12 and 1.0. |
| `src/services/provider/midnight/remote_prover.rs` | **Adapt** | HTTP client to `/prove-tx`. Protocol may have changed. Has panic on empty response (fix this). |

**What to preserve:**
- `MidnightProviderTrait` interface (get_balance, get_block_number, send_transaction, health_check, get_nonce)
- RPC selector + retry pattern (framework-level, no midnight-node dependency)
- Error classification (retriable vs provider-failure)
- `TransactionSubmissionResult` with dual hash tracking (extrinsic + pallet)

**What will break:**
- `mn_meta::api` — entire Subxt metadata namespace
- Transaction serialization: `midnight_node_ledger_helpers::serialize()`
- `Transaction<Proof, DefaultDB>` type signature
- Extrinsic construction: `create_unsigned()` / `validate()` / `submit()` flow

**Recommended approach:**
1. Pull `midnight-node` at `node-1.0.0-rc.1` (or latest stable)
2. Regenerate Subxt metadata bindings
3. Identify the equivalent of `send_mn_transaction` in new pallets
4. Rebuild serialization/submission pipeline

### 2.4 Signer (Adapt)

| File | Status | Notes |
|------|--------|-------|
| `src/services/signer/midnight/local_signer.rs` | **Adapt** | Ed25519 signing via `WalletSeed` → `SigningKey`. Core crypto likely stable, but `Wallet<DefaultDB>` construction may differ. |
| `src/services/signer/midnight/mod.rs` | **Reuse** | `MidnightSigner` enum (Local/Vault), trait definition. |

**Key pattern:** 32-byte seed → Ed25519 SigningKey → SHA-256 hash → sign → 64-byte signature.
Verify this signing scheme is unchanged in 1.0.

**Limitation:** Only `SignerConfig::Local` is supported. No AWS KMS, Vault, Turnkey, GCP KMS.
Consider if KMS support is needed for mainnet.

### 2.5 Transaction Domain (Rewrite)

| File | Status | Notes |
|------|--------|-------|
| `src/domain/transaction/midnight/midnight_transaction.rs` (~2100 lines) | **Rewrite** | Core transaction lifecycle. Heavily uses `LedgerContext<DefaultDB>`, `WalletSeed`, `StandardTrasactionInfo`, proof generation. |
| `src/domain/transaction/midnight/builder.rs` | **Rewrite** | `MidnightTransactionBuilder<D: DB>` — wraps midnight-node's transaction construction API. |
| `src/domain/transaction/midnight/types.rs` | **Adapt** | Wrapper types around midnight-node types. `ApplyStage` enum (from indexer) is likely stable. |

**What to preserve (patterns, not code):**
- Transaction lifecycle: prepare → submit → check status
- `prepare_transaction()`: incremental wallet sync → proof generation → serialize
- `submit_transaction()`: deserialize → send via provider → store dual hashes → enqueue status job
- `handle_transaction_status()`: query indexer → map ApplyStage → update repo model
- Fee calculation heuristic (~61k base + 20k per extra I/O) — **verify against 1.0**
- Change output always created (ensures indexer visibility via viewing key)

**What will break:**
- All `midnight_node_ledger_helpers` types: `InputInfo`, `OutputInfo`, `OfferInfo`, `StandardTrasactionInfo`
- `LedgerContext<DefaultDB>` — wallet + ledger state container
- Proof generation: `StandardTrasactionInfo::prove()` + `well_formed()` validation
- Serialization/deserialization of transaction data

### 2.6 Relayer Domain (Adapt)

| File | Status | Notes |
|------|--------|-------|
| `src/domain/relayer/midnight/midnight_relayer.rs` (~1000 lines) | **Adapt** | Orchestration layer. Most logic is framework-level (job enqueueing, health checks, status). Depends on midnight-node only through SyncManager and provider. |

**What to preserve:**
- `process_transaction_request()` — creates repo model, enqueues job
- `check_health()` — sync nonce, disable on failure with notification
- `get_status()` — returns `RelayerStatus::Midnight { balance, pending_count, nonce }`
- `initialize_relayer()` — genesis sync (index 0), nonce sync
- Unsupported operations: cancel, sign_data, sign_typed_data, rpc

**Key design decision:** Nonce = blockchain ledger index (not account nonce like EVM).
`sync_nonce()` fetches block height from provider, converts to next sequence.

### 2.7 Sync / Indexer (Adapt)

| File | Status | Notes |
|------|--------|-------|
| `src/services/sync/midnight/indexer/client.rs` (~580 lines) | **Adapt** | GraphQL client (HTTP + WebSocket). Indexer API moved to v4 — queries likely need updating. |
| `src/services/sync/midnight/indexer/types.rs` | **Adapt** | `WalletSyncEvent`, `ZswapChainStateUpdate`, `ApplyStage`, `ViewingKeyFormat`. Verify against current indexer schema. |
| `src/services/sync/midnight/handler/manager.rs` (~665 lines) | **Adapt** | `SyncManager` — coordinates sync, event dispatch, state persistence. Uses `LedgerContext` (needs updating). |
| `src/services/sync/midnight/handler/strategy.rs` | **Adapt** | `QuickSyncStrategy` — connects wallet, subscribes to updates. Protocol may differ. |
| `src/services/sync/midnight/handler/tracker.rs` | **Reuse** | Progress tracking, likely unchanged. |
| `src/services/sync/midnight/handler/events.rs` | **Adapt** | Event dispatch, applies merkle + tx updates to context. |

**Key pattern:** QuickSync trusts indexer with viewing key (read-only) for fast sync.
Alternative: full ledger download (much slower, more private).

**GraphQL operations to verify against current indexer:**
- `connect_wallet(viewing_key: ViewingKey!)` → session ID
- `subscribe_to_wallet_updates(session_id, start_index?, send_progress_updates?)` → `WalletSyncEvent` stream

### 2.8 Relayer State Repository (Reuse)

| File | Status | Notes |
|------|--------|-------|
| `src/repositories/relayer_state/mod.rs` | **Reuse** | `SyncStateTrait` — generic interface for sync state persistence. No midnight-node dependency. |
| `src/repositories/relayer_state/relayer_state_in_memory.rs` | **Reuse** | In-memory implementation for testing. |
| `src/repositories/relayer_state/relayer_state_redis.rs` | **Reuse** | Redis implementation for production. |

**Why this exists:** Midnight requires ZK proof context (merkle tree state).
Without persistence, restart = full re-sync from genesis. Storing
`last_synced_index` + serialized ledger context enables incremental sync.

**Trait interface (stable, reusable):**
```rust
trait SyncStateTrait: Send + Sync {
    async fn get_last_synced_index(relayer_id) -> Option<u64>;
    async fn get_ledger_context(relayer_id) -> Option<Vec<u8>>;
    async fn set_last_synced_index(relayer_id, index);
    async fn set_ledger_context(relayer_id, context: Vec<u8>);
    async fn update_if_greater(relayer_id, index) -> bool;  // Atomic
    async fn reset(relayer_id);
}
```

### 2.9 Bootstrap & API Integration (Reuse)

| File | Status | Notes |
|------|--------|-------|
| `src/bootstrap/config_processor.rs` | **Reuse** | Midnight variant in config processing |
| `src/bootstrap/initialize_app_state.rs` | **Reuse** | App state initialization with Midnight |
| `src/bootstrap/initialize_relayers.rs` | **Reuse** | Relayer initialization dispatch |
| `src/bootstrap/initialize_workers.rs` | **Reuse** | Worker initialization |
| `src/api/controllers/*.rs` | **Reuse** | API controllers (Midnight variants in enums) |
| `src/jobs/handlers/*.rs` | **Reuse** | Job handlers (Midnight dispatch) |
| `src/models/app_state.rs` | **Reuse** | AppState with RelayerStateRepository |

### 2.10 Docs & Examples (Adapt)

| File | Status | Notes |
|------|--------|-------|
| `docs/midnight.mdx` | **Adapt** | Update for mainnet, new API, new config |
| `examples/midnight-basic-example/` | **Adapt** | Update docker-compose, config for mainnet node/indexer/prover |
| `Dockerfile.midnight.development` | **Adapt** | Update base images, midnight-node version |
| `scripts/fixtures/generate_midnight_fixtures.*` | **Adapt** | Update for new types |

---

## 3. Key Architectural Decisions to Preserve

### 3.1 Dual Hash Tracking
Midnight stores both **extrinsic_tx_hash** (Substrate level) and **pallet_tx_hash**
(application level). Status queries use pallet hash via indexer, not extrinsic hash.

### 3.2 Remote Proof Server
ZK proofs are offloaded to a Docker-based proof server (`midnightnetwork/proof-server`).
The relayer sends transaction data + circuit keys, receives proven transaction back.
This keeps the relayer stateless w.r.t. proof generation.

### 3.3 Incremental Sync via Viewing Key
`QuickSyncStrategy` shares a read-only viewing key with the indexer to receive
only transactions relevant to the relayer's wallet. This is a deliberate
privacy/performance tradeoff.

### 3.4 Ledger Context Persistence
The serialized `LedgerContext` (wallet state + merkle tree) is persisted in Redis
via `RelayerStateRepository`. This avoids full genesis re-sync on restart.
Context is serialized with bincode.

### 3.5 Fee Calculation Heuristic
`Wallet::calculate_fee()` overestimates. The branch uses an empirical formula:
~61,000 base + ~20,000 per additional input/output. **Must be re-validated for 1.0.**

### 3.6 Change Output Requirement
A change output is always created even when balance exactly matches.
This ensures the indexer (filtering by viewing key) recognizes the transaction
as relevant to the sender wallet.

### 3.7 Nonce = Ledger Index
Unlike EVM account nonces, Midnight "nonce" is the blockchain ledger index
(merkle tree height). `sync_nonce()` fetches this from the provider and
converts to the relayer's next sequence number.

### 3.8 No Cancellation Support
Midnight transactions cannot be cancelled or replaced (no nonce-based replacement).
The cancel endpoint returns an error.

---

## 4. Dependency Changes (Investigated)

### 4.1 Crate Reorganization

The midnight-node repo structure has changed significantly. The relayer depends on
three sources, all of which have breaking changes:

#### `midnight-node-ledger-helpers` — STILL EXISTS, heavily refactored

**Old (node-0.12.0):** Single module, flat exports.
**New (node-1.0.0-rc.1):** Dual-version support (ledger 7 + ledger 8), with
`pub use ledger_8 as latest; pub use latest::*;`

Key changes:
- Internal structure now under `versions/common/` with separate files for
  `context.rs`, `wallet/`, `input.rs`, `output.rs`, `offer.rs`, `transaction.rs`
- Feature flags: `can-panic` (required for most wallet/context types), `erase-proof`, `test-utils`
- Edition upgraded to 2024

#### `midnight-node-res` — subxt_metadata REMOVED from this crate

**Old:** Exported `subxt_metadata::api` with pallet bindings.
**New:** Only exports config functions, serialization helpers, test data.

Subxt metadata moved to **NEW crate `midnight-node-metadata`** (package in same repo):
- Versioned APIs: `midnight_metadata_0_21_0`, `midnight_metadata_0_22_0`, `midnight_metadata_1_0_0`
- `pub use midnight_metadata_1_0_0 as midnight_metadata_latest;`
- Uses **subxt 0.44.0**

#### `midnight-ledger-prototype` — REPO GONE (404)

The relayer also depends on `midnight-ledger-prototype` at tag `ledger-4.0.0` for
`coin_structure::coin::{Commitment, Nullifier, SecretKey}`, `structure::ContractCall`,
and `transient_crypto::curve::Fr`.

This repo no longer exists. These types now live in `midnight-ledger` (public repo):
- `midnight-coin-structure` v2.0.1 (on crates.io)
- `midnight-transient-crypto` v2.0.1 (on crates.io)
- `midnight-ledger` v8.0.3 (on crates.io)

### 4.2 Type-by-Type Migration Map

| Type (old branch) | Status in 1.0 | Location | Breaking Changes |
|---|---|---|---|
| `LedgerContext<DefaultDB>` | **EXISTS** | `midnight-node-ledger-helpers` (needs `can-panic` feature) | `new_from_wallet_seeds()` now takes `(network_id, wallet_seeds)` — added first param |
| `DefaultDB` | **EXISTS** | `midnight-storage-core` (on crates.io) | Type alias for `db::InMemoryDB<sha2::Sha256>`, unchanged |
| `WalletSeed` | **EXISTS** | `midnight-node-ledger-helpers` | `Default` impl REMOVED (node-0.22.0). Enum: `Short([u8;16])`, `Medium([u8;32])`, `Long([u8;64])` |
| `Wallet<D>` | **EXISTS** | `midnight-node-ledger-helpers` | Now contains `root_seed`, `shielded`, `unshielded`, `dust` subwallets |
| `WalletKind` | **EXISTS** | `midnight-node-ledger-helpers` | Unchanged: `Legacy`, `NoLegacy` |
| `NetworkId` | **CHANGED** | `midnight-node-ledger-helpers` | Was enum (`MainNet`, `TestNet`, `DevNet`, `Undeployed`) → now String-based. Core ledger uses plain `String`. **~15 files affected.** |
| `Transaction<Proof, DefaultDB>` | **EXISTS** | `midnight-ledger` (on crates.io) | Now enum: `Standard`, `ClaimRewards`. Type params may differ. |
| `StandardTrasactionInfo` | **EXISTS** | `midnight-node-ledger-helpers` | Typo preserved. Now holds: intent mappings, offers, RNG, prover, funding seeds, dust registrations |
| `InputInfo<O>` | **EXISTS** | `midnight-node-ledger-helpers` | `token_type` may be `ShieldedTokenType` instead of `TokenType` |
| `OutputInfo<D>` | **EXISTS** | `midnight-node-ledger-helpers` | `token_type` → `ShieldedTokenType` |
| `OfferInfo<D>` | **CHANGED** | `midnight-node-ledger-helpers` | Now uses **trait objects**: `inputs: Vec<Box<dyn BuildInput<D>>>`, `outputs: Vec<Box<dyn BuildOutput<D>>>`, `transients: Vec<Box<dyn BuildTransient<D>>>`. Builder API change. |
| `NATIVE_TOKEN` | **RENAMED** | `midnight-coin-structure` | Now exported as `NIGHT` |
| `serialize()` / `deserialize()` | **RENAMED** | `midnight-node-ledger-helpers` | Now `serialize_untagged()` / `deserialize_untagged()`. Tagged variants may be removed. |
| `Proof` / `ProofMarker` | **EXISTS** | `midnight-ledger` | In `structure.rs` |
| `coin_structure::coin::*` | **MOVED** | Was in `midnight-ledger-prototype` (dead repo) → now `midnight-coin-structure` v2.0.1 on crates.io |
| `transient_crypto::curve::Fr` | **MOVED** | Was in `midnight-ledger-prototype` → now `midnight-transient-crypto` v2.0.1 on crates.io |

### 4.3 Cargo.toml Dependency Changes

**Old:**
```toml
midnight-node-ledger-helpers = { git = "https://github.com/midnightntwrk/midnight-node", package = "midnight-node-ledger-helpers", tag = "node-0.12.0" }
midnight-node-res = { git = "https://github.com/midnightntwrk/midnight-node", package = "midnight-node-res", tag = "node-0.12.0" }
midnight-ledger-prototype = { git = "...", tag = "ledger-4.0.0" }  # REPO DELETED
```

**New (target):**
```toml
# From midnight-node repo (git dependency, not on crates.io)
midnight-node-ledger-helpers = { git = "https://github.com/midnightntwrk/midnight-node", package = "midnight-node-ledger-helpers", tag = "node-1.0.0-rc.1", features = ["can-panic"] }
midnight-node-metadata = { git = "https://github.com/midnightntwrk/midnight-node", package = "midnight-node-metadata", tag = "node-1.0.0-rc.1" }

# From crates.io (replaces midnight-ledger-prototype)
midnight-coin-structure = "2.0.1"
midnight-storage-core = "1.1.0"
midnight-transient-crypto = "2.0.1"

# Subxt (must match midnight-node's version)
subxt = "0.44.0"
```

### 4.4 Node Breaking Changes by Version

| Version | Date | Key Changes |
|---------|------|-------------|
| **node-0.18.0** | Dec 2025 | `NetworkId` changed from enum to String. Fee mechanism overhauled to use DUST token. Chain ID prefix changed to `midnight_`. |
| **node-0.22.0** | Mar 2026 | Ledger upgraded 7 → 8.0.2. `WalletSeed::Default` REMOVED. On-disk storage format v2 (incompatible). Race condition fix in `LedgerContext::update_from_tx`. |
| **node-1.0.0-rc.1** | Apr 2026 | Dust address prefix `dust-addr` → `dust` (untagged serialization). Fallible contract calls + fallible inputs added. Multiple shielded coin inputs support. |

### 4.5 Indexer API v4 Changes

Current version: **Indexer 4.0.1, API v4**

Endpoints:
- HTTP: `POST https://<host>:<port>/api/v4/graphql`
- WebSocket: `wss://<host>:<port>/api/v4/graphql/ws` (Protocol: `graphql-transport-ws`)

**The relayer's indexer client hardcodes no API version path.** URLs in config must
include `/api/v4/graphql` or the client must append it.

#### Subscription Rename (BREAKING)

**Old (relayer's current code):**
```graphql
subscription WalletSync {
    wallet(sessionId: "...", index: ..., sendProgressUpdates: ...) { ... }
}
```

**New (v4):**
```graphql
subscription {
    shieldedTransactions(sessionId: HexEncoded!, index: Int, sendProgressUpdates: Boolean): ShieldedTransactionsEvent!
}
```

Return type is now `ShieldedTransactionsEvent` (union of `ViewingUpdate` and
`ShieldedTransactionsProgress`), replacing the old union naming.

#### Transaction Status Model (BREAKING)

**Old:** `applyStage` field with `ApplyStage` enum: `Pending | SucceedEntirely | SucceedPartially | FailEntirely`

**New:** `transactionResult` field with:
```graphql
transactionResult {
    status: TransactionResultStatus  # SUCCESS | PARTIAL_SUCCESS | FAILURE
    segments: [...]
}
```

New `Transaction` type also includes:
- `fees { paidFees, estimatedFees }`
- `contractActions`
- `unshieldedCreatedOutputs` / `unshieldedSpentOutputs`

#### Mutation Changes

`connect_wallet` → `connect` (same params: `viewingKey: ViewingKey!`, returns session ID)
New: `disconnect(sessionId: HexEncoded!): Unit!` — relayer should call this on cleanup.

#### Query Changes

- `TransactionOffset` is now a **oneOf** input: `{ hash: HexEncoded }` or `{ identifier: HexEncoded }`
- `BlockOffset` is now a **oneOf** input: `{ hash: HexEncoded }` or `{ height: Int }`
- Scalar types changed: `String` → `HexEncoded` for hashes/addresses/session IDs

#### New Subscriptions (available but not required)

- `blocks(offset: BlockOffset): Block!`
- `contractActions(address: HexEncoded!, offset: BlockOffset): ContractAction!`
- `unshieldedTransactions(address: UnshieldedAddress!, ...): UnshieldedTransactionsEvent!`
- `dustLedgerEvents(id: Int): DustLedgerEvent!`
- `zswapLedgerEvents(id: Int): ZswapLedgerEvent!`

#### Indexer Files to Update

| File | Required Changes |
|------|-----------------|
| `src/services/sync/midnight/indexer/client.rs` | Rename `wallet` → `shieldedTransactions` subscription. Update `transactions` query for new `TransactionOffset`/`HexEncoded` types. Use `transactionResult.status` instead of `applyStage`. Add `/api/v4/graphql` path. Add `disconnect` mutation. |
| `src/services/sync/midnight/indexer/types.rs` | Replace `ApplyStage` enum with `TransactionResultStatus { Success, PartialSuccess, Failure }`. Update `WalletSyncEvent` to match `ShieldedTransactionsEvent`. |
| `src/services/sync/midnight/handler/events.rs` | Update event conversion for renamed types. |
| `src/domain/transaction/midnight/types.rs` | Replace `ApplyStage` usage with new status enum. |
| `src/domain/transaction/midnight/midnight_transaction.rs` | Update `handle_transaction_status()` for new status model. |

### 4.6 Proof Server Changes

**Old (relayer's reference):** Unknown version, image `midnightnetwork/proof-server`
**Current:** Version **8.0.3**, image `midnightntwrk/proof-server:8.0.3`

Breaking changes in proof server 4.0.0 (May 2025):
- Migrated from **Pluto-Eris to BLS12-381** cryptographic scheme
- Segment IDs added to Zswap constructors (1=fallible, 0=guaranteed)
- `ZswapLocalStateNoKeys` → `ZswapLocalState`
- Transitioned to **data providers** instead of direct prover keys/parameters
- New env var: `MIDNIGHT_PARAM_SOURCE` for key material source override
- Objects must be `Storable` for MPT leaf storage

The serialization format sent to `/prove-tx` has changed due to the BLS12-381
migration. The remote prover client needs testing/updating.

### 4.7 Midnight Ledger Version History

The core ledger has gone through 5 major versions since the branch was written:

| Version | Key Changes |
|---------|-------------|
| **ledger-4** (branch uses this, from deleted `midnight-ledger-prototype`) | Pre-Dust, old proof system |
| **ledger-6** | Dust replaced shielded token 0 as fee; events overhaul; `network_id` String on `LedgerState`/`Transaction`; `ProvingProvider` abstraction; removed `inputFeeOverhead`/`outputFeeOverhead` |
| **ledger-6.1** | Real cost model with multi-dimensional costs replacing single gas cost |
| **ledger-7** | Breaking proof-system changes; system transactions/treasury |
| **ledger-8** (current) | Merkle tree canonicity fix; Dust registration changes; transcript partitioning; Rust edition 2024 |

### 4.8 Component Version Matrix (Current)

| Component | Version | Notes |
|-----------|---------|-------|
| Ledger | 8.0.3 | Core types, on crates.io |
| Node | 0.22.3 (stable) / 1.0.0-rc.1 (RC) | Git dependency for ledger-helpers |
| Proof Server | 8.0.3 | Docker: `midnightntwrk/proof-server:8.0.3` |
| Indexer | 4.0.1 | API v4, GraphQL |
| Midnight.js | 4.0.2 | TypeScript SDK (reference only) |
| Subxt | 0.44.0 | Must match node's version |

---

## 5. Recommended Implementation Order

### Phase 0: Dependencies Setup
1. Update `Cargo.toml`:
   - Replace `midnight-node` tag `node-0.12.0` → `node-1.0.0-rc.1` (or latest stable)
   - Replace `midnight-node-res` with `midnight-node-metadata` for subxt bindings
   - Replace `midnight-ledger-prototype` (dead repo) with crates.io deps:
     `midnight-coin-structure`, `midnight-storage-core`, `midnight-transient-crypto`
   - Update `subxt` to `0.44.0`
   - Add `features = ["can-panic"]` to `midnight-node-ledger-helpers`
2. Verify the project compiles with new deps (expect type errors — this validates connectivity)

### Phase 1: Foundation (Reuse-heavy, no midnight-node types)
3. Config parsing (`config/network/midnight.rs`) — add `"mainnet"` as valid network_id
4. Network models (`models/network/midnight/`)
5. Relayer config & policy models — consider if Dust fee fields needed
6. Transaction request models — verify offer/intent/fallible structure matches 1.0
7. Relayer state repository (copy as-is)
8. Bootstrap & API integration points (enum variants)

### Phase 2: Core Type Adaptation
9. Fix `NetworkId` usage everywhere (~15 files): enum variants → String-based API
10. Fix `LedgerContext::new_from_wallet_seeds()` calls: add `network_id` first param (6 call sites)
11. Fix `WalletSeed` usage: no more `Default` impl, explicit construction required
12. Fix `NATIVE_TOKEN` → `NIGHT` rename
13. Fix `serialize()`/`deserialize()` → `serialize_untagged()`/`deserialize_untagged()`
14. Address model — verify bech32m derivation, update `dust-addr` → `dust` prefix

### Phase 3: Services
15. Signer — verify Ed25519 scheme, update `Wallet<DefaultDB>` construction
16. Provider — replace `midnight-node-res::subxt_metadata` with `midnight-node-metadata::midnight_metadata_latest`
17. Provider — rebuild Subxt pallet call (`send_mn_transaction` equivalent in new metadata)
18. Remote prover — test `/prove-tx` with BLS12-381 serialization, update `data providers` pattern
19. Indexer client — apply all v4 changes:
    - Rename `wallet` → `shieldedTransactions` subscription
    - Replace `applyStage` → `transactionResult.status`
    - Update scalar types (`String` → `HexEncoded`)
    - Add `/api/v4/graphql` path handling
    - Add `disconnect` mutation

### Phase 4: Domain Logic
20. Transaction types — update `OfferInfo` for trait object API (`BuildInput`/`BuildOutput`/`BuildTransient`)
21. Transaction builder — adapt to new `StandardTrasactionInfo` fields (intent mappings, dust registrations)
22. Transaction lifecycle (prepare/submit/status) — update status mapping for `TransactionResultStatus`
23. Sync manager — update `LedgerContext` handling, verify serialization format (bincode still valid?)
24. Relayer domain — adapt nonce/balance logic, verify Dust fee handling

### Phase 5: Polish
25. Update docs for mainnet
26. Update docker-compose: image `midnightntwrk/proof-server:8.0.3`, node version, indexer v4
27. Update fixture generation scripts for new types
28. Integration testing against testnet/mainnet
29. Fee calculation re-validation against ledger-8 cost model

---

## 6. Files Changed Summary

Total: **115 files**, **~17,500 lines added**

| Category | Files | Lines | Reuse Level |
|----------|-------|-------|-------------|
| Config & models | ~25 | ~2,500 | High |
| Provider & signer | ~8 | ~1,200 | Low (rewrite) |
| Transaction domain | ~4 | ~2,650 | Low (rewrite) |
| Relayer domain | ~3 | ~1,150 | Medium (adapt) |
| Sync & indexer | ~12 | ~2,800 | Medium (adapt) |
| Repository (state) | ~4 | ~1,050 | High (reuse) |
| Bootstrap & API | ~12 | ~500 | High (reuse) |
| Docs & examples | ~10 | ~1,200 | Medium (adapt) |
| Tests & fixtures | ~5 | ~700 | Medium (adapt) |
| Other (Cargo, CI, typos) | ~10 | ~4,750 | Low (dependency updates) |
