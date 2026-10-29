# EVM transaction lifecycle races: model-checking report

Date: 2026-10-29. Code: `main` at `487bfd24`. Tool: TLA+ / TLC 1.7.4.

## Summary

We modeled one EVM transaction and every worker that can touch it at the
same time, then let the TLC model checker try every order of their steps.
We checked current `main` and three candidate designs against seven
properties, on a normal mempool chain and on an Arbitrum-style chain that
rejects future nonces.

- **Current `main` breaks all five safety properties.** It can send a
  request twice, orphan a nonce, lose the hash of a payload that landed,
  mark a mined transaction Failed, and broadcast a transfer after accepting
  its cancel. Every counterexample was replayed step by step against the
  Rust code.
- **One of these is new:** cancelling a Pending transaction while prepare
  runs orphans a nonce (finding 2). It is not covered by #836, #899 or #817.
- **A version check alone is not enough.** It stops double sends, but it
  still loses hashes, still trusts lagging nodes, cannot stop a stale
  worker from *broadcasting* (it guards writes, not sends), leaves the
  nonce counter ahead of the record, and adds a new bug: the status job
  stops polling when it loses a version conflict.
- **A lease lock without fencing fixes nothing.** When the lease expires
  mid-work, a second worker runs alongside the first, and every `main` bug
  comes back.
- **The full reconciler design passes all seven properties**, including
  "every transaction eventually reaches a final status". Removing any one
  of its five rules breaks a property, so each rule is required.
- **TLC found a hole in the reconciler design** before any code was
  written: the status job must look for receipts while the transaction is
  in Sent, not only in Submitted (section 6).

Results are identical on both chain types.

## 1. What was checked

The model (`EvmTxLifecycle.tla`) contains:

| Worker | Code it mirrors |
| --- | --- |
| 2 prepare jobs | `prepare_transaction` (`evm_transaction.rs:651`) |
| submit job | `submit_transaction` (`evm_transaction.rs:891`) |
| resubmit job | `resubmit_transaction` (`evm_transaction.rs:1080`) |
| cancel API call | `cancel_transaction` (`evm_transaction.rs:1261`) |
| per-tx status job | `handle_status_impl` (`status.rs:1062`) and the pending/sent/submitted handlers |
| nonce recovery | `reconcile_tx_nonce_state` (`status.rs:827`) |
| nonce health | `detect_nonce_gap_ahead` (fills gaps with a NOOP) |
| Redis record | `partial_update` Lua (`transaction_redis.rs:2094`): field merge plus the final-status guard |
| status job result | `handle_result` (`transaction_status_handler.rs:73`) |

Each model step is one indivisible step in the code: one `partial_update`
Lua call, one RPC call, or the local work between two of them.

Faults the checker may inject at any point:

- an RPC `send_raw_transaction` that returns an error after the chain
  accepted the payload;
- a lagging node that answers "no receipt" for a mined payload;
- a lease that expires while its holder is still working (lease design).

## 2. Properties

