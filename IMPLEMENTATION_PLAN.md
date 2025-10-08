# Implementation Plan: Centralized Domain Logic

**Strategy:** Big bang migration without backward compatibility  
**Goal:** Queue status check at creation, move all domain logic to status checker

---

## Design Decisions

### 1. ✅ Single Status Check Job Per Transaction
- Queue ONLY at transaction creation
- Remove all status check queuing after submit/resubmit
- Status checker handles entire lifecycle

### 2. ✅ Smart Status Check Timing
Status checker can handle transactions at any age:
```rust
// In status checker
if too_early_to_check(&tx) {
    // If created < 10 seconds ago, nothing to do yet
    return Ok(tx); // Job will retry
}
```

### 3. ✅ Duration-Based Failure Detection
```rust
// Timeout thresholds
const PREPARE_TIMEOUT: Duration = Duration::minutes(5);
const SUBMIT_TIMEOUT: Duration = Duration::minutes(2);
const CONFIRMATION_TIMEOUT: Duration = Duration::hours(1);

// In status checker
match tx.status {
    Pending => {
        if age_since_created > PREPARE_TIMEOUT {
            mark_as_failed("Failed to prepare within timeout");
        }
    }
    Sent => {
        if age_since_status_change > SUBMIT_TIMEOUT {
            mark_as_failed("Failed to submit within timeout");
        }
    }
    Submitted => {
        if age_since_sent > CONFIRMATION_TIMEOUT {
            mark_as_expired("Transaction expired");
        }
    }
}
```

---

## Implementation Steps

### Phase 1: Update Constants ✅
**File:** `src/constants/transactions.rs` (or create)

```rust
use chrono::Duration;

// Status check scheduling
pub const STATUS_CHECK_INITIAL_DELAY_SECONDS: i64 = 30;
pub const STATUS_CHECK_MIN_AGE_SECONDS: i64 = 10; // Don't check if too young

// Timeout thresholds for failure detection
pub const PREPARE_TIMEOUT_MINUTES: i64 = 5;    // Pending → Sent
pub const SUBMIT_TIMEOUT_MINUTES: i64 = 2;     // Sent → Submitted
pub const CONFIRMATION_TIMEOUT_HOURS: i64 = 1; // Submitted → Final

// Helper functions
pub fn get_prepare_timeout() -> Duration {
    Duration::minutes(PREPARE_TIMEOUT_MINUTES)
}

pub fn get_submit_timeout() -> Duration {
    Duration::minutes(SUBMIT_TIMEOUT_MINUTES)
}

pub fn get_confirmation_timeout() -> Duration {
    Duration::hours(CONFIRMATION_TIMEOUT_HOURS)
}
```

---

### Phase 2: Add Timing Helpers ✅
**File:** `src/domain/transaction/evm/utils.rs`

```rust
use chrono::{DateTime, Duration, Utc};
use crate::models::TransactionRepoModel;

/// Get age since transaction creation
pub fn get_age_since_created(tx: &TransactionRepoModel) -> Result<Duration, TransactionError> {
    let created = DateTime::parse_from_rfc3339(&tx.created_at)
        .map_err(|e| TransactionError::UnexpectedError(e.to_string()))?;
    Ok(Utc::now().signed_duration_since(created))
}

/// Get age since status last changed (uses sent_at for now)
pub fn get_age_since_status_change(tx: &TransactionRepoModel) -> Result<Duration, TransactionError> {
    // For Sent status, use sent_at
    if let Some(sent_at) = &tx.sent_at {
        let sent = DateTime::parse_from_rfc3339(sent_at)
            .map_err(|e| TransactionError::UnexpectedError(e.to_string()))?;
        return Ok(Utc::now().signed_duration_since(sent));
    }
    
    // Fallback to created_at
    get_age_since_created(tx)
}

/// Check if transaction is too young to check status
pub fn is_too_early_to_check(tx: &TransactionRepoModel) -> Result<bool, TransactionError> {
    let age = get_age_since_created(tx)?;
    Ok(age < Duration::seconds(crate::constants::STATUS_CHECK_MIN_AGE_SECONDS))
}
```

---

### Phase 3: Enhance Status Checker Domain Logic ✅
**File:** `src/domain/transaction/evm/status.rs`

Add new methods to the implementation:

