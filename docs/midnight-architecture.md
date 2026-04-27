# Midnight Integration — Architecture & Learnings

> Living document capturing all decisions, discoveries, and architecture
> from building Midnight support in the OpenZeppelin Relayer.
> Last updated: April 2026

---

## 1. High-Level Architecture

Midnight is added as a fourth network alongside EVM, Solana, and Stellar.
All Midnight code is behind `#[cfg(feature = "midnight")]` — the default
build is completely unaffected.

```
Config (JSON) → NetworkFileConfig::Midnight
             → NetworkRepoModel (midnight:{network})
             → MidnightNetwork (runtime model)
             → MidnightRelayer + MidnightTransaction (domain)
             → MidnightProvider + MidnightSigner (services)
             → SyncManager + LedgerContextManager (sync)
```

### Feature Flag

```toml
# Cargo.toml
[features]
midnight = [
    "dep:tokio-tungstenite",
    "dep:midnight-node-ledger-helpers",
    "dep:midnight-node-metadata",
    "dep:subxt",
]
```

### Crate Dependencies

| Crate | Source | Purpose |
|-------|--------|---------|
| `midnight-node-ledger-helpers` | git (node-0.22.3) | LedgerContext, wallet, tx types, serialization |
| `midnight-node-metadata` | git (node-0.22.3) | Subxt pallet bindings (`send_mn_transaction`) |
| `subxt` | crates.io (0.44) | Substrate client for unsigned extrinsics |
| `tokio-tungstenite` | crates.io (0.26) | WebSocket for indexer subscriptions |

---

## 2. Key Decisions & Rationale

### 2.1 Native Key Derivation in Rust

**Decision:** Derive bech32m addresses and viewing keys directly in Rust
using `midnight-node-ledger-helpers` wallet and the `bech32` crate.

**How it works:**
- The keystore's 32-byte raw key IS the wallet seed
- `LedgerContext::new_from_wallet_seeds()` derives the full wallet
- `wallet.unshielded.signing_key().verifying_key()` → bech32m encode → unshielded address
- `wallet.shielded.viewing_key(network)` → viewing key (built-in bech32m)
- Network HRP is derived from the config's `network` field (e.g., "preview")

**Previous approach (deprecated):** External TypeScript keygen script
(`scripts/midnight-keygen/keygen.mjs`) + `MIDNIGHT_ADDRESS`/`MIDNIGHT_VIEWING_KEY`
env vars. This caused seed mismatch issues because the keystore key and
keygen seed were different. The script is still useful as a standalone tool.

**Critical invariant:** The same 32-byte seed must be used by the signer,
the LedgerContextManager, and the keygen script (if used). Mismatch causes
UTXO owner validation failures during transaction building.

### 2.2 Bech32m HRP Convention (preview testnet)

**Discovery:** The preview testnet uses `_preview` suffix, NOT `_test`.

| Type | HRP |
|------|-----|
| Unshielded address | `mn_addr_preview` |
| Shielded address | `mn_shield-addr_preview` |
| Viewing key | `mn_shield-esk_preview` |

Other environments: `_preprod`, `_dev`, or no suffix (mainnet).

**How discovered:** The indexer rejected `mn_shield-esk_test` with:
`expected HRP mn_shield-esk_preview, but was mn_shield-esk_test`

### 2.3 Ledger WASM Key Version Mismatch

**Discovery:** `SecretKeys.fromSeed()` in `@midnight-ntwrk/ledger` WASM
produces v0.2 serialized keys, but `signatureVerifyingKey()` expects v1.0.

**Fix:** Strip 3-byte v0.2 header (`00 02 00`), prepend 2-byte v1.0 header
(`01 00`). v0.2 = 35 bytes, v1.0 = 34 bytes.

### 2.4 Two Address Types for Midnight

**Discovery:** Midnight has separate address types for its privacy model:
- **Shielded** (`mn_shield-addr_...`) — for ZK private transfers
- **Unshielded** (`mn_addr_...`) — for public transfers (tNIGHT)

tNIGHT is an unshielded token, so funding from Lace requires the
**unshielded** address. The keygen script derives both.

### 2.5 WebSocket for Indexer Sync (not HTTP)

**Decision:** Use WebSocket subscriptions for both balance sync and
shielded wallet sync.

**Rationale:** The indexer's `unshieldedTransactions` and
`shieldedTransactions` are **subscriptions** (WebSocket only), not queries.
There is no HTTP-based balance-by-address query due to Midnight's privacy
model.

**Protocol:** `graphql-transport-ws` — requires:
1. `Sec-WebSocket-Protocol: graphql-transport-ws` header
2. `connection_init` → wait for `connection_ack`
3. `subscribe` → receive `next` events → `complete` or idle timeout

### 2.6 Dedicated Balance Field in Sync State

**Decision:** Added `unshielded_balance: u128` field to `RelayerSyncState`
instead of overloading `ledger_context`.

