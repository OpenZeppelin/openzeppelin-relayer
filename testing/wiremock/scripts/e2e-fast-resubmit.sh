#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
WIREMOCK_DIR="${REPO_ROOT}/testing/wiremock"

DEFAULT_ARTIFACT_DIR="${DEFAULT_ARTIFACT_DIR:-/tmp/oz-relayer-fast-resubmit}"
ARTIFACT_DIR="${ARTIFACT_DIR:-${DEFAULT_ARTIFACT_DIR}}"
VERDICTS_FILE="${VERDICTS_FILE:-${ARTIFACT_DIR}/verdicts.json}"
REPORT_FILE="${REPORT_FILE:-${ARTIFACT_DIR}/report.md}"
RELAYER_LOG="${RELAYER_LOG:-${ARTIFACT_DIR}/relayer.log}"
HARNESS_LOG="${HARNESS_LOG:-${ARTIFACT_DIR}/harness.log}"

WIREMOCK_URL="${WIREMOCK_URL:-http://localhost:9090}"
WIREMOCK_PROXY_TARGET="${WIREMOCK_PROXY_TARGET:-https://soroban-testnet.stellar.org}"
RELAYER_ID="${RELAYER_ID:-stellar-fast-resubmit}"
RELAYER_PORT="${RELAYER_PORT:-18080}"
RELAYER_URL="${RELAYER_URL:-http://127.0.0.1:${RELAYER_PORT}}"
RELAYER_CONFIG_DIR="${WIREMOCK_DIR}/e2e"
RELAYER_CONFIG_FILE_NAME="fast-resubmit-config.json"
LOCAL_SIGNER_KEYSTORE="${REPO_ROOT}/testing/wiremock/e2e/local-signer.keystore"

API_KEY="${API_KEY:-multi-threaded-stellar-example-api-key}"
KEYSTORE_PASSPHRASE="${KEYSTORE_PASSPHRASE:-MtStellarRuntime123!}"
WEBHOOK_SIGNING_KEY="${WEBHOOK_SIGNING_KEY:-example-webhook-signing-key}"
RUN_ID="fast-resubmit-$(date +%s)-$$"
REDIS_CONTAINER="${REDIS_CONTAINER:-oz-relayer-e2e-redis-${RUN_ID}}"
REDIS_URL_FOR_RELAYER=""
RELAYER_PID=""
REQUESTED_SCENARIO="all"
PASS_COUNT=0
FAIL_COUNT=0
DYNAMIC_STUB_IDS=()

RUNNER_LINES_BEFORE=2146
README_LINES_BEFORE=261
CHAINED_MAPPING_FILES_BEFORE=9
CHAINED_MAPPING_LINES_BEFORE=276
DELETED_CHAINED_MAPPINGS=(
  testing/wiremock/mappings/stellar/stellar-insufficient-fee-armed-2.json
  testing/wiremock/mappings/stellar/stellar-insufficient-fee-armed-3.json
  testing/wiremock/mappings/stellar/stellar-insufficient-fee-armed-4.json
  testing/wiremock/mappings/stellar/stellar-insufficient-fee-armed-5.json
  testing/wiremock/mappings/stellar/stellar-insufficient-fee-armed-6.json
  testing/wiremock/mappings/stellar/stellar-insufficient-fee-armed-7.json
  testing/wiremock/mappings/stellar/stellar-try-again-later-armed-2.json
  testing/wiremock/mappings/stellar/stellar-try-again-later-armed-3.json
  testing/wiremock/mappings/stellar/stellar-try-again-later-armed-4.json
)

# Keep if-overcap last: it intentionally never lands on-chain and can leave the
# relayer-side sequence cache ahead of chain state.
ALL_SCENARIOS=(if-once if-escalation tal-window tal-window-closes if-overcap)

set_artifact_dir() {
  ARTIFACT_DIR="$1"
  VERDICTS_FILE="${ARTIFACT_DIR}/verdicts.json"
  REPORT_FILE="${ARTIFACT_DIR}/report.md"
  RELAYER_LOG="${ARTIFACT_DIR}/relayer.log"
  HARNESS_LOG="${ARTIFACT_DIR}/harness.log"
}