```rust
impl<P, RR, NR, TR, J, S, TCR, PC> EvmRelayerTransaction<P, RR, NR, TR, J, S, TCR, PC>
where
    // ... trait bounds
{
    /// Main status handler - enhanced with timeout logic
    pub async fn handle_status_impl(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        debug!("checking transaction status {}", tx.id);
        
        // 1. Early return if final state
        if is_final_state(&tx.status) {
            debug!(status = ?tx.status, "transaction already in final state");
            return Ok(tx);
        }
        
        // 2. Check if too early (just created)
        if is_too_early_to_check(&tx)? {
            debug!(
                tx_id = %tx.id,
                age_seconds = get_age_since_created(&tx)?.num_seconds(),
                "transaction too young to check, will retry later"
            );
            return Ok(tx);
        }
        
        // 3. Check for timeouts/expiry
        if let Some(expired_tx) = self.check_timeouts(&tx).await? {
            return Ok(expired_tx);
        }
        
        // 4. Check on-chain status
        let status = self.check_transaction_status(&tx).await?;
        
        debug!(status = ?status, "transaction status {}", tx.id);
        
        // 5. Handle based on status
        match status {
            TransactionStatus::Pending => self.handle_pending_state(tx).await,
            TransactionStatus::Sent => self.handle_sent_state(tx).await,
            TransactionStatus::Submitted => self.handle_submitted_state(tx).await,
            TransactionStatus::Mined => self.handle_mined_state(tx).await,
            TransactionStatus::Confirmed
            | TransactionStatus::Failed
            | TransactionStatus::Expired => self.handle_final_state(tx, status).await,
            _ => Err(TransactionError::UnexpectedError(format!(
                "Unexpected transaction status: {:?}",
                status
            ))),
        }
    }
    
    /// Check for various timeout conditions
    async fn check_timeouts(
        &self,
        tx: &TransactionRepoModel,
    ) -> Result<Option<TransactionRepoModel>, TransactionError> {
        let age = get_age_since_created(tx)?;
        
        match tx.status {
            TransactionStatus::Pending => {
                // Timeout if stuck in Pending too long
                if age > get_prepare_timeout() {
                    warn!(
                        tx_id = %tx.id,
                        age_minutes = age.num_minutes(),
                        "transaction stuck in Pending, marking as failed"
                    );
                    return Ok(Some(
                        self.mark_as_failed(
                            tx.clone(),
                            format!(
                                "Failed to prepare within {} minutes",
                                PREPARE_TIMEOUT_MINUTES
                            ),
                        )
                        .await?,
                    ));
                }
            }
            TransactionStatus::Sent => {
                // Timeout if prepared but not submitted
                let age_since_sent = get_age_since_status_change(tx)?;
                if age_since_sent > get_submit_timeout() {
                    warn!(
                        tx_id = %tx.id,
                        age_minutes = age_since_sent.num_minutes(),
                        "transaction stuck in Sent, marking as failed"
                    );
                    return Ok(Some(
                        self.mark_as_failed(
                            tx.clone(),
                            format!(
                                "Failed to submit within {} minutes",
                                SUBMIT_TIMEOUT_MINUTES
                            ),
                        )
                        .await?,
                    ));
                }
            }
            TransactionStatus::Submitted => {
                // Check valid_until if present
                if let Some(valid_until) = &tx.valid_until {
                    let expiry = DateTime::parse_from_rfc3339(valid_until)
                        .map_err(|e| TransactionError::UnexpectedError(e.to_string()))?;
                    
                    if Utc::now() > expiry {
                        info!(tx_id = %tx.id, "transaction expired");
                        return Ok(Some(self.mark_as_expired(tx.clone()).await?));
                    }
                }
                
                // Fallback timeout for confirmation
                let age_since_sent = get_age_since_status_change(tx)?;
                if age_since_sent > get_confirmation_timeout() {
                    warn!(
                        tx_id = %tx.id,
                        age_hours = age_since_sent.num_hours(),
                        "transaction confirmation timeout"
                    );
                    return Ok(Some(self.mark_as_expired(tx.clone()).await?));
                }
                
                // Check for too many attempts
                if too_many_attempts(tx) {
                    warn!(
                        tx_id = %tx.id,
                        attempts = tx.hashes.len(),
                        "too many attempts"
                    );
                    return Ok(Some(
                        self.mark_as_failed(
                            tx.clone(),
                            format!("Too many attempts: {}", tx.hashes.len()),
                        )
                        .await?,
                    ));
                }
            }
            _ => {}
        }
        
        Ok(None)
    }
    
    /// Mark transaction as failed
    async fn mark_as_failed(
        &self,
        tx: TransactionRepoModel,
        reason: String,
    ) -> Result<TransactionRepoModel, TransactionError> {
        info!(
            tx_id = %tx.id,
            reason = %reason,
            "marking transaction as failed"
        );
        
        let update = TransactionUpdateRequest {
            status: Some(TransactionStatus::Failed),
            status_reason: Some(reason),
            ..Default::default()
        };
        
        let updated_tx = self
            .transaction_repository()
            .partial_update(tx.id.clone(), update)
            .await?;
        
        self.send_transaction_update_notification(&updated_tx).await;
        
        Ok(updated_tx)
    }
    
    /// Mark transaction as expired
    async fn mark_as_expired(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        info!(tx_id = %tx.id, "marking transaction as expired");
        
        let update = TransactionUpdateRequest {
            status: Some(TransactionStatus::Expired),
            status_reason: Some("Transaction expired".to_string()),
            ..Default::default()
        };
        
        let updated_tx = self
            .transaction_repository()
            .partial_update(tx.id.clone(), update)
            .await?;
        
        self.send_transaction_update_notification(&updated_tx).await;
        
        Ok(updated_tx)
    }
    
    /// Handle transactions stuck in Sent (prepared but not submitted)
    async fn handle_sent_state(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        debug!(tx_id = %tx.id, "handling Sent state");
        
        // Transaction was prepared but submission job may have failed
        // Re-queue the submit job
        let age_since_sent = get_age_since_status_change(&tx)?;
        
        if age_since_sent > Duration::seconds(30) {
            warn!(
                tx_id = %tx.id,
                age_seconds = age_since_sent.num_seconds(),
                "transaction stuck in Sent, re-queuing submit job"
            );
            
            // Re-queue submit job
            self.send_transaction_submit_job(&tx).await?;
        }
        
        Ok(tx)
    }
}
```

