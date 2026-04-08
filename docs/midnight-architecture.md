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

### 8.3 Multi-RPC Failover

Midnight provider uses `first_rpc_url()` only. Should use the weighted
RPC selector pattern from other networks.

### 8.4 Production Signer

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
