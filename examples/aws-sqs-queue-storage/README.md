# OpenZeppelin Relayer SQS Queue Backend Example (LocalStack)

This example shows how to run the relayer with:

- `QUEUE_BACKEND=sqs`
- local SQS emulator (LocalStack)
- automatic queue + DLQ provisioning at startup

It is intended for local development/testing of the SQS queue backend.

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

Create a local signer keystore:

```bash
cargo run --example create_key -- \
  --password <DEFINE_YOUR_PASSWORD> \
  --output-dir examples/aws-sqs-queue-storage/config/keys \
  --filename local-signer.json
```

Create `.env`:

```bash
cp examples/aws-sqs-queue-storage/.env.example examples/aws-sqs-queue-storage/.env
```

Then set:

- `KEYSTORE_PASSPHRASE`
- `API_KEY`
- `WEBHOOK_SIGNING_KEY`

### Step 3: Configure notification webhook

Edit:

- `examples/aws-sqs-queue-storage/config/config.json`

Set `notifications[0].url` to your webhook endpoint (for example from [Webhook.site](https://webhook.site)).

### Step 4: Start services

```bash
docker compose -f examples/aws-sqs-queue-storage/docker-compose.yaml up
```

Services started:

- `relayer`
- `redis` (repository/locks)
- `localstack` (SQS emulator)
- `sqs-init` (creates required FIFO queues + DLQs and redrive policies)

### Step 5: Verify relayer

```bash
curl -X GET http://localhost:8080/api/v1/relayers \
  -H "Content-Type: application/json" \
  -H "AUTHORIZATION: Bearer YOUR_API_KEY"
```

## Queues created by this example

Main queues:

- `relayer-transaction-request.fifo`
- `relayer-transaction-submission.fifo`
- `relayer-status-check.fifo`
- `relayer-status-check-evm.fifo`
- `relayer-status-check-stellar.fifo`
- `relayer-notification.fifo`
- `relayer-token-swap-request.fifo`
- `relayer-relayer-health-check.fifo`

DLQs:

- `relayer-transaction-request-dlq.fifo`
- `relayer-transaction-submission-dlq.fifo`
- `relayer-status-check-dlq.fifo`
- `relayer-status-check-evm-dlq.fifo`
- `relayer-status-check-stellar-dlq.fifo`
- `relayer-notification-dlq.fifo`
- `relayer-token-swap-request-dlq.fifo`
- `relayer-relayer-health-check-dlq.fifo`

Redrive policy is configured automatically by `sqs-init`.

All queues are created with high-throughput FIFO mode (`DeduplicationScope=messageGroup`, `FifoThroughputLimit=perMessageGroupId`). For production AWS deployments, enable these attributes on the transaction-request, transaction-submission, and status-check queues to raise throughput from 300 to 70,000 messages/second per queue. See the [configuration docs](../../docs/configuration/index.mdx) for details.
