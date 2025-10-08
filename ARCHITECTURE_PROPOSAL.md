# Architecture Proposal: Centralize Domain Logic

## Problem Statement

Current architecture has domain logic scattered across handlers:
1. Handlers decide when to mark transactions as `Failed`
2. Status checker only engaged after submission
3. Status checker CAN handle `Pending` state but never gets the chance
4. Business rules duplicated across multiple handlers

## Proposed Solution

**Core Principle:** Handlers orchestrate job retries. Domain layer owns all state transitions.

---

## 1. Queue Status Check Immediately

### Current Flow
```
Create Transaction (Pending)
    ↓
Prepare (Pending → Sent)
    ↓
Submit (Sent → Submitted)
    ↓
Queue Status Check ← ONLY HERE
```

### Proposed Flow
```
Create Transaction (Pending)
    ↓
Queue Status Check ← START HERE (with delay)
    ↓
Prepare (Pending → Sent)
    ↓
Submit (Sent → Submitted)
    ↓
Status Check already running
```

### Implementation

**In transaction creation (API handler):**
```rust
// After creating transaction in DB
let tx = transaction_repo.create(tx_data).await?;

// Queue status check with initial delay (e.g., 30 seconds)
job_producer.produce_check_transaction_status_job(
    TransactionStatusCheck::new(tx.id.clone(), tx.relayer_id.clone()),
    Some(calculate_scheduled_timestamp(30)), // Initial delay
).await?;

// Queue prepare job
job_producer.produce_transaction_request_job(
    TransactionRequest::new(tx.id, tx.relayer_id),
    None,
).await?;
```

**Benefits:**
- Status checker monitors transaction from creation
- Handles stuck `Pending` transactions
- Single monitoring loop for entire lifecycle
- No gaps in monitoring coverage

---

## 2. Remove Domain Logic from Handlers

### Current Handler Logic (WRONG)
```rust
// transaction_request_handler.rs
pub async fn handle_transaction_result(...) {
    if attempt.current() >= max_retries {
        // ❌ Handler deciding business logic
        let update_request = TransactionUpdateRequest {
            status: Some(TransactionStatus::Failed),
            status_reason: Some("Failed after retries"),
            ..Default::default()
        };
        transaction_repo.partial_update(tx_id, update_request).await?;
        return Err(Error::Abort(...));
    }
    Err(Error::Failed(...)) // Retry
}
```

### Proposed Handler Logic (RIGHT)
```rust
// transaction_request_handler.rs
pub async fn handle_transaction_result(...) {
    match result {
        Ok(tx) => Ok(()),
        Err(e) => {
            // Just decide: retry or abort?
            // Don't touch transaction state!
            if attempt.current() >= max_retries {
                warn!("Max retries reached, stopping job");
                return Err(Error::Abort(Arc::new(e.into())));
            }
            // Retry
            Err(Error::Failed(Arc::new(e.into())))
        }
    }
}
```

**Status checker will handle marking Failed:**
```rust
// In status checker domain logic
async fn handle_status_impl(&self, tx: TransactionRepoModel) -> Result<...> {
    // Check if transaction has expired
    if self.is_expired(&tx) {
        return self.mark_as_failed(tx, "Transaction expired").await;
    }
    
    // Check if too many retry attempts
    if too_many_attempts(&tx) {
        return self.mark_as_failed(tx, "Too many attempts").await;
    }
    
    // Check if stuck in Pending too long
    if tx.status == TransactionStatus::Pending && self.stuck_too_long(&tx) {
        // Try NOOP or mark as Failed based on policy
        return self.handle_stuck_pending(tx).await;
    }
    
    // Normal status checking...
}
```

---

## 3. Handler Responsibility Matrix

### OLD (Current)
| Handler | Responsibilities |
|---------|-----------------|
| Request | ❌ Prepare TX + Mark Failed after retries |
| Submission | ❌ Submit TX + Mark Failed after retries |
| Status Check | ✅ Monitor TX, trigger resubmits |

### NEW (Proposed)
| Handler | Responsibilities |
|---------|-----------------|
| Request | ✅ Prepare TX, Retry on errors |
| Submission | ✅ Submit TX, Retry on errors |
| Status Check | ✅ **All state transitions**, Monitor, Resubmit, Mark Failed |

---

## 4. Status Checker as Single Source of Truth

### Enhanced Status Checker Responsibilities

