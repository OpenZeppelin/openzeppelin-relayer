#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
WIREMOCK_DIR="${REPO_ROOT}/testing/wiremock"
DEFAULT_ARTIFACT_DIR="/tmp/claude-501/codex-parity.lm3FQk"
ARTIFACT_DIR="${ARTIFACT_DIR:-${DEFAULT_ARTIFACT_DIR}}"
VERDICTS_FILE="${VERDICTS_FILE:-${ARTIFACT_DIR}/verdicts.json}"
REPORT_FILE="${REPORT_FILE:-${ARTIFACT_DIR}/report.md}"
RELAYER_LOG="${RELAYER_LOG:-${ARTIFACT_DIR}/relayer.log}"
HARNESS_LOG="${HARNESS_LOG:-${ARTIFACT_DIR}/harness.log}"
RELAYER_CONFIG_DIR="${WIREMOCK_DIR}/e2e"
RELAYER_CONFIG_FILE_NAME="fast-resubmit-config.json"
REQUESTED_SCENARIO="all"
MODE="matrix"
PROFILE="${PROFILE:-default}"
SQS_QUEUE_TYPE="${SQS_QUEUE_TYPE:-standard}"
PROD_ENV_JSON="${PROD_ENV_JSON:-}"
CHANNELS_ENV_FILE="${WIREMOCK_DIR}/e2e/env.channels-parity"
SUBSTITUTION_REPORT="${ARTIFACT_DIR}/parity-substitutions.md"
SOAK_VERDICT_FILE="${ARTIFACT_DIR}/soak-verdict.json"
SOAK_TX_FILE="${ARTIFACT_DIR}/soak-transactions.jsonl"
SOAK_STATUS_FILE="${ARTIFACT_DIR}/soak-statuses.jsonl"
SOAK_JOURNAL_FILE="${ARTIFACT_DIR}/wiremock-journal-soak.json"
SOAK_QUEUE_DEPTH_FILE="${ARTIFACT_DIR}/soak-queue-depth.jsonl"
SOAK_SEND_EVENTS_FILE="${ARTIFACT_DIR}/soak-send-events.jsonl"
SOAK_DOUBLE_FIRE_FILE="${ARTIFACT_DIR}/soak-double-fire-pairs.json"
SOAK_RELAYERS_FILE="${ARTIFACT_DIR}/soak-relayers.jsonl"
SOAK_RATE=""
SOAK_DURATION="120"
SOAK_IF_PCT="0"
SOAK_TAL_PCT="0"
SOAK_RELAYER_COUNT="10"
SOAK_POLL_TIMEOUT="300"

WIREMOCK_URL="${WIREMOCK_URL:-http://localhost:9090}"
RELAYER_ID="${RELAYER_ID:-stellar-fast-resubmit}"
RELAYER_PORT="${RELAYER_PORT:-18080}"
RELAYER_URL="${RELAYER_URL:-http://127.0.0.1:${RELAYER_PORT}}"
API_KEY="${API_KEY:-multi-threaded-stellar-example-api-key}"
KEYSTORE_PASSPHRASE="${KEYSTORE_PASSPHRASE:-MtStellarRuntime123!}"
WEBHOOK_SIGNING_KEY="${WEBHOOK_SIGNING_KEY:-example-webhook-signing-key}"
RUN_ID="fast-resubmit-$(date +%s)-$$"
REDIS_CONTAINER="${REDIS_CONTAINER:-oz-relayer-e2e-redis-${RUN_ID}}"
LOCALSTACK_CONTAINER="${LOCALSTACK_CONTAINER:-oz-relayer-e2e-localstack-${RUN_ID}}"
LOCALSTACK_IMAGE="${LOCALSTACK_IMAGE:-localstack/localstack:3}"
REDIS_URL_FOR_RELAYER=""
SQS_ENDPOINT_URL=""
SQS_QUEUE_URL_PREFIX_FOR_RELAYER=""
RELAYER_PID=""
SOAK_INJECTOR_PID=""
QUEUE_DEPTH_SAMPLER_PID=""
PASS_COUNT=0
FAIL_COUNT=0
SOAK_RELAYER_IDS=()
SOAK_RELAYER_ADDRESSES=()

# Keep if-overcap last: it intentionally never lands on-chain, so the relayer's
# local sequence cache advances past the chain sequence and can poison later cases.
ALL_SCENARIOS=(if-once if-escalation tal-window tal-window-closes if-overcap)
SQS_QUEUE_BASE_NAMES=(
  transaction-request
  transaction-submission
  status-check
  status-check-evm
  status-check-stellar
  notification
  token-swap-request
  relayer-health-check
)

set_artifact_dir() {
  ARTIFACT_DIR="$1"
  VERDICTS_FILE="${ARTIFACT_DIR}/verdicts.json"
  REPORT_FILE="${ARTIFACT_DIR}/report.md"
  RELAYER_LOG="${ARTIFACT_DIR}/relayer.log"
  HARNESS_LOG="${ARTIFACT_DIR}/harness.log"
  SUBSTITUTION_REPORT="${ARTIFACT_DIR}/parity-substitutions.md"
  SOAK_VERDICT_FILE="${ARTIFACT_DIR}/soak-verdict.json"
  SOAK_TX_FILE="${ARTIFACT_DIR}/soak-transactions.jsonl"
  SOAK_STATUS_FILE="${ARTIFACT_DIR}/soak-statuses.jsonl"
  SOAK_JOURNAL_FILE="${ARTIFACT_DIR}/wiremock-journal-soak.json"
  SOAK_QUEUE_DEPTH_FILE="${ARTIFACT_DIR}/soak-queue-depth.jsonl"
  SOAK_SEND_EVENTS_FILE="${ARTIFACT_DIR}/soak-send-events.jsonl"
  SOAK_DOUBLE_FIRE_FILE="${ARTIFACT_DIR}/soak-double-fire-pairs.json"
  SOAK_RELAYERS_FILE="${ARTIFACT_DIR}/soak-relayers.jsonl"
}

configure_artifact_paths() {
  if [[ "${PROFILE}" == "channels-parity" && "${MODE}" == "matrix" ]]; then
    VERDICTS_FILE="${ARTIFACT_DIR}/verdicts-parity-${SQS_QUEUE_TYPE}.json"
    RELAYER_LOG="${ARTIFACT_DIR}/relayer-parity-${SQS_QUEUE_TYPE}.log"
    HARNESS_LOG="${ARTIFACT_DIR}/harness-parity-${SQS_QUEUE_TYPE}.log"
  elif [[ "${MODE}" == "soak" ]]; then
    VERDICTS_FILE="${ARTIFACT_DIR}/soak-transactions.jsonl"
    RELAYER_LOG="${ARTIFACT_DIR}/relayer-soak.log"
    HARNESS_LOG="${ARTIFACT_DIR}/harness-soak.log"
  fi
}

usage() {
  printf 'usage: %s [-o artifact-dir] [--profile default|channels-parity] [--sqs-queue-type standard|fifo] <scenario|all>\n' "$0"
  printf '       %s soak [--duration SECONDS] [--if-pct PCT] [--tal-pct PCT] [--relayers N] [--profile channels-parity] [--sqs-queue-type standard|fifo] [-o artifact-dir]\n' "$0"
  printf 'scenarios: %s\n' "${ALL_SCENARIOS[*]}"
}