**Rationale:** Previous implementation stored balance as JSON bytes in the
`ledger_context` field, which coupled unrelated data. The
`set_sync_state()` method would wipe the balance when updating the sync
cursor. Separating them prevents state corruption.

### 2.7 Shared Sync State Store

**Decision:** Process-wide `OnceLock<Arc<RelayerStateRepositoryStorage>>`
set during app initialization, read by the transaction factory.

**Rationale:** The relayer factory has AppState access but the transaction
factory doesn't. Both need the same sync state to share balance and
LedgerContext data. The `set_shared_store()`/`get_shared_store()` pattern
ensures they use the same Arc instance.

### 2.8 Fail-Fast on Unpopulated LedgerContext

**Decision:** `prepare_transaction` checks for non-empty `network_id` in
the LedgerContext and returns `NotSupported` if the state hasn't been
bootstrapped.

**Rationale:** `StandardTrasactionInfo::build()` panics on empty UTXO sets
or missing parameters. Failing explicitly with a clear message is better
than a runtime panic.

### 2.9 Shared DUST Sync Task per Network (Not per Relayer)

**Decision:** One process-wide `SharedDustSyncTask` per network
subscribes to `dustLedgerEvents` and broadcasts decoded events to every
registered relayer wallet on that network.

**Rationale:** Without sharing, N relayers each re-decode the same
~34K historical DUST events on startup — linear indexer load + CPU per
relayer. The shared task also keeps DUST state live *between*
transaction builds, avoiding the staleness window that caused error
170 (`InvalidDustSpendProof`) in earlier iterations. The reference
Midnight wallet (`midnightntwrk/midnight-wallet`, TypeScript) doesn't
have this pattern because it's single-wallet per process; the sharing
optimisation is relayer-specific and well-motivated.

### 2.10 Per-Relayer Shielded Sync, Shared DUST Sync

**Decision:** Shielded state (`ZswapLocalState.coins`) is synced per
relayer via `handler/manager.rs::sync_shielded()`. DUST state is synced
process-wide via `SharedDustSyncTask`. They are intentionally split.

**Rationale:** Shielded sync is keyed off the wallet's
**`sessionId` / viewing key** — the indexer only emits events
decryptable by that specific key, so each relayer's shielded stream is
a different event sequence. Sharing would require demultiplexing by
origin, which is strictly more complex than running one subscription
per wallet. DUST events, by contrast, are shared chain-level state and
trivially broadcastable, so sharing is a pure win.

### 2.11 Direct `StandardTrasactionInfo` Builder (Not a Facade)

**Decision:** The transaction builder constructs `UnshieldedOfferInfo`
and `OfferInfo` directly and calls `StandardTrasactionInfo::add_intent`
/ `set_guaranteed_offer` without a facade layer.

**Rationale:** The reference Midnight wallet's TypeScript SDK wraps
these primitives in an `initSwap` / `balanceTransaction` /
`submitTransaction` facade intended for untrusted UI-originated JSON
paths. Our relayer sits behind a validated API boundary
(`MidnightTransactionRequest::validate`) and controls the full
transaction lifecycle — a facade would add indirection without
safety benefit. If the API grows a user-facing recipe language in the
future, revisit this.

---

## 3. Indexer v4 GraphQL Schema (verified via introspection)

### 3.1 Key Type Differences from Old Branch

| Field | Old branch assumption | Actual v4 schema |
|-------|----------------------|------------------|
| `Block.index` | `index: Option<u64>` | `height: Int!` |
| `Transaction.applyStage` | `ApplyStage` enum | **Removed** — no direct status field |
| `Transaction.identifiers` | `Vec<String>` | Removed |
| `Transaction.raw` | `Option<String>` | `HexEncoded!` (non-null) |
| `UnshieldedUtxo.address` | N/A | Field is `owner` |
| `TransactionOffset` | `{ hash: String }` | `{ hash: HexEncoded }` or `{ identifier: HexEncoded }` |

### 3.2 Subscription Types

| Subscription | Args | Returns |
|-------------|------|---------|
| `shieldedTransactions` | `sessionId: HexEncoded!, index: Int` | `RelevantTransaction \| ShieldedTransactionsProgress` |
| `unshieldedTransactions` | `address: UnshieldedAddress!, transactionId: Int` | `UnshieldedTransaction \| UnshieldedTransactionsProgress` |
| `blocks` | `offset: BlockOffset` | `Block!` |

### 3.3 RelevantTransaction (shielded sync)

```graphql
type RelevantTransaction {
    transaction: RegularTransaction!  # has raw, hash, startIndex, endIndex
    collapsedMerkleTree: CollapsedMerkleTree  # optional merkle update
}
```

### 3.4 UnshieldedTransaction (balance sync)