---

### Phase 4: Update Transaction Creation ✅
**File:** Where transactions are created (API handlers, likely in routes)

Find where `TransactionRepoModel` is created and saved, then add status check job:

```rust
// Example location: src/routes/transaction.rs or similar

pub async fn create_transaction_handler(
    request: Json<CreateTransactionRequest>,
    state: Data<AppState>,
) -> Result<Json<TransactionResponse>> {
    // 1. Validate and create transaction model
    let tx = TransactionRepoModel {
        id: Uuid::new_v4().to_string(),
        relayer_id: request.relayer_id.clone(),
        status: TransactionStatus::Pending,
        created_at: Utc::now().to_rfc3339(),
        // ... other fields
    };
    
    // 2. Save to database
    let saved_tx = state
        .transaction_repository()
        .create(tx)
        .await?;
    
    // 3. ✅ NEW: Queue status check job FIRST (with delay)
    state
        .job_producer()
        .produce_check_transaction_status_job(
            TransactionStatusCheck::new(
                saved_tx.id.clone(),
                saved_tx.relayer_id.clone(),
            ),
            Some(calculate_scheduled_timestamp(STATUS_CHECK_INITIAL_DELAY_SECONDS)),
        )
        .await?;
    
    // 4. Queue preparation job (immediate)
    state
        .job_producer()
        .produce_transaction_request_job(
            TransactionRequest::new(
                saved_tx.id.clone(),
                saved_tx.relayer_id.clone(),
            ),
            None,
        )
        .await?;
    
    Ok(Json(TransactionResponse::from(saved_tx)))
}
```

---

### Phase 5: Remove Status Check Queuing After Submit ✅
**File:** `src/domain/transaction/evm/evm_transaction.rs`

**Remove this block from `submit_transaction()`:**
```rust
// ❌ DELETE THIS:
if let Err(e) = self
    .job_producer
    .produce_check_transaction_status_job(
        TransactionStatusCheck::new(updated_tx.id.clone(), updated_tx.relayer_id.clone()),
        None,
    )
    .await
{
    error!(
        error = %e,
        tx_id = %updated_tx.id,
        "CRITICAL: failed to produce status check job for submitted transaction"
    );
}
```