```rust
impl EvmRelayerTransaction {
    pub async fn handle_status_impl(&self, tx: TransactionRepoModel) 
        -> Result<TransactionRepoModel, TransactionError> 
    {
        // 1. Early exit for final states
        if is_final_state(&tx.status) {
            return Ok(tx);
        }
        
        // 2. Check for expiry/timeout
        if let Some(expired_tx) = self.check_expiry(&tx).await? {
            return Ok(expired_tx);
        }
        
        // 3. Check for too many attempts
        if too_many_attempts(&tx) {
            return self.handle_too_many_attempts(tx).await;
        }
        
        // 4. Handle based on current status
        match tx.status {
            TransactionStatus::Pending => {
                self.handle_pending_state(tx).await
            }
            TransactionStatus::Sent => {
                self.handle_sent_state(tx).await
            }
            TransactionStatus::Submitted => {
                self.handle_submitted_state(tx).await
            }
            TransactionStatus::Mined => {
                self.handle_mined_state(tx).await
            }
            _ => Ok(tx)
        }
    }
    
    /// Check if transaction has expired based on valid_until
    async fn check_expiry(&self, tx: &TransactionRepoModel) 
        -> Result<Option<TransactionRepoModel>, TransactionError> 
    {
        if let Some(valid_until) = &tx.valid_until {
            let expiry = DateTime::parse_from_rfc3339(valid_until)
                .map_err(|e| TransactionError::UnexpectedError(e.to_string()))?;
            
            if Utc::now() > expiry {
                info!(tx_id = %tx.id, "Transaction expired");
                return Ok(Some(self.mark_as_expired(tx.clone()).await?));
            }
        }
        Ok(None)
    }
    
    /// Mark transaction as expired
    async fn mark_as_expired(&self, tx: TransactionRepoModel) 
        -> Result<TransactionRepoModel, TransactionError> 
    {
        let update = TransactionUpdateRequest {
            status: Some(TransactionStatus::Expired),
            status_reason: Some("Transaction expired".to_string()),
            ..Default::default()
        };
        
        let updated_tx = self.transaction_repository()
            .partial_update(tx.id.clone(), update)
            .await?;
        
        self.send_transaction_update_notification(&updated_tx).await;
        Ok(updated_tx)
    }
    
    /// Handle transaction with too many attempts
    async fn handle_too_many_attempts(&self, tx: TransactionRepoModel) 
        -> Result<TransactionRepoModel, TransactionError> 
    {
        warn!(tx_id = %tx.id, attempts = tx.hashes.len(), "Too many attempts");
        
        // For rollups, try NOOP
        if self.is_rollup_network(&tx).await? {
            if should_noop(&tx).await? {
                return self.handle_noop(&tx).await;
            }
        }
        
        // Otherwise mark as failed
        self.mark_as_failed(
            tx, 
            format!("Too many attempts: {}", tx.hashes.len())
        ).await
    }
    
    /// Mark transaction as failed
    async fn mark_as_failed(&self, tx: TransactionRepoModel, reason: String) 
        -> Result<TransactionRepoModel, TransactionError> 
    {
        let update = TransactionUpdateRequest {
            status: Some(TransactionStatus::Failed),
            status_reason: Some(reason),
            ..Default::default()
        };
        
        let updated_tx = self.transaction_repository()
            .partial_update(tx.id.clone(), update)
            .await?;
        
        self.send_transaction_update_notification(&updated_tx).await;
        Ok(updated_tx)
    }
    
    /// Handle stuck pending transactions
    async fn handle_pending_state(&self, tx: TransactionRepoModel) 
        -> Result<TransactionRepoModel, TransactionError> 
    {
        // Check how long it's been pending
        let age = get_age_since_created(&tx)?;
        
        // If stuck too long, consider NOOP or Failed
        if age > Duration::minutes(10) {
            warn!(tx_id = %tx.id, "Transaction stuck in Pending");
            
            if self.should_noop(&tx).await? {
                return self.handle_noop(&tx).await;
            }
            
            // If not NOOP-able, mark as failed
            return self.mark_as_failed(
                tx,
                "Stuck in Pending state".to_string()
            ).await;
        }
        
        Ok(tx)
    }
    
    /// Handle stuck sent transactions  
    async fn handle_sent_state(&self, tx: TransactionRepoModel)
        -> Result<TransactionRepoModel, TransactionError>
    {
        // Check how long it's been sent but not submitted
        let age = get_age_since_sent(&tx)?;
        
        if age > Duration::minutes(5) {
            warn!(tx_id = %tx.id, "Transaction stuck in Sent");
            
            // Transaction was prepared but never submitted
            // Could be a job queue failure
            // Re-queue submit job
            self.send_transaction_submit_job(&tx).await?;
        }
        
        Ok(tx)
    }
}
```

---

## 5. Simplified Handler Logic

### Request Handler (NEW)
```rust
pub async fn transaction_request_handler(
    job: Job<TransactionRequest>,
    state: Data<ThinData<DefaultAppState>>,
    attempt: Attempt,
    // ...
) -> Result<(), Error> {
    let result = handle_request(job.data, state.clone()).await;
    
    // Simple retry logic - no domain decisions
    match result {
        Ok(()) => Ok(()),
        Err(e) => {
            if attempt.current() >= WORKER_TRANSACTION_REQUEST_RETRIES {
                warn!("Max retries reached for preparation, aborting job");
                // Status checker will handle marking Failed if needed
                Err(Error::Abort(Arc::new(e.into())))
            } else {
                Err(Error::Failed(Arc::new(e.into())))
            }
        }
    }
}
```

