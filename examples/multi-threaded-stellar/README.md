# Multi-threaded Stellar concurrent-ordering example

This example runs the relayer on its **explicit multi-thread tokio pipeline runtime**
against the **public Stellar testnet** and Redis (the apalis queue backend), and
validates that concurrent transactions from a single relayer account stay correctly
ordered under the multi-thread runtime. The submission-ordering gate (`submit_gate`)
combined with the monotonic `sync_floor` ensures sequence numbers are submitted in
strict order even when multiple worker threads race to submit jobs concurrently —
preventing `TxBadSeq` from out-of-order submission across threads. This is spec 005,
task **T025**, and the **Constitution III non-negotiable gate**.

## Prerequisites

- Docker (compose v2)
- `bash`, `curl`, `jq` on the host
- The keystore `config/keys/local-signer.json` — generated separately with:
  ```bash
  cargo run --example create_key -- \
    --password 'MtStellarRuntime123!' \
    --output-dir examples/multi-threaded-stellar/config/keys \
    --filename local-signer.json \
    --force
  ```

## Quick start

```bash
cd examples/multi-threaded-stellar

# .env is already provided with the correct values for the committed keystore.
# Adjust TOKIO_WORKER_THREADS / ACTIX_WORKERS to taste if needed.
cp .env.example .env

docker compose up --build -d
```

Wait for the relayer to start (confirm with `docker compose logs relayer | grep "resolved runtime worker budget"`).

Then fire the load script:

```bash
API_KEY=multi-threaded-stellar-example-api-key ./scripts/load-stellar.sh
```

Tune the burst size and number of rounds:

```bash
CONCURRENCY=50 ROUNDS=3 API_KEY=multi-threaded-stellar-example-api-key ./scripts/load-stellar.sh
```

## What to look for

### TxBadSeq stays ~0

Check the metrics endpoint at `http://localhost:8081/metrics` for:

```
STELLAR_SUBMISSION_FAILURES{submit_status, result_code}
```

A `result_code` label of `tx_bad_seq` appearing here means the gate is not holding.
Under correct operation this counter should stay at zero even under a burst of 50+
concurrent requests.

### High transaction success rate

Transactions should reach `Submitted` / `Confirmed` status. You can poll the relayer:

```bash
curl -s -H "Authorization: Bearer ${API_KEY}" \
  http://localhost:8080/api/v1/relayers/mt-stellar/transactions | jq '.data[].status'
```

### Gate log line visible in debug logs

When bursts arrive out of order across worker threads, the gate defers the
out-of-sequence job and re-enqueues it. With `LOG_LEVEL=debug` you will see:

```
submit deferred: sequence ahead of account watermark, re-enqueuing submit job
```

followed by ordered submission proceeding. This confirms the gate is active and
preventing `TxBadSeq` rather than the burst happening to arrive in order by chance.

### Thread distribution on the Stellar path

JSON logs carry a `ThreadId(N)` token on every line. Bucketing pipeline lines by
thread confirms the Stellar path also spreads across the multi-thread runtime:

```bash
docker compose logs --no-color relayer \
  | grep -iE "prepare|submit|status check|transaction request" \
  | grep -oE "ThreadId\([0-9]+\)" \
  | sort | uniq -c | sort -rn
```

Multiple distinct `ThreadId`s confirm work is distributed, not pinned to one thread.

## Results (validated 2026-06-30)

**Run configuration:** one `mt-stellar` testnet relayer, `concurrent_transactions: true`,
`TOKIO_WORKER_THREADS=4` / `ACTIX_WORKERS=2`, a 100-deep concurrent self-payment burst
against a single account (`CONCURRENCY=50 ROUNDS=2`).

**Headline result — `TxBadSeq` = 0.** `stellar_submission_failures_total` never
incremented with a `tx_bad_seq` result code across the entire run. The ordering gate
fully prevented out-of-order submission failures.

Additional observations:

- The gate emitted ~1 500 `submit deferred: sequence ahead of account watermark`
  log lines — out-of-order submit jobs were correctly re-enqueued and retried, with
  the per-account watermark advancing by one sequence per ledger.
- Submit work spread across all 4 pipeline threads (ThreadId 3–6 visible in the JSON
  logs), confirming the Stellar path is not pinned to a single worker.

### Why most of a deep burst expires — and why that's correct

A single Stellar account can only land roughly one transaction per ledger (~5 s on
testnet), because each transaction consumes the next sequence number and the gate
submits them strictly in order. A burst deeper than the transaction validity window
(set by the default time bounds) will therefore see its tail reach terminal `expired`
(`Transaction time_bounds expired`) or `TxTooLate` — not because of a sequence error,
but because those transactions waited past their validity window while the gate
(correctly) held them behind earlier sequences.

In the 100-deep validation run: ~5 confirmed, ~94 expired, 1 TxTooLate, **0 TxBadSeq**.

This is the gate trading harmless expiry for correctness. Without the gate those same
transactions would have raced to the network out of order and produced the `TxBadSeq`
storms this feature exists to eliminate.

**Practical guidance:** to see a high confirmation rate rather than expiry, size the
burst to what one account can land within the transaction validity window. For example,
run `CONCURRENCY=10 ROUNDS=1` (or fewer in-flight), or spread load across multiple
relayer accounts. Deep single-account bursts — like this 100-deep run — are useful
precisely for stress-proving that ordering holds (0 TxBadSeq) under contention, not
for throughput benchmarking.

## Gotchas

**Account must be friendbot-funded before any transaction.**
Friendbot creates the account and gives it 10 000 testnet XLM. The load script does
this automatically and polls Horizon to confirm the account exists before sending any
transactions. If the account is already funded, friendbot returns an error — the
script detects this and continues.

**Testnet Horizon can rate-limit under high concurrency.**
Horizon may return `TRY_AGAIN_LATER` under a large burst. The relayer handles this
with retries. Do not conflate `TRY_AGAIN_LATER` with `TxBadSeq` — they are distinct
result codes. Keep `CONCURRENCY` in the 20–100 range to stay within testnet limits.

**Restarting the relayer orphans queued jobs.**
The relayer uses the in-memory transaction repository by default. Restarting it while
jobs are still in Redis causes `transaction not found` log spam as apalis workers pick
up jobs that no longer exist in memory. Between runs, flush Redis and let the relayer
recreate cleanly:

```bash
docker compose exec -T redis redis-cli flushall
docker compose restart relayer
```

**`LOG_LEVEL=debug` is required for full gate visibility.**
At `info` level the pipeline handlers only log startup. The gate log line and per-job
thread attribution only appear at `debug`. The default `.env` already sets
`LOG_LEVEL=debug`.

## Tear down

```bash
docker compose down -v
```
