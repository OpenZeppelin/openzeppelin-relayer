--------------------------- MODULE EvmTxLifecycle ---------------------------
(***************************************************************************)
(* One EVM transaction moving through the relayer, with every worker that  *)
(* can touch it running concurrently.  Each action is one indivisible step *)
(* in the Rust code: a single Redis Lua call (partial_update), a single    *)
(* RPC call, or a local computation between them.  Comments name the code. *)
(*                                                                         *)
(* Variant selects the design under test:                                  *)
(*   "main"       code on main today                                       *)
(*   "version"    main + compare-and-set on a record version; a lost CAS   *)
(*                returns Ok (as transaction_status_handler.rs:122 does)   *)
(*   "lease"      main + a TTL lease lock (DistributedLock, utils/redis.rs)*)
(*                around prepare/resubmit/cancel, with no fencing token    *)
(*   "reconciler" version CAS + idempotent atomic nonce claim + attempts   *)
(*                saved before broadcast + one decide() in the status job  *)
(*                                                                         *)
(* Mutation breaks one rule of "reconciler", to show each property can     *)
(* fail.  It is "none" for normal runs.                                    *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS Variant, Chain, Mutation, MaxSigs, ErrBudget, LagBudget, ExpireBudget,
          None   \* model value: "no value" (comparable with anything)

ASSUME Variant \in {"main", "version", "lease", "reconciler"}
ASSUME Chain \in {"mempool", "arbitrum"}
ASSUME Mutation \in {"none", "nonAtomicClaim", "sendBeforePersist",
                     "trustLaggingNode", "conflictStopsPolling", "noCasOnResubmit"}

Final == {"confirmed", "failed", "canceled"}
Prepares == {"P1", "P2"}
Slots == Prepares \cup {"S", "R", "C", "T", "N"}
\* P1/P2 prepare jobs, S submit job, R resubmit job, C cancel API call,
\* T the per-tx status job, N a nonce-recovery status check.

Versioned == Variant \in {"version", "reconciler"}
Recon == Variant = "reconciler"
Leased(s) == Variant = "lease" /\ s \in {"P1", "P2", "R", "C"}

VARIABLES
  rec,            \* the tx record in Redis (TransactionRepoModel, reduced)
  counter,        \* relayer nonce counter (transaction_counter_service)
  nextId,         \* next signature id; bounds how many payloads get signed
  chainNonce,     \* account nonce on chain
  pool,           \* payloads the chain holds but has not mined
  mined,          \* payloads mined
  accepted,       \* history: every payload the chain ever accepted
  w,              \* per-slot worker state: pc, local copy, nonce, payload
  q,              \* per-slot "job queued" flag
  tAlive,         \* the status job for this tx is still scheduled
  lock,           \* lease holder (Variant = "lease")
  cancelLeft,     \* the API may still issue one cancel
  cancelAccepted, \* history: the cancel API call succeeded
  lateSigned,     \* history: ids of "orig" payloads signed after cancel
  errLeft,        \* budget of RPC send errors (incl. error-after-landing)
  lagLeft,        \* budget of lagging-node "no receipt" answers
  expLeft         \* budget of lease expiries while the holder still runs

vars == <<rec, counter, nextId, chainNonce, pool, mined, accepted, w, q,
          tAlive, lock, cancelLeft, cancelAccepted, lateSigned,
          errLeft, lagLeft, expLeft>>

Idle == [pc |-> "idle", loc |-> None, nn |-> None, sig |-> None, res |-> None]

Init ==
  /\ rec = [st |-> "pending", ver |-> 0, nonce |-> None, kind |-> "orig",
            cur |-> None, hashes |-> {}, canceled |-> FALSE]
  /\ counter = 0
  /\ nextId = 1
  /\ chainNonce = 0
  /\ pool = {}
  /\ mined = {}
  /\ accepted = {}
  /\ w = [s \in Slots |-> Idle]
  /\ q = [s \in Slots |-> s = "P1"]   \* the request enqueues one prepare job
  /\ tAlive = TRUE                     \* schedule_initial_status_check
  /\ lock = None
  /\ cancelLeft = 1
  /\ cancelAccepted = FALSE
  /\ lateSigned = {}
  /\ errLeft = ErrBudget
  /\ lagLeft = LagBudget
  /\ expLeft = ExpireBudget

-----------------------------------------------------------------------------
(* partial_update (transaction_redis.rs:2094): field-level JSON merge in   *)
(* one Lua script.  Guard: a patch that sets status on a final record is   *)
(* dropped whole and the current record is returned.                       *)
Merge(r, p) == [f \in DOMAIN r |-> IF f \in DOMAIN p THEN p[f] ELSE r[f]]
Rejected(p) == rec.st \in Final /\ "st" \in DOMAIN p
\* Versioned designs bump the version only when the write changes the
\* record; a write that changes nothing (e.g. Submitted -> Submitted on a
\* resend) cannot invalidate anyone's copy.  This also keeps the model finite.
NewRec(p) ==
  IF Rejected(p) THEN rec
  ELSE LET m == Merge(rec, p)
       IN IF Versioned /\ m # rec THEN [m EXCEPT !.ver = rec.ver + 1] ELSE m
\* CAS: the write applies only if the version is the one this worker read.
Fresh(s) == ~Versioned \/ rec.ver = w[s].loc.ver
Release(s) == IF lock = s THEN None ELSE lock
Done(s) == w' = [w EXCEPT ![s] = Idle]

-----------------------------------------------------------------------------
(* The chain.  send_raw_transaction result, as classify_submission_error   *)
(* sees it.  Arbitrum rejects any nonce above the next one.                *)
Classify(p) ==
  IF p \in pool \/ p \in mined THEN "known"            \* AlreadyKnown
  ELSE IF p.n < chainNonce THEN "low"                   \* NonceTooLow
  ELSE IF Chain = "arbitrum" /\ p.n > chainNonce THEN "high"  \* NonceTooHigh
  ELSE "accept"
\* An accepted send may still return an error to us after it landed
\* (RPC retry/failover): "landedErr".  "err" did not land.
Outcomes(p) ==
  IF Classify(p) = "accept"
    THEN {"ok"} \cup (IF errLeft > 0 THEN {"landedErr", "err"} ELSE {})
    ELSE {Classify(p)}
Lands(o) == o \in {"ok", "landedErr"}
SendFx(p, o) ==
  /\ pool' = IF Lands(o) THEN pool \cup {p} ELSE pool
  /\ accepted' = IF Lands(o) THEN accepted \cup {p} ELSE accepted
  /\ errLeft' = IF o \in {"landedErr", "err"} THEN errLeft - 1 ELSE errLeft
SignFx(k) ==
  /\ nextId' = nextId + 1
  /\ lateSigned' = IF cancelAccepted /\ k = "orig"
                     THEN lateSigned \cup {nextId} ELSE lateSigned

-----------------------------------------------------------------------------
(* PREPARE  evm_transaction.rs:651 prepare_transaction                     *)
PrepStart(s) ==
  /\ w[s].pc = "idle" /\ q[s]
  /\ Leased(s) => lock = None
  /\ q' = [q EXCEPT ![s] = FALSE]
  /\ IF rec.st = "pending"                    \* ensure_status(Pending)
       THEN /\ w' = [w EXCEPT ![s] = [Idle EXCEPT !.pc = "alloc", !.loc = rec]]
            /\ lock' = IF Leased(s) THEN s ELSE lock
       ELSE UNCHANGED <<w, lock>>
  /\ UNCHANGED <<rec, counter, nextId, chainNonce, pool, mined, accepted,
                 tAlive, cancelLeft, cancelAccepted, lateSigned, errLeft,
                 lagLeft, expLeft>>

PrepAlloc(s) ==
  /\ w[s].pc = "alloc"
  /\ IF Recon /\ Mutation # "nonAtomicClaim"
       \* reconciler: one Lua script claims the nonce if the record has none,
       \* else returns the existing one.  A second prepare gets the same N.
       THEN IF rec.st = "pending"
              THEN LET r2 == IF rec.nonce = None
                               THEN [rec EXCEPT !.nonce = counter, !.ver = rec.ver + 1]
                               ELSE rec
                   IN /\ rec' = r2
                      /\ counter' = IF rec.nonce = None THEN counter + 1 ELSE counter
                      /\ w' = [w EXCEPT ![s].pc = "sign", ![s].loc = r2, ![s].nn = r2.nonce]
                      /\ UNCHANGED lock
              ELSE /\ Done(s) /\ lock' = Release(s) /\ UNCHANGED <<rec, counter>>
       \* main: evm_transaction.rs:781 reuse the nonce on the local copy,
       \* else :794 get_and_increment BEFORE anything is written.
       ELSE IF w[s].loc.nonce # None
              THEN /\ w' = [w EXCEPT ![s].pc = "sign", ![s].nn = w[s].loc.nonce]
                   /\ UNCHANGED <<rec, counter, lock>>
              ELSE /\ w' = [w EXCEPT ![s].pc = "presign", ![s].nn = counter]
                   /\ counter' = counter + 1
                   /\ UNCHANGED <<rec, lock>>
  /\ UNCHANGED <<q, nextId, chainNonce, pool, mined, accepted, tAlive,
                 cancelLeft, cancelAccepted, lateSigned, errLeft, lagLeft, expLeft>>

\* evm_transaction.rs:808 presign_update: partial_update(network_data with nonce)
PrepPresign(s) ==
  /\ w[s].pc = "presign"
  /\ LET p == [nonce |-> w[s].nn] IN
       IF Fresh(s)
         THEN /\ rec' = NewRec(p)
              /\ w' = [w EXCEPT ![s].pc = "sign", ![s].loc = NewRec(p)]
              /\ UNCHANGED lock
         ELSE /\ Done(s) /\ lock' = Release(s) /\ UNCHANGED rec
  /\ UNCHANGED <<counter, q, nextId, chainNonce, pool, mined, accepted, tAlive,
                 cancelLeft, cancelAccepted, lateSigned, errLeft, lagLeft, expLeft>>

\* evm_transaction.rs:826 sign_transaction (KMS / local signer)
PrepSign(s) ==
  /\ w[s].pc = "sign"
  /\ IF nextId > MaxSigs
       THEN /\ Done(s) /\ lock' = Release(s) /\ UNCHANGED <<nextId, lateSigned>>
       ELSE LET k == IF Recon THEN w[s].loc.kind ELSE "orig"
            IN /\ w' = [w EXCEPT ![s].pc = "postsign",
                                 ![s].sig = [n |-> w[s].nn, k |-> k, id |-> nextId]]
               /\ SignFx(k)
               /\ UNCHANGED lock
  /\ UNCHANGED <<rec, counter, q, chainNonce, pool, mined, accepted, tAlive,
                 cancelLeft, cancelAccepted, errLeft, lagLeft, expLeft>>

