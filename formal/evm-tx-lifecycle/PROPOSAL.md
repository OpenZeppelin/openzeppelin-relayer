# Proposal: race-safe EVM transaction lifecycle

Status: draft for discussion. Date: 2026-10-29. Based on `REPORT.md` in
this directory (model-checking results against `main` at `487bfd24`).

## 1. Problem

Several workers can act on one EVM transaction at the same time: prepare
(sometimes twice), submit, resubmit, the cancel API, the status job and
nonce recovery. Today they coordinate only through `partial_update`, a
field-level merge whose one guard is "do not change the status of a record
that is already final".

Model checking found that this lets current `main` break all five safety
properties we care about (REPORT.md section 5):

| # | What goes wrong | Issue |
| --- | --- | --- |
| 1 | Two prepares each take a nonce; both payloads mine | #836 |
| 2 | Cancel on a Pending tx races prepare and orphans a nonce | new |
| 3 | A resubmit lands but the RPC errors, so its hash is never saved | #899 |
| 4 | A lagging node's "no receipt" marks a mined tx Failed | #817 |
| 5 | A stale prepare overwrites an accepted cancel with the original | none |

How these happen in practice:

- **1:** if prepare takes longer than about 20 s (a slow RPC or a slow KMS
  signer), the status checker assumes it was lost and queues prepare
  again. Two prepares now run, and each takes a new nonce.
- **3:** our RPC client retries and fails over between providers, so a
  payload can land on chain even though the call we made returned an
  error. Resubmit saves the hash only after a successful call, so the
  landed payload's hash is never recorded.
- **4:** later, nonce recovery sees the nonce used, finds no receipt for
  any hash it knows (or asks a node that is behind), and marks Failed.
  The caller may retry and pay twice.

Most of these share one pattern: a worker reads the record, spends a
second or more signing or calling the RPC, then writes the record back
without checking whether someone else wrote in between. Findings 3 and 4
add a second pattern: treating an RPC error or a missing receipt as proof.

The open fixes (#845 for 1, #899 for 3, #862 for Arbitrum jams) each add
their own guard for one case, which makes the code harder to reason about
as a whole. The model shows why point fixes are not enough: a version
check alone fixes 1 but leaves 2 to 5 and adds a new bug, and a lease lock
without fencing fixes nothing (REPORT.md section 6).

## 2. Goals and non-goals

Goals, as checkable properties (definitions in REPORT.md section 2):

- NoDoubleEffect, NoNonceGap, NoLostHash, NoFalseFailure, CancelHonest
- PollingNeverStops and EventuallyFinal: no tx is abandoned or stuck
- Each step can ship on its own and leaves the system no worse
- Each step is checked against the model before merge

Non-goals:

- Changing gas pricing, fee bumping policy or balance checks
- Cross-relayer coordination or a per-relayer sequencer
- Changing Stellar or Solana flows (they may reuse the CAS primitive later)

## 3. Design in one paragraph

The transaction record in Redis is the single source of truth, and every
change to it is a compare-and-set on a version number. A worker that loses
the CAS does nothing further and leaves follow-up to the status job, which
never stops because of a conflict. A nonce is taken from the counter and
written to the record in one atomic, idempotent step. Every signed payload
is saved to the record (with a CAS) before it is broadcast, so the record
always knows every hash that could be on chain, and a worker holding a
stale copy cannot save and therefore cannot send. One pure function,
`decide()`, turns the record plus what the chain says into the next
action, and it only declares Failed when a node that is caught up says
none of the saved attempts mined.

This is the `reconciler` variant in the model. It passes all seven
properties, and removing any one of its five rules breaks one of them
(REPORT.md section 6, mutation table). So the five rules below are the
minimum, not a wish list.

## 4. The five rules

### Rule 1: versioned record, CAS on every lifecycle write

**Change.** Add `version: u64` to `TransactionRepoModel` (serde default 0,
so existing records load). Add a repository method:

```rust
async fn update_if(
    &self,
    tx_id: String,
    expected_version: u64,
    update: TransactionUpdateRequest,
) -> Result<TransactionRepoModel, RepositoryError>;
// Err(RepositoryError::ConcurrentUpdateConflict) when the version moved
```

In Redis this is the existing `partial_update` Lua
(`transaction_redis.rs:2094`) with one extra check at the top: if
`tx.version ~= expected` return a conflict marker; otherwise merge, set
`version = version + 1`, and update the indexes as today. The final-status
guard stays as a second line of defense. The in-memory repository gets the
same semantics so unit tests see conflicts.

**Which writes are versioned.** Every write that changes lifecycle state:
status, nonce, `network_data`, `hashes`, attempts, `is_canceled`,
`noop_count`. These go through `update_if`.

**Which writes are exempt.** Bookkeeping that never decides what is sent
or what the status is: the status-check failure counters in `metadata`
(`increment_status_check_failures`, `reset_status_check_consecutive_failures`),
index repair, `delete_at`. They keep their own Lua scripts and do **not**
bump the version, so a counter update never makes a lifecycle writer lose
its CAS.

**Why.** Findings 1, 2 and 5 are all "a writer holding an old copy wins".
A version is the fencing token that makes the old copy lose.

**Side benefit.** Stale reads become safe. Today a read from the replica
(`get_by_id`) that is a moment behind can lead to a wrong write. With CAS,
it leads to a conflict and a re-read, never to corruption.

### Rule 2: a lost CAS never ends the status job

**Change.** In `handle_result` (`transaction_status_handler.rs:73`), the
`is_concurrent_update_conflict` branch returns `Ok(())` today, which
completes the job. Change it to return
`Err(HandlerError::NotYetFinal(..))` (retry, re-read, decide again)
without touching the failure counters.

For the other jobs (prepare, submit, resubmit), a conflict means "the
record moved on without me": the job stops without any side effect (no
send, no follow-up job, no webhook), and the status job, which re-reads
every cycle (8 to 12 s), picks up from the new state.