### Submission Handler (NEW)
```rust
async fn handle_submission_result<TR>(
    result: Result<()>,
    attempt: Attempt,
    command: &TransactionCommand,
    transaction_id: String,
    transaction_repo: Arc<TR>,
) -> Result<(), Error> {
    let max_retries = get_max_retries(command);
    
    match result {
        Ok(()) => Ok(()),
        Err(e) => {
            if attempt.current() >= max_retries {
                match command {
                    TransactionCommand::Submit => {
                        // For fresh submissions, log and abort
                        // Status checker will mark as Failed
                        warn!(
                            tx_id = %transaction_id,
                            "Submit retries exhausted, status checker will handle"
                        );
                        Err(Error::Abort(Arc::new(e.to_string().into())))
                    }
                    _ => {
                        // For resubmit/cancel/resend, status checker continues
                        warn!(
                            tx_id = %transaction_id,
                            "Submission retries exhausted, status checker will retry"
                        );
                        Ok(())
                    }
                }
            } else {
                Err(Error::Failed(Arc::new(e.to_string().into())))
            }
        }
    }
}
```

---

## 6. Transaction Creation Flow (NEW)

```rust
// In API handler or transaction creation
pub async fn create_transaction(
    tx_request: TransactionRequest,
    state: &AppState,
) -> Result<TransactionRepoModel> {
    // 1. Create transaction in DB
    let tx = state.transaction_repository()
        .create(tx_request)
        .await?;
    
    // 2. Queue status check FIRST (with initial delay)
    state.job_producer()
        .produce_check_transaction_status_job(
            TransactionStatusCheck::new(tx.id.clone(), tx.relayer_id.clone()),
            Some(calculate_scheduled_timestamp(30)), // 30 second initial delay
        )
        .await?;
    
    // 3. Queue preparation job
    state.job_producer()
        .produce_transaction_request_job(
            TransactionRequest::new(tx.id.clone(), tx.relayer_id.clone()),
            None, // Immediate
        )
        .await?;
    
    Ok(tx)
}
```

---

## 7. Benefits

### ✅ Single Source of Truth
- All state transitions in domain layer
- Status checker is the orchestrator
- No duplicate logic across handlers

### ✅ Complete Monitoring Coverage
- Transactions monitored from creation
- Handles stuck `Pending` transactions
- Handles stuck `Sent` transactions (prepared but not submitted)
- No gaps in lifecycle

### ✅ Cleaner Handler Logic
- Handlers focus on retry orchestration
- No domain logic in handlers
- Easier to test and maintain

### ✅ Better Error Recovery
- Status checker can recover from job queue failures
- Re-queue submit if transaction stuck in Sent
- Centralized timeout/expiry logic

### ✅ Flexible Policies
- Timeout policies in one place
- Easy to adjust per network
- Clear business rules

---

## 8. Migration Steps

### Phase 1: Add Status Check at Creation
1. Update transaction creation to queue status check
2. Test that status checker handles all states
3. Verify no duplicate job queueing

### Phase 2: Move Failure Logic to Domain
1. Add expiry checking to status handler
2. Add too-many-attempts handling
3. Add stuck-in-state handling
4. Remove failure marking from handlers

### Phase 3: Simplify Handlers
1. Remove `mark_as_failed` logic from handlers
2. Simplify to just retry/abort decisions
3. Update tests

### Phase 4: Add Missing State Handlers
1. Implement `handle_sent_state` (currently might not exist)
2. Enhance `handle_pending_state` with timeout
3. Add comprehensive logging

---

## 9. Open Questions

1. **What delay for initial status check?**
   - 30 seconds? Too long for fast networks?
   - 10 seconds? Reasonable middle ground?
   - Per-network configuration?

2. **Should we keep status check queueing after submit?**
   - Pro: Immediate monitoring after critical operation
   - Con: Redundant if already scheduled
   - Solution: Check if job already exists before queueing?

3. **How to handle prepare job failures?**
   - Currently: Handler retries 20 times, then marks Failed
   - New: Handler retries 20 times, then aborts. Status checker marks Failed?
   - When should status checker mark Failed vs. let job retry?

4. **Transition strategy?**
   - Big bang migration?
   - Feature flag?
   - Gradual rollout per chain?

---

## 10. Example: Complete Flow (NEW)

```
1. User creates transaction
   └─ API creates TX in DB (status: Pending)
   └─ Queue: StatusCheck (delay: 30s)
   └─ Queue: TransactionRequest (immediate)

2. Request handler processes
   └─ prepare_transaction()
   └─ On success: status → Sent, queue Submit
   └─ On error: retry up to 20 times, then abort
   └─ (Status checker will handle if stuck)

3. Status checker (30s later)
   └─ Checks status
   └─ If Pending: check if stuck → NOOP or Failed
   └─ If Sent: check if stuck → re-queue Submit
   └─ If Submitted: check if stuck → Resubmit
   └─ Retries infinitely until final state

4. Submission handler processes
   └─ submit_transaction()
   └─ On success: status → Submitted
   └─ On error: retry 10 times, then abort
   └─ (Status checker continues monitoring)

5. Status checker continues
   └─ Monitors Submitted
   └─ Resubmits if stuck
   └─ Tracks confirmations
   └─ Marks Confirmed when done
```

**Key difference:** Status checker engaged from start, owns all state transitions.

