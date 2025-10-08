# Transaction Flow Diagrams

## 1. High-Level Job Flow

```
┌─────────────────────────────────────────────────────────────────────┐
│                         USER/API REQUEST                             │
└──────────────────────────────┬──────────────────────────────────────┘
                               │
                               ↓
┌─────────────────────────────────────────────────────────────────────┐
│                    Create Transaction (Pending)                      │
│                    Store in DB with status=Pending                   │
└──────────────────────────────┬──────────────────────────────────────┘
                               │
                               ↓
┌─────────────────────────────────────────────────────────────────────┐
│           TRANSACTION REQUEST HANDLER (20 retries)                   │
│  ┌────────────────────────────────────────────────────────────────┐ │
│  │ Domain: prepare_transaction()                                  │ │
│  │  • Validate balance (before nonce)                             │ │
│  │  • Get/increment nonce                                         │ │
│  │  • Save tx with nonce (recovery point)                         │ │
│  │  • Sign transaction                                            │ │
│  │  • Update status: Pending → Sent                               │ │
│  │  • Queue: TransactionSend::submit()                            │ │
│  └────────────────────────────────────────────────────────────────┘ │
│                                                                       │
│  On error after 20 retries: Mark as Failed                           │
└──────────────────────────────┬──────────────────────────────────────┘
                               │
                               ↓
┌─────────────────────────────────────────────────────────────────────┐
│      TRANSACTION SUBMISSION HANDLER (command-specific retries)       │
│  ┌────────────────────────────────────────────────────────────────┐ │
│  │ Command: Submit (10 retries)                                   │ │
│  │  Domain: submit_transaction()                                  │ │
│  │   • Send raw transaction to blockchain                         │ │
│  │   • Update status: Sent → Submitted                            │ │
│  │   • Queue status check job (best effort)                       │ │
│  │                                                                 │ │
│  │  On error after 10 retries: Mark as Failed                     │ │
│  └────────────────────────────────────────────────────────────────┘ │
│                                                                       │
│  ┌────────────────────────────────────────────────────────────────┐ │
│  │ Command: Resubmit/Cancel/Resend (2 retries)                    │ │
│  │  Domain: resubmit_transaction() / cancel via NOOP              │ │
│  │   • Bump gas price                                             │ │
│  │   • Send to blockchain                                         │ │
│  │   • Keep status: Submitted                                     │ │
│  │                                                                 │ │
│  │  On error after 2 retries: Return Ok()                         │ │
│  │  (Status checker will retry on next cycle ~30s)                │ │
│  └────────────────────────────────────────────────────────────────┘ │
└──────────────────────────────┬──────────────────────────────────────┘
                               │
                               ↓
┌─────────────────────────────────────────────────────────────────────┐
│        TRANSACTION STATUS HANDLER (infinite retries)                 │
│  ┌────────────────────────────────────────────────────────────────┐ │
│  │ Domain: handle_transaction_status()                            │ │
│  │  • Check on-chain status                                       │ │
│  │  • Update DB with latest status                                │ │
│  │  • Decision logic:                                             │ │
│  │    - Stuck > threshold? → Queue Resubmit job                   │ │
│  │    - Too many attempts? → Queue Cancel (NOOP)                  │ │
│  │    - Mined? → Track confirmations                              │ │
│  │    - Confirmed/Failed/Expired? → Final state                   │ │
│  │                                                                 │ │
│  │  Retries until: is_final_state(tx.status) == true              │ │
│  └────────────────────────────────────────────────────────────────┘ │
└──────────────────────────────┬──────────────────────────────────────┘
                               │
                               ↓
┌─────────────────────────────────────────────────────────────────────┐
│                    FINAL STATE REACHED                               │
│           (Confirmed / Failed / Expired / Canceled)                  │
│                                                                       │
│  • Status handler completes (returns Ok)                             │
│  • Notification sent (best effort)                                   │
│  • Delete timestamp set (for cleanup)                                │
└─────────────────────────────────────────────────────────────────────┘
```

---

## 2. Status State Machine