\* evm_transaction.rs:842 postsign_update (status Sent, network_data, hashes
\* = local copy's hashes + new hash), then :858 produce_submit_transaction_job.
PrepPost(s) ==
  /\ w[s].pc = "postsign"
  /\ LET sg == w[s].sig
         p == [st |-> "sent", nonce |-> w[s].nn, kind |-> sg.k, cur |-> sg,
               hashes |-> w[s].loc.hashes \cup {sg}]
     IN IF Fresh(s)
          THEN /\ rec' = NewRec(p)
               /\ q' = [q EXCEPT !["S"] = TRUE]
          ELSE UNCHANGED <<rec, q>>
  /\ Done(s) /\ lock' = Release(s)
  /\ UNCHANGED <<counter, nextId, chainNonce, pool, mined, accepted, tAlive,
                 cancelLeft, cancelAccepted, lateSigned, errLeft, lagLeft, expLeft>>

-----------------------------------------------------------------------------
(* SUBMIT  evm_transaction.rs:891 submit_transaction: send the raw bytes   *)
(* stored in the record (resend), then mark Submitted.                     *)
SubStart ==
  /\ w["S"].pc = "idle" /\ q["S"]
  /\ q' = [q EXCEPT !["S"] = FALSE]
  /\ IF rec.st \in {"sent", "submitted"} /\ rec.cur # None
       THEN w' = [w EXCEPT !["S"] = [Idle EXCEPT !.pc = "send", !.loc = rec]]
       ELSE UNCHANGED w
  /\ UNCHANGED <<rec, counter, nextId, chainNonce, pool, mined, accepted,
                 tAlive, lock, cancelLeft, cancelAccepted, lateSigned, errLeft,
                 lagLeft, expLeft>>

SubSend ==
  /\ w["S"].pc = "send"
  /\ LET p == w["S"].loc.cur IN
     \E o \in Outcomes(p):
       /\ SendFx(p, o)
       /\ CASE o \in {"ok", "known"} ->
                 w' = [w EXCEPT !["S"].pc = "mark"] /\ UNCHANGED q
            [] o \in {"landedErr", "err"} ->            \* Err -> job retried
                 Done("S") /\ q' = [q EXCEPT !["S"] = TRUE]
            [] o = "low" ->                    \* schedule_nonce_recovery_status_check
                 Done("S") /\ q' = [q EXCEPT !["N"] = TRUE]
            [] o = "high" -> Done("S") /\ UNCHANGED q
  /\ UNCHANGED <<rec, counter, nextId, chainNonce, mined, tAlive, lock,
                 cancelLeft, cancelAccepted, lateSigned, lagLeft, expLeft>>

SubMark ==
  /\ w["S"].pc = "mark"
  /\ rec' = IF Fresh("S") THEN NewRec([st |-> "submitted"]) ELSE rec
  /\ Done("S")
  /\ UNCHANGED <<counter, q, nextId, chainNonce, pool, mined, accepted, tAlive,
                 lock, cancelLeft, cancelAccepted, lateSigned, errLeft, lagLeft,
                 expLeft>>

-----------------------------------------------------------------------------
(* RESUBMIT  evm_transaction.rs:1080 resubmit_transaction: reprice, sign,  *)
(* send, and only then write network_data + hashes (main).                 *)
PersistFirst == Recon /\ Mutation # "sendBeforePersist"

ResStart ==
  /\ w["R"].pc = "idle" /\ q["R"]
  /\ Leased("R") => lock = None
  /\ q' = [q EXCEPT !["R"] = FALSE]
  /\ IF rec.st \in {"sent", "submitted"} /\ rec.nonce # None
       THEN /\ w' = [w EXCEPT !["R"] = [Idle EXCEPT !.pc = "sign", !.loc = rec]]
            /\ lock' = IF Leased("R") THEN "R" ELSE lock
       ELSE UNCHANGED <<w, lock>>
  /\ UNCHANGED <<rec, counter, nextId, chainNonce, pool, mined, accepted,
                 tAlive, cancelLeft, cancelAccepted, lateSigned, errLeft,
                 lagLeft, expLeft>>

\* :1106 calculate_bumped_gas_price + :1123 sign_transaction.  Once the
\* signature budget is used up, the model resends the stored bytes instead
\* (bounded repricing; keeps the model finite).
ResSign ==
  /\ w["R"].pc = "sign"
  /\ LET l == w["R"].loc IN
     IF nextId > MaxSigs
       THEN /\ w' = [w EXCEPT !["R"].pc = "send", !["R"].sig = l.cur, !["R"].res = "resend"]
            /\ UNCHANGED <<nextId, lateSigned>>
       ELSE /\ w' = [w EXCEPT !["R"].pc = IF PersistFirst THEN "persist" ELSE "send",
                              !["R"].sig = [n |-> l.nonce, k |-> l.kind, id |-> nextId]]
            /\ SignFx(l.kind)
  /\ UNCHANGED <<rec, counter, q, chainNonce, pool, mined, accepted, tAlive,
                 lock, cancelLeft, cancelAccepted, errLeft, lagLeft, expLeft>>