**Why.** The `version` variant fails PollingNeverStops exactly here: the
status job reads, a cancel bumps the version, the status write loses, the
job completes, and the tx mines with nobody watching. That branch is dead
code for EVM today and comes alive with Rule 1.

**Order.** This must ship **before** CAS checks are enforced.

### Rule 3: atomic, idempotent nonce claim

**Change.** One Lua script, given the tx key, the counter key and the
expected version:

```
if tx.version ~= expected            -> return conflict
if tx.status is final                -> return conflict
if tx.network_data.nonce is set      -> return that nonce (idempotent retry)
n = INCR counter - 1                 -- same script, same atomic step
tx.network_data.nonce = n; tx.version = tx.version + 1
SET tx; update nonce index; return n
```

Both keys live on the same Redis primary (the relayer uses one primary
plus read replicas, not Redis Cluster), so one script can touch both.
`prepare_transaction` replaces the `get_and_increment` call at
`evm_transaction.rs:797` and the presign `partial_update` at `:816` with
this script.

The cancel path for a Pending tx (`evm_transaction.rs:1282`) becomes an
`update_if` with the version it read. If prepare claimed a nonce in the
meantime, cancel loses the CAS, re-reads, and takes the NOOP path for a
tx that has a nonce.

**Why.** Finding 2 and the `version` variant's NoNonceGap failure both
come from the window between taking a nonce from the counter and saving
it on the record. The `nonAtomicClaim` mutation confirms that a CAS alone
does not close that window.

**Relation to #845.** #845 claims the nonce on the record with a guarded
Lua script (`partial_update_if_evm_nonce_unset`), which is most of this
rule. As reviewed, the counter is still advanced outside that script, and
a leaked nonce is healed afterwards by counter rewind or NOOP fill. Moving
the increment into the same script removes the leak instead of healing
it, and adding the version check makes the cancel race lose cleanly.
#845 can land first and be extended.

### Rule 4: save every signed attempt before broadcasting it

**Change.** Add `attempts: Vec<EvmAttempt>` to the EVM network data:

```rust
pub struct EvmAttempt {
    pub hash: String,
    pub raw: Vec<u8>,          // exact signed bytes, for resend
    pub kind: AttemptKind,     // Original | Noop
    pub nonce: u64,
    pub created_at: String,
    // fee fields used, for reprice decisions
}
```