usage() {
  printf 'usage: %s [-o artifact-dir] <scenario|all>\n' "$0"
  printf 'scenarios: %s\n' "${ALL_SCENARIOS[*]}"
}

parse_args() {
  local requested_set=false
  while (($#)); do
    case "$1" in
      -o|--output-dir)
        shift
        (($#)) || { usage >&2; exit 64; }
        set_artifact_dir "$1"
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
        [[ "${requested_set}" == "false" ]] || { usage >&2; exit 64; }
        REQUESTED_SCENARIO="$1"
        requested_set=true
        ;;
    esac
    shift
  done
}

log() {
  printf '[e2e-fast-resubmit] %s\n' "$*" >&2
  printf '[e2e-fast-resubmit] %s\n' "$*" >>"${HARNESS_LOG}"
}

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || { printf 'missing required command: %s\n' "$1" >&2; exit 127; }
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

write_key_address_tool() {
  local tool_dir="$1"
  mkdir -p "${tool_dir}/src"
  cat >"${tool_dir}/Cargo.toml" <<'EOF'
[package]
name = "stellar-keystore-address"
version = "0.1.0"
edition = "2021"

[dependencies]
ed25519-dalek = { version = "2.2", features = ["pkcs8"] }
oz-keystore = "0.1.4"
stellar-strkey = "0.0.14"
EOF
  cat >"${tool_dir}/src/main.rs" <<'EOF'
use std::{env, io, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let keystore = args.next().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "missing keystore path argument")
    })?;
    let passphrase = args.next().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "missing passphrase argument")
    })?;

    let raw_key = oz_keystore::LocalClient::load(PathBuf::from(keystore), passphrase);
    let key_bytes: [u8; 32] = raw_key.try_into().map_err(|raw: Vec<u8>| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("expected a 32-byte Stellar key, got {} bytes", raw.len()),
        )
    })?;
    let signing_key = ed25519_dalek::SigningKey::from_bytes(&key_bytes);
    let public_key = stellar_strkey::ed25519::PublicKey(signing_key.verifying_key().to_bytes());

    println!("{public_key}");
    Ok(())
}
EOF
}

derive_stellar_address() {
  local keystore="$1" tool_dir
  tool_dir="${ARTIFACT_DIR}/stellar-keystore-address"
  write_key_address_tool "${tool_dir}"
  CARGO_TARGET_DIR="${REPO_ROOT}/target" \
    cargo run --quiet --manifest-path "${tool_dir}/Cargo.toml" -- "${keystore}" "${KEYSTORE_PASSPHRASE}" 2>>"${HARNESS_LOG}"
}

bootstrap_local_signer_keystore() {
  local keystore_dir keystore_name address
  if [[ -f "${LOCAL_SIGNER_KEYSTORE}" ]]; then
    log "local signer keystore exists at ${LOCAL_SIGNER_KEYSTORE}; skipping bootstrap generation"
    return 0
  fi

  log "local signer keystore missing; generating ${LOCAL_SIGNER_KEYSTORE}"
  keystore_dir="$(dirname "${LOCAL_SIGNER_KEYSTORE}")"
  keystore_name="$(basename "${LOCAL_SIGNER_KEYSTORE}")"
  (
    cd "${REPO_ROOT}"
    cargo run --quiet --example create_key -- \
      --password "${KEYSTORE_PASSPHRASE}" \
      --output-dir "${keystore_dir}" \
      --filename "${keystore_name}"
  ) >>"${HARNESS_LOG}" 2>&1
  [[ -f "${LOCAL_SIGNER_KEYSTORE}" ]] || { log "keystore generation did not create ${LOCAL_SIGNER_KEYSTORE}"; return 1; }

  if ! address="$(derive_stellar_address "${LOCAL_SIGNER_KEYSTORE}")"; then
    log "failed to derive Stellar address from generated keystore"
    return 1
  fi
  [[ "${address}" == G* && "${#address}" -eq 56 ]] || { log "failed to derive Stellar address from generated keystore"; return 1; }
  ensure_funded "${address}"
  log "bootstrapped local signer keystore for ${address}"
}