parse_args() {
  local requested_set=false
  if (($#)) && [[ "$1" == "soak" ]]; then
    MODE="soak"
    REQUESTED_SCENARIO="soak"
    shift
  fi

  while (($#)); do
    case "$1" in
      -o|--output-dir)
        shift
        if (($# == 0)); then
          usage >&2
          exit 64
        fi
        set_artifact_dir "$1"
        ;;
      --profile)
        shift
        if (($# == 0)); then
          usage >&2
          exit 64
        fi
        PROFILE="$1"
        case "${PROFILE}" in
          default|channels-parity) ;;
          *)
            printf 'unsupported profile: %s\n' "${PROFILE}" >&2
            usage >&2
            exit 64
            ;;
        esac
        ;;
      --sqs-queue-type|--queue-type)
        shift
        if (($# == 0)); then
          usage >&2
          exit 64
        fi
        SQS_QUEUE_TYPE="$1"
        case "${SQS_QUEUE_TYPE}" in
          standard|fifo) ;;
          *)
            printf 'unsupported SQS queue type: %s\n' "${SQS_QUEUE_TYPE}" >&2
            usage >&2
            exit 64
            ;;
        esac
        ;;
      --rate)
        shift
        if (($# == 0)); then
          usage >&2
          exit 64
        fi
        # Accepted for older invocations; closed-loop soak does not use a submit rate.
        SOAK_RATE="$1"
        ;;
      --duration)
        shift
        if (($# == 0)); then
          usage >&2
          exit 64
        fi
        SOAK_DURATION="$1"
        ;;
      --if-pct)
        shift
        if (($# == 0)); then
          usage >&2
          exit 64
        fi
        SOAK_IF_PCT="$1"
        ;;
      --tal-pct)
        shift
        if (($# == 0)); then
          usage >&2
          exit 64
        fi
        SOAK_TAL_PCT="$1"
        ;;
      --relayers)
        shift
        if (($# == 0)); then
          usage >&2
          exit 64
        fi
        SOAK_RELAYER_COUNT="$1"
        ;;
      -h|--help)
        usage
        exit 0
        ;;
      -*)
        usage >&2
        exit 64
        ;;
      *)
        if [[ "${MODE}" == "soak" ]]; then
          usage >&2
          exit 64
        fi
        if [[ "${requested_set}" == "true" ]]; then
          usage >&2
          exit 64
        fi
        REQUESTED_SCENARIO="$1"
        requested_set=true
        ;;
    esac
    shift
  done

  if [[ "${MODE}" == "soak" ]]; then
    if [[ -n "${SOAK_RATE}" ]] && ! [[ "${SOAK_RATE}" =~ ^[0-9]+([.][0-9]+)?$ ]]; then
      printf 'soak --rate must be numeric when supplied\n' >&2
      exit 64
    fi
    if ! [[ "${SOAK_DURATION}" =~ ^[0-9]+$ ]]; then
      printf 'soak --duration must be an integer number of seconds\n' >&2
      exit 64
    fi
    if ! [[ "${SOAK_IF_PCT}" =~ ^[0-9]+$ && "${SOAK_TAL_PCT}" =~ ^[0-9]+$ ]]; then
      printf 'soak rejection percentages must be integer percentages\n' >&2
      exit 64
    fi
    if ! [[ "${SOAK_RELAYER_COUNT}" =~ ^[0-9]+$ ]] || (( SOAK_RELAYER_COUNT < 1 )); then
      printf 'soak --relayers must be a positive integer\n' >&2
      exit 64
    fi
  fi

  configure_artifact_paths
}

log() {
  printf '[e2e-fast-resubmit] %s\n' "$*" >&2
  printf '[e2e-fast-resubmit] %s\n' "$*" >>"${HARNESS_LOG}"
}

need_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    printf 'missing required command: %s\n' "$1" >&2
    exit 127
  fi
}

compose_cmd() {
  if docker compose version >/dev/null 2>&1; then
    printf 'docker compose'
  elif command -v docker-compose >/dev/null 2>&1; then
    printf 'docker-compose'
  else
    printf 'missing docker compose\n' >&2
    exit 127
  fi
}

cleanup() {
  local exit_code=$?
  if [[ -n "${SOAK_INJECTOR_PID}" ]] && kill -0 "${SOAK_INJECTOR_PID}" >/dev/null 2>&1; then
    log "stopping soak injector pid ${SOAK_INJECTOR_PID}"
    kill "${SOAK_INJECTOR_PID}" >/dev/null 2>&1 || true
    wait "${SOAK_INJECTOR_PID}" >/dev/null 2>&1 || true
  fi
  if [[ -n "${QUEUE_DEPTH_SAMPLER_PID}" ]] && kill -0 "${QUEUE_DEPTH_SAMPLER_PID}" >/dev/null 2>&1; then
    log "stopping queue depth sampler pid ${QUEUE_DEPTH_SAMPLER_PID}"
    kill "${QUEUE_DEPTH_SAMPLER_PID}" >/dev/null 2>&1 || true
    wait "${QUEUE_DEPTH_SAMPLER_PID}" >/dev/null 2>&1 || true
  fi
  if [[ -n "${RELAYER_PID}" ]] && kill -0 "${RELAYER_PID}" >/dev/null 2>&1; then
    log "stopping relayer pid ${RELAYER_PID}"
    kill "${RELAYER_PID}" >/dev/null 2>&1 || true
    wait "${RELAYER_PID}" >/dev/null 2>&1 || true
  fi
  if docker ps -a --format '{{.Names}}' | grep -qx "${REDIS_CONTAINER}"; then
    log "stopping redis container ${REDIS_CONTAINER}"
    docker rm -f "${REDIS_CONTAINER}" >/dev/null 2>&1 || true
  fi
  if docker ps -a --format '{{.Names}}' | grep -qx "${LOCALSTACK_CONTAINER}"; then
    log "stopping LocalStack container ${LOCALSTACK_CONTAINER}"
    docker rm -f "${LOCALSTACK_CONTAINER}" >/dev/null 2>&1 || true
  fi
  log "stopping WireMock compose stack"
  # shellcheck disable=SC2086
  $(compose_cmd) -f "${WIREMOCK_DIR}/docker-compose.yaml" down >/dev/null 2>&1 || true
  exit "${exit_code}"
}

wait_for_http() {
  local url="$1"
  local label="$2"
  local timeout_s="${3:-60}"
  local deadline=$(( $(date +%s) + timeout_s ))
  until curl -fsS "${url}" >/dev/null 2>&1; do
    if (( $(date +%s) >= deadline )); then
      log "timed out waiting for ${label} at ${url}"
      return 1
    fi
    sleep 1
  done
}

start_wiremock() {
  log "starting WireMock on ${WIREMOCK_URL}"
  # shellcheck disable=SC2086
  WIREMOCK_PORT=9090 WIREMOCK_PROXY_TARGET="${WIREMOCK_PROXY_TARGET:-https://soroban-testnet.stellar.org}" \
    $(compose_cmd) -f "${WIREMOCK_DIR}/docker-compose.yaml" up -d wiremock >>"${HARNESS_LOG}" 2>&1
  wait_for_http "${WIREMOCK_URL}/__admin/health" "WireMock" 90
}

start_redis() {
  log "starting disposable Redis container ${REDIS_CONTAINER}"
  docker rm -f "${REDIS_CONTAINER}" >/dev/null 2>&1 || true
  docker run -d --name "${REDIS_CONTAINER}" -p 127.0.0.1::6379 redis:bookworm >>"${HARNESS_LOG}" 2>&1

  local deadline=$(( $(date +%s) + 60 ))
  until docker exec "${REDIS_CONTAINER}" redis-cli ping 2>/dev/null | grep -q PONG; do
    if (( $(date +%s) >= deadline )); then
      log "timed out waiting for Redis"
      return 1
    fi
    sleep 1
  done

  local redis_port
  redis_port="$(docker port "${REDIS_CONTAINER}" 6379/tcp | sed -E 's/.*:([0-9]+)$/\1/')"
  REDIS_URL_FOR_RELAYER="redis://127.0.0.1:${redis_port}"
  log "Redis is ready at ${REDIS_URL_FOR_RELAYER}"
}

aws_sqs() {
  AWS_ACCESS_KEY_ID=test \
  AWS_SECRET_ACCESS_KEY=test \
  AWS_SESSION_TOKEN=test \
  AWS_REGION=us-east-1 \
  AWS_PAGER= \
    aws --cli-connect-timeout 3 --cli-read-timeout 10 --endpoint-url "${SQS_ENDPOINT_URL}" sqs "$@"
}

start_localstack() {
  log "starting LocalStack SQS emulator ${LOCALSTACK_CONTAINER}"
  docker rm -f "${LOCALSTACK_CONTAINER}" >/dev/null 2>&1 || true
  docker run -d --name "${LOCALSTACK_CONTAINER}" \
    -e SERVICES=sqs \
    -e DEBUG=0 \
    -p 127.0.0.1::4566 \
    "${LOCALSTACK_IMAGE}" >>"${HARNESS_LOG}" 2>&1

  local localstack_port
  localstack_port="$(docker port "${LOCALSTACK_CONTAINER}" 4566/tcp | sed -E 's/.*:([0-9]+)$/\1/')"
  SQS_ENDPOINT_URL="http://127.0.0.1:${localstack_port}"

  local deadline=$(( $(date +%s) + 120 ))
  until curl -fsS "${SQS_ENDPOINT_URL}/_localstack/health" 2>/dev/null \
    | jq -e '.services.sqs == "running" or .services.sqs == "available"' >/dev/null; do
    if (( $(date +%s) >= deadline )); then
      log "timed out waiting for LocalStack SQS at ${SQS_ENDPOINT_URL}"
      return 1
    fi
    sleep 1
  done
  log "LocalStack SQS is ready at ${SQS_ENDPOINT_URL}"
}

create_sqs_queues() {
  local suffix=""
  local first_queue_name first_queue_url queue_name queue_url base_name
  if [[ "${SQS_QUEUE_TYPE}" == "fifo" ]]; then
    suffix=".fifo"
  fi

  first_queue_name="relayer-stellar-transaction-request${suffix}"
  for base_name in "${SQS_QUEUE_BASE_NAMES[@]}"; do
    queue_name="relayer-stellar-${base_name}${suffix}"
    if [[ "${SQS_QUEUE_TYPE}" == "fifo" ]]; then
      queue_url="$(aws_sqs create-queue \
        --queue-name "${queue_name}" \
        --attributes FifoQueue=true,ContentBasedDeduplication=false \
        --query QueueUrl \
        --output text)"
    else
      queue_url="$(aws_sqs create-queue \
        --queue-name "${queue_name}" \
        --query QueueUrl \
        --output text)"
    fi
    log "created SQS ${SQS_QUEUE_TYPE} queue ${queue_name}: ${queue_url}"
    if [[ "${queue_name}" == "${first_queue_name}" ]]; then
      first_queue_url="${queue_url}"
    fi
  done

  if [[ -z "${first_queue_url:-}" ]]; then
    log "failed to derive SQS_QUEUE_URL_PREFIX from created queues"
    return 1
  fi

  SQS_QUEUE_URL_PREFIX_FOR_RELAYER="${first_queue_url%transaction-request${suffix}}"
  log "derived SQS_QUEUE_URL_PREFIX=${SQS_QUEUE_URL_PREFIX_FOR_RELAYER}"
}

channels_prod_env_json() {
  if [[ -n "${PROD_ENV_JSON}" ]]; then
    printf '%s' "${PROD_ENV_JSON}"
  else
    printf '%s' "${ARTIFACT_DIR}/channels-env-sanitized.json"
  fi
}

generate_channels_parity_env() {
  local prod_json local_env_json reasons_json
  prod_json="$(channels_prod_env_json)"
  if [[ ! -f "${prod_json}" ]]; then
    log "missing sanitized prod env JSON: ${prod_json}"
    return 1
  fi

  local_env_json="${ARTIFACT_DIR}/channels-parity-env.json"
  reasons_json="${ARTIFACT_DIR}/channels-parity-substitution-reasons.json"

  jq -n \
    --slurpfile prod "${prod_json}" \
    --arg api_key "${API_KEY}" \
    --arg keystore_passphrase "${KEYSTORE_PASSPHRASE}" \
    --arg webhook_key "${WEBHOOK_SIGNING_KEY}" \
    --arg redis_url "${REDIS_URL_FOR_RELAYER}" \
    --arg app_port "${RELAYER_PORT}" \
    --arg config_dir "${RELAYER_CONFIG_DIR}" \
    --arg config_file_name "${RELAYER_CONFIG_FILE_NAME}" \
    --arg endpoint "${SQS_ENDPOINT_URL}" \
    --arg prefix "${SQS_QUEUE_URL_PREFIX_FOR_RELAYER}" \
    --arg queue_type "${SQS_QUEUE_TYPE}" \
    --arg run_id "${RUN_ID}" \
    '
    ($prod[0].env // {}) + {
      API_KEY: $api_key,
      API_KEY_HEADER: "Authorization",
      APP_PORT: $app_port,
      AWS_ACCESS_KEY_ID: "test",
      AWS_ACCOUNT_ID: "000000000000",
      AWS_EC2_METADATA_DISABLED: "true",
      AWS_ENDPOINT_URL: $endpoint,
      AWS_ENDPOINT_URL_SQS: $endpoint,
      AWS_SECRET_ACCESS_KEY: "test",
      AWS_SESSION_TOKEN: "test",
      CONFIG_DIR: $config_dir,
      CONFIG_FILE_NAME: $config_file_name,
      ENABLE_SWAGGER: "false",
      HOST: "127.0.0.1",
      KEYSTORE_PASSPHRASE: $keystore_passphrase,
      METRICS_ENABLED: "false",
      PLUGIN_ADMIN_SECRET: "local-dev-plugin-admin-secret-32bytes",
      QUEUE_BACKEND: "sqs",
      REDIS_CONNECTION_TIMEOUT_MS: "30000",
      REDIS_KEY_PREFIX: $run_id,
      REDIS_POOL_MAX_SIZE: "128",
      REDIS_POOL_TIMEOUT_MS: "60000",
      REDIS_READER_POOL_MAX_SIZE: "128",
      REDIS_READER_URL: $redis_url,
      REDIS_URL: $redis_url,
      REPOSITORY_STORAGE_TYPE: "redis",
      RPC_ALLOWED_HOSTS: "localhost,127.0.0.1",
      RPC_BLOCK_PRIVATE_IPS: "false",
      SQS_QUEUE_TYPE: $queue_type,
      SQS_QUEUE_URL_PREFIX: $prefix,
      STELLAR_NETWORK: "testnet",
      STORAGE_ENCRYPTION_KEY: "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=",
      WEBHOOK_SIGNING_KEY: $webhook_key
    }' >"${local_env_json}"

  jq -r '
    to_entries
    | sort_by(.key)
    | .[]
    | "\(.key)=\(.value | @sh)"
  ' "${local_env_json}" >"${CHANNELS_ENV_FILE}"
  chmod 600 "${CHANNELS_ENV_FILE}"

  jq -n '
    {
      API_KEY: "Prod secret is redacted; local API client and relayer share a dev value.",
      API_KEY_HEADER: "Sanitized prod value was redacted; the code uses Authorization locally.",
      APP_PORT: "Local harness binds the relayer to the requested test port.",
      AWS_ACCESS_KEY_ID: "Dummy credential required by the AWS SDK for LocalStack.",
      AWS_ACCOUNT_ID: "LocalStack default test account.",
      AWS_EC2_METADATA_DISABLED: "Avoid host metadata lookups during local tests.",
      AWS_ENDPOINT_URL: "Point the AWS SDK at the LocalStack edge endpoint.",
      AWS_ENDPOINT_URL_SQS: "Point the SQS SDK client at the LocalStack edge endpoint.",
      AWS_SECRET_ACCESS_KEY: "Dummy credential required by the AWS SDK for LocalStack.",
      AWS_SESSION_TOKEN: "Dummy credential required by the AWS SDK for LocalStack.",
      CONFIG_DIR: "Use the WireMock e2e config directory.",
      CONFIG_FILE_NAME: "Use the selected fast-resubmit e2e config file.",
      ENABLE_SWAGGER: "Disable local docs server surface; not relevant to queue parity.",
      HOST: "Bind only to localhost for the harness.",
      KEYSTORE_PASSPHRASE: "Prod uses external secrets; local e2e signer keystore requires this dev passphrase.",
      METRICS_ENABLED: "Disable the separate metrics listener to avoid local port conflicts.",
      PLUGIN_ADMIN_SECRET: "Prod secret is redacted; local dev value is sufficient because no plugins are configured.",
      REDIS_CONNECTION_TIMEOUT_MS: "Give the local Docker Redis emulator more startup/connection headroom.",
      REDIS_KEY_PREFIX: "Unique per run to isolate local Redis keys.",
      REDIS_POOL_MAX_SIZE: "Bound local Docker Redis connections; prod pool size is too large for the disposable emulator.",
      REDIS_POOL_TIMEOUT_MS: "Allow config bootstrap to wait through local Redis connection churn.",
      REDIS_READER_POOL_MAX_SIZE: "Bound local Docker Redis read connections; prod pool size is too large for the disposable emulator.",
      REDIS_READER_URL: "Use the disposable local Redis container for reads.",
      REDIS_URL: "Use the disposable local Redis container for repository storage.",
      RPC_ALLOWED_HOSTS: "Allow the WireMock localhost RPC endpoint.",
      RPC_BLOCK_PRIVATE_IPS: "Permit localhost RPC for the test harness.",
      SQS_QUEUE_TYPE: "Explicitly bracket standard and FIFO behavior; prod type is unknown.",
      SQS_QUEUE_URL_PREFIX: "Derived from queues created in the LocalStack SQS emulator.",
      STELLAR_NETWORK: "Run against Stellar testnet only.",
      STORAGE_ENCRYPTION_KEY: "Prod secret is redacted; local 32-byte base64 AES key.",
      WEBHOOK_SIGNING_KEY: "Prod secret is redacted; local dev value is sufficient because no webhooks are configured."
    }' >"${reasons_json}"

  {
    printf '# Channels parity substitutions\n\n'
    printf 'Generated env file: `%s`\n\n' "${CHANNELS_ENV_FILE}"
    printf '| Variable | Prod sanitized value | Local value | Why |\n'
    printf '|---|---|---|---|\n'
    jq -rn \
      --slurpfile prod "${prod_json}" \
      --slurpfile local "${local_env_json}" \
      --slurpfile reasons "${reasons_json}" '
      ($prod[0].env // {}) as $p
      | ($local[0] // {}) as $l
      | ($reasons[0] // {}) as $r
      | ((($p | keys_unsorted) + ($l | keys_unsorted)) | unique | sort)[] as $key
      | select(($p[$key] // null) != ($l[$key] // null))
      | [
          $key,
          (($p[$key] // "<unset>") | tostring),
          (($l[$key] // "<unset>") | tostring),
          ($r[$key] // "Local harness-only setting or emulator/runtime derived value.")
        ]
      | @tsv' \
      | while IFS=$'\t' read -r key prod local why; do
          printf '| `%s` | `%s` | `%s` | %s |\n' "${key}" "${prod}" "${local}" "${why}"
        done
    printf '\n## Prod knobs intentionally kept verbatim\n\n'
    jq -r '
      .env
      | with_entries(select(.key
          | test("^(TOKIO_WORKER_THREADS|ACTIX_WORKERS|BACKGROUND_WORKER_|TRANSACTION_EXPIRATION_HOURS|REQUEST_TIMEOUT_SECONDS|SQS_.*_(POLLER_COUNT|WAIT_TIME_SECONDS)|DISTRIBUTED_MODE)$")))
      | to_entries
      | sort_by(.key)
      | .[]
      | "- `\(.key)=\(.value)`"
    ' "${prod_json}"
    printf '\n## Config-file substitution\n\n'
    printf -- '- Stellar network config is not present in the prod env JSON. The harness uses `%s/%s`, whose `rpc_urls` point at `%s` and whose network is `testnet`.\n' \
      "${RELAYER_CONFIG_DIR}" "${RELAYER_CONFIG_FILE_NAME}" "${WIREMOCK_URL}"
    if [[ "${MODE}" == "soak" ]]; then
      printf '\n## Soak topology substitution\n\n'
      printf -- '- Soak mode generated `%s` relayers, one local signer keystore per relayer, under the gitignored directory `%s`.\n' \
        "${SOAK_RELAYER_COUNT}" "${RELAYER_CONFIG_DIR}"
      printf -- '- Submissions are closed-loop per relayer: each generated account has at most one in-flight transaction, and the next transaction is submitted only after the previous one reaches a terminal status.\n'
      printf -- '- There is no open-loop submit rate in soak mode; throughput emerges from `%s` independent relayer loops.\n' \
        "${SOAK_RELAYER_COUNT}"
    fi
    printf '\n## Scenario validity exception\n\n'
    printf -- '- `tal-window-closes` sends an explicit 10-minute `valid_until` in every profile, including channels-parity. The scenario verifies checker handoff mechanics, and the ladder rescue lands at about 110s+ transaction age, which collides with the default roughly 2-minute Stellar envelope validity plus confirmation latency. That product-level interplay is flagged separately in the PR; this harness exception keeps the handoff test from failing on validity policy.\n'
  } >"${SUBSTITUTION_REPORT}"

  log "generated channels parity env ${CHANNELS_ENV_FILE}"
  log "wrote substitution ledger ${SUBSTITUTION_REPORT}"
}

start_relayer() {
  log "building relayer binary"
  (
    cd "${REPO_ROOT}"
    cargo build --bin openzeppelin-relayer
  ) >>"${RELAYER_LOG}" 2>&1

  log "starting relayer at ${RELAYER_URL}; logs: ${RELAYER_LOG}"
  if [[ "${PROFILE}" == "channels-parity" ]]; then
    (
      cd "${REPO_ROOT}"
      set -a
      # shellcheck disable=SC1090
      source "${CHANNELS_ENV_FILE}"
      set +a
      exec "${REPO_ROOT}/target/debug/openzeppelin-relayer"
    ) >>"${RELAYER_LOG}" 2>&1 &
  else
    (
      cd "${REPO_ROOT}"
      API_KEY="${API_KEY}" \
      KEYSTORE_PASSPHRASE="${KEYSTORE_PASSPHRASE}" \
      WEBHOOK_SIGNING_KEY="${WEBHOOK_SIGNING_KEY}" \
      HOST=127.0.0.1 \
      APP_PORT="${RELAYER_PORT}" \
      CONFIG_DIR="${RELAYER_CONFIG_DIR}" \
      CONFIG_FILE_NAME="${RELAYER_CONFIG_FILE_NAME}" \
      REDIS_URL="${REDIS_URL_FOR_RELAYER}" \
      QUEUE_BACKEND=redis \
      REPOSITORY_STORAGE_TYPE=in_memory \
      REDIS_KEY_PREFIX="${RUN_ID}" \
      METRICS_ENABLED=false \
      ENABLE_SWAGGER=false \
      LOG_LEVEL=debug \
      LOG_FORMAT=compact \
      RPC_BLOCK_PRIVATE_IPS=false \
      RPC_ALLOWED_HOSTS=localhost,127.0.0.1 \
      TOKIO_WORKER_THREADS=4 \
      ACTIX_WORKERS=2 \
      exec "${REPO_ROOT}/target/debug/openzeppelin-relayer"
    ) >>"${RELAYER_LOG}" 2>&1 &
  fi
  RELAYER_PID=$!
  printf '%s\n' "${RELAYER_PID}" >"${ARTIFACT_DIR}/relayer.pid"
  wait_for_http "${RELAYER_URL}/api/v1/health" "relayer" 120
}

restart_relayer_after_soak_funding() {
  if [[ "${MODE}" != "soak" ]]; then
    return 0
  fi

  log "restarting relayer after soak account funding"
  if [[ -n "${RELAYER_PID}" ]] && kill -0 "${RELAYER_PID}" >/dev/null 2>&1; then
    kill "${RELAYER_PID}" >/dev/null 2>&1 || true
    wait "${RELAYER_PID}" >/dev/null 2>&1 || true
    RELAYER_PID=""
  fi

  if docker ps --format '{{.Names}}' | grep -qx "${REDIS_CONTAINER}"; then
    docker exec "${REDIS_CONTAINER}" redis-cli FLUSHALL >>"${HARNESS_LOG}" 2>&1 || true
  fi

  start_relayer
}

api_get() {
  local path="$1"
  curl -sS --connect-timeout 3 --max-time 15 -H "Authorization: Bearer ${API_KEY}" "${RELAYER_URL}${path}"
}

api_post_json() {
  local path="$1"
  local payload="$2"
  local tmp http body
  tmp="$(mktemp)"
  http="$(curl -sS --connect-timeout 3 --max-time 15 -o "${tmp}" -w '%{http_code}' \
    -H "Authorization: Bearer ${API_KEY}" \
    -H "Content-Type: application/json" \
    -X POST "${RELAYER_URL}${path}" \
    -d "${payload}" || true)"
  body="$(cat "${tmp}")"
  rm -f "${tmp}"
  if [[ ! "${http}" =~ ^2 ]]; then
    printf '%s' "${body}"
    return 1
  fi
  printf '%s' "${body}"
}

discover_relayer_address() {
  local body address
  body="$(api_get "/api/v1/relayers/${RELAYER_ID}")"
  address="$(jq -r '.data.address // empty' <<<"${body}")"
  if [[ -z "${address}" ]]; then
    log "failed to discover relayer address from response: ${body}"
    return 1
  fi
  printf '%s' "${address}"
}

ensure_funded() {
  local address="$1"
  if curl -fsS "https://horizon-testnet.stellar.org/accounts/${address}" >/dev/null 2>&1; then
    log "relayer account already funded: ${address}"
    return 0
  fi

  local attempt
  for attempt in 1 2 3 4 5; do
    log "funding relayer account with friendbot: ${address} (attempt ${attempt}/5)"
    curl -fsS "https://friendbot.stellar.org/?addr=${address}" >>"${HARNESS_LOG}" 2>&1 || true
    sleep $((attempt * 2))
    if curl -fsS "https://horizon-testnet.stellar.org/accounts/${address}" >/dev/null 2>&1; then
      return 0
    fi
  done

  log "failed to fund relayer account after retries: ${address}"
  return 1
}

soak_relayer_id() {
  local index="$1"
  printf 'stellar-fast-resubmit-%s' "${index}"
}

generate_soak_relayer_config() {
  if [[ "${MODE}" != "soak" ]]; then
    return 0
  fi

  local generated_dir relayer_index signer_id relayer_id filename
  generated_dir="${WIREMOCK_DIR}/e2e/generated-relayers/${RUN_ID}"
  RELAYER_CONFIG_DIR="${generated_dir}"
  RELAYER_CONFIG_FILE_NAME="fast-resubmit-config.json"

  mkdir -p "${generated_dir}"
  SOAK_RELAYER_IDS=()

  log "generating soak config with ${SOAK_RELAYER_COUNT} relayers in ${generated_dir}"
  for ((relayer_index = 1; relayer_index <= SOAK_RELAYER_COUNT; relayer_index++)); do
    relayer_id="$(soak_relayer_id "${relayer_index}")"
    signer_id="local-signer-${relayer_index}"
    filename="${signer_id}.keystore"
    SOAK_RELAYER_IDS+=("${relayer_id}")

    (
      cd "${REPO_ROOT}"
      cargo run --example create_key -- \
        --password "${KEYSTORE_PASSPHRASE}" \
        --output-dir "testing/wiremock/e2e/generated-relayers/${RUN_ID}" \
        --filename "${filename}" \
        --force
    ) >>"${HARNESS_LOG}" 2>&1
  done

  jq \
    --argjson relayers "${SOAK_RELAYER_COUNT}" \
    --arg run_id "${RUN_ID}" \
    '
    .relayers = [
      range(1; $relayers + 1) as $i
      | {
          id: ("stellar-fast-resubmit-" + ($i | tostring)),
          name: ("Stellar Fast Resubmit E2E " + ($i | tostring)),
          network: "testnet",
          paused: false,
          network_type: "stellar",
          signer_id: ("local-signer-" + ($i | tostring)),
          policies: {
            fee_payment_strategy: "relayer",
            min_balance: 0
          }
        }
    ]
    | .signers = [
      range(1; $relayers + 1) as $i
      | {
          id: ("local-signer-" + ($i | tostring)),
          type: "local",
          config: {
            path: ("testing/wiremock/e2e/generated-relayers/" + $run_id + "/local-signer-" + ($i | tostring) + ".keystore"),
            passphrase: {
              type: "env",
              value: "KEYSTORE_PASSPHRASE"
            }
          }
        }
    ]' \
    "${WIREMOCK_DIR}/e2e/fast-resubmit-config.json" \
    >"${generated_dir}/${RELAYER_CONFIG_FILE_NAME}"
}

discover_soak_relayers() {
  local relayer_id body address
  : >"${SOAK_RELAYERS_FILE}"
  SOAK_RELAYER_ADDRESSES=()

  for relayer_id in "${SOAK_RELAYER_IDS[@]}"; do
    body="$(api_get "/api/v1/relayers/${relayer_id}")"
    address="$(jq -r '.data.address // empty' <<<"${body}")"
    if [[ -z "${address}" ]]; then
      log "failed to discover relayer address for ${relayer_id}: ${body}"
      return 1
    fi
    ensure_funded "${address}"
    SOAK_RELAYER_ADDRESSES+=("${address}")
    jq -nc \
      --arg relayer_id "${relayer_id}" \
      --arg address "${address}" \
      '{relayer_id:$relayer_id,address:$address}' \
      >>"${SOAK_RELAYERS_FILE}"
  done
}

reset_wiremock() {
  curl -fsS -X POST "${WIREMOCK_URL}/__admin/scenarios/reset" >/dev/null
  curl -fsS -X DELETE "${WIREMOCK_URL}/__admin/requests" >/dev/null
}

arm_scenario() {
  local scenario_name="$1"
  local state="$2"
  curl -fsS -X PUT "${WIREMOCK_URL}/__admin/scenarios/${scenario_name}/state" \
    -H 'Content-Type: application/json' \
    -d "$(jq -nc --arg state "${state}" '{state:$state}')" >/dev/null
}

scenario_state() {
  local scenario_name="$1"
  curl -fsS "${WIREMOCK_URL}/__admin/scenarios" \
    | jq -r --arg name "${scenario_name}" '.scenarios[] | select(.name == $name) | .state // "Started"'
}

soak_scenarios_idle() {
  local if_state tal_state
  if_state="$(scenario_state "stellar-insufficient-fee" 2>/dev/null || printf 'unknown')"
  tal_state="$(scenario_state "stellar-try-again-later" 2>/dev/null || printf 'unknown')"
  [[ "${if_state}" == "Started" && "${tal_state}" == "Started" ]]
}

send_stats_json() {
  local memo="${1:-}"
  curl -fsS "${WIREMOCK_URL}/__admin/requests" | jq -c --arg memo "${memo}" '
    def body_json:
      (.request.bodyAsString // .request.body // "{}") as $body
      | if ($body | type) == "object" then
          $body
        else
          ($body | try fromjson catch {})
        end;
    def rpc_method:
      (body_json | .method // "");
    def tx_xdr:
      (body_json | ((try .params.transaction catch null) // (try .params[0].transaction catch null) // ""));
    def envelope_text:
      try (tx_xdr | @base64d) catch "";
    def to_epoch:
      if type == "number" then
        if . > 100000000000 then (. / 1000) else . end
      else
        (tostring | sub("\\+0000$"; "Z") | sub("\\.[0-9]+Z$"; "Z") | fromdateiso8601)
      end;
    [ .requests[]
      | select(.request.method == "POST")
      | select(rpc_method == "sendTransaction")
      | select($memo == "" or (envelope_text | contains($memo)))
      | (.request.loggedDate // .loggedDate // .loggedDateString)
      | to_epoch
    ] | sort as $times
    | {
        send_calls: ($times | length),
        gaps_s: [
          range(1; ($times | length)) as $i
          | (((($times[$i] - $times[$i - 1]) * 10) | round) / 10)
        ]
      }'
}

scenario_rpc_name() {
  case "$1" in
    if-*) printf 'stellar-insufficient-fee' ;;
    tal-*) printf 'stellar-try-again-later' ;;
    *) return 1 ;;
  esac
}

scenario_arm_state() {
  case "$1" in
    if-once) printf 'armed' ;;
    if-escalation) printf 'armed-3' ;;
    if-overcap) printf 'armed-7' ;;
    tal-window) printf 'armed-3' ;;
    tal-window-closes) printf 'armed-4' ;;
    *) return 1 ;;
  esac
}

scenario_expected_gaps() {
  case "$1" in
    if-once) printf '[5]' ;;
    if-escalation) printf '[5,10,15]' ;;
    if-overcap) printf '[5,10,15,20,25,30]' ;;
    tal-window) printf '[5,10,15]' ;;
    tal-window-closes) printf '[5,10,15,10]' ;;
    *) return 1 ;;
  esac
}

scenario_expected_calls() {
  case "$1" in
    if-once) printf '2' ;;
    if-escalation) printf '4' ;;
    if-overcap) printf '7' ;;
    tal-window) printf '4' ;;
    tal-window-closes) printf '5' ;;
    *) return 1 ;;
  esac
}

