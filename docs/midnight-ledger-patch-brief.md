# `midnight-ledger` Patch — Problem Brief

*Seeking a second opinion on how to handle the local `midnight-ledger` fork we currently rely on. This document is self-contained — a reviewer should not need access to the repo to evaluate.*

---

## TL;DR

We maintain a **local source patch** of `midnight-ledger` (v8.0.2) that makes **four fields** on `DustLocalState` public and adjusts the transaction tag version. Without the patch, our DUST proof-verification path (required to submit any Midnight transaction that spends DUST for fees) fails with `InvalidDustSpendProof`. The patch is now tracked in `patches/midnight-ledger-8.0.2.patch` with an apply script at `patches/apply.sh`.

---

## 1. Context

Midnight's `midnight-ledger` library separates ledger state into two halves:

| Side | Type | Owner | Contains |
|---|---|---|---|
| **Wallet-side** | `DustLocalState` | `DustWallet` | The wallet's own commitment tree, generation tree, DUST params |
| **Chain-side** | `LedgerState.dust` | Validators / block builders | The public root history, global tree state, DUST parameters |

In normal Midnight architecture these live in **different processes**:

- A **wallet / client** holds `DustLocalState` and uses it to propose transactions.
- A **validator / node** holds `LedgerState` and uses it to verify transactions (specifically the DUST spend proof against `root_history[ctime]`).

In **our relayer**, both roles run inside one process. The `LedgerContext` holds the chain-side `LedgerState`, and we attach wallets whose `DustLocalState` is private to each wallet.

---

## 2. The bug without the patch

When we receive DUST events from the indexer and call `DustLocalState::replay_events(events)`, the wallet's trees advance. But the **chain-side `LedgerState.dust`** does not — its `root_history` still points at old tree roots.

When we then build a transaction, the DUST spend proof is generated against the wallet's **current** commitment root, but the validator verifies against `LedgerState.dust.utxo.root_history[ctime]` — which doesn't contain that root. Submission fails with:

```
InvalidDustSpendProof (node runtime error 170)
```

Additionally, `ParamChange` events (dust generation rate, caps) update `DustLocalState.params` but **not** `LedgerState.parameters.dust`. `speculative_spend()` reads `state.parameters.dust` and computes `updated_value=0` for UTXOs that `wallet.wallet_balance()` correctly sums — resulting in a phantom "Insufficient DUST" during fee payment even when the wallet has plenty.

---

## 3. The fix we applied

`src/services/sync/midnight/handler/ledger_context.rs::sync_dust_trees_to_ledger_ctx` reads five fields off the wallet's `DustLocalState` and writes them into a deserialized-and-modified `LedgerState`:

```rust
// Pseudo-code of what the fix does
let (commitment_tree, c_ff, generating_tree, g_ff, dust_params) =
    wallet.dust.dust_local_state.as_ref().map(|s| (
        s.commitment_tree.clone(),            // ← patched to pub
        s.commitment_tree_first_free,         // ← patched to pub
        s.generating_tree.clone(),            // ← patched to pub
        s.generating_tree_first_free,         // ← patched to pub
        s.params.clone(),                     // ← patched to pub
    ));

// ... then injected into LedgerState.dust and LedgerState.parameters.dust,
// and commitment/generating roots inserted into root_history[tblock].
```

**Upstream `DustLocalState`** (simplified from `dust.rs:1328`):

```rust
pub struct DustLocalState<D: DB> {
    generating_tree: MerkleTree<DustGenerationInfo, D>,       // private upstream
    generating_tree_first_free: u64,                          // private upstream
    commitment_tree: MerkleTree<(), D>,                       // private upstream
    commitment_tree_first_free: u64,                          // private upstream
    night_indices: HashMap<InitialNonce, u64, D>,             // stays private
    dust_utxos: HashMap<DustNullifier, DustWalletUtxoState, D>, // stays private
    sync_time: Timestamp,
    params: DustParameters,                                   // private upstream
}
```

**Our patched version** flips the five referenced fields (plus `sync_time` incidentally, total six) to `pub`. Nothing else is changed — no logic, no new methods, no removed code.

Relayer `Cargo.toml` wires it in:

```toml
[patch.crates-io]
# Local patch exposing DustLocalState tree fields as pub so our sync code
# can write wallet-side DUST merkle state back into LedgerState.
midnight-ledger = { path = "/tmp/midnight-ledger-patched" }
```

---

## 4. Why this is a problem

### 4.1 Fragile location

`/tmp` is wiped on reboot on some systems. Every developer needs to recreate the patched directory before `cargo build` will succeed. There is a recovery procedure in `docs/midnight-migration-guide.md`, but it's a manual step that doesn't scale — CI doesn't have it, new contributors hit it immediately, and anyone running `docker build` in a clean container hits it too.

### 4.2 No lineage tracking

The patched directory is not a git repo. We have no record of:
- What upstream commit / tag it was forked from
- What exact lines we changed
- Whether a contributor may have accidentally modified other files

We effectively maintain a parallel "diff from upstream" that lives only in a developer's head.

### 4.3 Upstream version bumps are manual

When we bumped from `node-0.22.3` → `node-1.0.0-rc.1` recently, the patched crate needed to be manually re-forked, re-patched, and re-dropped into `/tmp`. This is the kind of task that is easy to forget and hard to audit — a subtle upstream change could silently break our assumptions about `root_history` semantics without the patch-reapplication process noticing.

### 4.4 Blocks production deployment posture