delete_dynamic_stubs() {
  local id
  if ((${#DYNAMIC_STUB_IDS[@]} > 0)); then
    for id in "${DYNAMIC_STUB_IDS[@]}"; do
      curl -fsS -X DELETE "${WIREMOCK_URL}/__admin/mappings/${id}" >/dev/null 2>&1 || true
    done
  fi
  DYNAMIC_STUB_IDS=()
}

cleanup() {
  local exit_code=$?
  delete_dynamic_stubs
  if [[ -n "${RELAYER_PID}" ]] && kill -0 "${RELAYER_PID}" >/dev/null 2>&1; then
    log "stopping relayer pid ${RELAYER_PID}"
    kill "${RELAYER_PID}" >/dev/null 2>&1 || true
    wait "${RELAYER_PID}" >/dev/null 2>&1 || true
  fi
  if docker ps -a --format '{{.Names}}' | grep -qx "${REDIS_CONTAINER}"; then
    log "stopping redis container ${REDIS_CONTAINER}"
    docker rm -f "${REDIS_CONTAINER}" >/dev/null 2>&1 || true
  fi
  log "stopping WireMock compose stack"
  # shellcheck disable=SC2086
  $(compose_cmd) -f "${WIREMOCK_DIR}/docker-compose.yaml" down >/dev/null 2>&1 || true
  exit "${exit_code}"
}

wait_for_http() {
  local url="$1" label="$2" timeout_s="${3:-60}"
  local deadline=$(( $(date +%s) + timeout_s ))
  until curl -fsS "${url}" >/dev/null 2>&1; do
    (( $(date +%s) < deadline )) || { log "timed out waiting for ${label} at ${url}"; return 1; }
    sleep 1
  done
}

start_wiremock() {
  log "starting WireMock on ${WIREMOCK_URL}"
  # shellcheck disable=SC2086
  WIREMOCK_PORT=9090 WIREMOCK_PROXY_TARGET="${WIREMOCK_PROXY_TARGET}" \
    $(compose_cmd) -f "${WIREMOCK_DIR}/docker-compose.yaml" up -d wiremock >>"${HARNESS_LOG}" 2>&1
  wait_for_http "${WIREMOCK_URL}/__admin/health" "WireMock" 90
}

start_redis() {
  local deadline redis_port
  log "starting disposable Redis container ${REDIS_CONTAINER}"
  docker rm -f "${REDIS_CONTAINER}" >/dev/null 2>&1 || true
  docker run -d --name "${REDIS_CONTAINER}" -p 127.0.0.1::6379 redis:bookworm >>"${HARNESS_LOG}" 2>&1
  deadline=$(( $(date +%s) + 60 ))
  until docker exec "${REDIS_CONTAINER}" redis-cli ping 2>/dev/null | grep -q PONG; do
    (( $(date +%s) < deadline )) || { log "timed out waiting for Redis"; return 1; }
    sleep 1
  done
  redis_port="$(docker port "${REDIS_CONTAINER}" 6379/tcp | sed -E 's/.*:([0-9]+)$/\1/')"
  REDIS_URL_FOR_RELAYER="redis://127.0.0.1:${redis_port}"
  log "Redis is ready at ${REDIS_URL_FOR_RELAYER}"
}

start_relayer() {
  log "building relayer binary"
  ( cd "${REPO_ROOT}" && cargo build --bin openzeppelin-relayer ) >>"${RELAYER_LOG}" 2>&1

  log "starting relayer at ${RELAYER_URL}; logs: ${RELAYER_LOG}"
  (
    cd "${REPO_ROOT}"
    env \
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
      "${REPO_ROOT}/target/debug/openzeppelin-relayer"
  ) >>"${RELAYER_LOG}" 2>&1 &
  RELAYER_PID=$!
  printf '%s\n' "${RELAYER_PID}" >"${ARTIFACT_DIR}/relayer.pid"
  wait_for_http "${RELAYER_URL}/api/v1/health" "relayer" 120
}

api_get() {
  curl -sS --connect-timeout 3 --max-time 15 -H "Authorization: Bearer ${API_KEY}" "${RELAYER_URL}$1"
}

api_post_json() {
  local tmp http body
  tmp="$(mktemp)"
  http="$(curl -sS --connect-timeout 3 --max-time 15 -o "${tmp}" -w '%{http_code}' \
    -H "Authorization: Bearer ${API_KEY}" -H "Content-Type: application/json" \
    -X POST "${RELAYER_URL}$1" -d "$2" || true)"
  body="$(<"${tmp}")"
  rm -f "${tmp}"
  [[ "${http}" =~ ^2 ]] || { printf '%s' "${body}"; return 1; }
  printf '%s' "${body}"
}

discover_relayer_address() {
  local body address
  body="$(api_get "/api/v1/relayers/${RELAYER_ID}")"
  address="$(jq -r '.data.address // empty' <<<"${body}")"
  [[ -n "${address}" ]] || { log "failed to discover relayer address from response: ${body}"; return 1; }
  printf '%s' "${address}"
}

ensure_funded() {
  local address="$1" attempt
  if curl -fsS "https://horizon-testnet.stellar.org/accounts/${address}" >/dev/null 2>&1; then
    log "relayer account already funded: ${address}"
    return 0
  fi
  for attempt in 1 2 3 4 5; do
    log "funding relayer account with friendbot: ${address} (attempt ${attempt}/5)"
    curl -fsS "https://friendbot.stellar.org/?addr=${address}" >>"${HARNESS_LOG}" 2>&1 || true
    sleep $((attempt * 2))
    curl -fsS "https://horizon-testnet.stellar.org/accounts/${address}" >/dev/null 2>&1 && return 0
  done
  log "failed to fund relayer account after retries: ${address}"
  return 1
}

reset_wiremock() {
  delete_dynamic_stubs
  curl -fsS -X POST "${WIREMOCK_URL}/__admin/scenarios/reset" >/dev/null
  curl -fsS -X DELETE "${WIREMOCK_URL}/__admin/requests" >/dev/null
}

scenario_meta() {
  case "$1" in
    if-once) printf 'insufficient-fee\tstellar-insufficient-fee\t1\t2\t5\t120\n' ;;
    if-escalation) printf 'insufficient-fee\tstellar-insufficient-fee\t3\t4\t5 10 15\t120\n' ;;
    if-overcap) printf 'insufficient-fee\tstellar-insufficient-fee\t7\t7\t5 10 15 20 25 30\t120\n' ;;
    tal-window) printf 'try-again-later\tstellar-try-again-later\t3\t4\t5 10 15\t120\n' ;;
    tal-window-closes) printf 'try-again-later\tstellar-try-again-later\t4\t5\t5 10 15 10\t300\n' ;;
    *) return 1 ;;
  esac
}