```
                    ┌─────────────┐
                    │   PENDING   │ (Created)
                    └──────┬──────┘
                           │ prepare_transaction()
                           │  • validate
                           │  • get nonce
                           │  • sign
                           ↓
                    ┌─────────────┐
                    │    SENT     │ (Prepared, ready to submit)
                    └──────┬──────┘
                           │ submit_transaction()
                           │  • send_raw_transaction()
                           ↓
                    ┌─────────────┐
            ┌──────→│  SUBMITTED  │←──────┐
            │       └──────┬──────┘       │
            │              │              │
            │              │ check status │
            │              ↓              │
            │       ┌─────────────┐       │
            │       │  on-chain?  │       │
            │       └──────┬──────┘       │
            │              │              │
            │              ├─ Not found? ─┤
            │              │              │
            │              ├─ Stuck? ─────┘ resubmit_transaction()
            │              │                (bump gas, resend)
            │              ↓
            │       ┌─────────────┐
            │       │    MINED    │ (In a block)
            │       └──────┬──────┘
            │              │ wait for confirmations
            │              ↓
            │       ┌─────────────┐
            │       │  CONFIRMED  │ ◄─── FINAL STATE ✓
            │       └─────────────┘
            │
            │ (on error)
            ↓
     ┌─────────────┐
     │   FAILED    │ ◄─── FINAL STATE ✗
     └─────────────┘

     ┌─────────────┐
     │  CANCELED   │ ◄─── FINAL STATE ✗ (user/system initiated)
     └─────────────┘

     ┌─────────────┐
     │   EXPIRED   │ ◄─── FINAL STATE ✗ (timeout)
     └─────────────┘
```

---

## 3. EVM Transaction Lifecycle (Detailed)

```
┌─────────────────────────────────────────────────────────────────────┐
│                     PREPARE TRANSACTION                              │
└─────────────────────────────────────────────────────────────────────┘
│
├─ 1. Defensive status check
│    if status != Pending: return Ok(tx)  ← prevents wasteful retries
│
├─ 2. Estimate gas limit (with fallback to default)
│
├─ 3. Calculate gas price params
│
├─ 4. ✅ VALIDATE BALANCE (BEFORE nonce consumption)
│    │  ┌─ InsufficientBalance? → Mark Failed, return Ok(tx)
│    │  └─ RPC error? → Return Err (will retry)
│
├─ 5. Get or increment nonce
│    │  if tx.nonce exists: reuse (retry scenario)
│    │  else: get next nonce from counter
│
├─ 6. ✅ SAVE NONCE TO DB (recovery point for signing failures)
│    │  if new nonce: DB update
│    │  else: skip update
│
├─ 7. Sign transaction (may take time with external KMS)
│    │  if error: retry can recover using saved nonce
│
├─ 8. Save signed transaction
│    │  status: Pending → Sent
│    │  store: raw tx, hash
│
└─ 9. Queue submit job

┌─────────────────────────────────────────────────────────────────────┐
│                      SUBMIT TRANSACTION                              │
└─────────────────────────────────────────────────────────────────────┘
│
├─ 1. Defensive status check
│    if status not in [Pending, Sent, Submitted]: return Ok(tx)
│
├─ 2. ✅ CRITICAL: Send transaction to blockchain
│    │  send_raw_transaction(raw)
│    │  ← This is the point of no return
│
├─ 3. ✅ DEFENSIVE: Update database
│    │  status: → Submitted
│    │  sent_at: now
│    │  
│    │  if DB update fails:
│    │    - Log CRITICAL error
│    │    - Return Ok(tx)  ← Don't retry! TX is on-chain
│
├─ 4. ✅ BEST EFFORT: Queue status check
│    │  if job queue fails:
│    │    - Log CRITICAL error  
│    │    - Continue (cleanup handler will catch)
│
└─ 5. Send notification (best effort, logged only)

┌─────────────────────────────────────────────────────────────────────┐
│                   CHECK TRANSACTION STATUS                           │
└─────────────────────────────────────────────────────────────────────┘
│
├─ 1. Early exit for final states
│
├─ 2. Query blockchain for transaction receipt
│
├─ 3. Match on status:
│    │
│    ├─ SUBMITTED:
│    │   │  Age > threshold (with exponential backoff)?
│    │   │  ├─ Yes → Check if should NOOP (rollup, too many attempts)
│    │   │  │         └─ Queue Resubmit job
│    │   │  └─ No → Wait
│    │   │
│    ├─ PENDING:
│    │   │  Should NOOP?
│    │   │  └─ Yes → Prepare NOOP, Queue Submit
│    │   │
│    ├─ MINED:
│    │   │  Enough confirmations?
│    │   │  └─ Yes → Update to Confirmed (FINAL)
│    │   │
│    ├─ CONFIRMED / FAILED / EXPIRED:
│    │   │  Already final
│    │   │
│    └─ Update status if needed

└─ 4. Return updated tx
      - If final state: Handler completes
      - Else: Handler retries (infinite)
```