\* reconciler only: save the new attempt (CAS) before broadcasting it.
ResPersist ==
  /\ w["R"].pc = "persist"
  /\ LET sg == w["R"].sig
         p == [cur |-> sg, hashes |-> w["R"].loc.hashes \cup {sg}]
     IN IF Fresh("R") \/ Mutation = "noCasOnResubmit"
          THEN /\ rec' = NewRec(p)
               /\ w' = [w EXCEPT !["R"].pc = "send", !["R"].loc = NewRec(p)]
               /\ UNCHANGED lock
          ELSE /\ Done("R") /\ lock' = Release("R") /\ UNCHANGED rec
  /\ UNCHANGED <<counter, q, nextId, chainNonce, pool, mined, accepted, tAlive,
                 cancelLeft, cancelAccepted, lateSigned, errLeft, lagLeft, expLeft>>

\* :1129 send_raw_transaction
ResSend ==
  /\ w["R"].pc = "send"
  /\ LET sg == w["R"].sig
         isNew == w["R"].res # "resend"
     IN \E o \in Outcomes(sg):
       /\ SendFx(sg, o)
       /\ CASE o = "ok" ->
                 /\ w' = [w EXCEPT !["R"].pc = "write", !["R"].res = IF isNew THEN "new" ELSE "old"]
                 /\ UNCHANGED <<q, lock>>
            [] o = "known" ->                               \* was_already_submitted
                 /\ w' = [w EXCEPT !["R"].pc = "write", !["R"].res = "old"]
                 /\ UNCHANGED <<q, lock>>
            [] o = "low" ->
                 /\ w' = [w EXCEPT !["R"].pc = "write", !["R"].res = "old"]
                 /\ q' = [q EXCEPT !["N"] = TRUE]
                 /\ UNCHANGED lock
            [] o = "high" ->                                \* handle_nonce_too_high
                 /\ Done("R") /\ lock' = Release("R") /\ UNCHANGED q
            [] o \in {"landedErr", "err"} ->
                 \* main :1172 return Err -> job retried, nothing written.
                 \* reconciler: bytes are saved; decide() resends later.
                 /\ Done("R") /\ lock' = Release("R")
                 /\ q' = IF PersistFirst THEN q ELSE [q EXCEPT !["R"] = TRUE]
  /\ UNCHANGED <<rec, counter, nextId, chainNonce, mined, tAlive, cancelLeft,
                 cancelAccepted, lateSigned, lagLeft, expLeft>>