arm_state_for_count() {
  if (( $1 <= 1 )); then printf 'armed'; else printf 'armed-%s' "$1"; fi
}

fast_log_for_kind() {
  case "$1" in
    insufficient-fee) printf 'enqueueing fast resubmit after insufficient fee' ;;
    try-again-later) printf 'enqueueing fast resubmit after TRY_AGAIN_LATER' ;;
  esac
}

register_chained_stubs() {
  local kind="$1" rejection_count="$2" base_file scenario display_name description
  local i next_state mapping response id
  (( rejection_count > 1 )) || return 0
  case "${kind}" in
    insufficient-fee)
      base_file="${WIREMOCK_DIR}/mappings/stellar/stellar-insufficient-fee.json"
      scenario="stellar-insufficient-fee"
      display_name="ERROR (insufficient fee)"
      description="Returns ERROR with tx_insufficient_fee XDR, then decrements to the existing one-shot armed state."
      ;;
    try-again-later)
      base_file="${WIREMOCK_DIR}/mappings/stellar/stellar-try-again-later.json"
      scenario="stellar-try-again-later"
      display_name="TRY_AGAIN_LATER"
      description="Returns TRY_AGAIN_LATER, then decrements to the existing one-shot armed state."
      ;;
  esac
  for ((i = 2; i <= rejection_count; i++)); do
    next_state="armed"
    (( i == 2 )) || next_state="armed-$((i - 1))"
    mapping="$(jq -c --arg name "Stellar sendTransaction -> ${display_name} ${i} of ${i}" \
      --arg required "armed-${i}" --arg next "${next_state}" --arg description "${description}" \
      '.name=$name | .requiredScenarioState=$required | .newScenarioState=$next | .metadata.description=$description' \
      "${base_file}")"
    response="$(curl -fsS -X POST "${WIREMOCK_URL}/__admin/mappings" -H 'Content-Type: application/json' -d "${mapping}")"
    id="$(jq -r '.id // empty' <<<"${response}")"
    [[ -n "${id}" ]] || { log "WireMock did not return an id for ${scenario} armed-${i}"; return 1; }
    DYNAMIC_STUB_IDS+=("${id}")
  done
}