**Replace with comment:**
```rust
// Status check job already scheduled at transaction creation
// No need to queue again here
```

**Also remove from Stellar:**
**File:** `src/domain/transaction/stellar/submit.rs`

Remove the status check job queuing block (around line 83-91).

---

### Phase 6: Simplify Handlers ✅

#### A. Request Handler
**File:** `src/jobs/handlers/transaction_request_handler.rs`

```rust
pub async fn transaction_request_handler(
    job: Job<TransactionRequest>,
    state: Data<ThinData<DefaultAppState>>,
    attempt: Attempt,
    _worker: Worker<Context>,
    task_id: TaskId,
    _ctx: RedisContext,
) -> Result<(), Error> {
    if let Some(request_id) = job.request_id.clone() {
        set_request_id(request_id);
    }

    debug!("handling transaction request {}", job.data.transaction_id);

    let result = handle_request(job.data, state.clone()).await;

    // Simplified: just retry logic, no domain decisions
    handle_prepare_result(result, attempt)
}

fn handle_prepare_result(result: Result<()>, attempt: Attempt) -> Result<(), Error> {
    match result {
        Ok(()) => Ok(()),
        Err(e) => {
            if attempt.current() >= WORKER_TRANSACTION_REQUEST_RETRIES {
                warn!(
                    attempt = attempt.current(),
                    error = %e,
                    "preparation retries exhausted, aborting job - status checker will handle timeout"
                );
                Err(Error::Abort(Arc::new(e.to_string().into())))
            } else {
                debug!(
                    attempt = attempt.current(),
                    error = %e,
                    "preparation failed, will retry"
                );
                Err(Error::Failed(Arc::new(e.to_string().into())))
            }
        }
    }
}
```

**Remove:**
- All `handle_transaction_result` calls
- All transaction status updates in handler

#### B. Submission Handler
**File:** `src/jobs/handlers/transaction_submission_handler.rs`

```rust
async fn handle_submission_result<TR>(
    result: Result<()>,
    attempt: Attempt,
    command: &TransactionCommand,
    transaction_id: String,
    _transaction_repo: Arc<TR>, // No longer needed!
) -> Result<(), Error>
where
    TR: TransactionRepository,
{
    let max_retries = get_max_retries(command);
    let command_name = format!("{:?}", command);

    match result {
        Ok(()) => Ok(()),
        Err(e) if attempt.current() < max_retries => {
            debug!(
                tx_id = %transaction_id,
                command = %command_name,
                attempt = attempt.current(),
                max_retries,
                "submission failed, will retry"
            );
            Err(Error::Failed(Arc::new(e.to_string().into())))
        }
        Err(e) => {
            // Max retries exhausted - just abort
            // Status checker will handle marking as Failed if needed
            match command {
                TransactionCommand::Submit => {
                    warn!(
                        tx_id = %transaction_id,
                        command = %command_name,
                        attempts = attempt.current(),
                        "submission retries exhausted - status checker will handle timeout"
                    );
                    Err(Error::Abort(Arc::new(e.to_string().into())))
                }
                TransactionCommand::Resubmit | TransactionCommand::Cancel { .. } | TransactionCommand::Resend => {
                    warn!(
                        tx_id = %transaction_id,
                        command = %command_name,
                        attempts = attempt.current(),
                        "submission retries exhausted, status checker will retry on next cycle"
                    );
                    Ok(()) // Status checker continues
                }
            }
        }
    }
}
```

**Remove:**
- Transaction status update logic
- Database access from handler

#### C. Remove `handle_transaction_result` Helper
**File:** `src/jobs/handlers/mod.rs`

Delete the entire `handle_transaction_result` function (no longer needed).

---

### Phase 7: Update Prepare Transaction Domain Logic ✅
**File:** `src/domain/transaction/evm/evm_transaction.rs`

In `prepare_transaction()`, remove the balance failure marking logic:

```rust
// OLD (around line 375-395):
if let Err(balance_error) = self.ensure_sufficient_balance(...).await {
    match &balance_error {
        TransactionError::InsufficientBalance(_) => {
            // Mark as failed...
            let update = TransactionUpdateRequest { ... };
            let updated_tx = self.transaction_repository.partial_update(...).await?;
            return Ok(updated_tx);
        }
        _ => return Err(balance_error),
    }
}

// NEW:
// Just validate balance - let status checker handle marking as Failed
self.ensure_sufficient_balance(price_params.total_cost).await?;
```