```graphql
type UnshieldedTransaction {
    transaction: Transaction!
    createdUtxos: [UnshieldedUtxo!]!
    spentUtxos: [UnshieldedUtxo!]!
}
```

### 3.5 Mutations

```graphql
mutation { connect(viewingKey: ViewingKey!): HexEncoded! }
mutation { disconnect(sessionId: HexEncoded!): Unit! }
```

---

## 4. Transaction Pipeline

### 4.1 Transaction Lifecycle

```
POST /transactions
    → validate request (guaranteed_offer, intents, fallible_offers)
    → create TransactionRepoModel (status: Pending)
    → enqueue TransactionRequest job

prepare_transaction (job handler):
    → check LedgerContext has chain state
    → build StandardTrasactionInfo with:
        - UnshieldedOfferInfo (inputs/outputs from request)
        - funding seeds (for DUST fee payment)
        - proof provider (RemoteProofServer)
    → .prove() → builds, pays fees, generates ZK proofs
    → serialize() → tagged binary (midnight:transaction[v9]...)
    → compute pallet_tx_hash from proven transaction
    → store serialized hex + pallet_hash in network_data
    → status: Sent

submit_transaction (job handler):
    → read serialized hex from network_data.hash
    → submit via provider (JSON-RPC or Subxt)
    → store extrinsic_tx_hash
    → schedule status check (absolute timestamp)
    → status: Submitted

handle_transaction_status (status check job):
    → query indexer by pallet_hash (or extrinsic_hash fallback)
    → if found: Confirmed or Failed
    → if not found or pending: reschedule check
```

### 4.2 Serialization Format

Raw transaction bytes use Midnight's tagged serialization:
```
midnight:transaction[v9](signature[v1],proof,pedersen-schnorr[v1]):h...
```

This is produced by `midnight_node_ledger_helpers::serialize()` and is
NOT standard Substrate SCALE encoding.

### 4.3 Extrinsic Submission

The serialized bytes are wrapped in a Substrate unsigned extrinsic:
```rust
let mn_tx = mn_meta::tx().midnight().send_mn_transaction(tx_bytes);
let extrinsic = api.tx().create_unsigned(&mn_tx);
extrinsic.submit().await;
```

### 4.4 Dual Hash Tracking

| Hash type | Source | Used for |
|-----------|--------|----------|
| `extrinsic_tx_hash` | Substrate RPC return value | Substrate-level tracking |
| `pallet_tx_hash` | `proven_tx.transaction_hash()` | Indexer queries, status checks |

---

## 5. Midnight-Node-Ledger-Helpers Crate API

### 5.1 Key Types

```rust
// Wallet & keys
WalletSeed::Medium([u8; 32])
LedgerContext<DefaultDB>  // manages wallet + ledger state
Wallet<DefaultDB>         // shielded + unshielded + dust subwallets

// Transaction building
StandardTrasactionInfo<D>  // NOTE: typo in crate, "Transaction"
UnshieldedOfferInfo<D>     // inputs + outputs for unshielded transfers
UtxoSpendInfo<WalletSeed>  // input UTXO to spend
UtxoOutputInfo<WalletSeed> // output UTXO to create
IntentInfo<D>              // wraps unshielded offers + contract actions

// Proof & serialization
ProofProvider<D>           // trait for ZK proof generation
RemoteProofServer          // HTTP-based implementation
serialize() / deserialize() // tagged (midnight:transaction[v9]...)
serialize_untagged()       // raw bytes without tag header

// Types
NIGHT                      // native token type constant
FinalizedTransaction<D>    // = Transaction<Signature, ProofMarker, PureGeneratorPedersen, D>
SerdeTransaction<S, P, D>  // wrapper: Midnight(Transaction) | System(SystemTransaction)
```

### 5.2 LedgerContext Population

The LedgerContext needs chain state to function. Two approaches:

**Approach A: Indexer sync (current)**
- Connect shielded subscription → receive raw tx bytes
- `midnight_node_ledger_helpers::deserialize()` → `FinalizedTransaction`
- Wrap in `SerdeTransaction::Midnight(tx)`
- `LedgerContext::update_from_tx(&serde_tx, &block_context)`

**Approach B: Direct state read (TODO)**
- Use Subxt `state_getStorage` to read ledger pallet storage
- Deserialize `LedgerState<DefaultDB>` from raw bytes
- `LedgerContext::update_ledger_state_from_bytes(bytes)`

Approach B is more efficient — one RPC call vs syncing entire chain.

### 5.3 Transaction Building Flow

```rust
let mut tx_info = StandardTrasactionInfo::new_from_context(context, prover, None);
tx_info.set_funding_seeds(vec![wallet_seed]);
tx_info.add_intent(0, Box::new(IntentInfo {
    guaranteed_unshielded_offer: Some(UnshieldedOfferInfo { inputs, outputs }),
    fallible_unshielded_offer: None,
    actions: vec![],
}));
let proven_tx = tx_info.prove().await?;
let bytes = serialize(&proven_tx)?;
```