`hashes` stays, derived from `attempts`, so readers and the API do not
change. Every path that signs follows the same order:

1. sign
2. `update_if(version)`: append the attempt, set it as current
3. broadcast the saved bytes
4. on success: `update_if` to Submitted; on error: nothing to undo

If step 2 loses the CAS, the bytes are thrown away unsent. If step 3
errors after the chain accepted the payload, the hash is already saved.

**Resend versus reprice.** Two separate actions:

- **Resend:** send the current attempt's saved bytes again. Used when a
  send fails or the tx is not yet seen. It does not create an attempt and
  does not count toward backoff.
- **Reprice:** sign a new attempt with a higher fee, only when the
  backoff timer says so, or when the kind changes (cancel turns the next
  attempt into a NOOP).

Resending identical bytes is idempotent on every chain; signing is the
only step that creates new possible effects.

**One field, one job.** Today `hashes` is both the list of known hashes
and the attempt counter that drives the resubmit backoff. If we save a
hash before every send (as #899 does), failed sends during an RPC outage
grow the list and the backoff grows very fast. With the split above,
each attempt is one priced transaction, so "number of attempts" and
"number of reprices" mean the same thing again. Backoff counts attempts,
never sends, and gets an upper limit (today `timeout * 2^(n-1)` has no
cap and overflows around n = 50 with `ARBITRUM_TIME_TO_RESUBMIT = 20000`).

**Bounded size.** Cap stored attempts (for example 10), dropping `raw`
but keeping the hash of older ones. The cap limits storage only; it must
never cancel or fail a tx, since an RPC outage of a few minutes is not a
reason to give up on a valid transaction.

**Why.** Finding 3 (lost hash) and the `version` variant's CancelHonest
failure: a CAS guards writes, not sends, so a stale resubmit can still
broadcast. Saving before sending turns "can I write?" into "can I send?".
The `sendBeforePersist` and `noCasOnResubmit` mutations each break a
property, so both the order and the CAS are required.

**Relation to #899.** #899 records the resubmission hash before broadcast,
which is step 2 without the CAS. Adding the version check closes the
stale-resubmit hole the model found in the `version` variant.

### Rule 5: one `decide()` owns status transitions

**Change.** Move the decisions in `handle_status_impl` (`status.rs:1062`),
the pending/sent/submitted handlers and `reconcile_tx_nonce_state`
(`status.rs:827`) into one pure function:

```rust
fn decide(tx: &TransactionRepoModel, obs: &ChainObservation, now: Instant) -> Action;

enum Action {
    MarkMined { attempt: usize },  // receipt found for a saved attempt
    MarkFailed,                    // caught-up node: nonce used, no attempt mined
    Resend { attempt: usize },
    Reprice,                       // sign a new attempt (Rule 4)
    SignNoop,                      // cancel requested or expired
    Requeue(Job),                  // e.g. prepare for a Pending tx
    Wait,                          // unresolved, check again later
}
```

The handler gathers a `ChainObservation` (receipts for every saved attempt,
the account nonce, and the block height of the node that answered), calls
`decide()`, then applies the action with `update_if`. A conflict sends it
round again (Rule 2). Because `decide()` is pure, its table can be unit
tested exhaustively and compared line by line with the model.

The decision table, in order:

| Record state | Observation | Action |
| --- | --- | --- |
| any non-final | receipt for any saved attempt | MarkMined (Confirmed after depth, as today) |
| Sent or Submitted | no receipt, nonce not used yet | Resend or Reprice per backoff; SignNoop if cancel requested or expired |
| Sent or Submitted | nonce used, no receipt, node caught up | MarkFailed |
| Sent or Submitted | nonce used, no receipt, node behind | Wait |
| Pending, no nonce | (none) | Requeue prepare |
| Pending, nonce claimed | stuck past timeout | Requeue prepare (idempotent: Rule 3 returns the same nonce) |

Two rules in this table came from the model:

- **Receipts are checked in Sent too**, not only in Submitted. A cancel
  can bump the version between submit's broadcast and its "mark
  Submitted" write, leaving a mined tx in Sent. Without this row the
  first reconciler design never reached a final status (REPORT.md
  section 6).
- **"Caught up" is required for Failed.** A node is caught up for this
  question when its head is at or past the block where it reports the
  nonce as used, and the receipt lookup goes to that same node (or one at
  least as far ahead). Otherwise the answer is Wait. With Rule 4 there
  are no unknown hashes, so "nonce used, no saved attempt mined, on a
  caught-up node" really means another transaction took the nonce. The
  `trustLaggingNode` mutation breaks NoFalseFailure.

`valid_until` expiry keeps today's behavior (sign a NOOP, not Failed).

**Why not "wait until `valid_until`, then mark Failed"?** It is the
obvious fix for #817, but `valid_until` is a deadline, not proof: a tx
can mine before it and a lagging node can still say "no receipt" after
it. It is also slow (8 h by default), so a real failure, where another
transaction took the nonce, would be reported hours late. The caught-up
rule gives a sound answer as soon as one exists.

## 5. Cancel and replace

**Today.** The cancel API reads the tx, builds a NOOP (one RPC call), then
writes it with no check. A resubmit or a second prepare running at the
same time can write the original back (finding 5). The API answers
"cancelled" and the transfer still goes out; nobody notices. Replace
(`replace_transaction`) has the same read, build, write shape and the
same race. This is a bug on `main` today, not a side effect of this
proposal.

**With the rules above**, cancel becomes a request that is settled on
chain:

1. The API does `update_if`: set `is_canceled`, keep status. If the tx is
   Pending with no nonce, it goes straight to Canceled. If a nonce was
   claimed, the next attempt will be a NOOP.
2. `decide()` sees `is_canceled` and returns SignNoop; the NOOP is saved
   before it is sent (Rule 4).
3. Whichever attempt mines decides the outcome: the NOOP (Canceled) or an
   original already in flight (Confirmed).

Replace follows the same path with a new payload instead of a NOOP.

**What the API tells the caller when its write loses the CAS:**

| Option | What the caller sees | Trade-off |
| --- | --- | --- |
| Return 200 anyway | "Cancelled" | Not true; this is the bug |
| Return 409 Conflict | "Changed by someone else, try again" | Honest, but a new status code for clients and the SDK |
| Reload and retry inside the API | Almost always success | A little more code; after the reload the tx may already be mined, and then the existing "cannot cancel" error is the right answer |

Recommended: reload and retry inside the API, bounded to a few attempts.

**What success means.** Even a clean write does not guarantee the
transfer will not happen: an original already broadcast can still mine
before the NOOP. Today the API returns `200` with the tx
(`api/controllers/relayer.rs:678`), which reads as "cancelled". The honest
response is "cancel requested", for example `202` with the current
status, with the final outcome reported by status and webhook. **This is
an API change and needs a product decision**; the safety rules do not
depend on it, only the wording of the promise does.

## 6. What about locks?

A per-tx lock (or a lease) can still be useful to avoid wasted work, such
as two prepares signing at the same time. It is optional. The model shows
a lease without a fencing token fixes nothing, and with the version as
the fencing token the lock adds no safety. If added, it must never be the
thing that decides whether a write or send is allowed.

## 7. Arbitrum (#843)

**The problem is throughput, not a race.** Most chains have a mempool:
nonce 5 can be sent before nonce 4 lands, and the node holds it. Arbitrum
rejects any nonce above the next one. In a backlog we therefore move one
transaction per status check (8 to 12 s), and if requests arrive faster
than that, the queue never drains.

**Fix: chaining.** When nonce N is accepted, send N+1 straight away. This
is the useful core of #862, and we keep it, gated on `is_arbitrum()`. The
wider "no mempool" network flag also covers Optimism networks, which do
not need it.

**What we drop from #862.** The rest of #862 solves the other bugs again
in its own way: it saves before sending on some networks only, rolls back
from an old copy (the same stale-copy race as finding 1), and lowers the
attempt limit to 10, which can cancel a valid tx after about 3.5 minutes
of RPC outage. Rules 1, 3 and 4 make all of that unnecessary.

**Model.** Verdicts were identical on the strict-nonce chain within the
model's bounds. Strict nonces make gaps much more expensive (later
transactions are rejected, not queued), so Rule 3 matters most there.
Chaining lands last, on top of these rules.