\* :1194 update: network_data (from the LOCAL copy), hashes (local copy +
\* new hash), status Submitted.  Overwrites whatever was written meanwhile.
ResWrite ==
  /\ w["R"].pc = "write"
  /\ LET sg == w["R"].sig
         p == IF w["R"].res = "new" /\ ~PersistFirst
                THEN [st |-> "submitted", cur |-> sg, kind |-> sg.k,
                      hashes |-> w["R"].loc.hashes \cup {sg}]
                ELSE [st |-> "submitted"]
     IN rec' = IF Fresh("R") \/ Mutation = "noCasOnResubmit" THEN NewRec(p) ELSE rec
  /\ Done("R") /\ lock' = Release("R")
  /\ UNCHANGED <<counter, q, nextId, chainNonce, pool, mined, accepted, tAlive,
                 cancelLeft, cancelAccepted, lateSigned, errLeft, lagLeft, expLeft>>

-----------------------------------------------------------------------------
(* CANCEL  evm_transaction.rs:1261 cancel_transaction (one API call).      *)
CanStart ==
  /\ w["C"].pc = "idle" /\ cancelLeft > 0
  /\ cancelLeft' = cancelLeft - 1
  /\ w' = [w EXCEPT !["C"].pc = "read"]
  /\ UNCHANGED <<rec, counter, nextId, chainNonce, pool, mined, accepted, q,
                 tAlive, lock, cancelAccepted, lateSigned, errLeft, lagLeft, expLeft>>

