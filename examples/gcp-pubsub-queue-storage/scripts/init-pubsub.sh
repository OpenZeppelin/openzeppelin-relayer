#!/bin/sh
# Provisions all 8 topics + 8 subscriptions against the Pub/Sub emulator, then
# exits. The emulator has no admin CLI that honors PUBSUB_EMULATOR_HOST, so we
# create resources via its REST API (idempotent: re-creating returns 409, which
# is harmless). Mirrors examples/aws-sqs-queue-storage/scripts/init-sqs.sh.
set -eu

host="${PUBSUB_EMULATOR_HOST:-pubsub-emulator:8085}"
project="${PUBSUB_PROJECT_ID:-test-project}"
prefix="${PUBSUB_TOPIC_PREFIX:-relayer-}"
base="http://${host}/v1/projects/${project}"

# Wait until the emulator REST API is reachable.
until curl -sf "${base}/topics" >/dev/null 2>&1; do
  echo "waiting for Pub/Sub emulator at ${host}..."
  sleep 1
done

create_pair() {
  queue="$1"
  topic="${prefix}${queue}"
  sub="${prefix}${queue}-sub"

  # PUT is idempotent-ish: a 409 (already exists) is fine for repeated runs.
  curl -s -o /dev/null -X PUT "${base}/topics/${topic}"
  curl -s -o /dev/null -X PUT "${base}/subscriptions/${sub}" \
    -H 'Content-Type: application/json' \
    -d "{\"topic\":\"projects/${project}/topics/${topic}\",\"ackDeadlineSeconds\":10}"
  echo "created ${topic} + ${sub}"
}

# The 8 queue types (must match QueueType::queue_name()).
create_pair "transaction-request"
create_pair "transaction-submission"
create_pair "status-check"
create_pair "status-check-evm"
create_pair "status-check-stellar"
create_pair "notification"
create_pair "token-swap-request"
create_pair "relayer-health-check"

echo "Pub/Sub emulator initialized: 8 topics + 8 subscriptions (prefix '${prefix}')."