scenario_fast_log() {
  case "$1" in
    if-*) printf 'enqueueing fast resubmit after insufficient fee' ;;
    tal-*) printf 'enqueueing fast resubmit after TRY_AGAIN_LATER' ;;
    *) return 1 ;;
  esac
}

submit_fresh_transaction() {
  local destination="$1"
  local scenario="$2"
  local memo valid_until payload body tx_id
  memo="fr-$(date +%s)-${RANDOM}"
  valid_until=""
  if [[ "${scenario}" == "tal-window-closes" ]]; then
    valid_until="$(( $(date +%s) + 600 ))"
  fi
  payload="$(jq -nc \
    --arg destination "${destination}" \
    --arg memo "${memo}" \
    --arg valid_until "${valid_until}" \
    '({
      network: "testnet",
      operations: [
        {
          type: "payment",
          destination: $destination,
          amount: "1",
          asset: {type: "native"}
        }
      ],
      memo: {type: "text", value: $memo}
    } + (if $valid_until == "" then {} else {valid_until: $valid_until} end))')"
  body="$(api_post_json "/api/v1/relayers/${RELAYER_ID}/transactions" "${payload}")" || {
    printf '%s' "${body}"
    return 1
  }
  tx_id="$(jq -r '.data.id // empty' <<<"${body}")"
  if [[ -z "${tx_id}" ]]; then
    log "${scenario}: missing tx id in response: ${body}"
    return 1
  fi
  jq -nc --arg tx_id "${tx_id}" --arg memo "${memo}" '{tx_id:$tx_id,memo:$memo}'
}