CanRead ==
  /\ w["C"].pc = "read"
  /\ Leased("C") => lock = None
  /\ IF rec.st = "pending" /\ (~Recon \/ rec.nonce = None)
       THEN /\ w' = [w EXCEPT !["C"].pc = "cpend", !["C"].loc = rec]
            /\ lock' = IF Leased("C") THEN "C" ELSE lock
       ELSE IF rec.st \in {"pending", "sent", "submitted"}
              THEN /\ w' = [w EXCEPT !["C"].pc = "cnoop", !["C"].loc = rec]
                   /\ lock' = IF Leased("C") THEN "C" ELSE lock
              ELSE /\ Done("C") /\ UNCHANGED lock      \* cannot cancel
  /\ UNCHANGED <<rec, counter, nextId, chainNonce, pool, mined, accepted, q,
                 tAlive, cancelLeft, cancelAccepted, lateSigned, errLeft,
                 lagLeft, expLeft>>

\* Conflict: "version" returns an error to the API caller; "reconciler"
\* reloads and retries.
CanConflict ==
  IF Recon THEN w' = [w EXCEPT !["C"] = [Idle EXCEPT !.pc = "read"]] ELSE Done("C")

\* :1282 Pending -> update status Canceled
CanPend ==
  /\ w["C"].pc = "cpend"
  /\ IF Fresh("C")
       THEN /\ rec' = NewRec([st |-> "canceled"])
            /\ cancelAccepted' = (cancelAccepted \/ rec.st \notin Final)
            /\ Done("C")
       ELSE /\ CanConflict /\ UNCHANGED <<rec, cancelAccepted>>
  /\ lock' = Release("C")
  /\ UNCHANGED <<counter, nextId, chainNonce, pool, mined, accepted, q, tAlive,
                 cancelLeft, lateSigned, errLeft, lagLeft, expLeft>>