## 8. Prior art: OpenZeppelin Defender

Defender avoided most of these races by design, and the rules above line
up with what it did:

| Defender | Here |
| --- | --- |
| Picks the nonce once, when the request arrives; never prepares twice | Rule 3: atomic, idempotent claim; re-prepare returns the same nonce |
| Saves the signed transaction, then sends it | Rule 4 |
| "Retry" resends the same bytes and is not an attempt; "resubmit" signs a new, higher-priced tx | Rule 4: resend versus reprice |
| Never marks Failed only because a receipt is missing | Rule 5: caught-up rule |

## 9. What happens to each open PR

| PR | Becomes |
| --- | --- |
| #845 (#836 double send) | Reduced to the atomic nonce claim (Rule 3) on top of the version check. Its per-field guards ("only if nonce empty", "only if status is still X", append-only hashes) are replaced by the version check |
| #899 (#894, lost hash) | Extended: save the whole attempt (bytes, hash, priced time) with a CAS, resend versus reprice, attempts separate from backoff |
| #862 (#843, Arbitrum) | Closed; replaced by a small PR with chaining and the head check only, gated on `is_arbitrum()`, landing last |
| #817 (false failure) | New PR as part of `decide()` (Rule 5), using the caught-up rule rather than waiting for `valid_until` |