arm_scenario() {
  curl -fsS -X PUT "${WIREMOCK_URL}/__admin/scenarios/$1/state" \
    -H 'Content-Type: application/json' \
    -d "$(jq -nc --arg state "$2" '{state:$state}')" >/dev/null
}

send_stats_json() {
  curl -fsS "${WIREMOCK_URL}/__admin/requests" | jq -c --arg memo "${1:-}" '
    def body_json: (.request.bodyAsString // .request.body // "{}") as $b | if ($b|type) == "object" then $b else ($b|try fromjson catch {}) end;
    def tx_xdr: body_json | ((try .params.transaction catch null) // (try .params[0].transaction catch null) // "");
    def to_epoch: if type == "number" then (if . > 100000000000 then ./1000 else . end) else (tostring | sub("\\+0000$";"Z") | sub("\\.[0-9]+Z$";"Z") | fromdateiso8601) end;
    [.requests[]
      | select(.request.method == "POST")
      | select((body_json | .method // "") == "sendTransaction")
      | select($memo == "" or ((try (tx_xdr | @base64d) catch "") | contains($memo)))
      | (.request.loggedDate // .loggedDate // .loggedDateString) | to_epoch
    ] | sort as $t | {send_calls: ($t|length), gaps_s: [range(1; $t|length) as $i | (((($t[$i] - $t[$i-1]) * 10) | round) / 10)]}'
}

submit_fresh_transaction() {
  local destination="$1" scenario="$2" memo valid_until payload body tx_id
  memo="fr-$(date +%s)-${RANDOM}"
  valid_until=""
  [[ "${scenario}" == "tal-window-closes" ]] && valid_until="$(( $(date +%s) + 600 ))"
  payload="$(jq -nc --arg destination "${destination}" --arg memo "${memo}" --arg valid_until "${valid_until}" \
    '{network:"testnet",operations:[{type:"payment",destination:$destination,amount:"1",asset:{type:"native"}}],memo:{type:"text",value:$memo}}
     + if $valid_until == "" then {} else {valid_until:$valid_until} end')"
  body="$(api_post_json "/api/v1/relayers/${RELAYER_ID}/transactions" "${payload}")" || { printf '%s' "${body}"; return 1; }
  tx_id="$(jq -r '.data.id // empty' <<<"${body}")"
  [[ -n "${tx_id}" ]] || { log "${scenario}: missing tx id in response: ${body}"; return 1; }
  jq -nc --arg tx_id "${tx_id}" --arg memo "${memo}" '{tx_id:$tx_id,memo:$memo}'
}

poll_transaction() {
  local tx_id="$1" scenario="$2" timeout_s="$3" body status reason
  local deadline=$(( $(date +%s) + timeout_s ))
  status=""
  reason=""
  while (( $(date +%s) < deadline )); do
    body="$(api_get "/api/v1/relayers/${RELAYER_ID}/transactions/${tx_id}")" || true
    status="$(jq -r '.data.status // empty' <<<"${body}" 2>/dev/null || true)"
    reason="$(jq -r '.data.status_reason // ""' <<<"${body}" 2>/dev/null || true)"
    case "${scenario}:${status}" in
      if-overcap:failed|*:confirmed|*:mined|*:expired|*:canceled|if-once:submitted)
        jq -nc --arg status "${status}" --arg reason "${reason}" '{status:$status,reason:$reason}'
        return 0
        ;;
    esac
    sleep 2
  done
  jq -nc --arg status "${status:-timeout}" --arg reason "${reason}" '{status:$status,reason:$reason}'
}

log_bool_seen() {
  awk -v tx_id="$1" -v needle="$2" 'index($0, tx_id) && index($0, needle) { found=1; exit } END { exit found ? 0 : 1 }' "${RELAYER_LOG}" \
    && printf 'true' || printf 'false'
}

gap_within() {
  awk -v actual="$1" -v expected="$2" -v idx="$3" -v scenario="$4" \
    'BEGIN { if (scenario == "tal-window-closes" && idx == 3) ok = actual >= 10; else ok = actual >= expected - 1 && actual <= expected + 8; exit ok ? 0 : 1 }'
}

append_note() {
  if [[ -n "${notes}" ]]; then notes="${notes}; $1"; else notes="$1"; fi
}

evaluate_pass() {
  local scenario="$1" stats="$2" final_status="$3" status_reason="$4" fast_seen="$5" checker_seen="$6"
  local expected_calls="$7" expected_gaps="$8" send_calls gaps_csv notes pass i min_count
  local gaps=() expected=()
  notes=""
  send_calls="$(jq -r '.send_calls' <<<"${stats}")"
  gaps_csv="$(jq -r '.gaps_s | join(",")' <<<"${stats}")"
  [[ -z "${gaps_csv}" ]] || IFS=',' read -r -a gaps <<<"${gaps_csv}"
  expected=(${expected_gaps})

  [[ "${send_calls}" == "${expected_calls}" ]] || append_note "send_calls expected ${expected_calls} got ${send_calls}"
  [[ "${#gaps[@]}" == "${#expected[@]}" ]] || append_note "gaps expected length ${#expected[@]} got ${#gaps[@]}"
  min_count="${#gaps[@]}"
  (( ${#expected[@]} < min_count )) && min_count="${#expected[@]}"
  for ((i = 0; i < min_count; i++)); do
    gap_within "${gaps[$i]}" "${expected[$i]}" "${i}" "${scenario}" || append_note "gap[${i}] expected ${expected[$i]}s got ${gaps[$i]}s"
  done

  [[ "${fast_seen}" == "true" ]] || append_note "fast log not seen"
  if [[ "${scenario}" == "tal-window-closes" ]]; then
    [[ "${checker_seen}" == "true" ]] || append_note "checker log not seen"
  else
    [[ "${checker_seen}" == "false" ]] || append_note "checker log unexpectedly seen"
  fi

  case "${scenario}" in
    if-overcap)
      [[ "${final_status}" == "failed" ]] || append_note "final_status expected failed got ${final_status}"
      [[ "${status_reason}" == *"insufficient fee retry limit exceeded (6)"* ]] || append_note "status_reason missing retry cap"
      ;;
    if-once)
      [[ "${final_status}" == "confirmed" || "${final_status}" == "submitted" || "${final_status}" == "mined" ]] || append_note "final_status expected confirmed/submitted got ${final_status}"
      ;;
    *)
      [[ "${final_status}" == "confirmed" || "${final_status}" == "mined" ]] || append_note "final_status expected confirmed got ${final_status}"
      ;;
  esac

  [[ -z "${notes}" ]] && pass=true || pass=false
  jq -nc --argjson pass "${pass}" --arg notes "${notes}" '{pass:$pass,notes:$notes}'
}

emit_verdict() {
  local scenario="$1" stats="$2" final_status="$3" fast_seen="$4" checker_seen="$5" pass="$6" notes="$7" line
  line="$(jq -nc --arg scenario "${scenario}" --argjson send_calls "$(jq '.send_calls' <<<"${stats}")" \
    --argjson gaps_s "$(jq -c '.gaps_s' <<<"${stats}")" --arg final_status "${final_status}" \
    --argjson fast_log_seen "${fast_seen}" --argjson checker_log_seen "${checker_seen}" \
    --argjson pass "${pass}" --arg notes "${notes}" \
    '{scenario:$scenario,send_calls:$send_calls,gaps_s:$gaps_s,final_status:$final_status,fast_log_seen:$fast_log_seen,checker_log_seen:$checker_log_seen,pass:$pass,notes:$notes}')"
  printf '%s\n' "${line}"
  printf '%s\n' "${line}" >>"${VERDICTS_FILE}"
}

run_scenario() {
  local scenario="$1" address="$2" kind rpc_scenario rejection_count expected_calls expected_gaps timeout_s
  local arm_state submitted tx_id memo poll stats final_status status_reason fast_seen checker_seen eval pass notes
  IFS=$'\t' read -r kind rpc_scenario rejection_count expected_calls expected_gaps timeout_s < <(scenario_meta "${scenario}")

  arm_state="$(arm_state_for_count "${rejection_count}")"
  log "running ${scenario}: arm ${rpc_scenario} -> ${arm_state}"
  reset_wiremock
  register_chained_stubs "${kind}" "${rejection_count}"
  arm_scenario "${rpc_scenario}" "${arm_state}"

  if ! submitted="$(submit_fresh_transaction "${address}" "${scenario}")"; then
    stats='{"send_calls":0,"gaps_s":[]}'
    emit_verdict "${scenario}" "${stats}" "submit_failed" false false false "transaction submit API failed: ${submitted}"
    delete_dynamic_stubs
    FAIL_COUNT=$((FAIL_COUNT + 1))
    return 0
  fi

  tx_id="$(jq -r '.tx_id // empty' <<<"${submitted}")"
  memo="$(jq -r '.memo // empty' <<<"${submitted}")"
  if [[ -z "${tx_id}" || -z "${memo}" ]]; then
    stats='{"send_calls":0,"gaps_s":[]}'
    emit_verdict "${scenario}" "${stats}" "submit_failed" false false false "transaction submit API returned malformed body: ${submitted}"
    delete_dynamic_stubs
    FAIL_COUNT=$((FAIL_COUNT + 1))
    return 0
  fi
  log "${scenario}: tx_id=${tx_id} memo=${memo}"

  poll="$(poll_transaction "${tx_id}" "${scenario}" "${timeout_s}")"
  final_status="$(jq -r '.status' <<<"${poll}")"
  status_reason="$(jq -r '.reason' <<<"${poll}")"
  stats="$(send_stats_json "${memo}")"
  fast_seen="$(log_bool_seen "${tx_id}" "$(fast_log_for_kind "${kind}")")"
  checker_seen="$(log_bool_seen "${tx_id}" "re-enqueueing submit job for stuck Sent transaction")"
  eval="$(evaluate_pass "${scenario}" "${stats}" "${final_status}" "${status_reason}" "${fast_seen}" "${checker_seen}" "${expected_calls}" "${expected_gaps}")"
  pass="$(jq -r '.pass' <<<"${eval}")"
  notes="$(jq -r '.notes' <<<"${eval}")"
  emit_verdict "${scenario}" "${stats}" "${final_status}" "${fast_seen}" "${checker_seen}" "${pass}" "${notes}"
  delete_dynamic_stubs

  if [[ "${pass}" == "true" ]]; then
    PASS_COUNT=$((PASS_COUNT + 1))
  else
    FAIL_COUNT=$((FAIL_COUNT + 1))
    grep -F "${tx_id}" "${RELAYER_LOG}" >"${ARTIFACT_DIR}/${scenario}-${tx_id}.log" || true
    log "${scenario}: failed; tx-scoped log excerpt: ${ARTIFACT_DIR}/${scenario}-${tx_id}.log"
  fi
}

select_scenarios() {
  local requested="$1" scenario
  if [[ "${requested}" == "all" ]]; then
    printf '%s\n' "${ALL_SCENARIOS[@]}"
    return 0
  fi
  for scenario in "${ALL_SCENARIOS[@]}"; do
    [[ "${scenario}" != "${requested}" ]] || { printf '%s\n' "${scenario}"; return 0; }
  done
  usage >&2
  exit 64
}

count_chained_mapping_lines() {
  local file total=0
  while IFS= read -r file; do
    total=$((total + $(wc -l <"${file}" | tr -d ' ')))
  done < <(find "${WIREMOCK_DIR}/mappings/stellar" \( -name 'stellar-insufficient-fee-armed-*.json' -o -name 'stellar-try-again-later-armed-*.json' \))
  printf '%s' "${total}"
}

write_report() {
  local runner_lines_after readme_lines_after chained_files_after chained_lines_after passed total mapping
  runner_lines_after="$(wc -l <"${BASH_SOURCE[0]}" | tr -d ' ')"
  readme_lines_after="$(wc -l <"${WIREMOCK_DIR}/README.md" | tr -d ' ')"
  chained_files_after="$(find "${WIREMOCK_DIR}/mappings/stellar" \( -name 'stellar-insufficient-fee-armed-*.json' -o -name 'stellar-try-again-later-armed-*.json' \) | wc -l | tr -d ' ')"
  chained_lines_after="$(count_chained_mapping_lines)"
  passed="$(jq -s 'map(select(.pass == true)) | length' "${VERDICTS_FILE}")"
  total="$(jq -s 'length' "${VERDICTS_FILE}")"

  {
    printf '# Stellar fast-resubmit E2E report\n\n'
    printf -- '- Generated at: `%s`\n' "$(date -u +%FT%TZ)"
    printf -- '- Artifact directory: `%s`\n' "${ARTIFACT_DIR}"
    printf -- '- Verdicts: `%s`\n' "${VERDICTS_FILE}"
    printf -- '- Relayer log: `%s`\n' "${RELAYER_LOG}"
    printf -- '- Harness log: `%s`\n\n' "${HARNESS_LOG}"
    printf '## Slimming summary\n\n'
    printf '| Item | Before | After |\n|---|---:|---:|\n'
    printf '| Runner lines | %s | %s |\n' "${RUNNER_LINES_BEFORE}" "${runner_lines_after}"
    printf '| README lines | %s | %s |\n' "${README_LINES_BEFORE}" "${readme_lines_after}"
    printf '| Static chained mapping files | %s | %s |\n' "${CHAINED_MAPPING_FILES_BEFORE}" "${chained_files_after}"
    printf '| Static chained mapping lines | %s | %s |\n\n' "${CHAINED_MAPPING_LINES_BEFORE}" "${chained_lines_after}"
    printf 'Deleted static chained mappings; equivalent scenario-state stubs are now registered dynamically and removed between scenarios:\n\n'
    for mapping in "${DELETED_CHAINED_MAPPINGS[@]}"; do printf -- '- `%s`\n' "${mapping}"; done
    printf '\nKept: WireMock proxy, disposable Redis queue backend, Cargo-built relayer, Stellar testnet account funding, five scenario checks, JSONL verdicts, report, and teardown.\n'
    printf '\nDeleted: channels-parity profile, LocalStack/SQS setup, env generation, ledger/substitution reporting, closed-loop soak mode, multi-relayer generation, and residual scoring.\n\n'
    printf '## Matrix verdicts\n\n'
    printf 'Passed `%s/%s` scenarios.\n\n' "${passed}" "${total}"
    printf '```jsonl\n'
    cat "${VERDICTS_FILE}"
    printf '```\n\n'
    printf 'Each verdict has `scenario`, `send_calls`, `gaps_s`, `final_status`, `fast_log_seen`, `checker_log_seen`, `pass`, and `notes`.\n'
  } >"${REPORT_FILE}"
}

main() {
  local scenarios=() scenario address
  parse_args "$@"
  mkdir -p "${ARTIFACT_DIR}"
  : >"${HARNESS_LOG}"
  : >"${RELAYER_LOG}"
  : >"${VERDICTS_FILE}"

  need_cmd docker
  need_cmd curl
  need_cmd jq
  need_cmd cargo

  local selection
  if ! selection="$(select_scenarios "${REQUESTED_SCENARIO}")"; then
    exit 64
  fi
  scenarios=()
  while IFS= read -r scenario; do
    [[ -n "${scenario}" ]] && scenarios+=("${scenario}")
  done <<< "${selection}"
  if (( ${#scenarios[@]} == 0 )); then
    log "error: no scenarios selected for '${REQUESTED_SCENARIO}'" >&2
    exit 64
  fi

  trap cleanup EXIT

  bootstrap_local_signer_keystore

  start_wiremock
  start_redis
  start_relayer

  address="$(discover_relayer_address)"
  ensure_funded "${address}"
  log "using relayer address ${address}"
  for scenario in "${scenarios[@]}"; do run_scenario "${scenario}" "${address}"; done

  write_report
  log "pass=${PASS_COUNT} fail=${FAIL_COUNT}; verdicts=${VERDICTS_FILE}; report=${REPORT_FILE}"
  (( FAIL_COUNT == 0 ))
}

main "$@"