scenario_poll_timeout() {
  case "$1" in
    tal-window-closes) printf '300' ;;
    *) printf '120' ;;
  esac
}

poll_transaction() {
  local tx_id="$1"
  local scenario="$2"
  local timeout_s="${3:-120}"
  local deadline=$(( $(date +%s) + timeout_s ))
  local body status reason

  status=""
  reason=""
  while (( $(date +%s) < deadline )); do
    body="$(api_get "/api/v1/relayers/${RELAYER_ID}/transactions/${tx_id}")" || true
    status="$(jq -r '.data.status // empty' <<<"${body}" 2>/dev/null || true)"
    reason="$(jq -r '.data.status_reason // ""' <<<"${body}" 2>/dev/null || true)"

    case "${scenario}:${status}" in
      if-overcap:failed|*:confirmed|*:mined|*:expired|*:canceled)
        jq -nc --arg status "${status}" --arg reason "${reason}" '{status:$status, reason:$reason}'
        return 0
        ;;
      if-once:submitted)
        jq -nc --arg status "${status}" --arg reason "${reason}" '{status:$status, reason:$reason}'
        return 0
        ;;
    esac
    sleep 2
  done

  jq -nc --arg status "${status:-timeout}" --arg reason "${reason}" '{status:$status, reason:$reason}'
}

log_bool_seen() {
  local tx_id="$1"
  local needle="$2"
  if awk -v tx_id="${tx_id}" -v needle="${needle}" \
    'index($0, tx_id) && index($0, needle) { found = 1; exit } END { exit found ? 0 : 1 }' \
    "${RELAYER_LOG}"; then
    printf 'true'
  else
    printf 'false'
  fi
}

evaluate_pass() {
  local scenario="$1"
  local stats="$2"
  local final_status="$3"
  local status_reason="$4"
  local fast_seen="$5"
  local checker_seen="$6"
  local expected_calls expected_gaps
  expected_calls="$(scenario_expected_calls "${scenario}")"
  expected_gaps="$(scenario_expected_gaps "${scenario}")"

  jq -nc \
    --arg scenario "${scenario}" \
    --argjson stats "${stats}" \
    --arg status "${final_status}" \
    --arg reason "${status_reason}" \
    --argjson expected_calls "${expected_calls}" \
    --argjson expected_gaps "${expected_gaps}" \
    --argjson fast_seen "${fast_seen}" \
    --argjson checker_seen "${checker_seen}" '
      def gap_ok($actual; $expected; $idx; $scenario):
        if $scenario == "tal-window-closes" and $idx == 3 then
          $actual >= 10
        else
          ($actual >= ($expected - 1) and $actual <= ($expected + 8))
        end;

      ($stats.gaps_s // []) as $gaps
      | [
          if $stats.send_calls == $expected_calls then empty else "send_calls expected \($expected_calls) got \($stats.send_calls)" end,
          if ($gaps | length) == ($expected_gaps | length) then empty else "gaps expected length \(($expected_gaps | length)) got \(($gaps | length))" end,
          (range(0; ([($gaps | length), ($expected_gaps | length)] | min)) as $i
            | if gap_ok($gaps[$i]; $expected_gaps[$i]; $i; $scenario) then empty else "gap[\($i)] expected \($expected_gaps[$i])s got \($gaps[$i])s" end),
          if $fast_seen then empty else "fast log not seen" end,
          if ($scenario == "tal-window-closes") then
            if $checker_seen then empty else "checker log not seen" end
          else
            if ($checker_seen | not) then empty else "checker log unexpectedly seen" end
          end,
          if ($scenario == "if-overcap") then
            if $status == "failed" then empty else "final_status expected failed got \($status)" end,
            if ($reason | contains("insufficient fee retry limit exceeded (6)")) then empty else "status_reason missing retry cap" end
          elif ($scenario == "if-once") then
            if ($status == "confirmed" or $status == "submitted" or $status == "mined") then empty else "final_status expected confirmed/submitted got \($status)" end
          else
            if ($status == "confirmed" or $status == "mined") then empty else "final_status expected confirmed got \($status)" end
          end
        ] as $notes
      | {pass: (($notes | length) == 0), notes: ($notes | join("; "))}'
}

emit_verdict() {
  local scenario="$1"
  local stats="$2"
  local final_status="$3"
  local fast_seen="$4"
  local checker_seen="$5"
  local pass="$6"
  local notes="$7"
  local line
  line="$(jq -nc \
    --arg scenario "${scenario}" \
    --argjson send_calls "$(jq '.send_calls' <<<"${stats}")" \
    --argjson gaps_s "$(jq -c '.gaps_s' <<<"${stats}")" \
    --arg final_status "${final_status}" \
    --argjson fast_log_seen "${fast_seen}" \
    --argjson checker_log_seen "${checker_seen}" \
    --argjson pass "${pass}" \
    --arg notes "${notes}" \
    '{
      scenario: $scenario,
      send_calls: $send_calls,
      gaps_s: $gaps_s,
      final_status: $final_status,
      fast_log_seen: $fast_log_seen,
      checker_log_seen: $checker_log_seen,
      pass: $pass,
      notes: $notes
    }')"
  printf '%s\n' "${line}"
  printf '%s\n' "${line}" >>"${VERDICTS_FILE}"
}

run_scenario() {
  local scenario="$1"
  local address="$2"
  local rpc_scenario arm_state submitted tx_id memo poll stats final_status status_reason fast_seen checker_seen eval pass notes
  rpc_scenario="$(scenario_rpc_name "${scenario}")"
  arm_state="$(scenario_arm_state "${scenario}")"

  log "running ${scenario}: arm ${rpc_scenario} -> ${arm_state}"
  reset_wiremock
  arm_scenario "${rpc_scenario}" "${arm_state}"

  if ! submitted="$(submit_fresh_transaction "${address}" "${scenario}")"; then
    stats='{"send_calls":0,"gaps_s":[]}'
    emit_verdict "${scenario}" "${stats}" "submit_failed" false false false "transaction submit API failed: ${submitted}"
    FAIL_COUNT=$((FAIL_COUNT + 1))
    return 0
  fi
  tx_id="$(jq -r '.tx_id // empty' <<<"${submitted}")"
  memo="$(jq -r '.memo // empty' <<<"${submitted}")"
  if [[ -z "${tx_id}" || -z "${memo}" ]]; then
    stats='{"send_calls":0,"gaps_s":[]}'
    emit_verdict "${scenario}" "${stats}" "submit_failed" false false false "transaction submit API returned malformed body: ${submitted}"
    FAIL_COUNT=$((FAIL_COUNT + 1))
    return 0
  fi
  log "${scenario}: tx_id=${tx_id} memo=${memo}"

  poll="$(poll_transaction "${tx_id}" "${scenario}" "$(scenario_poll_timeout "${scenario}")")"
  final_status="$(jq -r '.status' <<<"${poll}")"
  status_reason="$(jq -r '.reason' <<<"${poll}")"
  stats="$(send_stats_json "${memo}")"
  fast_seen="$(log_bool_seen "${tx_id}" "$(scenario_fast_log "${scenario}")")"
  checker_seen="$(log_bool_seen "${tx_id}" "re-enqueueing submit job for stuck Sent transaction")"

  eval="$(evaluate_pass "${scenario}" "${stats}" "${final_status}" "${status_reason}" "${fast_seen}" "${checker_seen}")"
  pass="$(jq -r '.pass' <<<"${eval}")"
  notes="$(jq -r '.notes' <<<"${eval}")"
  emit_verdict "${scenario}" "${stats}" "${final_status}" "${fast_seen}" "${checker_seen}" "${pass}" "${notes}"

  if [[ "${pass}" == "true" ]]; then
    PASS_COUNT=$((PASS_COUNT + 1))
  else
    FAIL_COUNT=$((FAIL_COUNT + 1))
    grep -F "${tx_id}" "${RELAYER_LOG}" >"${ARTIFACT_DIR}/${scenario}-${tx_id}.log" || true
    log "${scenario}: failed; tx-scoped log excerpt: ${ARTIFACT_DIR}/${scenario}-${tx_id}.log"
  fi
}

