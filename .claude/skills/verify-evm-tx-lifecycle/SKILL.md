---
name: verify-evm-tx-lifecycle
description: Model-check the EVM transaction lifecycle (prepare, submit, resubmit, cancel, status checks, nonce recovery) for races with TLA+/TLC. Use when changing src/domain/transaction/evm/{evm_transaction,status}.rs, partial_update or other transaction writes in src/repositories/transaction/transaction_redis.rs, src/jobs/handlers/transaction_status_handler.rs, or the nonce counter; when designing a fix for double sends, lost hashes, false Failed status, nonce gaps, or cancel races; or when `python3 formal/evm-tx-lifecycle/drift.py check` reports drift.
---

# Verify the EVM transaction lifecycle

`formal/evm-tx-lifecycle/EvmTxLifecycle.tla` models one EVM transaction and
every worker that can touch it, running at the same time: two prepare jobs,
the submit job, the resubmit job, a cancel API call, the per-tx status job,
and a nonce-recovery check. TLC explores every order of their steps, with
RPC sends that error after landing, a node that lags on receipts, and an
Arbitrum-style chain that rejects future nonces.

Each model action is one indivisible step in the Rust code: one Redis Lua
call (`partial_update`), one RPC call, or local work between them. The
action comments name the code line.

## The properties (what users are promised)

