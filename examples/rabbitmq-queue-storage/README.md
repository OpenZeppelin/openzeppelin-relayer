# OpenZeppelin Relayer RabbitMQ Queue Backend Example

This example runs the relayer with `QUEUE_BACKEND=rabbitmq` against a **real
broker** — the official [`rabbitmq:4-management`](https://hub.docker.com/_/rabbitmq)
image — entirely locally. No emulator, no cloud account, and **no provisioning
script**: the relayer declares its own 8 durable queues at startup, so bring-up
is a single `docker compose up`.

Redis is used for the repository, distributed locks, and deferred-job
scheduling (RabbitMQ has no native arbitrary delay, so scheduled jobs and retry
backoff are held in Redis and published to the broker when due).

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
  --output-dir examples/rabbitmq-queue-storage/config/keys \
  --filename local-signer.json
```

Create `.env`:

```bash
cp examples/rabbitmq-queue-storage/.env.example examples/rabbitmq-queue-storage/.env
```

Then set:

- `KEYSTORE_PASSPHRASE` (the password from Step 2)
- `API_KEY` (clients present it as `Authorization: Bearer <API_KEY>`)
- `WEBHOOK_SIGNING_KEY`

### Step 3: Configure notification webhook (optional)

Edit `examples/rabbitmq-queue-storage/config/config.json` and add a notification
endpoint (for example from [Webhook.site](https://webhook.site)) if you want
delivery callbacks.

### Step 4: Start services

```bash
docker compose -f examples/rabbitmq-queue-storage/docker-compose.yaml up
```

Services started:

- `relayer` — `QUEUE_BACKEND=rabbitmq`,
  `RABBITMQ_URL=amqp://guest:guest@rabbitmq:5672/%2f`, `DISTRIBUTED_MODE=false`
- `redis` — repository, distributed locks, and the deferred-job scheduled sets
- `rabbitmq` — `rabbitmq:4-management` (AMQP on `:5672`, management UI on `:15672`)

The relayer waits for the broker's healthcheck, then connects, **declares the 8
durable queues**, starts one consumer per queue plus the due-sweep and cron
tasks, and reports healthy.

### Step 5: Verify

```bash
curl -s localhost:8080/api/v1/health                 # relayer liveness
open http://localhost:15672                           # management UI (guest/guest)
```

In the management UI's **Queues** tab you should see the 8 queues below, each
with one consumer.

### Step 6: Submit a transaction

Submit a transaction via the API and watch the logs: the per-queue consumers
pick it up (publish → consume → handler) and the status-check re-checks run
until the transaction reaches a final state. In the management UI you can watch
messages flow through `relayer-transaction-request`,
`relayer-transaction-submission`, and `relayer-status-check-stellar`.

## Queues created by this example

For the default prefix `relayer`, the relayer declares these 8 durable classic
queues at startup (all `durable=true`, non-exclusive, non-auto-delete, no extra
arguments), published via the **default exchange** (`routing_key = queue name`)
— no custom exchanges or bindings:

| # | Queue |
|---|---|
| 1 | `relayer-transaction-request` |
| 2 | `relayer-transaction-submission` |
| 3 | `relayer-status-check` |
| 4 | `relayer-status-check-evm` |
| 5 | `relayer-status-check-stellar` |
| 6 | `relayer-notification` |
| 7 | `relayer-token-swap-request` |
| 8 | `relayer-relayer-health-check` |

No dead-letter exchanges are required (see below).

## How scheduling, retries, and exhaustion work

RabbitMQ has no native arbitrary delayed delivery (the delayed-message-exchange
plugin is archived and incompatible with RabbitMQ 4.3+), so deferred jobs and
retry backoff are held in a **Redis sorted set and published when due** (the
apalis store-and-run-when-due pattern). The broker queues therefore only ever
carry jobs that are due to run now. The scheduled sets live under
`oz-relayer:rabbitmq:scheduled:*`.

- A failed **bounded** job is re-enqueued with an incremented `x-retry-attempt`;
  once its budget (`max_retries`) is exhausted it **stops retrying**, and the
  transaction's terminal state in the repository is the durable record — the
  relayer does **not** publish to a dead-letter exchange (matching the
  Redis/SQS/Pub/Sub backends). Handler panics and timeouts count as failed
  attempts too, so a consistently-failing bounded job stops at `max_retries`.
- **Status-check** queues are unbounded: they re-run (with increasing,
  network-aware backoff) until the transaction reaches a final state.

Publishes are **confirmed**: `produce` returns only after the broker acks a
persistent message to a durable queue (so an accepted job is on disk). A handler
that exceeds the 600s timeout is cancelled and routed through the retry path; an
unacked delivery stays reserved to its consumer while the channel lives (no
double-processing). `Ctrl-C` drains in-flight work without acking incomplete
jobs — anything unacked is requeued by the broker.

## Exercise the reliability story (optional but recommended)

```bash
# Broker restart: durable queues + persistent messages survive, and the relayer
# reconnects by itself (no relayer restart) within ~1 minute.
docker compose -f examples/rabbitmq-queue-storage/docker-compose.yaml restart rabbitmq
# → relayer logs reconnection; processing resumes; /health reports degraded
#   while the broker is down, healthy after it returns.

# Backlog observability: stop the consumers, enqueue work, then resume.
docker compose -f examples/rabbitmq-queue-storage/docker-compose.yaml stop relayer
# (submit a transaction)
docker compose -f examples/rabbitmq-queue-storage/docker-compose.yaml start relayer
# → the queue_depth{backend="rabbitmq",queue_type} gauge and /health depth show
#   the backlog (never 0 while queued), then drain after resume.
```

Depth and health are read with a passive `queue_declare` over the live
connection — no management API, token source, or background snapshot task is
needed, and it works identically here and in production.

## Shut down

```bash
docker compose -f examples/rabbitmq-queue-storage/docker-compose.yaml down
# graceful: in-flight jobs drain; unacked work is requeued by the broker.
# Add -v to also drop the redis/rabbitmq volumes.
```

## Production notes

Full operator reference: [`contracts/config-and-queues.md`](../../specs/003-rabbitmq-queue-backend/contracts/config-and-queues.md)
and the [configuration docs](../../docs/configuration/index.mdx).

- **TLS:** use `amqps://…` for the broker URL; tune the heartbeat with the
  standard URI query parameter, e.g. `?heartbeat=20`.
- **Credentials are secret:** `RABBITMQ_URL` embeds the broker password. The
  relayer redacts it in every log/error (scheme + host + port + vhost only).
  This example's `rabbitmq.conf` (`loopback_users = none`) and `guest:guest` are
  **local-dev only** — in production create a dedicated user with scoped
  permissions on `^relayer-.*` (`configure` + `write` + `read`).
- **Locked-down brokers / quorum queues:** if the app user lacks `configure`
  permission, or you pre-provision **quorum** queues, set
  `RABBITMQ_PASSIVE_QUEUES=true`. The relayer then **verifies** the 8 queues
  exist (needs only `write` + `read`) and fails fast naming anything missing,
  instead of declaring them. (A non-passive declare against a pre-existing
  quorum queue fails with `406 inequivalent arg` by design.)
- **Managed offerings:** works on **Amazon MQ for RabbitMQ** and **CloudAMQP** —
  **no broker plugins required** (classic durable queues + app-declared
  topology). Point `RABBITMQ_URL` at the managed `amqps://` endpoint.
- **Redis remains required** (repositories, distributed cron locks, and
  deferred-job scheduling).
- **Multi-instance:** set `DISTRIBUTED_MODE=true` so cron tasks coordinate via
  Redis distributed locks (at-most-once across the fleet).