| Property | What users are promised | Harm when broken |
| --- | --- | --- |
| NoDoubleEffect | One request never produces two mined transfers | Funds sent twice (#836) |
| NoNonceGap | Every nonce taken from the counter is used or still held | All later transactions of the relayer stall; Arbitrum rejects them |
| NoLostHash | Every payload the chain accepted is recorded in `hashes` | Receipt lookups miss it; a mined tx can be marked Failed (#899) |
| NoFalseFailure | `Failed` means none of the tx's payloads mined | Caller retries and pays twice (#817) |
| CancelHonest | After a cancel is accepted, the record keeps the NOOP and no new original is broadcast | A "cancelled" transfer still goes out |
| PollingNeverStops | While the tx is not final, its status job is still scheduled | Nobody looks at the tx again |
| EventuallyFinal | Every tx reaches a final status | Stuck transactions |

## 3. Designs compared

| Variant | Description |
| --- | --- |
| `main` | The code as it is today |
| `version` | `main` plus a compare-and-set version on every lifecycle write. A lost CAS returns `Ok`, which is what `handle_result` does today on `ConcurrentUpdateConflict` |
| `lease` | `main` plus a TTL lock (`DistributedLock`) around prepare, resubmit and cancel, without a fencing token |
| `reconciler` | Version CAS on every lifecycle write, plus an idempotent atomic nonce claim, each signed attempt saved before broadcast, and one `decide()` in the status job that keeps polling after conflicts and marks Failed only on an answer from a non-lagging node |

## 4. Results

Same on mempool and Arbitrum.

| Variant | NoDoubleEffect | NoNonceGap | NoLostHash | NoFalseFailure | CancelHonest | PollingNeverStops | EventuallyFinal |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `main` | FAIL | FAIL | FAIL | FAIL | FAIL | pass | not run |
| `version` | pass | FAIL | FAIL | FAIL | FAIL | **FAIL** | not run |
| `lease` | FAIL | FAIL | FAIL | FAIL | FAIL | pass | not run |
| `reconciler` | pass | pass | pass | pass | pass | pass | pass |

EventuallyFinal was run only on `reconciler`: the other variants already
fail safety properties, and their state graphs are too large for the
liveness check.

## 5. Findings on current `main`

Each finding is a counterexample from TLC, shortened to the steps that
matter. Full traces are in `results/mempool/main.<property>.txt`.

### Finding 1: one request mined twice (NoDoubleEffect, #836)

1. Prepare job A takes nonce 0 from the counter (`get_and_increment`,
   `evm_transaction.rs:797`). Nothing is saved yet.
2. The status job sees the tx still Pending and queues another prepare.
3. Prepare job B reads the tx: still no nonce.
4. A saves nonce 0, signs, writes Sent and queues submit.
5. B takes nonce 1 and writes Sent with its own payload. The record keeps
   both hashes.
6. Submit sends both payloads; both mine.

Root cause: the nonce is taken from the counter before the record says the
tx has one, and nothing stops a second writer holding an older copy.

### Finding 2 (new): cancel orphans a nonce (NoNonceGap)

1. Prepare takes nonce 0 and saves it.
2. Cancel reads the tx while it is still Pending.
3. Prepare signs and writes Sent.
4. Cancel continues on its Pending branch and calls
   `update_transaction_status(Canceled)` (`evm_transaction.rs:1282`).
5. The write succeeds: the Lua guard (`transaction_redis.rs:2150`) only
   rejects status changes on records that are already final, and Sent is
   not final.

Nonce 0 is now taken but never broadcast. Every later transaction from
that relayer waits behind it until nonce health fills the gap; on an
Arbitrum-style chain they are rejected.

### Finding 3: a landed payload loses its hash (NoLostHash, #899)

1. Cancel writes the NOOP and queues resubmit.
2. Resubmit signs the NOOP and sends it (`evm_transaction.rs:1145`).
3. The chain accepts it, but the RPC returns an error.
4. Resubmit writes hashes only after a successful send
   (`evm_transaction.rs:1234`), so the NOOP's hash is never saved.

The payload can now mine under a hash the relayer does not know, which
leads straight into finding 4.

### Finding 4: a mined tx marked Failed (NoFalseFailure, #817)

1. The original payload mines.
2. A resubmit's send triggers nonce recovery.
3. Nonce recovery (`status.rs:827`) asks a node that is behind; it answers
   "no receipt".
4. The code treats that as proof and writes Failed.

The caller retries and pays twice.

### Finding 5: a cancelled transfer still goes out (CancelHonest)

1. Prepare job B starts from a copy read before the cancel.
2. Cancel writes the NOOP payload and `is_canceled`
   (`evm_transaction.rs:1305`).
3. B writes Sent with a newly signed original over the NOOP, and replaces
   `hashes` with its own.

The API said the transfer was cancelled; the original is broadcast and
the NOOP's hash is gone. `partial_update` merges fields without any
version, so a stale writer always wins.

### Common cause

Findings 1, 2 and 5 share one cause: `partial_update` has no notion of
"the record changed since I read it". The only guard is "the record is
final", and these races all happen before that. Findings 3 and 4 are about
belief: an RPC error does not mean the send failed, and "no receipt" from
one node does not mean the tx did not mine.

## 6. Why the candidate designs fail or pass

### `version`: version check only

- **Passes NoDoubleEffect.** The second prepare's write loses the CAS.
- **NoNonceGap still fails**, via the same cancel race as finding 2: the
  counter moved, the record did not.
- **CancelHonest fails in a new way.** A resubmit holding a pre-cancel copy
  signs and *broadcasts* a new original. Its later write loses the CAS,
  but the payload is already on the network. A version check guards
  writes, not sends.
- **NoLostHash and NoFalseFailure fail** exactly as on `main`: a CAS does
  not change what the code believes about RPC errors and lagging nodes.
- **PollingNeverStops fails (new bug).** The status job reads the tx, a
  cancel bumps the version, the status job's write loses the CAS, and
  `handle_result` returns `Ok(())` on `ConcurrentUpdateConflict`, which
  ends the job. The tx then mines with nothing watching it. That branch is
  dead code for EVM today; adding a version check brings it to life.

### `lease`: lock without fencing

Every `main` failure reappears. Example (NoDoubleEffect): prepare A holds
the lease and takes nonce 0, the lease expires, prepare B acquires it,
reads the tx before A saved the nonce, takes nonce 1, and both payloads
mine. A lease reduces how often workers overlap; without a fencing token
checked at write time it cannot stop the overlap from doing harm.

### `reconciler`: passes everything

To show each rule is required, we removed them one at a time. Each removal
breaks the property it protects, within about a second of checking:

| Rule removed | Property broken | Counterexample |
| --- | --- | --- |
| Atomic nonce claim | NoNonceGap | Prepare takes nonce 0, cancel wins the CAS, prepare's save loses it; nonce 0 orphaned |
| Save attempt before send | NoLostHash | Resubmit sends the NOOP before saving it; the chain has it, `hashes` does not |
| Trust only non-lagging nodes | NoFalseFailure | Status job believes a lagging node and marks Failed after the original mined |
| Keep polling after a conflict | PollingNeverStops | Submit's write moves the version; the status job loses the CAS and stops |
| CAS on resubmit | CancelHonest | Resubmit holding a pre-cancel copy saves and broadcasts a new original |

**Design hole TLC found.** The first version of `decide()` looked for
receipts only in Submitted. The liveness check failed:

1. Submit sends the payload.
2. A cancel bumps the version.
3. Submit's "mark Submitted" write loses its CAS, so the tx stays in Sent.
4. The payload mines, but `decide()` in Sent only resends, forever.

Fix, now part of the design: `decide()` checks receipts for every saved
attempt in Sent as well as Submitted. More generally, losing a CAS must
never leave the tx in a state the reconciler cannot get out of.

## 7. What a working architecture needs

These follow directly from the results. A full proposal is a separate
document.

1. **A version on the record, checked on every lifecycle write.** Without
   it, stale writers win (findings 1, 2, 5). Bookkeeping writes (failure
   counters) can stay unversioned.
2. **A lost CAS never ends the status job.** It means "re-read and decide
   again". Today's `Ok(())` on `ConcurrentUpdateConflict` must change
   before any version check ships.
3. **Nonce claim and record update in one atomic, idempotent step.** A
   nonce must never exist only in the counter (finding 2, `version`).
4. **Every signed attempt saved, with a CAS, before it is broadcast.**
   This makes "the version check guards writes, not sends" safe: a stale
   worker cannot save, so it cannot send. It also means a send error can
   never lose a hash (finding 3).
5. **One `decide()` owns status transitions.** It checks receipts for all
   saved attempts in Sent and Submitted, and treats "no receipt" as
   unresolved unless the answer comes from a node known to be caught up
   (finding 4).
6. **Locks are optional.** They can cut wasted work, but safety comes from
   rules 1 to 5, not from the lock.

## 8. Confidence and limits

- **Bounds.** 2 concurrent prepares, 1 resubmit, 1 cancel, 2 signatures
  (`main`, `version`, `lease`) or 3 (`reconciler`), 1 send error, 1
  lagging answer, 1 lease expiry. Every failure needs only 2 workers, so
  these bounds are enough to find them. A pass holds within the bounds.
- **Liveness** (EventuallyFinal) was checked without send errors or
  lagging nodes: with them on, TLC did not finish within 2 hours. The
  safety properties include both faults.
- **One transaction.** Harm to later transactions is inferred from
  NoNonceGap, not modeled.
- **Abstracted.** Gas pricing and balances, timing and `valid_until`, the
  circuit breaker, Mined vs Confirmed, reverted receipts, webhooks and
  Redis failover. `main`'s hash recovery is modeled generously (it checks
  every known hash).
- **Design, not Rust.** A bug inside one step (a wrong field in a patch)
  is caught only if the model step mirrors the code exactly.
- **The check can fail.** Each reconciler rule was removed and caught. A
  scratch change to the real code (deleting the final-status guard in
  `partial_update`) was flagged by the drift check and caught by TLC in
  one second; the code was then restored and the baseline confirmed.

## 9. Reproduce

Needs Java 11+ and tla2tools 1.7.4 (`run.sh` prints the download command).

```bash
python3 formal/evm-tx-lifecycle/drift.py check          # modeled code unchanged?
cd formal/evm-tx-lifecycle
./run.sh main NoNonceGap mempool | python3 trace.py     # one finding, readable
./run.sh reconciler NoLostHash mempool sendBeforePersist | python3 trace.py
./matrix.sh mempool main version lease                  # about 10 minutes
./matrix.sh arbitrum                                    # about 1 hour
```

A failing check returns in about a second; a passing one explores the
whole state space (up to 76M states, 6 to 12 minutes). The project skill
`.claude/skills/verify-evm-tx-lifecycle/SKILL.md` describes how to keep the
model in step with code changes.