## 10. Rollout

Version checks cannot be switched on while some pods still write without
bumping the version: an old pod's write would not move the version, and a
new pod's CAS would succeed over it. During a rolling deploy that is no
worse than today, with one exception: as soon as any pod enforces CAS,
conflicts start to happen, and on a pod without PR 1 a conflict ends the
status job. So PR 1 ships first and the check is enforced in a separate
step.

| PR | Change | Fixes | Safe on its own because |
| --- | --- | --- | --- |
| 1 | `handle_result`: conflict retries instead of completing; a losing worker sends no job or webhook | prerequisite for PollingNeverStops | conflicts do not occur for EVM yet; no behavior change today |
| 2 | Add `version`; every lifecycle write bumps it; exempt writes do not; no checks yet | prerequisite | writes behave as today, plus a counter |
| 3 | `update_if` and in-memory parity; CAS behind a flag `EVM_TX_CAS_ENABLED` (off) | 1, 5 once on | flag off means no behavior change |
| 4 | Enable the flag after all pods run PR 2; cancel and replace reload and retry on conflict | 1, 5 | PR 1 already makes conflicts retry |
| 5 | Atomic idempotent nonce claim (reduced #845); cancel-on-Pending via CAS | 2 | needs PR 4 for the cancel race |
| 6 | Attempts list, save before send, resend vs reprice, backoff counts attempts with a cap (extended #899) | 3, stale broadcast | each path changes order only |
| 7 | `decide()` with Sent receipts and caught-up rule (#817) | 4, EventuallyFinal | pure function, testable in isolation |
| 8 | Cancel and replace as requests (`202`), if approved | honest API | API-only |
| 9 | Arbitrum chaining and head check, `is_arbitrum()` only (replaces #862) | #843 jams | builds on 5 to 7 |

Migration and rollback: old records have no version and read as 0, so no
data migration is needed. The flag turns CAS off; the version field and
attempts list are additive and ignored by older releases, so rolling back
works.

## 11. How each PR is verified

- **Model.** Each PR moves one `main` branch of `EvmTxLifecycle.tla` to
  match the code (the target behavior is already written as the
  `reconciler` branches). Run the project skill
  `verify-evm-tx-lifecycle`: drift check, update the action, run the
  affected properties, `drift.py update`. Expected progression: PR 4
  turns NoDoubleEffect green; PR 5 NoNonceGap; PR 6 NoLostHash and
  CancelHonest; PR 7 NoFalseFailure and EventuallyFinal. PR 1 must keep
  PollingNeverStops green throughout.
- **Lua tests.** Redis-gated tests for `update_if` (conflict, bump,
  exempt writes do not bump) and the nonce claim (idempotent retry,
  final status, conflict).
- **Race tests.** For each finding, a test that interleaves the two
  workers in the order of the TLC trace (the trace files in
  `results/mempool/main.*.txt` are the scripts) and asserts the property.
- **`decide()` table tests.** One test per row of the decision table,
  including "nonce used, node behind" returning Wait.
- **Backoff tests.** Failed sends during a simulated RPC outage do not
  grow the attempt count or the wait.

## 12. Alternatives considered

| Alternative | Why not |
| --- | --- |
| Version check only | Passes NoDoubleEffect only; adds the stop-polling bug (REPORT.md 6) |
| Lease or distributed lock without fencing | Fails every property `main` fails; expiry lets two holders run |
| Per-relayer single writer (actor or queue partition) | Would also work, but changes the job system and serializes all transactions of a relayer; larger change for the same guarantees |
| Keep point fixes (#845, #899, #862) as they are | Each fixes one symptom with its own guard; the model shows the remaining races |

## 13. Differences from the "version, save, resend" proposal

A parallel proposal (version check, save before send, send again instead
of sign again, plus separate fixes for #817 and #843) shares this
document's structure: fix the common cause once, keep the Arbitrum fix
small and last. Several of its points are folded in above (sections 1, 4,
5, 7, 8, 9). It corresponds to the model's `version` variant plus Rule 4,
and on its own it leaves these gaps:

| Gap | Model evidence | Covered here by |
| --- | --- | --- |
| "On conflict, stop; the status checker will look again" does not hold for the status checker itself: `handle_result` returns `Ok(())` on conflict and ends polling | `version` fails PollingNeverStops | Rule 2, PR 1 first |
| The version check does not make nonce allocation safe: the counter moves even when the record write loses | `version` fails NoNonceGap; `nonAtomicClaim` mutation | Rule 3 |
| With CAS on, a cancel can make submit's "mark Submitted" lose, leaving a mined tx in Sent, where the status job does not look for receipts | liveness failure in the first reconciler design | Rule 5, Sent row |
| Waiting for `valid_until` before Failed is not proof and takes 8 h; the false Failed comes from nonce recovery | `trustLaggingNode` mutation | Rule 5, caught-up rule |
| "The version goes up on every write" includes the failure-counter scripts, which would invalidate every worker on each failed status check | (design review) | Rule 1, exempt writes |
| Cancel with a clean write still does not guarantee the original will not mine | `main` and `version` fail CancelHonest | Section 5, cancel as a request |

## 14. Risks and open questions

- **API wording for cancel and replace** (section 5) needs a product
  decision.
- **Record size.** Storing raw bytes per attempt; bounded by the attempt
  cap.
- **Extra Redis round trip** per signed attempt (save before send). One
  Lua call, on a path that already makes several.
- **Conflict storms.** A busy tx could see repeated conflicts between the
  status job and resubmit. Conflicts are cheap (no send), and the status
  job's backoff spreads retries; add a metric for conflicts per tx.
- **"Caught up" across providers.** The RPC client retries and fails
  over, so one logical request can hit different nodes. The receipt
  lookup must be pinned to the node that reported the nonce, or compare
  head heights. Needs a look at the provider layer.
- **Model scope.** One transaction, small bounds, liveness without
  send errors or lagging nodes (REPORT.md section 8). Cross-transaction
  effects are inferred, not checked. Replace is not modeled yet; it has
  the same shape as cancel and should be added before PR 8.

## 15. Finding to fix map

| Finding | Rule(s) | Mutation that proves the rule is needed |
| --- | --- | --- |
| 1 Double send | 1, 3 | (passes with CAS; atomic claim keeps it gap-free) |
| 2 Cancel orphans nonce | 1, 3 | `nonAtomicClaim` |
| 3 Lost hash | 4 | `sendBeforePersist` |
| 4 False Failed | 4, 5 | `trustLaggingNode` |
| 5 Cancel overwritten | 1, 4 | `noCasOnResubmit` |
| Stop polling (new with CAS) | 2 | `conflictStopsPolling` |
| Stuck in Sent (design hole) | 5 | found by the liveness check |