---

## 6. Live Testnet Findings (preview environment)

### 6.1 Verified Endpoints

| Service | URL | Status |
|---------|-----|--------|
| RPC | `https://rpc.preview.midnight.network` | 16+ peers, synced |
| Indexer v4 | `https://indexer.preview.midnight.network/api/v4/graphql` | Responding |
| Indexer WS | `wss://indexer.preview.midnight.network/api/v4/graphql/ws` | Connected |
| Proof server | `https://lace-proof-pub.preview.midnight.network` | Available |
| Faucet | `https://faucet.preview.midnight.network/` | Available |
| Explorer | `https://explorer.preview.midnight.network` | Available |

### 6.2 Funding Verified

- Sent tNIGHT from Lace wallet to relayer's unshielded address
- Confirmed on-chain at block 185,475 with value 1,000,000
- Second transaction also confirmed (total 2,000,000)
- Balance correctly synced via WebSocket subscription
- API returns `balance: "2000000"` and `nonce: "205000+"` (live)

### 6.3 Component Versions

| Component | Version |
|-----------|---------|
| Ledger | 8.0.2 |
| Node | 0.22.3 |
| Proof Server | 8.0.3 |
| Indexer | 4.0.1 |
| Midnight.js | 4.0.2 |
| Subxt | 0.44 |

---

## 7. File Map

### New Files

| File | Purpose |
|------|---------|
| `src/config/config_file/network/midnight.rs` | Network config (indexer_urls, prover_url, TTL) |
| `src/models/network/midnight/` | MidnightNetwork runtime model |
| `src/models/transaction/request/midnight.rs` | Transaction request (offers, intents) |
| `src/models/rpc/midnight/` | RPC request/response types |
| `src/domain/relayer/midnight/` | MidnightRelayer (Relayer trait impl) |
| `src/domain/transaction/midnight/` | MidnightTransaction (Transaction trait impl) |
| `src/services/provider/midnight/mod.rs` | JSON-RPC provider |
| `src/services/provider/midnight/subxt_client.rs` | Subxt unsigned extrinsic submission |
| `src/services/provider/midnight/proof_server.rs` | RemoteProofServer (ProofProvider impl) |
| `src/services/provider/midnight/tx_builder.rs` | MidnightTxBuilder scaffold |
| `src/services/signer/midnight/` | Ed25519 local signer |
| `src/services/sync/midnight/indexer/` | GraphQL client + types |
| `src/services/sync/midnight/handler/manager.rs` | SyncManager (WS sync + state) |
| `src/services/sync/midnight/handler/ledger_context.rs` | LedgerContextManager |
| `src/repositories/relayer_state/` | Sync state repo (in-memory + Redis) |
| `config/networks/midnight.json` | Default network config |
| `scripts/midnight-keygen/` | TypeScript key derivation tool |
| `docs/midnight-migration-guide.md` | Migration guide from old branch |
| `docs/midnight-integration-test-plan.md` | Integration test plan |

### Modified Files (key ones)

| File | What changed |
|------|-------------|
| `Cargo.toml` | midnight feature + deps |
| `src/models/address.rs` | `Address::Midnight` variant |
| `src/models/app_state.rs` | `relayer_state_repository` field |
| `src/domain/relayer/mod.rs` | `NetworkRelayer::Midnight` + all dispatch arms |
| `src/domain/transaction/mod.rs` | `NetworkTransaction::Midnight` + factory |
| `src/services/signer/mod.rs` | `NetworkSigner::Midnight` |
| `src/constants/status_check.rs` | Midnight-specific thresholds |
| `typos.toml` | `StandardTrasactionInfo` exclusion |

---

## 8. Remaining Work

### 8.1 LedgerContext Population (Implemented)

The LedgerContext is now populated via a three-step bootstrap during
relayer initialization:

1. **Subxt runtime API** — reads `network_id` and `LedgerParameters`
   from the node via `get_network_id()` and `get_ledger_parameters()`.
   Requires WSS URL (auto-converted from HTTPS config).

2. **Unshielded UTXO injection** — the `unshieldedTransactions` WS
   subscription collects full UTXO details (`owner`, `value`, `tokenType`,
   `intentHash`, `outputIndex`). These are deserialized, wrapped as `Utxo`
   objects, and injected into the LedgerState via serialize→modify→inject.

3. **Shielded sync** (optional) — feeds shielded transaction events into
   `LedgerContext::update_from_tx()` for ZK wallet state.

After bootstrap, `StandardTrasactionInfo::build()` can find UTXOs for
spending and compute fees/TTL from the parameters.

### 8.2 Subxt Extrinsic Submission (TODO)

The `MidnightSubxtClient` is built but not yet wired into
`submit_transaction`. Currently uses JSON-RPC `author_submitExtrinsic`.
The Subxt path (`send_mn_transaction` pallet call) is the correct
approach used by Midnight's own toolkit.