\* :1295 prepare_noop_update_request (RPC for gas price happens between
\* CanRead and this step), :1302 partial_update, :1307 resubmit job.
CanNoop ==
  /\ w["C"].pc = "cnoop"
  /\ IF Fresh("C")
       THEN /\ rec' = NewRec([kind |-> "noop", canceled |-> TRUE])
            /\ cancelAccepted' = (cancelAccepted \/ rec.st \notin Final)
            /\ q' = [q EXCEPT !["R"] = TRUE]
            /\ Done("C")
       ELSE /\ CanConflict /\ UNCHANGED <<rec, cancelAccepted, q>>
  /\ lock' = Release("C")
  /\ UNCHANGED <<counter, nextId, chainNonce, pool, mined, accepted, tAlive,
                 cancelLeft, lateSigned, errLeft, lagLeft, expLeft>>

-----------------------------------------------------------------------------
(* STATUS JOB  status.rs:1062 handle_status_impl, retried until final by   *)
(* transaction_status_handler.rs (NotYetFinal).                            *)
FreePrepares == {p \in Prepares : ~q[p] /\ w[p].pc = "idle"}
EnqueuePrepare ==
  IF FreePrepares = {} THEN q
  ELSE [q EXCEPT ![CHOOSE p \in FreePrepares : TRUE] = TRUE]
LagChoices == IF lagLeft > 0 THEN {FALSE, TRUE} ELSE {FALSE}
FinalFor(l, p) == IF l.canceled /\ p.k = "noop" THEN "canceled" ELSE "confirmed"

\* A status write that loses the CAS.  "version": handler returns Ok on
\* ConcurrentUpdateConflict and the job ends.  "reconciler": retry.
StatusWrite(p) ==
  IF Fresh("T")
    THEN rec' = NewRec(p) /\ UNCHANGED tAlive
    ELSE /\ UNCHANGED rec
         /\ tAlive' = IF Recon /\ Mutation # "conflictStopsPolling" THEN tAlive ELSE FALSE

TStart ==
  /\ w["T"].pc = "idle" /\ tAlive
  /\ w' = [w EXCEPT !["T"] = [Idle EXCEPT !.pc = "act", !.loc = rec]]
  /\ UNCHANGED <<rec, counter, nextId, chainNonce, pool, mined, accepted, q,
                 tAlive, lock, cancelLeft, cancelAccepted, lateSigned, errLeft,
                 lagLeft, expLeft>>

TAct ==
  /\ w["T"].pc = "act"
  /\ Done("T")
  /\ LET l == w["T"].loc IN
     CASE l.st \in Final ->
            /\ tAlive' = FALSE
            /\ UNCHANGED <<rec, q, lagLeft>>
       [] l.st = "pending" ->                     \* status.rs:745 re-queue prepare
            /\ q' = EnqueuePrepare
            /\ UNCHANGED <<rec, tAlive, lagLeft>>
       [] l.st = "sent" /\ ~Recon ->              \* handle_sent_state: resend
            /\ q' = [q EXCEPT !["S"] = TRUE]
            /\ UNCHANGED <<rec, tAlive, lagLeft>>
       \* reconciler: decide() looks for receipts of every saved attempt in
       \* Sent as well as Submitted.  (A first version only did so in
       \* Submitted; TLC found that a lost "mark Submitted" write then
       \* strands a mined tx in Sent forever.)
       [] l.st = "submitted" \/ (Recon /\ l.st = "sent") ->
            \E lag \in LagChoices:
              LET seen == IF lag THEN {} ELSE l.hashes \cap mined IN
              /\ lagLeft' = IF lag THEN lagLeft - 1 ELSE lagLeft
              /\ IF seen # {}
                   \* status.rs:207 receipt (current hash, or :285 historical)
                   THEN \E p \in seen: StatusWrite([st |-> FinalFor(l, p)]) /\ UNCHANGED q
                   ELSE IF Recon
                     \* decide(): Failed only if a node at or past the head
                     \* where the nonce was read shows no receipt for ANY
                     \* saved attempt.  A lagging answer is "unresolved".
                     THEN IF (~lag \/ Mutation = "trustLaggingNode")
                             /\ l.nonce # None /\ chainNonce > l.nonce
                            THEN StatusWrite([st |-> "failed"]) /\ UNCHANGED q
                            ELSE \E a \in {"S", "R"}:      \* resend or reprice
                                   q' = [q EXCEPT ![a] = TRUE] /\ UNCHANGED <<rec, tAlive>>
                     \* main: status.rs:515 should_resubmit -> resubmit job
                     ELSE q' = [q EXCEPT !["R"] = TRUE] /\ UNCHANGED <<rec, tAlive>>
  /\ UNCHANGED <<counter, nextId, chainNonce, pool, mined, accepted, lock,
                 cancelLeft, cancelAccepted, lateSigned, errLeft, expLeft>>

