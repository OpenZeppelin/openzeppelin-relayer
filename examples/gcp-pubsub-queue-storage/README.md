# OpenZeppelin Relayer Pub/Sub Queue Backend Example

This example runs the relayer with `QUEUE_BACKEND=pubsub` in two modes:

- **Local (default):** the
  [GCP Pub/Sub emulator](https://cloud.google.com/pubsub/docs/emulator) with
  automatic topic + subscription provisioning at startup — no real GCP project
  required. See [Getting Started](#getting-started).
- **Real GCP:** your own project's topics/subscriptions with Application Default
  Credentials, via `docker-compose.gcp.yaml`. See
  [Running against real GCP](#running-against-real-gcp).

Both modes use Redis for the repository, distributed locks, and deferred-job
scheduling.

## Getting Started

### Prerequisites

- [Docker](https://docs.docker.com/get-docker/)
- [Docker Compose](https://docs.docker.com/compose/install/)
- Rust (for signer key generation)

### Step 1: Clone the repository

```bash
git clone https://github.com/OpenZeppelin/openzeppelin-relayer
cd openzeppelin-relayer
```

### Step 2: Create a signer

```bash
cargo run --example create_key -- \
  --password <DEFINE_YOUR_PASSWORD> \
  --output-dir examples/gcp-pubsub-queue-storage/config/keys \
  --filename local-signer.json
```

Create `.env`:

```bash
cp examples/gcp-pubsub-queue-storage/.env.example examples/gcp-pubsub-queue-storage/.env
```

Then set:

- `KEYSTORE_PASSPHRASE`
- `API_KEY`
- `WEBHOOK_SIGNING_KEY`

### Step 3: Configure notification webhook

Edit `examples/gcp-pubsub-queue-storage/config/config.json` and set
`notifications[0].url` to your webhook endpoint (for example from
[Webhook.site](https://webhook.site)).

### Step 4: Start services

```bash
docker compose -f examples/gcp-pubsub-queue-storage/docker-compose.yaml up
```

Services started:

- `relayer` — `QUEUE_BACKEND=pubsub`, `PUBSUB_PROJECT_ID=test-project`,
  `PUBSUB_EMULATOR_HOST=pubsub-emulator:8085`, `DISTRIBUTED_MODE=false`
- `redis` — repository, distributed locks, and the deferred-job scheduled sets
- `pubsub-emulator` — `gcloud beta emulators pubsub start`
- `pubsub-init` — creates all 8 topics + 8 subscriptions, then exits

`pubsub-init` runs to completion **before** the relayer starts, so all required
resources exist when the backend probes them at startup.

### Step 5: Verify relayer

```bash
curl -X GET http://localhost:8080/api/v1/relayers \
  -H "Content-Type: application/json" \
  -H "AUTHORIZATION: Bearer YOUR_API_KEY"
```

### Step 6: Submit a transaction

Submit a transaction via the API and watch the logs: you'll see the
per-subscription pubsub workers pick it up (publish → pull → handler) and the
status-check re-checks run until the transaction reaches a final state.

## Topics + subscriptions created by this example

For prefix `relayer-`, `pubsub-init` creates these 8 topic/subscription pairs:

| Topic | Subscription |
|---|---|
| `relayer-transaction-request` | `relayer-transaction-request-sub` |
| `relayer-transaction-submission` | `relayer-transaction-submission-sub` |
| `relayer-status-check` | `relayer-status-check-sub` |
| `relayer-status-check-evm` | `relayer-status-check-evm-sub` |
| `relayer-status-check-stellar` | `relayer-status-check-stellar-sub` |
| `relayer-notification` | `relayer-notification-sub` |
| `relayer-token-swap-request` | `relayer-token-swap-request-sub` |
| `relayer-relayer-health-check` | `relayer-relayer-health-check-sub` |

No dead-letter topics are required (see below).

## How scheduling, retries, and exhaustion work

Pub/Sub has no native delayed delivery, so deferred jobs and retry backoff are
held in a **Redis sorted set and published when due** (the apalis
store-and-run-when-due pattern). The topic therefore only ever carries
already-due jobs.

- A failed **bounded** job is re-enqueued with an incremented `retry_attempt`;
  once its budget (`max_retries`) is exhausted it **stops retrying**, and the
  transaction's terminal state in the repository is the durable record — the
  relayer does **not** publish to a dead-letter topic (matching the Redis/SQS
  backends).
- **Status-check** queues are unbounded: they re-run (with increasing backoff)
  until the transaction reaches a final state.

A handler that runs longer than a subscription's default ack deadline is **not**
double-processed — the worker extends the lease to 600s (Pub/Sub's max) and
bounds the handler to it. `Ctrl-C` triggers a graceful shutdown that drains
in-flight work without acking incomplete jobs.

## Backlog depth under the emulator

Backlog depth is read from **Cloud Monitoring**, which the emulator does not
serve, so depth counts are reported as **unavailable** locally (not `0`) — the
health endpoint's reachability still works. Against real GCP, depth populates
from `subscription/num_undelivered_messages`.

## Running against real GCP

The same backend runs against a real GCP project using `docker-compose.gcp.yaml`
(the emulator + auto-provisioning are replaced by your real topics/subscriptions
+ ADC). Complete the signer, `.env`, and webhook setup from
[Getting Started](#getting-started) (Steps 1–3), then:

### Step A: Provision topics + subscriptions

The relayer never creates resources — it probes them at startup and fails fast if
any are missing. Create them with the idempotent helper (or your own Terraform):

```bash
PUBSUB_PROJECT_ID=my-project ./examples/gcp-pubsub-queue-storage/scripts/provision-gcp.sh
```

It uses an authenticated `gcloud` (run `gcloud auth login` first) and prints the
IAM bindings to apply — unlike `init-pubsub.sh`, which targets the emulator.

### Step B: Provide credentials (ADC)

The relayer's principal needs `roles/pubsub.publisher` + `roles/pubsub.subscriber`
(or `roles/pubsub.editor`) and `roles/monitoring.viewer` (for the backlog-depth
read). Provide Application Default Credentials either way:

- **Service-account key (default):** place the key JSON at
  `examples/gcp-pubsub-queue-storage/config/keys/gcp-sa-key.json` (gitignored,
  mounted read-only). The relayer fails to start if it is missing.
- **Your gcloud ADC:** set `GCP_CREDENTIALS_FILE` in `.env` to its path:

  ```bash
  GCP_CREDENTIALS_FILE="$HOME/.config/gcloud/application_default_credentials.json"
  ```

In production (GKE/Cloud Run), prefer Workload Identity or the GCE metadata
server over a key file.

### Step C: Set the project and start

Set `PUBSUB_PROJECT_ID` in `.env`, then:

```bash
docker compose -f examples/gcp-pubsub-queue-storage/docker-compose.gcp.yaml up
```

Against real GCP the startup log shows `depth_read=true`: the backend reads
backlog depth from Cloud Monitoring (`subscription/num_undelivered_messages`,
~1–3 min sampling lag), whereas under the emulator that read is *unavailable*.
The depth is exposed per queue as the `queue_depth` Prometheus gauge (labels
`backend`, `queue_type`) — metrics are off by default in this compose, so set
`METRICS_ENABLED=true` and publish `:8081` to scrape it. `/api/v1/ready` reports
aggregate queue health; `/api/v1/health` is a plain liveness probe.

See the [configuration docs](../../docs/configuration/index.mdx) for production
provisioning details.
