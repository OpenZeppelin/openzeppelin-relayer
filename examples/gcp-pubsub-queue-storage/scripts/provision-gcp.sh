#!/usr/bin/env bash
# Provision the 8 topics + 8 subscriptions on REAL GCP (not the emulator).
#
# The relayer never creates resources — it probes them at startup and fails fast
# if any are missing. This is the operator-side provisioning step for production
# (a lightweight alternative to Terraform). Idempotent: existing resources are
# left untouched, so it's safe to re-run.
#
# Usage:
#   PUBSUB_PROJECT_ID=my-project ./provision-gcp.sh
#   # or:  ./provision-gcp.sh my-project
#
# Optional env:
#   PUBSUB_TOPIC_PREFIX  (default: relayer-)
#   ACK_DEADLINE         (default: 10; the worker raises each message to 600s at
#                         pull time, so the subscription default is not critical)
#
# Requires an authenticated gcloud (`gcloud auth login` / service account).
set -euo pipefail

PROJECT="${PUBSUB_PROJECT_ID:-${1:-}}"
PREFIX="${PUBSUB_TOPIC_PREFIX:-relayer-}"
ACK="${ACK_DEADLINE:-10}"

if [ -z "${PROJECT}" ]; then
  echo "usage: PUBSUB_PROJECT_ID=<project> $0   (or: $0 <project>)" >&2
  exit 1
fi
command -v gcloud >/dev/null 2>&1 || { echo "error: gcloud CLI not found" >&2; exit 1; }

# The 8 queue types (must match QueueType::queue_name()).
queues="transaction-request transaction-submission status-check status-check-evm \
status-check-stellar notification token-swap-request relayer-health-check"

echo "Provisioning Pub/Sub in project '${PROJECT}' (prefix '${PREFIX}')..."
for q in ${queues}; do
  topic="${PREFIX}${q}"
  sub="${PREFIX}${q}-sub"

  if gcloud pubsub topics describe "${topic}" --project="${PROJECT}" >/dev/null 2>&1; then
    echo "  topic exists:        ${topic}"
  else
    gcloud pubsub topics create "${topic}" --project="${PROJECT}" >/dev/null
    echo "  created topic:       ${topic}"
  fi

  if gcloud pubsub subscriptions describe "${sub}" --project="${PROJECT}" >/dev/null 2>&1; then
    echo "  subscription exists: ${sub}"
  else
    gcloud pubsub subscriptions create "${sub}" \
      --topic="${topic}" --ack-deadline="${ACK}" --project="${PROJECT}" >/dev/null
    echo "  created subscription: ${sub}"
  fi
done

echo ""
echo "Done: 8 topics + 8 subscriptions ensured in '${PROJECT}'."
echo ""
echo "Next, grant the relayer's ADC principal (service account) the required roles:"
echo "  gcloud projects add-iam-policy-binding ${PROJECT} \\"
echo "    --member=\"serviceAccount:<RELAYER_SA>@${PROJECT}.iam.gserviceaccount.com\" \\"
echo "    --role=\"roles/pubsub.editor\"          # or pubsub.publisher + pubsub.subscriber"
echo "  gcloud projects add-iam-policy-binding ${PROJECT} \\"
echo "    --member=\"serviceAccount:<RELAYER_SA>@${PROJECT}.iam.gserviceaccount.com\" \\"
echo "    --role=\"roles/monitoring.viewer\"      # for the backlog-depth read"
echo ""
echo "Then run the relayer with QUEUE_BACKEND=pubsub, PUBSUB_PROJECT_ID=${PROJECT},"
echo "PUBSUB_TOPIC_PREFIX=${PREFIX}, ADC configured, and PUBSUB_EMULATOR_HOST unset."