The status checker will mark it as Failed after the timeout if preparation keeps failing.

---

## Migration Checklist

### Step 1: Add New Code ✅
- [ ] Add timeout constants
- [ ] Add timing helper functions
- [ ] Add `check_timeouts()` method to status checker
- [ ] Add `mark_as_failed()` method to status checker
- [ ] Add `mark_as_expired()` method to status checker  
- [ ] Add `handle_sent_state()` method to status checker
- [ ] Enhance `handle_status_impl()` with early exit and timeout checks

### Step 2: Update Transaction Creation ✅
- [ ] Find transaction creation points (API handlers)
- [ ] Add status check job queuing at creation
- [ ] Test that jobs are queued correctly

### Step 3: Remove Old Code ✅
- [ ] Remove status check queuing from `submit_transaction()` (EVM)
- [ ] Remove status check queuing from `submit_transaction()` (Stellar)
- [ ] Remove failure marking from `prepare_transaction()` (EVM)
- [ ] Remove failure marking from handlers
- [ ] Remove `handle_transaction_result()` helper
- [ ] Simplify request handler
- [ ] Simplify submission handler

### Step 4: Testing ✅
- [ ] Test: Transaction created → status check queued
- [ ] Test: Stuck in Pending → marked Failed after timeout
- [ ] Test: Stuck in Sent → submit re-queued, then marked Failed after timeout
- [ ] Test: Stuck in Submitted → resubmitted, then expired after timeout
- [ ] Test: Too early check → returns Ok without doing anything
- [ ] Test: Handler retries exhaust → job aborts, status checker handles
- [ ] Test: No duplicate status check jobs

### Step 5: Cleanup ✅
- [ ] Remove unused imports
- [ ] Update documentation
- [ ] Remove old comments about "CRITICAL" job queue failures
- [ ] Run linter and fix warnings

---

## Testing Scenarios

### 1. Happy Path
```
Create → Status check queued (30s)
      → Prepare queued (0s)
Prepare succeeds → Sent
Submit succeeds → Submitted
Status check runs → monitors → Confirmed
```

### 2. Stuck in Pending
```
Create → Status check queued (30s)
      → Prepare queued (0s)
Prepare fails repeatedly
After 5 minutes → Status check marks Failed
```

### 3. Stuck in Sent
```
Create → Status check queued (30s)
      → Prepare queued (0s)
Prepare succeeds → Sent
Submit job never runs (queue failure)
After 2 minutes → Status check marks Failed
```

### 4. Stuck in Submitted
```
Create → Status check queued (30s)
Prepare → Submit → Submitted
Transaction stuck on-chain
After 1 hour → Status check marks Expired
```

### 5. Too Early Check
```
Create → Status check queued (30s)
Status check runs at 15s → too early → returns Ok
Job retries later → normal processing
```

---

## Expected Benefits

### Before:
- ❌ Status check only after submit
- ❌ Pending transactions never monitored
- ❌ Sent transactions never monitored
- ❌ Domain logic in handlers
- ❌ Duplicate failure marking logic

### After:
- ✅ Single status check from creation
- ✅ All states monitored
- ✅ Duration-based timeouts
- ✅ All domain logic in domain layer
- ✅ Handlers only orchestrate retries
- ✅ No duplicate jobs
- ✅ Cleaner, more maintainable code

---

## Rollout Strategy

**Big Bang Migration (per your request):**

1. Create feature branch
2. Implement all changes
3. Test thoroughly in dev environment
4. Deploy to staging
5. Verify all scenarios work
6. Deploy to production
7. Monitor for 24 hours
8. If issues: rollback
9. If success: merge to main

**No backward compatibility needed.**

---

## Open Questions (Resolved)

✅ **Should we keep status check queueing after submit?**  
→ NO. Only one status check per transaction, queued at creation.

✅ **How to handle prepare job failures?**  
→ Duration-based timeout. If Pending > 5 minutes → Failed.

✅ **Migration strategy?**  
→ Big bang, no backward compatibility.

---

## Next Steps

Ready to implement! Shall we start with:
1. Adding the timeout constants and helper functions?
2. Enhancing the status checker with timeout logic?
3. Finding and updating transaction creation points?

Let me know which phase you'd like to tackle first!