### 8.3 DUST Fee Token

Every Midnight transaction requires DUST fees. The relayer needs DUST
tokens in the wallet alongside tNIGHT. Without DUST, the
`StandardTrasactionInfo::build()` fee payment loop will fail.
DUST is generated from tNIGHT via the Dust registration process.

### 8.4 Multi-RPC Failover

Midnight provider uses `first_rpc_url()` only. Should use the weighted
RPC selector pattern from other networks.

### 8.5 Contract Call Relaying (Deferred)

The tagged `MidnightContractAction { Call {...} }` path in the request
model is **accepted by the schema but rejected at `validate()`**. A
PR-2 attempt to wire it through to the builder surfaced two library
constraints that make generic JSON-parameterised contract relaying
infeasible with the current `midnight-node-ledger-helpers` shape.
See §11 for the constraint details, the options surveyed, and the
deferral rationale. Requires upstream work on Midnight's tooling (or
a compiled-in contract registry in the relayer) before it can ship.

### 8.6 Production Signer

Only `SignerConfig::Local` is supported. No AWS KMS, Vault, Turnkey,
or GCP KMS. Address/viewing key derivation would need crate integration
for non-local signers.

---

## 9. Status Check Constants

```rust
MIDNIGHT_MAX_CONSECUTIVE_STATUS_FAILURES: 100  // ~5 min at 3s backoff
MIDNIGHT_MAX_TOTAL_STATUS_FAILURES: 300        // ~15 min safety net
```

Matches Stellar profile (similar ~6s block time).

---

## 10. Test Configuration

```bash
# Environment variables
KEYSTORE_PASSPHRASE="test"
REDIS_URL="redis://localhost:6379"
API_KEY="midnight-test-api-key-0123456789abcdef"  # 32+ chars
CONFIG_FILE_NAME="midnight-test-config.json"
MIDNIGHT_ADDRESS="mn_addr_preview1..."
MIDNIGHT_VIEWING_KEY="mn_shield-esk_preview1..."

# Run
cargo run --features midnight

# API
curl -H "Authorization: Bearer $API_KEY" \
  http://localhost:8080/api/v1/relayers/midnight-testnet/status
```

---

## 11. Contract Invocation — Library Constraints & Deferred Support

This section documents why generic contract-call relaying (PR-2 in the
2026-04-22 `atomic-greeting-boole` plan) was **skipped after surface
analysis** and the specific library constraints that drove the deferral.
Decision recorded here so future work doesn't re-walk the same path.

### 11.1 What we wanted to build

The `MidnightTransactionRequest` API model already carries:

```rust
pub struct MidnightIntentRequest {
    pub segment_id: u16,
    // ... offer slots ...
    pub actions: Vec<MidnightContractAction>,
}

// PR-2 target — flipped from the current empty-struct placeholder:
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MidnightContractAction {
    Call {
        contract_address: String,
        entry_point: String,
        arguments: serde_json::Value,
        key_location: String,
    },
}
```

Intended behaviour: a caller submits `{"type":"call", ...}` inside an
intent; the relayer constructs a library `CallInfo`, places it in
`IntentInfo.actions`, and the proof server does the rest. The relayer
would be a **contract-agnostic proxy** — call any deployed contract
without compiled-in knowledge of it.

### 11.2 Why this doesn't fit the current library

Two verified findings from
`midnight-node-ledger-helpers`:

**(a) `CallInfo.key: &'static str`** (`contract/call.rs:22-28`)

```rust
pub struct CallInfo<C: Contract<D>, D: DB + Clone> {
    pub type_: C,
    pub address: ContractAddress,
    pub key: &'static str,           // <- static lifetime
    pub input: Box<dyn Any + Send + Sync>,
    pub _marker: PhantomData<D>,
}
```

