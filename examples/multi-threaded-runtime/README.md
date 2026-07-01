# Multi-threaded runtime example

This example runs the relayer on its **explicit multi-thread tokio pipeline runtime**
against a local [anvil](https://book.getfoundry.sh/anvil/) node and Redis (the apalis
queue backend), and shows how to **observe that the queue workers actually spread
across worker threads** instead of pinning to one.

That observation is the open decision gate from the runtime spec
(`specs/005-multithreaded-runtime`, decision **D8** / task **T004**): re-homing the
apalis `Monitor` onto the multi-thread runtime only delivers its benefit if apalis
distributes its workers across threads rather than pinning to a single
`current_thread` `LocalSet`. This example is how you prove it.

## Why this needs a running stack (what was "in the way")

The runtime code is done — what T004 needs is **runtime evidence**:

1. **A running stack** — relayer + Redis (apalis). → Docker compose here. ✅
2. **Sustained job load** — so the queue workers are actually busy. → `scripts/generate-load.sh`. ✅
3. **A way to see which thread each job ran on** — → JSON/pretty logs already include the
   OS thread id (`with_thread_ids`), so every log line carries a `ThreadId(N)` token. No
   profiler or `tokio-console` needed. ✅

So: **yes, Docker works**, and that's what this folder provides.

## What's in here

```
config/config.json          # one EVM relayer on the local anvil network, local signer
config/keys/local-signer.json  # committed TEST keystore (passphrase: MultiThread123!)
docker-compose.yaml         # relayer (multi-thread) + redis + anvil
.env.example                # API key, keystore passphrase, worker-thread sizing
scripts/generate-load.sh    # funds the relayer on anvil and fires ~N tx/s
```

The relayer uses the repo's `localhost-anvil-docker` network
(`config/networks/local-anvil-docker.json`, mounted into the container).

> The keystore is a throwaway generated with
> `cargo run --example create_key -- --password 'MultiThread123!' --output-dir config/keys --filename local-signer.json --force`.
> Its address is funded on anvil at runtime by the load script — **never use it on a real network.**

## Run it

```bash
cd examples/multi-threaded-runtime
cp .env.example .env            # adjust TOKIO_WORKER_THREADS / ACTIX_WORKERS to taste
docker compose up --build -d
```

At startup the relayer logs the resolved budget — confirm it matches your `.env`:

```bash
docker compose logs relayer | grep -i "resolved runtime worker budget"
# ...vcpu=... actix_workers=2 tokio_worker_threads=4...
```

You can also read the gauge:

```bash
curl -s localhost:8081/metrics | grep worker_threads
# worker_threads{runtime="pipeline"} 4
# worker_threads{runtime="http"} 2
```

## Drive load

In another terminal (needs `curl` + `jq`):

```bash
cd examples/multi-threaded-runtime
RATE=30 DURATION=120 API_KEY=multi-threaded-example-key ./scripts/generate-load.sh
```

The script resolves the relayer address, funds it on anvil (`anvil_setBalance`), then
fires self-transfers at ~`RATE`/s.

## Validate apalis thread distribution (the T004 / D8 check)

Logs are JSON with the thread id on every line, so each line carries a `ThreadId(N)`
token. **While the load is running**, count which threads are doing the transaction
pipeline work:

```bash
# Pipeline-stage log lines (prepare/submit/status), bucketed by thread:
docker compose logs --no-color relayer \
  | grep -iE "prepare|submit|status check|transaction request" \
  | grep -oE "ThreadId\([0-9]+\)" \
  | sort | uniq -c | sort -rn
```

Interpretation:

- **PASS (distributes):** the pipeline lines are spread across **multiple** distinct
  `ThreadId`s — ideally close to `TOKIO_WORKER_THREADS`. The work-stealing runtime is
  using the cores. ✅
- **FAIL (pinned):** essentially **all** pipeline lines come from a **single**
  `ThreadId`. apalis is pinning to one thread → re-homing the Monitor did not
  parallelize the queue workers, and the re-home approach (task T008) must be revised
  before relying on it. ❌

A coarse, message-agnostic cross-check (every relayer log line, per thread):

```bash
docker compose logs --no-color relayer | grep -oE "ThreadId\([0-9]+\)" | sort | uniq -c | sort -rn
```

Tip: set `LOG_LEVEL=debug` in `.env` for more per-stage lines, and raise `RATE` for a
sharper signal. If the field name in your build differs, the `ThreadId(N)` token is
emitted regardless of format — `LOG_FORMAT=pretty` prints it inline too.

> Even if some transactions fail (e.g. pricing/estimation against anvil), the check
> still holds: the prepare/submit handlers run on the pipeline worker threads whether
> or not the transaction ultimately confirms — it's the **thread** carrying the work
> we're measuring.

## Record the result

Write the outcome (distributes vs pins, and the distinct-thread count under load) into
`specs/005-multithreaded-runtime/research.md` under **D8**, and flip **T004** in
`tasks.md`. If it pins, stop and revise the Monitor re-home before depending on it.

## Tear down

```bash
docker compose down -v
```