---

## 4. Retry Strategy Matrix

```
┌──────────────────────┬──────────────┬──────────────────┬──────────────┐
│ Handler              │ Max Retries  │ On Max Reached   │ Notes        │
├──────────────────────┼──────────────┼──────────────────┼──────────────┤
│ Request (Prepare)    │ 20           │ Mark Failed      │ Fresh tx     │
├──────────────────────┼──────────────┼──────────────────┼──────────────┤
│ Submission: Submit   │ 10           │ Mark Failed      │ First submit │
├──────────────────────┼──────────────┼──────────────────┼──────────────┤
│ Submission: Resubmit │ 2            │ Return Ok()      │ Status       │
│                      │              │ (status checker  │ checker      │
│                      │              │  will retry)     │ takes over   │
├──────────────────────┼──────────────┼──────────────────┼──────────────┤
│ Submission: Cancel   │ 2            │ Return Ok()      │ Status       │
│                      │              │ (status checker  │ checker      │
│                      │              │  will retry)     │ takes over   │
├──────────────────────┼──────────────┼──────────────────┼──────────────┤
│ Submission: Resend   │ 2            │ Return Ok()      │ Status       │
│                      │              │ (status checker  │ checker      │
│                      │              │  will retry)     │ takes over   │
├──────────────────────┼──────────────┼──────────────────┼──────────────┤
│ Status Check         │ usize::MAX   │ Never (infinite) │ Runs until   │
│                      │              │                  │ final state  │
├──────────────────────┼──────────────┼──────────────────┼──────────────┤
│ Notification         │ 10           │ Abort job        │ Best effort  │
├──────────────────────┼──────────────┼──────────────────┼──────────────┤
│ Cleanup              │ 10           │ Abort job        │ Cron job     │
└──────────────────────┴──────────────┴──────────────────┴──────────────┘
```

---

## 5. Error Classification and Handling

```
┌─────────────────────────────────────────────────────────────────────┐
│                         ERROR TAXONOMY                               │
└─────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────┐
│ PERMANENT ERRORS (Don't Retry)  │
└─────────────────────────────────┘
│
├─ InsufficientBalance
│   Action: Mark transaction as Failed immediately
│   Handler: Return Ok(failed_tx) to stop job retries
│
├─ ValidationError
│   Action: Mark transaction as Failed
│   Handler: Return Ok(failed_tx)
│
├─ InvalidType
│   Action: Mark transaction as Failed
│   Handler: Return Ok(failed_tx)
│
└─ SignerError (some cases)
    Action: Depends on error type
    Handler: May retry if transient

┌─────────────────────────────────┐
│ TRANSIENT ERRORS (Retry Safe)   │
└─────────────────────────────────┘
│
├─ UnexpectedError (RPC issues)
│   Action: Retry with backoff
│   Handler: Return Err(...) to trigger job retry
│
├─ ProviderError
│   Action: Retry
│   Handler: Return Err(...)
│
├─ RepositoryError (DB connection)
│   Action: Retry
│   Handler: Return Err(...)
│   Special: If after on-chain submit, log but don't propagate
│
└─ SignerError (timeout, network)
    Action: Retry (nonce is saved)
    Handler: Return Err(...)

┌─────────────────────────────────┐
│ SPECIAL HANDLING                │
└─────────────────────────────────┘
│
├─ Transaction already in final state
│   Action: Nothing to do
│   Handler: Return Ok(tx) immediately
│
├─ Transaction in unexpected state
│   Action: Log warning, skip operation
│   Handler: Return Ok(tx) to prevent wasteful retries
│
└─ DB failure after on-chain success
    Action: Log CRITICAL error
    Handler: Return Ok(tx) - transaction is valid
    Fallback: Cleanup handler will reconcile
```

---

## 6. Job Dependencies