An entry-point name from a JSON request body is a `String` with
request-scoped lifetime. The library's `&'static str` constraint can
only be satisfied by:

1. `Box::leak(entry_point.into_boxed_str())` — leaks memory per unique
   entry point seen. Small in absolute terms but unbounded and
   unaesthetic.
2. A compile-time registry of interned entry-point names — requires
   the relayer to declare every entry point it will ever accept, at
   build time. Defeats "contract-agnostic proxy".

**(b) `CallInfo.input: Box<dyn Any + Send + Sync>` + per-contract
`Contract::transcript`** (`contract/mod.rs:61-67`)

There is **no** `from_json(serde_json::Value) -> AlignedValue` helper.
The `Contract` trait's `transcript` method is responsible for
encoding the input bytes to an `AlignedValue`, and each deployed
contract has its own Rust `Contract` impl that knows its ABI. This
means:

- The relayer cannot encode arguments generically.
- Each contract the relayer supports needs a compiled-in `Contract`
  impl — i.e., the relayer's binary must be rebuilt to add a new
  contract.
- A JSON-only `arguments` field from the request body has no generic
  encoding path; the call would either fail at build time or produce
  silently incorrect proofs.

**(c) No contract artifacts provisioned**

Contract calls also require a VK/prover/ZKIR artifact bundle on disk
that `Resolver::new` can load (`/tmp/midnight-ledger-patched/src/prove.rs:49-69`).
A repo-wide search turned up zero `*.zkir` / `*.verifier` / `*.prover`
/ `manifest.json` files. Even if (a) and (b) were solved, live
smoke-testing would require deploying a contract to preview testnet
and exporting its artifact bundle externally — blocked on tooling we
don't control.

### 11.3 Options surveyed

| Option | What | Why not chosen |
|---|---|---|
| **A. Skip (chosen)** | Defer contract Call support indefinitely; ship PR-3 (shielded) next | Honest — library design doesn't match the dynamic-relay use case; PR-3 is higher user value anyway |
| B. Narrow compiled-in registry | Hard-code a fixed list of `Contract` impls the relayer is built with; `key_location` selects among them; entry-point name `Box::leak`'d | 1-2 weeks of work, narrow applicability, still leaks memory per entry-point string |
| C. Pre-encoded args (`arguments: "0xHEX"`) | Callers hand-encode `AlignedValue` bytes and submit as hex | Pushes entire ABI burden onto callers; still hits `'static str` leak; worst ergonomics |
| D. Reach out to Midnight team | Ask about an alternate API path we may have missed | Reasonable future step; not a shipping plan on its own |

### 11.4 What the API contract looks like today

The request schema still accepts `intents` with `actions` for
forward-compatibility. `validate()` returns a precise 400 with:

```
intents[N].actions: contract actions are not yet supported by the
Midnight transaction builder
```

When a future release unlocks contract calls (via Option B, an
upstream library change, or something else), dropping one branch in
`validate()` and wiring the builder's intent loop will be the shape
of the change — the public API surface stays stable.

### 11.5 What would need to change upstream

For a purely generic-relay approach to be viable, the library would
need to expose:

1. A variant of `CallInfo` whose `key` is `Cow<'static, str>` or
   `String`, not `&'static str`.
2. A generic args encoder — either `AlignedValue::from_bytes`
   (take pre-encoded) or a runtime-parameterisable `Contract` trait
   object that can do ABI lookup from a manifest file.
3. A stable artifact-bundle format and a reference build pipeline
   from the contract sources (Compact / Midnight DSL) to
   `{contract}/keys`, `{contract}/zkir`, `{contract}/verifier`.

Items (1) and (2) are library changes; (3) is a Midnight tooling
concern. Until at least (1) and (2) land upstream, the relayer can at
best offer Option B.

### 11.6 Decision log

- **Date:** 2026-04-22
- **Decision:** Skip PR-2; keep the API surface reservation; proceed
  directly to PR-3 (shielded transactions).
- **Owner:** `midnight-rebuild` branch work.
- **Revisit when:** either (a) Midnight ships library changes per §11.5,
  or (b) a concrete user need emerges for relaying against a specific
  set of known contracts (Option B becomes worth the cost).

---

## 12. Shielded (ZSWAP) Transactions — PR-3 v1

PR-3 v1 wired shielded offer construction into the transaction builder.
This section documents what's shipped, what's verified, and the scope
frontier for a future PR-3 v2.

### 12.1 What's shipped in PR-3 v1

- **API**: `MidnightOfferRequest::Shielded { inputs, outputs }` accepted
  at the top-level `guaranteed_offer` slot only. Intent-nested and
  `fallible_offers[]` shielded variants remain rejected with clear
  errors pointing callers at the supported shape.
- **Builder**: `build_shielded_offer` helper in
  `src/domain/transaction/midnight/midnight_transaction.rs` translates
  the request into a library `OfferInfo<DefaultDB>` using
  `InputInfo<WalletSeed>` + `OutputInfo<WalletSeed | ShieldedWallet<D>>`.
- **Routing**: top-level `guaranteed_offer` dispatches on `kind` —
  unshielded routes to the existing segment-1 intent path, shielded
  routes to `StandardTrasactionInfo::set_guaranteed_offer`.
- **No library patch needed**: `InputInfo<WalletSeed>` takes the wallet
  seed directly; library resolves secret keys internally via
  `context.with_wallet_from_seed`. `coin_secret_key()` exposure on
  `LocalSigner` was dropped from scope.
- **No new sync task**: existing per-relayer `sync_shielded()` at
  `handler/manager.rs:419` already populates the wallet's shielded
  state. The plan's `SharedShieldedSyncTask` sibling was not built —
  deferred unless multi-relayer load justifies it.

### 12.2 Library constraints that shaped v1