submit_soak_transaction() {
  local relayer_id="$1"
  local destination="$2"
  local index="$3"
  local memo payload body tx_id submitted_at
  memo="s${index}"
  submitted_at="$(date -u +%FT%TZ)"
  payload="$(jq -nc \
    --arg destination "${destination}" \
    --arg memo "${memo}" \
    '{
      network: "testnet",
      operations: [
        {
          type: "payment",
          destination: $destination,
          amount: "1",
          asset: {type: "native"}
        }
      ],
      memo: {type: "text", value: $memo}
    }')"

  if body="$(api_post_json "/api/v1/relayers/${relayer_id}/transactions" "${payload}")"; then
    tx_id="$(jq -r '.data.id // empty' <<<"${body}")"
    jq -nc \
      --arg index "${index}" \
      --arg relayer_id "${relayer_id}" \
      --arg destination "${destination}" \
      --arg tx_id "${tx_id}" \
      --arg memo "${memo}" \
      --arg submitted_at "${submitted_at}" \
      '{index: ($index | tonumber), relayer_id: $relayer_id, destination: $destination, tx_id: $tx_id, memo: $memo, submitted_at: $submitted_at, submit_ok: true}'
  else
    jq -nc \
      --arg index "${index}" \
      --arg relayer_id "${relayer_id}" \
      --arg destination "${destination}" \
      --arg memo "${memo}" \
      --arg submitted_at "${submitted_at}" \
      --arg error "${body}" \
      '{index: ($index | tonumber), relayer_id: $relayer_id, destination: $destination, tx_id: null, memo: $memo, submitted_at: $submitted_at, submit_ok: false, error: $error}'
  fi
}