```
┌──────────────────────────────────────────────────────────────────────┐
│                          JOB FLOW CHART                              │
└──────────────────────────────────────────────────────────────────────┘

  TransactionRequest
         │
         ↓
  ┌─────────────────┐
  │  prepare_tx()   │
  └────────┬────────┘
           │
           ↓ (queues)
  ┌─────────────────────────────────┐
  │  TransactionSend::submit        │
  │   ├─ submit_transaction()       │
  │   └─ (queues) StatusCheck       │
  └────────┬────────────────────────┘
           │
           ↓
  ┌─────────────────────────────────┐
  │  TransactionStatusCheck         │
  │   └─ handle_status()            │
  │        ├─ stuck? → queues:      │
  │        │    TransactionSend::    │
  │        │    resubmit             │
  │        │                         │
  │        └─ retries infinitely     │
  │           until final            │
  └─────────────────────────────────┘
           │
           ↓
  ┌─────────────────────────────────┐
  │  TransactionSend::resubmit      │
  │   ├─ resubmit_transaction()     │
  │   ├─ 2 retries                  │
  │   └─ status checker continues   │
  └─────────────────────────────────┘

  Side Jobs (independent):
  
  ┌─────────────────────────────────┐
  │  NotificationSend               │
  │   └─ send webhook               │
  │   └─ 10 retries                 │
  └─────────────────────────────────┘

  ┌─────────────────────────────────┐
  │  TransactionCleanup (cron)      │
  │   └─ delete old final txs       │
  └─────────────────────────────────┘
```

---

## 7. Defensive Patterns Applied

### Pattern 1: Early Status Check
```rust
// At start of critical methods
if let Err(e) = ensure_status(&tx, ExpectedStatus, Some("method_name")) {
    warn!("Transaction not in expected status, skipping");
    return Ok(tx);  // Don't retry, nothing to do
}
```

**Applied in:**
- `prepare_transaction()` - checks for Pending
- `submit_transaction()` - checks for Sent/Submitted/Pending
- `resubmit_transaction()` - checks for Sent/Submitted/Pending

### Pattern 2: Non-Propagating DB Updates After Critical Operations
```rust
// Critical operation succeeded (e.g., on-chain submission)
blockchain.send_transaction(&raw).await?;

// Now update DB - but don't fail if this fails
match db.update(tx_id, update).await {
    Ok(tx) => tx,
    Err(e) => {
        error!("CRITICAL: DB update failed but tx is on-chain");
        tx  // Return original, don't propagate error
    }
}
```

**Applied in:**
- `submit_transaction()` - after send_raw_transaction
- `resubmit_transaction()` - after send_raw_transaction

### Pattern 3: Best-Effort Job Queuing
```rust
// Queue next job, but don't fail if queuing fails
if let Err(e) = job_producer.produce_job(...).await {
    error!("CRITICAL: Failed to queue job");
    // Don't propagate error, rely on fallback mechanisms
}
```

**Applied in:**
- Status check job queuing after submit
- Status check re-queuing on RPC failure

### Pattern 4: Permanent Error Early Exit
```rust
match error {
    TransactionError::InsufficientBalance(_) => {
        // Permanent error - mark as failed
        let failed_tx = db.update(tx.id, mark_failed_update).await?;
        // Return Ok to stop job retries
        return Ok(failed_tx);
    }
    _ => {
        // Transient error - propagate for retry
        return Err(error);
    }
}
```

**Applied in:**
- `prepare_transaction()` - balance check
- All handlers - error classification

---

## 8. Monitoring Points

**Key Metrics to Track:**

1. **Transaction Lifecycle Times**
   - Pending → Sent (preparation time)
   - Sent → Submitted (submission time)
   - Submitted → Confirmed (confirmation time)

2. **Failure Points**
   - Preparation failures (before nonce)
   - Preparation failures (after nonce, during signing)
   - Submission failures (before on-chain)
   - DB update failures (after on-chain) ⚠️ CRITICAL
   - Status check job queue failures ⚠️ CRITICAL

3. **Retry Counts**
   - Request handler retries before failure
   - Submission handler retries (per command type)
   - Status check iterations before final state

4. **Status Check Effectiveness**
   - Resubmit trigger frequency
   - NOOP trigger frequency
   - Time from stuck detection to resubmission

5. **Error Rates by Type**
   - InsufficientBalance rate
   - RPC/Provider errors
   - Validation errors
   - Signer errors