1. **`Segment::Guaranteed` hardcode.** Helpers' `InputInfo::build` /
   `OutputInfo::build` implementations pin the zswap `Segment` to
   `Guaranteed`. Fallible-shielded offers cannot be expressed via the
   helpers; they would require bypassing helpers and going to the raw
   `midnight-zswap` crate. PR-3 v1 reserves the API field for
   fallible-shielded but rejects it at `validate()`.

   *Cross-reference*: the Midnight Foundation's reference wallet
   (`midnightntwrk/midnight-wallet`, TypeScript) does **not** document
   or expose a shield-op path either. This is a library-level design
   constraint shared by every consumer of the helpers crate, not a
   relayer-specific gap. Bypassing it is the PR-3 v2 spike tracked in
   §12.5 option 2.

2. **Panics on empty spendable set.** `InputInfo::min_match_coin`
   (`helpers/src/versions/common/input.rs:73-76`) panics if no coin
   matches the requested `(token_type, value)` tuple, and `State::spend`
   `.expect()`s on failure. These surface through the existing
   `AssertUnwindSafe(tx_info.prove()).catch_unwind()` wrapper at the
   call site as `TransactionError::UnexpectedError` rather than
   crashing the worker — behaviour unchanged from the DUST path.

3. **Tagged-tx balance enforcement.** The chain-side validator rejects
   transactions where any segment's `(inputs - outputs)` delta is
   negative — the same constraint the unshielded path has. A transfer
   that consumes nothing but creates shielded outputs gets a clean
   `invalid balance -X for token Shielded(...) in segment 0` error.
   PR-3 v1's tx-pending-after-abort case (see §12.3) is *not* this
   error — it's an unrelated status-machine issue.

### 12.3 Verification status

**Code path green** (verified end-to-end via preview-testnet smoke):

- Request parses and validates with `{kind: "shielded"}` top-level.
- Builder constructs `OfferInfo` with shielded output; no panics.
- Proof server successfully produces a shielded proof (~1s).
- Tx reaches the submit path.

**Not yet verified** (v1 scope does not allow expressing):

- Chain-confirmed shielded tx. The library's
  `Segment::Guaranteed`-hardcode + current API shape make it impossible
  to express a **shield operation** (unshielded input + shielded output
  in one atomic tx). A shield op is the only way for the test wallet to
  acquire shielded funds — without those funds a pure shielded-to-X
  transfer cannot balance.

### 12.4 Known status-machine issue (not PR-3 scope)

A prepare-phase failure (e.g. the unbalanced-offer error above) raises
an `AbortError` and exhausts job retries, but the tx row in Redis
stays at `status: "pending"` rather than transitioning to `failed`.
This is a pre-existing bug in the transaction_request_handler, not
introduced by PR-3. Separate fix.

### 12.5 What PR-3 v2 would need

To land a shippable shield-op path:

1. **Express mixed offers.** Add a top-level
   `shield_output_offer: Option<MidnightOfferRequest>` (or similar) so
   a request can combine a segment-1 unshielded input offer with a
   top-level shielded output offer in one tx.
2. **OR: fallible-shielded via raw zswap.** Bypass helpers'
   `OutputInfo::build` and construct the `Output<ProofPreimage, D>`
   directly with the caller-specified `Segment::Fallible(id)`. More
   invasive; higher risk of state-divergence bugs.
3. **Balance pre-check at validate().** Before hitting the prover,
   verify `(inputs_sum - outputs_sum)` is non-negative per token per
   segment. Surfaces the failure at HTTP 400 rather than as a stuck
   pending tx.
4. **Smoke harness**: once (1) lands, the dev wallet can be seeded
   with shielded funds via a shield-op tx, unlocking shielded-to-self
   / shielded-to-external smoke tests.

### 12.6 Decision log

- **Date:** 2026-04-22
- **Decision:** Ship PR-3 v1 with the current scope (top-level shielded
  offer only, validation-gated, no on-chain confirmation yet) rather
  than expanding scope to mid-PR for the mixed-offer shape.
- **Rationale:** The library's `Segment::Guaranteed` hardcode is a
  design constraint, not a coding mistake. Working around it deserves
  its own PR with its own review cycle. Shipping v1 lets subsequent
  PRs build on a reviewed foundation.
- **Revisit when:** user demand for shield-ops surfaces, or we
  encounter a downstream PR that needs to spend shielded funds.

---

## 13. Shielded Fallible Offers (PR-3 v2) — Retargeting Wrapper Spike

PR-3 v2 spiked a workaround for the `Segment::Guaranteed` hardcode
identified in §12.2 and reached a useful-but-partial result.

### 13.1 What shipped

- New `fallible_shielded_offers: Vec<MidnightFallibleOfferRequest>`
  field on `MidnightTransactionRequest`. Each entry carries a
  `segment_id: u16` and a `kind: "shielded"` offer.