The `/tmp` path is hardcoded into `Cargo.toml`. It cannot be relied on in:
- Release Docker builds
- CI pipelines (GitHub Actions runners are ephemeral)
- Downstream consumers of this repo (a relayer-operator cloning the repo as a fork)

### 4.5 Hidden contract with the library

By accessing private fields, we've formed a **dependency on the library's internal layout** that upstream has no obligation to preserve. A refactor that splits `commitment_tree` into two fields, or moves it into a nested struct, would break us silently at compile time — fine for us, but fine only *if* we notice during a routine `cargo update`.

---

## 5. Alternatives we've considered

| Option | Pros | Cons |
|---|---|---|
| **A. Status quo** (local `/tmp` patch) | Works now; minimal diff | All of §4 above |
| **B. Fork to our own git repo** (e.g. `openzeppelin/midnight-ledger-patched`) | CI-friendly, trackable, reproducible builds, same patch payload | We now own a fork's maintenance burden; must track upstream tags and re-patch for each bump |
| **C. Upstream PR**: add public getters (`pub fn commitment_tree(&self) -> &MerkleTree<...>`) or a single `DustLocalState::export_trees_and_params() -> TreeExport` | Clean separation; no privacy violation; no fork | Needs upstream buy-in, round-trip time with Midnight team, and a release cycle; doesn't unblock us today |
| **D. Upstream PR**: add a dedicated `apply_wallet_trees_to_ledger_state(&DustLocalState, &mut LedgerState)` helper | Library-sanctioned way to bridge wallet ↔ chain state — encodes our use case as a first-class operation | Same upstream-latency cost as C, plus library author may reject because it couples roles they prefer to keep separate |
| **E. Serialize-deserialize hack** — serialize the wallet and deserialize as a validator-perspective view | No library patch | Requires a shared wire format for both types (there isn't one); likely slower and more brittle than a field copy |
| **F. Rearchitect our code** — don't attempt to pre-compute proof-verifiable state locally; submit and let the chain give us whatever root is needed, retrying on mismatch | No library patch; closer to Midnight's intended separation of roles | A retry loop is the wrong abstraction for proof submission (each retry is expensive, network-visible, and can't actually recover if our wallet's root was never a chain root to begin with). This is what we tried first and it didn't work. |

### Not considered viable

- Moving the patched crate inside our repo under `third_party/` — legally fine (Apache-2.0) but substantially bigger than a local fork because we'd also vendor `base-crypto`, `transient-crypto`, and friends. The library is not self-contained.
- Runtime reflection / `unsafe` to bypass field privacy — brittle and violates the purpose of field privacy in a library we don't trust at the memory-safety level.

---

## 6. Our tentative recommendation

**Short-term (now):** move the patch from `/tmp/midnight-ledger-patched` to a **real fork** at `openzeppelin/midnight-ledger` tracked by git tag (Option B). Cargo patch entry becomes:

```toml
midnight-ledger = { git = "https://github.com/openzeppelin/midnight-ledger", tag = "node-1.0.0-rc.1-pub-dust-fields" }
```

This eliminates §4.1, §4.2, §4.4 immediately. §4.3 and §4.5 remain but become auditable — each upstream bump produces a visible PR in the fork with the re-applied patch, reviewable and lineage-traceable.

**Long-term (parallel track):** open an upstream PR for Option C (public getters). Even if the Midnight team doesn't want to merge the specific fields, getting their input on how they'd *prefer* us to bridge wallet ↔ chain state would tell us whether Option D is worth attempting, or whether the answer is that we should not be running both roles in one process at all (in which case the fix is architectural — split proof verification out into a separate concern that relies on indexer-provided `root_history`).

---

## 7. Questions for the second reviewer

1. Is a **fork-with-patches** an acceptable posture for a production relayer, or should we block on upstream? If upstream is required, who pushes it — us or the Midnight team?
2. Is there a way to achieve our goal **without** reading the wallet's trees? Specifically: can we fetch the current `LedgerState` bytes from the node RPC directly at tx-build time, avoiding any need to reconstruct it locally from events? (We believed earlier this wasn't available; this premise is worth challenging.)
3. If we go with Option B, should the fork live under the `openzeppelin/` GitHub org, or under a dedicated `midnight-*` namespace? The latter signals a more neutral "patches we'd like to upstream" posture.
4. Is there a concern about the patch being a **correctness risk** we haven't recognised? E.g. is there any scenario where a wallet's `commitment_tree` could legally diverge from `LedgerState.dust.utxo.commitments` without that being a bug we need to detect rather than paper over?

---

## Appendix — exact call site

File: `src/services/sync/midnight/handler/ledger_context.rs`

```rust
pub fn sync_dust_trees_to_ledger_ctx(
    context: &Arc<LedgerContext<DefaultDB>>,
    seed: &WalletSeed,
) -> Result<(), LedgerContextError> {
    let wallet_dust = context.with_wallet_from_seed(seed.clone(), |wallet| {
        wallet.dust.dust_local_state.as_ref().map(|state| (
            state.commitment_tree.clone(),
            state.commitment_tree_first_free,
            state.generating_tree.clone(),
            state.generating_tree_first_free,
            state.params.clone(),
        ))
    });
    // ... injects into LedgerState.dust + LedgerState.parameters.dust, then
    //     inserts the roots into root_history[tblock].
}
```

Called once per DUST-event batch from `SharedDustSyncTask::try_mark_ready` (`src/services/sync/midnight/shared.rs:617`), guarded by a `last_synced_applied_id` change detector so it only runs when the cursor has actually advanced.