| Property | Plain meaning | Harm when broken |
| --- | --- | --- |
| `NoDoubleEffect` | One request never produces two mined transfers | Funds sent twice (#836) |
| `NoNonceGap` | Every nonce taken from the counter is used or still held | Every later tx of the relayer stalls; on Arbitrum it is rejected |
| `NoLostHash` | Every payload the chain accepted is in `hashes` | Receipt lookups miss it; a mined tx can be marked Failed (#899) |
| `NoFalseFailure` | `Failed` means none of the tx's payloads mined | Caller retries and pays twice (#817) |
| `CancelHonest` | After a cancel is accepted, the record keeps the NOOP and no newly signed original is broadcast | "Cancelled" transfer still goes out |
| `PollingNeverStops` | While the tx is not final, its status job is still scheduled | Nobody ever looks at the tx again |
| `EventuallyFinal` | Every tx reaches a final status (liveness) | Stuck tx |

## The variants

`Variant` picks the design. `main` is the code as it is. The others are
designs that were considered in the EVM race proposal:

- `version`: main + compare-and-set on a record version; a lost CAS returns
  `Ok` (what `handle_result` in transaction_status_handler.rs does today on
  `ConcurrentUpdateConflict`).
- `lease`: main + a TTL lease lock (`DistributedLock`) on prepare, resubmit
  and cancel, without a fencing token. The lease can expire mid-work.
- `reconciler`: version CAS on every lifecycle write + idempotent atomic
  nonce claim + each signed attempt saved before broadcast + one `decide()`
  in the status job that keeps polling after conflicts, checks receipts for
  every saved attempt in Sent and Submitted, and marks Failed only on an
  answer from a node that is not lagging.

`Mutation` breaks one rule of `reconciler` at a time, to show the check can
fail: `nonAtomicClaim`, `sendBeforePersist`, `trustLaggingNode`,
`conflictStopsPolling`, `noCasOnResubmit`.

## When to use this

1. Run the drift check first. It fingerprints the 14 Rust functions the
   model mirrors:

   ```bash
   python3 formal/evm-tx-lifecycle/drift.py check
   ```

   No drift and your change does not touch those files: you are done.
   Drift: the output lists the changed functions and the model actions that
   mirror them. Continue.

2. Decide whether the change alters behavior the model captures: what a
   step reads or writes, the order of writes and sends, which fields a
   `partial_update` patch carries, the final-status guard in the Lua script,
   when jobs get queued, how errors are classified, or whether a conflict
   ends the status job. Logging, metrics, notifications, gas math, and
   refactors that keep the steps the same do not matter.

3. If behavior changed, edit the matching action in `EvmTxLifecycle.tla`
   (on `main` the `Variant = "main"` branches are the code as it is). Keep
   one action per indivisible step. If you are implementing the reconciler
   design, its rules are the `Recon` branches; code and model must agree.

4. Run the checks (below) and compare with the expected results.

5. When the model matches the code again, record it:

   ```bash
   python3 formal/evm-tx-lifecycle/drift.py update
   ```

## Running

Needs Java 11+ and tla2tools 1.7.4. `run.sh` finds the jar in
`~/.cache/tla/` or `~/.cache/guardian-tla/` (or `$TLA_JAR`), checks its
sha256, and prints the download command if it is missing.

```bash
cd formal/evm-tx-lifecycle
./run.sh <variant> <property> [mempool|arbitrum] [mutation]   # one check
./run.sh reconciler NoLostHash mempool | python3 trace.py       # readable trace
./matrix.sh mempool                                             # all variants
./matrix.sh arbitrum reconciler                                 # one variant
```

`run.sh` prints `PASS` or `FAIL` and exits 0, 1, or 2 (setup or model
error). `trace.py` turns a counterexample into numbered steps in the code's
words; `matrix.sh` writes them to `results/<chain>/<variant>.<property>.txt`.

Cost, on a 10-core laptop: a failing check finds its trace in about a
second. A passing check explores the whole state space: `main
PollingNeverStops` is 76M states (6 minutes); each `reconciler` safety
check takes 9 to 12 minutes at `MAX_SIGS=3`; `reconciler EventuallyFinal`
is 15.6M states (9 minutes). `./matrix.sh mempool main version lease`
takes about 10 minutes, `./matrix.sh arbitrum` about an hour.

`EventuallyFinal` runs on `reconciler` only, with `ERR_BUDGET=0
LAG_BUDGET=0` (`matrix.sh` sets this and cuts the run off after
`LIVE_TIMEOUT` seconds). With those budgets at 1, TLC's liveness pass did
not finish in 2 hours. Faults are still covered by the safety checks.
For a quick check after a change, run only the properties your change can
affect and the `reconciler` mutations.

## Expected results

Same on `mempool` and `arbitrum` (`main`, `version`, `lease` at
`MAX_SIGS=2`; `reconciler` at `MAX_SIGS=3`; send-error, lagging-node and
lease-expiry budgets 1, except `EventuallyFinal`, which runs with send-error
and lagging-node budgets 0):

| Variant | NoDoubleEffect | NoNonceGap | NoLostHash | NoFalseFailure | CancelHonest | PollingNeverStops | EventuallyFinal |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `main` | FAIL | FAIL | FAIL | FAIL | FAIL | pass | n/a |
| `version` | pass | FAIL | FAIL | FAIL | FAIL | FAIL | n/a |
| `lease` | FAIL | FAIL | FAIL | FAIL | FAIL | pass | n/a |
| `reconciler` | pass | pass | pass | pass | pass | pass | pass |

Each mutation of `reconciler` fails its property within a second:
`nonAtomicClaim` breaks `NoNonceGap`, `sendBeforePersist` breaks
`NoLostHash`, `trustLaggingNode` breaks `NoFalseFailure`,
`conflictStopsPolling` breaks `PollingNeverStops`, `noCasOnResubmit` breaks
`CancelHonest`.

What the `main` failures are (traces in `results/<chain>/main.*.txt`):

- `NoDoubleEffect`: two prepare jobs each take a nonce from
  `get_and_increment` before anything is saved; both payloads mine (#836).
- `NoNonceGap`: cancel reads the tx while Pending, prepare takes nonce 0 and
  moves on, then cancel's status-only `update_transaction_status(Canceled)`
  is accepted (the Lua guard only protects final records). Nonce 0 is
  orphaned.
- `NoLostHash`: resubmit's broadcast lands but the RPC returns an error, so
  the new hash is never written (#899).
- `NoFalseFailure`: the original mines, then nonce recovery asks a lagging
  node, gets no receipt, and marks the tx Failed (#817).
- `CancelHonest`: a prepare holding a pre-cancel copy writes Sent with its
  original payload over the NOOP.

Why the other designs fail: `version` stops polling when a status write
loses its CAS (`handle_result` returns `Ok` on `ConcurrentUpdateConflict`),
cannot stop a stale copy from broadcasting (CAS guards writes, not sends),
and leaves the nonce counter ahead of the record. `lease` has no fencing: an
expired lease lets a second prepare take a second nonce while the first
holder keeps running.

A rule TLC found for `reconciler`: `decide()` must check receipts in Sent as
well as Submitted. A cancel bumps the version, submit's "mark Submitted"
then loses its CAS, and the tx stays Sent with a mined payload; without
the Sent receipt check it never becomes final.

If a check differs from this table, read the trace before anything else.

## Real bug or modeling error?

Read the trace from `trace.py` and replay it against the code, step by step:

- Each step names a function and line. Open it and confirm the code can do
  that step from that state: same fields read, same patch written, same
  job queued. If one step cannot happen in the code, the model is wrong
  there: fix the action, not the property.
- Steps that only happen because a budget ran out are bounded-model
  artifacts, not bugs. The tell is `nextId` exceeding `MAX_SIGS`: prepare
  aborts at `PrepSign`, or resubmit falls back to resending. Rerun with
  `MAX_SIGS=4`; a real bug survives a larger bound.
- `EventuallyFinal` traces end in "loops back to state N". Check that every
  worker in the loop is one the real system keeps rerunning (the status job
  retries until final; jobs that return `Err` are retried).
- Never weaken a property or add an assumption to make a check pass. If the
  property is wrong for the product, say so to the user.

## Scope and what is not covered

- One transaction. Cross-transaction effects (a gap blocking the next tx)
  are inferred from `NoNonceGap`, not modeled.
- Bounds: 2 concurrent prepares, 1 resubmit slot, 1 cancel, `MAX_SIGS`
  signatures, 1 send error, 1 lagging answer, 1 lease expiry. Liveness is
  checked without send errors or lagging answers. Races here
  need 2 workers; a pass is a pass within these bounds only.
- Abstracted: gas pricing and balance checks, timing and timeouts (any
  delay is possible), `valid_until` expiry, the circuit breaker, Mined vs
  Confirmed (merged), receipt status reverted, webhooks, Redis failover.
  Main's hash recovery is modeled as checking all known hashes, which is
  generous to main.
- It checks the design, not the Rust. A Rust bug inside one step (wrong
  field in a patch) is caught only if the action mirrors the code exactly.

## Files

- `EvmTxLifecycle.tla`: the model and properties
- `run.sh`, `matrix.sh`: run TLC; pinned tla2tools version and checksum
- `trace.py`: counterexample to readable steps
- `drift.py`, `anchors.lock`: fingerprints of the modeled Rust functions
- `results/`: last matrix traces (`*.txt` kept, `*.raw` ignored)