soak_rejection_counts_json() {
  curl -fsS "${WIREMOCK_URL}/__admin/requests" 2>/dev/null | jq -c '
    def body_json:
      (.request.bodyAsString // .request.body // "{}") as $body
      | if ($body | type) == "object" then
          $body
        else
          ($body | try fromjson catch {})
        end;
    def rpc_method:
      (body_json | .method // "");
    def response_text:
      [
        (.response.body // empty),
        (.responseDefinition.body // empty),
        ((.responseDefinition.jsonBody // empty) | tojson?),
        ((.response.jsonBody // empty) | tojson?)
      ] | map(select(. != null)) | join(" ");
    def event_kind:
      response_text as $r
      | if ($r | contains("TRY_AGAIN_LATER")) then
          "tal_reject"
        elif (($r | contains("AAAAAAAAY/n////3AAAAAA==")) or ($r | ascii_downcase | contains("insufficient"))) then
          "if_reject"
        else
          "accepted"
        end;
    [ .requests[]
      | select(.request.method == "POST")
      | select(rpc_method == "sendTransaction")
      | event_kind
    ] as $events
    | {
        send_calls: ($events | length),
        if_rejections: ($events | map(select(. == "if_reject")) | length),
        tal_rejections: ($events | map(select(. == "tal_reject")) | length)
      }' || printf '{"send_calls":0,"if_rejections":0,"tal_rejections":0}'
}

soak_injector_loop() {
  local total_pct=$((SOAK_IF_PCT + SOAK_TAL_PCT))
  if (( total_pct <= 0 )); then
    while true; do sleep 60; done
  fi

  local counts send_calls if_rejections tal_rejections desired_if desired_tal deficit_if deficit_tal rejection_type

  while true; do
    if soak_scenarios_idle; then
      counts="$(soak_rejection_counts_json)"
      send_calls="$(jq -r '.send_calls // 0' <<<"${counts}")"
      if_rejections="$(jq -r '.if_rejections // 0' <<<"${counts}")"
      tal_rejections="$(jq -r '.tal_rejections // 0' <<<"${counts}")"
      desired_if=$((send_calls * SOAK_IF_PCT / 100))
      desired_tal=$((send_calls * SOAK_TAL_PCT / 100))
      deficit_if=$((desired_if - if_rejections))
      deficit_tal=$((desired_tal - tal_rejections))
      rejection_type="none"
      if (( deficit_if > 0 || deficit_tal > 0 )); then
        if (( deficit_if >= deficit_tal )); then
          rejection_type="if"
        else
          rejection_type="tal"
        fi
      fi
      case "${rejection_type}" in
        if)
          arm_scenario "stellar-insufficient-fee" "armed" || true
          ;;
        tal)
          arm_scenario "stellar-try-again-later" "armed" || true
          ;;
      esac
    fi
    sleep 0.5
  done
}

submission_queue_url() {
  local suffix=""
  if [[ "${SQS_QUEUE_TYPE}" == "fifo" ]]; then
    suffix=".fifo"
  fi
  printf '%stransaction-submission%s' "${SQS_QUEUE_URL_PREFIX_FOR_RELAYER}" "${suffix}"
}

sample_submission_queue_depth_loop() {
  if [[ "${PROFILE}" != "channels-parity" ]]; then
    while true; do sleep 60; done
  fi

  local queue_url attrs visible invisible delayed depth sampled_at
  queue_url="$(submission_queue_url)"
  while true; do
    sampled_at="$(date -u +%FT%TZ)"
    if attrs="$(aws_sqs get-queue-attributes \
        --queue-url "${queue_url}" \
        --attribute-names ApproximateNumberOfMessages ApproximateNumberOfMessagesNotVisible ApproximateNumberOfMessagesDelayed \
        --output json 2>/dev/null)"; then
      visible="$(jq -r '.Attributes.ApproximateNumberOfMessages // "0"' <<<"${attrs}")"
      invisible="$(jq -r '.Attributes.ApproximateNumberOfMessagesNotVisible // "0"' <<<"${attrs}")"
      delayed="$(jq -r '.Attributes.ApproximateNumberOfMessagesDelayed // "0"' <<<"${attrs}")"
      depth=$((visible + invisible + delayed))
      jq -nc \
        --arg sampled_at "${sampled_at}" \
        --arg queue_url "${queue_url}" \
        --argjson visible "${visible}" \
        --argjson invisible "${invisible}" \
        --argjson delayed "${delayed}" \
        --argjson depth "${depth}" \
        '{sampled_at:$sampled_at, queue_url:$queue_url, visible:$visible, invisible:$invisible, delayed:$delayed, depth:$depth}' \
        >>"${SOAK_QUEUE_DEPTH_FILE}"
    fi
    sleep 2
  done
}

poll_soak_transaction() {
  local relayer_id="$1"
  local tx_id="$2"
  local timeout_s="${3:-${SOAK_POLL_TIMEOUT}}"
  local deadline=$(( $(date +%s) + timeout_s ))
  local status_body status reason

  status=""
  reason=""
  while true; do
    status_body="$(api_get "/api/v1/relayers/${relayer_id}/transactions/${tx_id}")" || true
    status="$(jq -r '.data.status // "unknown"' <<<"${status_body}" 2>/dev/null || printf 'unknown')"
    reason="$(jq -r '.data.status_reason // ""' <<<"${status_body}" 2>/dev/null || printf '')"

    case "${status}" in
      confirmed|mined|failed|expired|canceled)
        jq -nc \
          --arg status "${status}" \
          --arg reason "${reason}" \
          --arg terminal_at "$(date -u +%FT%TZ)" \
          '{status:$status, status_reason:$reason, terminal_at:$terminal_at}'
        return 0
        ;;
    esac

    if (( $(date +%s) >= deadline )); then
      jq -nc \
        --arg status "${status:-timeout}" \
        --arg reason "${reason}" \
        --arg terminal_at "$(date -u +%FT%TZ)" \
        '{status:$status, status_reason:$reason, terminal_at:$terminal_at, poll_timeout:true}'
      return 0
    fi
    sleep 2
  done
}

run_soak_relayer_loop() {
  local relayer_slot="$1"
  local relayer_id="$2"
  local address="$3"
  local end_epoch="$4"
  local tx_part_file="$5"
  local status_part_file="$6"
  local local_index index submitted tx_id submit_ok poll

  local_index=0
  while (( $(date +%s) < end_epoch )); do
    local_index=$((local_index + 1))
    index=$((relayer_slot * 1000000 + local_index))
    submitted="$(submit_soak_transaction "${relayer_id}" "${address}" "${index}")"
    printf '%s\n' "${submitted}" >>"${tx_part_file}"

    submit_ok="$(jq -r '.submit_ok // false' <<<"${submitted}")"
    tx_id="$(jq -r '.tx_id // empty' <<<"${submitted}")"
    if [[ "${submit_ok}" == "true" && -n "${tx_id}" ]]; then
      poll="$(poll_soak_transaction "${relayer_id}" "${tx_id}" "${SOAK_POLL_TIMEOUT}")"
      jq -nc --argjson tx "${submitted}" --argjson poll "${poll}" '$tx + $poll' >>"${status_part_file}"
    else
      jq -nc --argjson tx "${submitted}" '$tx + {status:"submit_failed", status_reason:($tx.error // "")}' >>"${status_part_file}"
      sleep 1
    fi
  done
}

write_soak_verdict() {
  local status_summary journal_summary depth_summary txbadseq_count
  curl -fsS "${WIREMOCK_URL}/__admin/requests" >"${SOAK_JOURNAL_FILE}"
  txbadseq_count="$(awk 'index($0, "Transaction submission error: TxBadSeq") { count++ } END { print count + 0 }' "${RELAYER_LOG}")"

  status_summary="$(jq -s -c '
    map(select(.submit_ok == true and (.tx_id // "") != "")) as $txs
    | ($txs | map(select(.status == "confirmed" or .status == "mined")) | length) as $success
    | {
        tx_count: ($txs | length),
        api_submit_ok_count: ($txs | length),
        success_count: $success,
        eventual_success_rate: (if ($txs | length) == 0 then 0 else (($success * 10000 / ($txs | length)) | round / 100) end),
        creation_distribution_by_relayer: (
          $txs
          | group_by(.relayer_id // "unknown")
          | map({relayer_id: (.[0].relayer_id // "unknown"), tx_count: length})
        ),
        status_counts: (
          $txs
          | group_by(.status // "unknown")
          | map({status: (.[0].status // "unknown"), count: length})
        ),
        failure_reason_counts: (
          $txs
          | map(select((.status // "") == "failed" or (.status // "") == "expired"))
          | group_by(.status_reason // "")
          | map({reason: (.[0].status_reason // ""), count: length})
          | sort_by(-.count)
        )
      }' "${SOAK_STATUS_FILE}")"

  journal_summary="$(jq -c --slurpfile txs <(jq -s "." "${SOAK_STATUS_FILE}") '
    def body_json:
      (.request.bodyAsString // .request.body // "{}") as $body
      | if ($body | type) == "object" then
          $body
        else
          ($body | try fromjson catch {})
        end;
    def rpc_method:
      (body_json | .method // "");
    def tx_xdr:
      (body_json | ((try .params.transaction catch null) // (try .params[0].transaction catch null) // ""));
    def memo_from_xdr:
      try (. | @base64d | capture("(?<memo>s[0-9]+)").memo) catch null;
    def response_json:
      [
        (.response.body // empty | try fromjson catch empty),
        (.response.jsonBody // empty),
        (.responseDefinition.body // empty | try fromjson catch empty),
        (.responseDefinition.jsonBody // empty)
      ] | map(select(type == "object")) | .[0] // {};
    def response_status:
      response_json | .result.status // "";
    def to_epoch:
      if type == "number" then
        if . > 100000000000 then (. / 1000) else . end
      else
        (tostring | sub("\\+0000$"; "Z") | sub("\\.[0-9]+Z$"; "Z") | fromdateiso8601)
      end;
    def response_text:
      [
        (.response.body // empty),
        (.responseDefinition.body // empty),
        ((.responseDefinition.jsonBody // empty) | tojson?),
        ((.response.jsonBody // empty) | tojson?)
      ] | map(select(. != null)) | join(" ");
    def event_kind:
      response_text as $r
      | response_status as $status
      | if ($status == "TRY_AGAIN_LATER" or ($r | contains("TRY_AGAIN_LATER"))) then
          "tal_reject"
        elif (
          ($status == "ERROR" and (($r | contains("AAAAAAAAY/n////3AAAAAA==")) or ($r | ascii_downcase | contains("insufficient"))))
          or (($r | contains("AAAAAAAAY/n////3AAAAAA==")) or ($r | ascii_downcase | contains("insufficient")))
        ) then
          "if_reject"
        elif $status == "DUPLICATE" then
          "duplicate"
        elif $status == "PENDING" then
          "accepted"
        elif $status == "ERROR" then
          "error"
        else
          "accepted"
        end;
    def expected_rescue_delay($kind; $attempt):
      if $kind == "if_reject" and $attempt <= 6 then
        5 * $attempt
      elif $kind == "tal_reject" and $attempt <= 3 then
        5 * $attempt
      else
        null
      end;
    def percentile($p):
      sort as $s
      | if ($s | length) == 0 then null
        else $s[((($s | length) - 1) * $p / 100 | ceil)]
        end;
    def tx_for_memo($memo; $tx_by_memo):
      if ($memo // "") == "" then {} else ($tx_by_memo[$memo] // {}) end;
    def successful:
      (.status == "confirmed" or .status == "mined");
    def in_list($items; $value):
      ($items | index($value)) != null;

    ($txs[0] // []) as $txs
    | ($txs | map(select((.memo // "") != "") | {key: .memo, value: .}) | from_entries) as $tx_by_memo
    | [ .requests[]
      | select(.request.method == "POST")
      | select(rpc_method == "sendTransaction")
      | tx_xdr as $xdr
      | ($xdr | memo_from_xdr) as $memo
      | tx_for_memo($memo; $tx_by_memo) as $tx
      | {
          t: ((.request.loggedDate // .loggedDate // .loggedDateString) | to_epoch),
          logged_at: (.request.loggedDateString // .loggedDateString // ""),
          xdr: $xdr,
          memo: $memo,
          tx_id: ($tx.tx_id // null),
          relayer_id: ($tx.relayer_id // null),
          response_status: response_status,
          kind: event_kind
        }
    ] | sort_by(.t) as $raw_events
    | [
        ($raw_events | map(select((.memo // "") != "")) | sort_by(.memo // "", .t) | group_by(.memo // "")[])
        | sort_by(.t) as $group
        | range(0; ($group | length)) as $i
        | $group[$i] + {
            send_attempt: ($i + 1),
            rejection_attempt: ([$group[0:($i + 1)][] | select(.kind == "if_reject" or .kind == "tal_reject")] | length)
          }
      ] as $attributed_events
    | ($raw_events | map(select((.memo // "") == ""))) as $unattributed_events
    | (($attributed_events + $unattributed_events) | sort_by(.t)) as $events
    | [ ($events | map(select((.memo // "") != "")) | sort_by(.memo // "", .t) | group_by(.memo // "")[])
        | sort_by(.t) as $group
        | range(0; (($group | length) - 1)) as $i
        | $group[$i] as $event
        | $group[$i + 1] as $next
        | select(($event.kind == "if_reject" or $event.kind == "tal_reject") and (($event.memo // "") != ""))
        | expected_rescue_delay($event.kind; $event.rejection_attempt) as $expected_delay
        | ((($next.t - $event.t) * 1000 | round) / 1000) as $latency_s
        | {
            memo: $event.memo,
            tx_id: $event.tx_id,
            relayer_id: $event.relayer_id,
            kind: $event.kind,
            attempt: $event.rejection_attempt,
            send_attempt: $event.send_attempt,
            rejected_at: $event.logged_at,
            next_at: $next.logged_at,
            next_kind: $next.kind,
            next_response_status: $next.response_status,
            expected_delay_s: $expected_delay,
            latency_s: $latency_s,
            residual_s: (if $expected_delay == null then null else ((($latency_s - $expected_delay) * 1000 | round) / 1000) end),
            residual_scored: ($expected_delay != null),
            exclusion_reason: (
              if $expected_delay != null then null
              elif $event.kind == "tal_reject" and $event.rejection_attempt > 3 then "tal_ladder"
              else "outside_fast_resubmit_cap"
              end
            )
          }
      ] as $rescue_events
    | [ ($events | sort_by(.memo // "") | group_by(.memo // "")[])
        | select((.[0].memo // "") != "")
        | sort_by(.t) as $group
        | range(1; ($group | length)) as $i
        | ($group[$i].t - $group[$i - 1].t) as $gap
        | select($gap < 1)
        | $group[$i - 1] as $previous
        | $group[$i] as $current
        | tx_for_memo($current.memo; $tx_by_memo) as $tx
        | ($previous.xdr == $current.xdr) as $same_envelope
        | ($tx | successful) as $terminal_success
        | (
            $current.kind == "accepted"
            or $current.kind == "duplicate"
            or $current.response_status == "PENDING"
            or $current.response_status == "DUPLICATE"
          ) as $extra_send_accepted
        | {
            memo: $current.memo,
            tx_id: $current.tx_id,
            relayer_id: $current.relayer_id,
            gap_s: (($gap * 1000 | round) / 1000),
            previous_at: $previous.logged_at,
            current_at: $current.logged_at,
            previous_kind: $previous.kind,
            current_kind: $current.kind,
            previous_response_status: $previous.response_status,
            current_response_status: $current.response_status,
            same_envelope: $same_envelope,
            terminal_status: ($tx.status // null),
            terminal_success: $terminal_success,
            extra_send_accepted: $extra_send_accepted,
            classification: (
              if ($same_envelope | not) then "UNSAFE"
              elif $terminal_success then "SAFE"
              elif $extra_send_accepted then "SAFE"
              else "UNSAFE"
              end
            ),
            classification_reason: (
              if ($same_envelope | not) then "divergent_envelopes"
              elif $terminal_success then "terminal_success"
              elif $extra_send_accepted then "extra_send_accepted"
              else "no_terminal_success_or_duplicate_acceptance"
              end
            ),
            previous_xdr: $previous.xdr,
            current_xdr: $current.xdr
          }
      ] as $double_fire_pairs
    | ($rescue_events | map(select(.residual_scored == true))) as $scored_rescues
    | ($scored_rescues | map(.residual_s)) as $residual_values
    | ($events | map(select((.memo // "") != "") | .memo) | unique) as $submitted_memos
    | ($submitted_memos | map({key: ., value: true}) | from_entries) as $submitted_by_memo
    | ($txs | map(select((.tx_id // "") != ""))) as $submitted_txs
    | ($events | map(select(.kind == "if_reject" and ((.memo // "") != "")) | .memo) | unique) as $if_rejected_memos
    | ($events | map(select((.kind == "if_reject" or .kind == "tal_reject") and ((.memo // "") != "")) | .memo) | unique) as $any_rejected_memos
    | ($submitted_txs | map(select(in_list($if_rejected_memos; .memo // "")))) as $if_cohort
    | ($submitted_txs | map(select(in_list($any_rejected_memos; .memo // "") | not))) as $clean_cohort
    | {
        send_events: $events,
        send_calls: ($events | length),
        unique_txs_with_submission: ($submitted_memos | length),
        transactions_without_submission_count: (
          $submitted_txs
          | map(select((($submitted_by_memo[.memo] // false) | not)))
          | length
        ),
        expired_without_any_submission_count: (
          $submitted_txs
          | map(select((.status // "") == "expired" and (($submitted_by_memo[.memo] // false) | not)))
          | length
        ),
        unattributed_send_calls: ($events | map(select((.memo // "") == "")) | length),
        send_distribution_by_relayer: (
          $events
          | sort_by(.relayer_id // "unknown")
          | group_by(.relayer_id // "unknown")
          | map({
              relayer_id: (.[0].relayer_id // "unknown"),
              send_calls: length,
              unique_txs: (map(.memo) | map(select(. != null)) | unique | length)
            })
        ),
        if_rejections: ($events | map(select(.kind == "if_reject")) | length),
        tal_rejections: ($events | map(select(.kind == "tal_reject")) | length),
        if_cohort_count: ($if_cohort | length),
        if_cohort_success_count: ($if_cohort | map(select(successful)) | length),
        if_cohort_success_rate: (
          if ($if_cohort | length) == 0 then null
          else ((($if_cohort | map(select(successful)) | length) * 10000 / ($if_cohort | length)) | round / 100)
          end
        ),
        clean_cohort_count: ($clean_cohort | length),
        clean_cohort_success_count: ($clean_cohort | map(select(successful)) | length),
        clean_cohort_success_rate: (
          if ($clean_cohort | length) == 0 then null
          else ((($clean_cohort | map(select(successful)) | length) * 10000 / ($clean_cohort | length)) | round / 100)
          end
        ),
        rescue_events: $rescue_events,
        rescue_latency_count: ($rescue_events | length),
        rescue_latency_p50_s: ($rescue_events | map(.latency_s) | percentile(50)),
        rescue_latency_p95_s: ($rescue_events | map(.latency_s) | percentile(95)),
        rescue_residual_count: ($scored_rescues | length),
        rescue_residual_p50_s: ($residual_values | percentile(50)),
        rescue_residual_p95_s: ($residual_values | percentile(95)),
        rescue_residual_max_s: ($residual_values | max),
        rescue_residual_values_s: $residual_values,
        rescue_ladder_excluded_count: ($rescue_events | map(select(.exclusion_reason == "tal_ladder")) | length),
        rescue_ladder_excluded_events: ($rescue_events | map(select(.exclusion_reason == "tal_ladder"))),
        double_fire_count: ($double_fire_pairs | length),
        safe_double_fire_count: ($double_fire_pairs | map(select(.classification == "SAFE")) | length),
        unsafe_double_fire_count: ($double_fire_pairs | map(select(.classification == "UNSAFE")) | length),
        double_fire_pairs: $double_fire_pairs
      }' "${SOAK_JOURNAL_FILE}")"

  jq -c '.send_events[]' <<<"${journal_summary}" >"${SOAK_SEND_EVENTS_FILE}"
  jq '.double_fire_pairs' <<<"${journal_summary}" >"${SOAK_DOUBLE_FIRE_FILE}"

  if [[ -s "${SOAK_QUEUE_DEPTH_FILE}" ]]; then
    depth_summary="$(jq -s -c '{max_submission_queue_depth: (map(.depth) | max)}' "${SOAK_QUEUE_DEPTH_FILE}")"
  else
    depth_summary='{"max_submission_queue_depth":null}'
  fi

  jq -n \
    --argjson relayers "${SOAK_RELAYER_COUNT}" \
    --argjson duration "${SOAK_DURATION}" \
    --argjson requested_if_pct "${SOAK_IF_PCT}" \
    --argjson requested_tal_pct "${SOAK_TAL_PCT}" \
    --argjson txbadseq_count "${txbadseq_count}" \
    --argjson status "${status_summary}" \
    --argjson journal "${journal_summary}" \
    --argjson depth "${depth_summary}" \
    --arg double_fire_file "${SOAK_DOUBLE_FIRE_FILE}" \
    --arg send_events_file "${SOAK_SEND_EVENTS_FILE}" '
    ($status.tx_count // 0) as $tx_count
    | ($journal.rescue_residual_p95_s // null) as $residual_p95
    | ($status.eventual_success_rate // 0) as $success_rate
    | ($journal.if_cohort_success_rate // null) as $if_success_rate
    | ($journal.clean_cohort_success_rate // null) as $clean_success_rate
    | ($journal.double_fire_count // 0) as $double_fire_count
    | ($journal.unsafe_double_fire_count // 0) as $unsafe_double_fire_count
    | ($journal.send_calls // 0) as $send_calls
    | ($journal.expired_without_any_submission_count // 0) as $expired_without_submission
    | {
        tx_count: $tx_count,
        relayers: $relayers,
        duration_s: $duration,
        topology: "closed_loop",
        requested_if_pct: $requested_if_pct,
        requested_tal_pct: $requested_tal_pct,
        achieved_if_pct: (if $send_calls == 0 then 0 else ((($journal.if_rejections // 0) * 10000 / $send_calls) | round / 100) end),
        achieved_tal_pct: (if $send_calls == 0 then 0 else ((($journal.tal_rejections // 0) * 10000 / $send_calls) | round / 100) end),
        send_calls: $send_calls,
        unique_txs_with_submission: ($journal.unique_txs_with_submission // 0),
        transactions_without_submission_count: ($journal.transactions_without_submission_count // 0),
        expired_without_any_submission_count: $expired_without_submission,
        unattributed_send_calls: ($journal.unattributed_send_calls // 0),
        if_rejections: ($journal.if_rejections // 0),
        tal_rejections: ($journal.tal_rejections // 0),
        eventual_success_rate: $success_rate,
        success_count: ($status.success_count // 0),
        api_submit_ok_count: ($status.api_submit_ok_count // 0),
        if_cohort_count: ($journal.if_cohort_count // 0),
        if_cohort_success_count: ($journal.if_cohort_success_count // 0),
        if_cohort_success_rate: $if_success_rate,
        clean_cohort_count: ($journal.clean_cohort_count // 0),
        clean_cohort_success_count: ($journal.clean_cohort_success_count // 0),
        clean_cohort_success_rate: $clean_success_rate,
        creation_distribution_by_relayer: ($status.creation_distribution_by_relayer // []),
        send_distribution_by_relayer: ($journal.send_distribution_by_relayer // []),
        status_counts: ($status.status_counts // []),
        failure_reason_counts: ($status.failure_reason_counts // []),
        rescue_latency_count: ($journal.rescue_latency_count // 0),
        rescue_latency_p50_s: ($journal.rescue_latency_p50_s // null),
        rescue_latency_p95_s: ($journal.rescue_latency_p95_s // null),
        rescue_residual_count: ($journal.rescue_residual_count // 0),
        rescue_residual_p50_s: ($journal.rescue_residual_p50_s // null),
        rescue_residual_p95_s: $residual_p95,
        rescue_residual_max_s: ($journal.rescue_residual_max_s // null),
        rescue_residual_values_s: ($journal.rescue_residual_values_s // []),
        rescue_events: ($journal.rescue_events // []),
        rescue_ladder_excluded_count: ($journal.rescue_ladder_excluded_count // 0),
        rescue_ladder_excluded_events: ($journal.rescue_ladder_excluded_events // []),
        double_fire_count: $double_fire_count,
        safe_double_fire_count: ($journal.safe_double_fire_count // 0),
        unsafe_double_fire_count: $unsafe_double_fire_count,
        double_fire_pairs: ($journal.double_fire_pairs // []),
        head_of_line_txbadseq_count: $txbadseq_count,
        convoy_check_txbadseq_eq_0: ($txbadseq_count == 0),
        max_submission_queue_depth: $depth.max_submission_queue_depth,
        send_events_file: $send_events_file,
        double_fire_pairs_file: $double_fire_file,
        pass_criteria: {
          clean_cohort_success_rate_ge_98pct: ($clean_success_rate != null and $clean_success_rate >= 98),
          if_cohort_success_rate_ge_90pct: ($if_success_rate != null and $if_success_rate >= 90),
          rescue_residual_p95_le_10s: ($residual_p95 != null and $residual_p95 <= 10),
          unsafe_double_fire_count_eq_0: ($unsafe_double_fire_count == 0),
          expired_without_any_submission_eq_0: ($expired_without_submission == 0)
        },
        pass: (
          ($clean_success_rate != null and $clean_success_rate >= 98)
          and ($if_success_rate != null and $if_success_rate >= 90)
          and ($residual_p95 != null and $residual_p95 <= 10)
          and ($unsafe_double_fire_count == 0)
          and ($expired_without_submission == 0)
        ),
        notes: ([
          if $residual_p95 == null then "No scored fast-resubmit rejection with a later same-memo send was observable in the WireMock journal." else empty end,
          if $if_success_rate == null then "No insufficient-fee cohort was observable in the WireMock journal." else empty end,
          if $clean_success_rate == null then "No clean cohort was observable in the transaction ledger." else empty end
        ] | join("; "))
      }' >"${SOAK_VERDICT_FILE}"
}

run_soak() {
  local end_epoch relayer_slot worker_slot relayer_id address tx_parts_dir status_parts_dir part_file pid tx_count
  local -a worker_pids

  reset_wiremock
  : >"${SOAK_TX_FILE}"
  : >"${SOAK_STATUS_FILE}"
  : >"${SOAK_QUEUE_DEPTH_FILE}"
  : >"${SOAK_SEND_EVENTS_FILE}"
  printf '[]\n' >"${SOAK_DOUBLE_FIRE_FILE}"
  tx_parts_dir="${ARTIFACT_DIR}/soak-transactions-parts"
  status_parts_dir="${ARTIFACT_DIR}/soak-status-parts"
  rm -rf "${tx_parts_dir}"
  rm -rf "${status_parts_dir}"
  mkdir -p "${tx_parts_dir}"
  mkdir -p "${status_parts_dir}"

  log "starting soak injector: requested IF=${SOAK_IF_PCT}% TAL=${SOAK_TAL_PCT}%"
  soak_injector_loop &
  SOAK_INJECTOR_PID=$!

  sample_submission_queue_depth_loop &
  QUEUE_DEPTH_SAMPLER_PID=$!

  end_epoch=$(( $(date +%s) + SOAK_DURATION ))
  worker_pids=()
  if [[ -n "${SOAK_RATE}" ]]; then
    log "soak: ignoring legacy --rate=${SOAK_RATE}; closed-loop throughput emerges from relayer count"
  fi
  log "soak: closed-loop duration=${SOAK_DURATION}s relayers=${SOAK_RELAYER_COUNT}"
  for ((relayer_slot = 0; relayer_slot < SOAK_RELAYER_COUNT; relayer_slot++)); do
    worker_slot=$((relayer_slot + 1))
    relayer_id="${SOAK_RELAYER_IDS[relayer_slot]}"
    address="${SOAK_RELAYER_ADDRESSES[relayer_slot]}"
    run_soak_relayer_loop \
      "${worker_slot}" \
      "${relayer_id}" \
      "${address}" \
      "${end_epoch}" \
      "${tx_parts_dir}/${worker_slot}.jsonl" \
      "${status_parts_dir}/${worker_slot}.jsonl" &
    worker_pids+=("$!")
  done

  while (( $(date +%s) < end_epoch )); do
    sleep 1
  done

  if [[ -n "${SOAK_INJECTOR_PID}" ]]; then
    log "soak: duration elapsed; stopping rejection injector while workers drain"
    kill "${SOAK_INJECTOR_PID}" >/dev/null 2>&1 || true
    wait "${SOAK_INJECTOR_PID}" >/dev/null 2>&1 || true
    SOAK_INJECTOR_PID=""
  fi

  for pid in "${worker_pids[@]}"; do
    wait "${pid}" || true
  done

  : >"${SOAK_TX_FILE}"
  : >"${SOAK_STATUS_FILE}"
  for ((relayer_slot = 1; relayer_slot <= SOAK_RELAYER_COUNT; relayer_slot++)); do
    part_file="${tx_parts_dir}/${relayer_slot}.jsonl"
    if [[ -s "${part_file}" ]]; then
      cat "${part_file}" >>"${SOAK_TX_FILE}"
    fi
    part_file="${status_parts_dir}/${relayer_slot}.jsonl"
    if [[ -s "${part_file}" ]]; then
      cat "${part_file}" >>"${SOAK_STATUS_FILE}"
    fi
  done
  tx_count="$(wc -l <"${SOAK_TX_FILE}" | tr -d ' ')"

  log "soak: submitted ${tx_count} transactions; closed-loop workers drained"

  if [[ -n "${QUEUE_DEPTH_SAMPLER_PID}" ]]; then
    kill "${QUEUE_DEPTH_SAMPLER_PID}" >/dev/null 2>&1 || true
    wait "${QUEUE_DEPTH_SAMPLER_PID}" >/dev/null 2>&1 || true
    QUEUE_DEPTH_SAMPLER_PID=""
  fi

  write_soak_verdict
  if jq -e '.pass == true' "${SOAK_VERDICT_FILE}" >/dev/null; then
    PASS_COUNT=$((PASS_COUNT + 1))
  else
    FAIL_COUNT=$((FAIL_COUNT + 1))
  fi
}

write_gap_table_for_file() {
  local file="$1"
  local label="$2"
  if [[ ! -f "${file}" ]]; then
    return 0
  fi

  printf '### %s\n\n' "${label}"
  printf 'Verdicts: `%s`\n\n' "${file}"
  printf '| Scenario | Expected gaps (s) | Measured gaps (s) | Skew (s) | Send calls | Final status | Pass | Notes |\n'
  printf '|---|---|---|---|---:|---|---|---|\n'
  jq -r '
    def expected($scenario):
      {
        "if-once": [5],
        "if-escalation": [5,10,15],
        "if-overcap": [5,10,15,20,25,30],
        "tal-window": [5,10,15],
        "tal-window-closes": [5,10,15,10]
      }[$scenario];
    expected(.scenario) as $expected
    | (.gaps_s // []) as $gaps
    | [range(0; ([($gaps | length), ($expected | length)] | min)) as $i
        | (((($gaps[$i] - $expected[$i]) * 10) | round) / 10)
      ] as $skew
    | [
        .scenario,
        ($expected | map(tostring) | join(", ")),
        ($gaps | map(tostring) | join(", ")),
        ($skew | map(tostring) | join(", ")),
        (.send_calls | tostring),
        .final_status,
        (.pass | tostring),
        ((.notes // "") | gsub("\\|"; "\\|"))
      ]
    | "| \(.[0]) | \(.[1]) | \(.[2]) | \(.[3]) | \(.[4]) | \(.[5]) | \(.[6]) | \(.[7]) |"
  ' "${file}"
  printf '\n'
}

write_report() {
  {
    printf '# Stellar fast-resubmit E2E report\n\n'
    printf -- '- Profile: `%s`\n' "${PROFILE}"
    printf -- '- Mode: `%s`\n' "${MODE}"
    if [[ "${PROFILE}" == "channels-parity" ]]; then
      printf -- '- SQS queue type for this run: `%s`\n' "${SQS_QUEUE_TYPE}"
    fi
    printf -- '- Generated at: `%s`\n\n' "$(date -u +%FT%TZ)"

    printf '## Queue and endpoint discovery\n\n'
    if [[ "${PROFILE}" == "channels-parity" ]]; then
      printf 'The SQS backend builds queue URLs from `SQS_QUEUE_URL_PREFIX` plus these queue names: `%s`.\n\n' "${SQS_QUEUE_BASE_NAMES[*]}"
      printf 'For this run the harness started LocalStack community image `%s`, created `%s` queues, and set:\n\n' "${LOCALSTACK_IMAGE}" "${SQS_QUEUE_TYPE}"
      printf -- '- `AWS_ENDPOINT_URL=%s`\n' "${SQS_ENDPOINT_URL}"
      printf -- '- `AWS_ENDPOINT_URL_SQS=%s`\n' "${SQS_ENDPOINT_URL}"
      printf -- '- `SQS_QUEUE_URL_PREFIX=%s`\n' "${SQS_QUEUE_URL_PREFIX_FOR_RELAYER}"
      printf -- '- `SQS_QUEUE_TYPE=%s`\n\n' "${SQS_QUEUE_TYPE}"
      printf 'Standard queues exercise native SQS `DelaySeconds`. FIFO queues exercise the worker-side `target_scheduled_on` visibility-timeout deferral path.\n\n'
      printf 'Matrix gap checks allow up to 8s of late skew to absorb LocalStack FIFO visibility-timeout delivery jitter while still enforcing send counts, retry order, terminal state, and checker handoff.\n\n'
    else
      printf 'Default profile uses Redis as the queue backend and local Redis only for the harness.\n\n'
    fi

    printf '## Harness isolation\n\n'
    printf 'The matrix runs `if-overcap` last because that scenario intentionally exhausts retries without landing on-chain, which can leave the local sequence cache ahead of chain state.\n\n'
    printf '`tal-window-closes` sends an explicit 10-minute `valid_until` in every profile. This is a scenario-scoped exception because the test verifies checker handoff timing; the ladder rescue lands at roughly 110s+ transaction age and otherwise collides with default Stellar envelope validity plus confirmation latency.\n\n'

    printf '## Substitution ledger\n\n'
    if [[ -f "${SUBSTITUTION_REPORT}" ]]; then
      printf 'Full ledger: `%s`\n\n' "${SUBSTITUTION_REPORT}"
      printf 'Key non-parity items: LocalStack replaces AWS SQS, local Docker Redis replaces Elasticache repository storage, Stellar mainnet is changed to testnet, RPC traffic is routed through WireMock, and redacted secrets use local dev values. Prod worker/thread/concurrency/timeout knobs from the sanitized JSON are kept verbatim unless listed in the ledger.\n\n'
    else
      printf 'No channels-parity substitution ledger was generated for this run.\n\n'
    fi

    if [[ "${MODE}" == "soak" ]]; then
      printf '## Remaining prod infidelities\n\n'
      printf -- '- LocalStack replaces AWS SQS and local Docker Redis replaces production repository storage.\n'
      printf -- '- Stellar testnet replaces production network liquidity, ledger cadence, and fee market conditions.\n'
      printf -- '- WireMock injects one-shot rejection responses and then passes through to the real testnet RPC; it does not emulate a sustained congested validator quorum.\n'
      printf -- '- Local generated relayer accounts stand in for production channel accounts, but the soak ordering is production-faithful: one in-flight transaction per account.\n\n'
    fi

    printf '## Matrix verdicts\n\n'
    if [[ "${PROFILE}" == "channels-parity" ]]; then
      write_gap_table_for_file "${ARTIFACT_DIR}/verdicts-parity-standard.json" "STANDARD queues"
      write_gap_table_for_file "${ARTIFACT_DIR}/verdicts-parity-fifo.json" "FIFO queues"
    else
      write_gap_table_for_file "${VERDICTS_FILE}" "Default Redis queues"
    fi

    printf '## Soak verdict\n\n'
    if [[ -f "${SOAK_VERDICT_FILE}" ]]; then
      printf 'Verdict: `%s`\n\n' "${SOAK_VERDICT_FILE}"
      printf '```json\n'
      jq '.' "${SOAK_VERDICT_FILE}"
      printf '```\n\n'
      printf 'Interpretation: pass requires clean cohort success at or above 98%%, insufficient-fee cohort success at or above 90%%, p95 rescue residual at or below 10s, zero unsafe same-tx double fires less than 1s apart, and zero expired transactions that never reached `sendTransaction`. Residuals subtract the designed fast-resubmit delay from the observed rescue gap: insufficient-fee attempt `n` expects `5s * n` through the retry cap, TRY_AGAIN_LATER attempts 1-3 expect `5s * n`, and later TRY_AGAIN_LATER rescues are reported as status-check ladder territory. Soak send attribution decodes the generated memo from each WireMock `sendTransaction` envelope and joins it to the tx ledger.\n\n'
      printf '### Cohort breakdown\n\n'
      printf '| Cohort | Transactions | Successes | Success rate |\n'
      printf '|---|---:|---:|---:|\n'
      jq -r '
        [
          ["Clean", (.clean_cohort_count // 0), (.clean_cohort_success_count // 0), (.clean_cohort_success_rate // null)],
          ["Insufficient-fee", (.if_cohort_count // 0), (.if_cohort_success_count // 0), (.if_cohort_success_rate // null)]
        ][]
        | "| \(.[0]) | \(.[1]) | \(.[2]) | \(if .[3] == null then "n/a" else ((.[3] | tostring) + "%") end) |"
      ' "${SOAK_VERDICT_FILE}"
      printf '\n'
      printf '### Rescue residuals\n\n'
      jq -r '
        "- Scored residuals: `\(.rescue_residual_count // 0)`; p50=`\(.rescue_residual_p50_s // "n/a")s`, p95=`\(.rescue_residual_p95_s // "n/a")s`, max=`\(.rescue_residual_max_s // "n/a")s`.\n" +
        "- Raw residual values: `\((.rescue_residual_values_s // []) | @json)`.\n" +
        "- TAL ladder exclusions: `\(.rescue_ladder_excluded_count // 0)`."
      ' "${SOAK_VERDICT_FILE}"
      printf '\n\n'
      printf '| Memo | Kind | Attempt | Observed gap | Expected delay | Residual | Scored | Next kind | Exclusion |\n'
      printf '|---|---|---:|---:|---:|---:|---|---|---|\n'
      jq -r '
        def seconds($v):
          if $v == null then "n/a" else (($v | tostring) + "s") end;
        (.rescue_events // [])
        | sort_by(.memo, .attempt, .rejected_at)
        | .[]
        | [
            (.memo // ""),
            (.kind // ""),
            (.attempt // ""),
            seconds(.latency_s),
            seconds(.expected_delay_s),
            seconds(.residual_s),
            ((.residual_scored // false) | tostring),
            (.next_kind // ""),
            (.exclusion_reason // "")
          ]
        | "| \(.[0]) | \(.[1]) | \(.[2]) | \(.[3]) | \(.[4]) | \(.[5]) | \(.[6]) | \(.[7]) | \(.[8]) |"
      ' "${SOAK_VERDICT_FILE}"
      printf '\n'
      printf '### Convoy check\n\n'
      jq -r '
        "- `head_of_line_txbadseq_count`: `\(.head_of_line_txbadseq_count // 0)`\n" +
        "- `convoy_check_txbadseq_eq_0`: `\(.convoy_check_txbadseq_eq_0 // false)`"
      ' "${SOAK_VERDICT_FILE}"
      printf '\n\n'
      printf '### Double-fire classification\n\n'
      jq -r '
        "- Total pairs: `\(.double_fire_count // 0)`; safe: `\(.safe_double_fire_count // 0)`; unsafe: `\(.unsafe_double_fire_count // 0)`."
      ' "${SOAK_VERDICT_FILE}"
      printf '\n\n'
      if jq -e '(.double_fire_count // 0) > 0' "${SOAK_VERDICT_FILE}" >/dev/null; then
        printf 'Evidence file: `%s`\n\n' "${SOAK_DOUBLE_FIRE_FILE}"
        printf '| Memo | Gap | Classification | Reason | Same envelope | Terminal status | Previous kind | Current kind |\n'
        printf '|---|---:|---|---|---|---|---|---|\n'
        jq -r '
          (.double_fire_pairs // [])
          | sort_by(.memo, .previous_at)
          | .[]
          | [
              (.memo // ""),
              ((.gap_s // "n/a") | tostring),
              (.classification // ""),
              (.classification_reason // ""),
              ((.same_envelope // false) | tostring),
              (.terminal_status // "n/a"),
              (.previous_kind // ""),
              (.current_kind // "")
            ]
          | "| \(.[0]) | \(.[1])s | \(.[2]) | \(.[3]) | \(.[4]) | \(.[5]) | \(.[6]) | \(.[7]) |"
        ' "${SOAK_VERDICT_FILE}"
        printf '\n'
      else
        printf 'No double-fire pairs observed; evidence file: `%s`\n\n' "${SOAK_DOUBLE_FIRE_FILE}"
      fi
      if jq -e '.creation_distribution_by_relayer and .send_distribution_by_relayer' "${SOAK_VERDICT_FILE}" >/dev/null; then
        printf '### Per-relayer distribution\n\n'
        printf '| Relayer | Created txs | sendTransaction calls | Unique txs sent |\n'
        printf '|---|---:|---:|---:|\n'
        jq -r '
          (.creation_distribution_by_relayer // []) as $created
          | (.send_distribution_by_relayer // []) as $sent
          | ((($created | map(.relayer_id)) + ($sent | map(.relayer_id))) | unique | sort)[] as $id
          | ($created | map(select(.relayer_id == $id))[0].tx_count // 0) as $created_count
          | ($sent | map(select(.relayer_id == $id))[0].send_calls // 0) as $send_calls
          | ($sent | map(select(.relayer_id == $id))[0].unique_txs // 0) as $unique_txs
          | "| \($id) | \($created_count) | \($send_calls) | \($unique_txs) |"
        ' "${SOAK_VERDICT_FILE}"
        printf '\n'
      fi
    else
      printf 'No soak verdict exists in this artifact directory yet.\n\n'
    fi

    printf '## Artifacts\n\n'
    printf -- '- Current verdict file: `%s`\n' "${VERDICTS_FILE}"
    printf -- '- STANDARD matrix verdicts: `%s`\n' "${ARTIFACT_DIR}/verdicts-parity-standard.json"
    printf -- '- FIFO matrix verdicts: `%s`\n' "${ARTIFACT_DIR}/verdicts-parity-fifo.json"
    printf -- '- Soak verdict: `%s`\n' "${SOAK_VERDICT_FILE}"
    printf -- '- Relayer log: `%s`\n' "${RELAYER_LOG}"
    printf -- '- Harness log: `%s`\n' "${HARNESS_LOG}"
    printf -- '- WireMock soak journal: `%s`\n' "${SOAK_JOURNAL_FILE}"
    printf -- '- Soak send events: `%s`\n' "${SOAK_SEND_EVENTS_FILE}"
    printf -- '- Soak double-fire evidence: `%s`\n\n' "${SOAK_DOUBLE_FIRE_FILE}"

    printf '## Feature-implicating findings\n\n'
    local any_failure=false
    local finding_files=()
    if [[ "${PROFILE}" == "channels-parity" ]]; then
      finding_files=(
        "${ARTIFACT_DIR}/verdicts-parity-standard.json"
        "${ARTIFACT_DIR}/verdicts-parity-fifo.json"
      )
    elif [[ "${MODE}" == "matrix" ]]; then
      finding_files=("${VERDICTS_FILE}")
    fi
    for file in "${finding_files[@]}"; do
      if [[ -f "${file}" ]] && jq -e 'select(.pass == false)' "${file}" >/dev/null; then
        any_failure=true
        jq -r --arg file "${file}" '"- `\($file)` \(.scenario): \(.notes)"' "${file}" | sed '/: $/d'
      fi
    done
    if [[ -f "${SOAK_VERDICT_FILE}" ]] && ! jq -e '.pass == true' "${SOAK_VERDICT_FILE}" >/dev/null; then
      any_failure=true
      if [[ "${MODE}" == "soak" ]]; then
        jq -r '
          "- Closed-loop convoy check: `head_of_line_txbadseq_count=\(.head_of_line_txbadseq_count // 0)`.\n" +
          "- Clean cohort: `\(.clean_cohort_success_count // 0)/\(.clean_cohort_count // 0)` succeeded (`\(.clean_cohort_success_rate // "n/a")%`).\n" +
          "- IF cohort: `\(.if_cohort_success_count // 0)/\(.if_cohort_count // 0)` succeeded (`\(.if_cohort_success_rate // "n/a")%`).\n" +
          (if (.pass_criteria.rescue_residual_p95_le_10s == false) then
            "- Rescue residual failed: `p95=\(.rescue_residual_p95_s // "n/a")s` exceeds `10s`.\n"
          else "" end) +
          (if (.pass_criteria.unsafe_double_fire_count_eq_0 == false) then
            "- Unsafe double-fire failed: `unsafe_double_fire_count=\(.unsafe_double_fire_count // 0)`; classified evidence is in `\(.double_fire_pairs_file)`.\n"
          else "" end) +
          (if (.pass_criteria.expired_without_any_submission_eq_0 == false) then
            "- Expired without submission failed: `expired_without_any_submission_count=\(.expired_without_any_submission_count // 0)`.\n"
          else "" end)
        ' "${SOAK_VERDICT_FILE}"
      else
        printf -- '- `%s`: soak pass criteria failed.\n' "${SOAK_VERDICT_FILE}"
      fi
    fi
    if [[ "${any_failure}" == "false" ]]; then
      printf 'None observed in the available verdicts.\n'
    fi
  } >"${REPORT_FILE}"
}

main() {
  parse_args "$@"

  local requested="${REQUESTED_SCENARIO}"
  local scenarios=()
  mkdir -p "${ARTIFACT_DIR}"
  : >"${HARNESS_LOG}"
  : >"${RELAYER_LOG}"
  : >"${VERDICTS_FILE}"
  trap cleanup EXIT

  need_cmd docker
  need_cmd curl
  need_cmd jq
  need_cmd cargo
  if [[ "${PROFILE}" == "channels-parity" ]]; then
    need_cmd aws
  fi
  generate_soak_relayer_config

  if [[ "${MODE}" == "matrix" ]]; then
    if [[ "${requested}" == "all" ]]; then
      scenarios=("${ALL_SCENARIOS[@]}")
    else
      local found=false
      for scenario in "${ALL_SCENARIOS[@]}"; do
        if [[ "${scenario}" == "${requested}" ]]; then
          found=true
          scenarios=("${scenario}")
        fi
      done
      if [[ "${found}" != "true" ]]; then
        usage >&2
        exit 64
      fi
    fi
  fi

  start_wiremock
  start_redis
  if [[ "${PROFILE}" == "channels-parity" ]]; then
    start_localstack
    create_sqs_queues
    generate_channels_parity_env
  fi
  start_relayer

  if [[ "${MODE}" == "soak" ]]; then
    discover_soak_relayers
    log "funded ${SOAK_RELAYER_COUNT} soak relayer accounts before measurement"
    restart_relayer_after_soak_funding
    discover_soak_relayers
    log "using ${SOAK_RELAYER_COUNT} soak relayer accounts; ledger=${SOAK_RELAYERS_FILE}"
    run_soak
  else
    local address
    address="$(discover_relayer_address)"
    ensure_funded "${address}"
    log "using relayer address ${address}"

    local scenario
    for scenario in "${scenarios[@]}"; do
      run_scenario "${scenario}" "${address}"
    done
  fi

  write_report
  log "pass=${PASS_COUNT} fail=${FAIL_COUNT}; verdicts=${VERDICTS_FILE}; report=${REPORT_FILE}"
  if (( FAIL_COUNT == 0 )); then
    exit 0
  fi
  exit 1
}

main "$@"