- Three wrapper types in `midnight_transaction.rs` —
  `RetargetingShieldedInput`, `RetargetingShieldedOutputSelf`,
  `RetargetingShieldedOutputExternal` — that `impl BuildInput<D>` /
  `BuildOutput<D>` for our purposes. Each one calls helpers'
  `InputInfo::build` / `OutputInfo::build` (which produces a
  `Segment::Guaranteed`-tagged zswap primitive) then invokes
  `zswap::Input::retarget_segment(u16)` / `Output::retarget_segment(u16)`
  to rewrite the segment field on the proof's public transcript.
- New helper `build_fallible_shielded_offer(offer, seed, segment_id)`
  produces an `OfferInfo<D>` whose eventual `OfferInfo::build` output
  has each `Input`/`Output`'s `segment` field set to `segment_id`,
  **not** `Segment::Guaranteed`.
- Builder routing: if `fallible_shielded_offers` is non-empty, the
  builder collects each entry into a `HashMap<u16, OfferInfo<D>>` and
  calls `StandardTrasactionInfo::set_fallible_offers`.

### 13.2 What this proved

Verified live (preview testnet, 2026-04-22):

1. The retargeting wrapper pattern compiles, links, and runs.
2. `OfferInfo::build` correctly propagates the wrapper's retargeted
   zswap primitive into the final `ZswapOffer`.
3. The proof server accepts and produces a valid ZK proof for a
   shielded offer whose outputs are tagged at a fallible segment.
4. The `StandardTransaction` assembled by helpers' build pipeline
   contains `fallible_coins[N] = ZswapOffer { outputs[...].segment = N }`.

**The segment-retargeting workaround works.** This was the unknown.

### 13.3 What this didn't prove — the balance finding

The live test attempted a "shield op": unshielded input in segment 1
(via top-level `guaranteed_offer`) paired with shielded output at
segment 1 (via `fallible_shielded_offers[0]`). The chain's proof
server produced a proof, but tx-build failed at the library's balance
check with:

```
invalid balance -1000000 for token Shielded(ShieldedTokenType(...))
in segment 1; balance must be positive
```

**Unshielded NIGHT and shielded NIGHT are separate tokens for
balance-validation purposes**, even though they wrap the same 32-byte
`HashOutput`. The chain enforces balance per (segment × token × privacy
kind) independently. A "shield op" built as `{unshielded_input} +
{shielded_output}` cannot balance — the unshielded segment is
over-supplied and the shielded segment is under-supplied, both are
violations.

### 13.4 The `Transient` primitive — next spike

The zswap `Offer` struct has four fields:

```rust
pub struct Offer<P: Storable<D>, D: DB> {
    pub inputs: Array<Input<P, D>, D>,
    pub outputs: Array<Output<P, D>, D>,
    pub transient: Array<Transient<P, D>, D>,  // <-- unused by us
    pub deltas: Array<Delta, D>,
}
```

`helpers/src/versions/common/transient.rs:34` has
`impl BuildTransient<D> for TransientInfo<WalletSeed, WalletSeed>`.
A `Transient` is conceptually a coin that both appears and disappears
inside a single transaction — the natural primitive for shielding
(unshielded coin consumed → shielded coin produced within same tx)
and unshielding (reverse).

**Next investigation**: read `zswap/src/construct.rs:432` (the
`Transient::new` constructor) and `transient.rs` in helpers to
understand whether a `TransientInfo<WalletSeed, WalletSeed>` with
matching token types represents a shield op, unshield op, or
something else entirely. If it's the shield primitive, PR-3 v3 adds
`build_transient_shield(...)` + a `shield_actions: Vec<ShieldAction>`
API field, and the work is well-scoped.

### 13.5 What's usable from v2 today

Even without `Transient`, the v2 work ships **shielded-to-shielded
transfers at arbitrary fallible segments** once the wallet has
shielded funds. A tx with `fallible_shielded_offers: [{segment_id: N,
offer: {kind: shielded, inputs: [1 NIGHT self], outputs: [1 NIGHT
recipient]}}]` is balanced (+1M - 1M = 0 in the shielded segment) and
will build + submit correctly. Not tested end-to-end because the dev
wallet has no shielded funds; this capability activates as soon as
shielded funds appear via any means (Lace shield op, faucet, external
transfer).

### 13.6 Decision log

- **Date:** 2026-04-22
- **Decision:** Ship the retargeting wrapper pattern + the
  `fallible_shielded_offers` API field. Do not attempt `Transient`
  in this PR.
- **Rationale:** The spike yielded a technical breakthrough
  (retargeting works) and a diagnostic finding (shield-op needs
  `Transient`, not `Input + Output`). Shipping the working piece
  preserves learning; the `Transient` investigation deserves its own
  PR with its own scope.
- **Revisit when:** a user wants to do a shield/unshield op through
  the relayer, or once we have shielded funds in a dev wallet and
  want to exercise the shielded-to-shielded path.