(* NONCE RECOVERY  status.rs:827 reconcile_tx_nonce_state, triggered by a  *)
(* NonceTooLow send.  RPC *errors* skip the verdict; a lagging node's      *)
(* Ok(None) does not.                                                      *)
NStart ==
  /\ w["N"].pc = "idle" /\ q["N"]
  /\ q' = [q EXCEPT !["N"] = FALSE]
  /\ w' = [w EXCEPT !["N"] = [Idle EXCEPT !.pc = "act", !.loc = rec]]
  /\ UNCHANGED <<rec, counter, nextId, chainNonce, pool, mined, accepted,
                 tAlive, lock, cancelLeft, cancelAccepted, lateSigned, errLeft,
                 lagLeft, expLeft>>

NWrite(p) == rec' = IF Fresh("N") THEN NewRec(p) ELSE rec

NAct ==
  /\ w["N"].pc = "act"
  /\ Done("N")
  /\ LET l == w["N"].loc IN
     IF Recon \/ l.st \in Final \/ l.nonce = None
       THEN UNCHANGED <<rec, lagLeft>>        \* reconciler: decide() in T
       ELSE \E lag \in LagChoices:
         LET seen == IF lag THEN {} ELSE l.hashes \cap mined IN
         /\ lagLeft' = IF lag THEN lagLeft - 1 ELSE lagLeft
         /\ IF ~lag /\ l.cur # None /\ l.cur \in mined THEN UNCHANGED rec      \* :840 defer
            ELSE IF seen # {}                                  \* :865 historical
              THEN \E p \in seen: NWrite([st |-> FinalFor(l, p)])
            ELSE IF chainNonce > l.nonce                       \* :910 consumed
              THEN NWrite([st |-> "failed"])
            ELSE UNCHANGED rec
  /\ UNCHANGED <<counter, q, nextId, chainNonce, pool, mined, accepted, tAlive,
                 lock, cancelLeft, cancelAccepted, lateSigned, errLeft, expLeft>>

-----------------------------------------------------------------------------
(* ENVIRONMENT                                                             *)
\* status.rs:537 detect_nonce_gap_ahead + nonce health job: a filler NOOP
\* takes an unused nonce below this tx's nonce.
FillGap ==
  /\ rec.st = "submitted" /\ rec.nonce # None /\ rec.nonce > chainNonce
  /\ ~\E p \in pool : p.n = chainNonce
  /\ LET f == [n |-> chainNonce, k |-> "fill", id |-> 0] IN
       /\ pool' = pool \cup {f}
       /\ accepted' = accepted \cup {f}
  /\ UNCHANGED <<rec, counter, nextId, chainNonce, mined, w, q, tAlive, lock,
                 cancelLeft, cancelAccepted, lateSigned, errLeft, lagLeft, expLeft>>

\* A block includes one payload at the next nonce; others at that nonce drop.
Mine ==
  \E p \in pool :
    /\ p.n = chainNonce
    /\ mined' = mined \cup {p}
    /\ pool' = {x \in pool : x.n # p.n}
    /\ chainNonce' = chainNonce + 1
    /\ UNCHANGED <<rec, counter, nextId, accepted, w, q, tAlive, lock,
                   cancelLeft, cancelAccepted, lateSigned, errLeft, lagLeft, expLeft>>

\* The lease TTL runs out while its holder is still working.
Expire ==
  /\ Variant = "lease" /\ lock # None /\ expLeft > 0
  /\ lock' = None
  /\ expLeft' = expLeft - 1
  /\ UNCHANGED <<rec, counter, nextId, chainNonce, pool, mined, accepted, w, q,
                 tAlive, cancelLeft, cancelAccepted, lateSigned, errLeft, lagLeft>>

PrepStep(s) == PrepAlloc(s) \/ PrepPresign(s) \/ PrepSign(s) \/ PrepPost(s)

Next ==
  \/ \E s \in Prepares : PrepStart(s) \/ PrepStep(s)
  \/ SubStart \/ SubSend \/ SubMark
  \/ ResStart \/ ResSign \/ ResPersist \/ ResSend \/ ResWrite
  \/ CanStart \/ CanRead \/ CanPend \/ CanNoop
  \/ TStart \/ TAct \/ NStart \/ NAct
  \/ FillGap \/ Mine \/ Expire

\* Each job slot makes progress when it can.  Only the lease variant needs
\* strong fairness (SpecLease): there a job start is enabled only while the
\* lock is free, which can flicker.  SF is much more expensive to check.
CommonFair ==
  /\ \A s \in Prepares : WF_vars(PrepStep(s))
  /\ WF_vars(SubStart \/ SubSend \/ SubMark)
  /\ WF_vars(ResSign \/ ResPersist \/ ResSend \/ ResWrite)
  /\ WF_vars(CanPend \/ CanNoop)
  /\ WF_vars(TStart \/ TAct) /\ WF_vars(NStart \/ NAct)
  /\ WF_vars(FillGap) /\ WF_vars(Mine)
Fairness ==
  /\ CommonFair
  /\ \A s \in Prepares : WF_vars(PrepStart(s))
  /\ WF_vars(ResStart) /\ WF_vars(CanRead)
LeaseFairness ==
  /\ CommonFair
  /\ \A s \in Prepares : SF_vars(PrepStart(s))
  /\ SF_vars(ResStart) /\ SF_vars(CanRead)

Spec == Init /\ [][Next]_vars /\ Fairness
SpecLease == Init /\ [][Next]_vars /\ LeaseFairness

-----------------------------------------------------------------------------
(* PROPERTIES                                                              *)

\* One request never produces two mined transfers.
NoDoubleEffect == Cardinality({p \in mined : p.k = "orig"}) <= 1

\* Every nonce taken from the counter is either used on chain or still held
\* by the record, a running worker, or the pool.  An orphan blocks every
\* later transaction of the relayer (on Arbitrum outright).
Held(n) ==
  \/ rec.nonce = n /\ rec.st \notin {"canceled", "failed"}
  \/ \E s \in Slots : w[s].pc # "idle" /\ w[s].nn = n
  \/ \E s \in Slots : w[s].pc # "idle" /\ w[s].sig # None /\ w[s].sig.n = n
  \/ \E p \in pool : p.n = n
NoNonceGap == \A n \in 0..(counter - 1) : n < chainNonce \/ Held(n)

\* Every payload the chain accepted is recorded in the tx's hashes (or a
\* worker is still about to record it).  An unrecorded hash cannot be
\* found by receipt lookups.
InFlight(p) == \E s \in Slots : w[s].pc # "idle" /\ w[s].sig = p
NoLostHash == \A p \in accepted : p.k = "fill" \/ p \in rec.hashes \/ InFlight(p)

\* Failed means none of this tx's payloads mined.
NoFalseFailure == rec.st = "failed" => ~\E p \in mined : p.k \in {"orig", "noop"}

\* After a cancel is accepted: the record keeps the NOOP until final, and
\* the relayer never broadcasts an original payload it signed after that.
CancelHonest ==
  /\ (rec.canceled /\ rec.st \notin Final) => rec.kind = "noop"
  /\ \A p \in accepted : p.k = "orig" => p.id \notin lateSigned

\* Every tx reaches a final status.
EventuallyFinal == <>(rec.st \in Final)

\* Safety form of the liveness bug that matters most: while the tx is not
\* final, its status job is still scheduled.  If it is not, nothing will
\* ever look at the tx again.  Cheap enough to check on every variant.
PollingNeverStops == rec.st \notin Final => tAlive

=============================================================================
