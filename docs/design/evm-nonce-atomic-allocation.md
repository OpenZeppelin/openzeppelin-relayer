# EVM nonce allocation: recovery preflight

All quick-triage signals apply: recoverable work, external spend, concurrent workers and deadlines.

## Authority and contract

The primary Redis transaction body owns the transaction's assigned EVM nonce. The payer counter
owns the next unallocated nonce; the nonce index is the gap scanner's discovery path. Chain receipts
own execution. Queue state, a lost reply or a null nonce in a stale worker grants no new allocation.
Signing/broadcast remains irreversible and is allowed only after durable assignment. No counter
rewind, filler transaction, expiry extension or new transaction UUID is part of this fix.

The Redis allocator must commit the counter, exact transaction body and nonce index together.
It reads and serializes JSON in Rust, then compares the original body, counter and relayer lookup
inside one Lua operation before writes. Exact integer bytes never pass through Lua cjson/f64.
It validates Pending EVM status, payer, relayer and transaction ID, refuses canceled/final/missing/
malformed/occupied states, and uses checked u64 increment. Concurrent changes retry a bounded
number of times, then return an error; Redis failures never fall back to a separate increment.
The Redis backend and transaction repository must share the configured prefix/primary, as in the existing bootstrap; missing/mismatched bindings fail closed. Memory storage retains its existing non-durable behavior and makes no crash-safety claim.

## Cross-layer matrix

| Transaction nonce / state | Counter/index | Queue or reply | Classification / action |
|---|---|---|---|
| Pending, none | unchanged, slot empty | live/retried/missing | Allocate all three records atomically; normal queue recovery if missing. |
| Pending, assigned | advanced, index points to this tx | reply lost / retry | Reuse primary body; never increment again. |
| Pending, assigned | concurrent allocation beyond it | retry | Preserve this nonce and later allocations. |
| Sent/Submitted/Mined/final | any | stale prepare | Refuse allocator; reload through existing lifecycle. |
| Missing/wrong relayer/payer/non-EVM/malformed | any | any | Fail closed, no writes or signing. |
| Pending, none | chosen index belongs to another tx | any | Refuse inconsistency, no writes or counter rewind. |
| Pending, none | counter/body changes during await | any | CAS rejects; reload, bounded retry. |
| Assigned or not | Redis unavailable/ambiguous reply | any | Error; normal recovery reads original primary body. |
| Legacy gap already present | advanced | any | No automatic repair by this change; separately owned remediation. |

These rows cover present/missing/invalid bodies, absent/valid/malformed/overflow counters,
empty/self/other indexes, all status classes, and queue presence independently.

## Safety, liveness and typestate

Never expose an increment without its transaction nonce/index. Never overwrite a concurrent
transaction mutation, assign an occupied nonce or recreate a missing transaction. Assigned
Pending work may resume signing with the original nonce; final or progressed work may not be
resurrected by allocation. An ambiguous reply is not authorization to allocate another nonce.

If durable Pending work and its queue recovery remain live, assignment/signing eventually resumes
unless Redis/provider/signing remains unavailable, the transaction expires, ownership/configuration
changes or the bounded contention budget is exhausted. Those conditions retain existing retry or
terminal policy, not additional nonce authority. Host disk loss, Redis Cluster multi-slot operation,
rollback to an older binary and mixed old/new allocator writers are not qualified here.

## Concurrency and diagnostics

Actors: preparation workers, native recovery, status/cancel writers and nonce-health scanners.
CAS compares primary bytes immediately before the irreversible allocation write group. Nonce
health sees the nonce index in the same atomic group and cannot mistake an allocated slot for
an empty gap. Existing cross-process serial-preparation requirements remain separate; this does
not assert exactly-once outer signing or fix unrelated stale post-sign writers.

Diagnostics distinguish missing scope/body, wrong owner/state, malformed/overflow counter,
occupied slot, Redis failure and exhausted contention. Retry authority is bounded to the original
UUID. No TTL-based dedup or expiring reservation is introduced.

## Crash and verification boundaries

Before atomic commit: no allocation. After commit but before reply: body/index/counter all survive
and recovery reuses the nonce. During signing: existing body nonce survives. After signed persistence
but before broadcast: discard dead sender's held RPC and require vendor recovery to broadcast.
A separate request must then use and mine with the immediately following nonce.

Keep the pinned unmodified INCR reply-loss reproducer. The patched image uses the successful
allocation-script reply as its crash boundary, with transaction nonce already present. Test concurrent
same/different UUID allocation, changed body/counter between read and commit, lost reply, scope and
status refusal, overflow/malformed/occupied/missing records, exact JSON and nonce-index visibility.
Require real OZ+Redis SIGKILL recovery with the strict original-nonce gate, plus the later signed
boundary. No staging authority is implied. Gap check: all technique sections and failure classes
above are explicit; native observations, not this document, determine qualification.
