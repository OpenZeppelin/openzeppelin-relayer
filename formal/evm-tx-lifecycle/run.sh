#!/usr/bin/env bash
# Model-check one (variant, property, chain) of EvmTxLifecycle.tla with TLC.
#
#   ./run.sh <variant> <property> [chain] [mutation]
#     variant:  main | version | lease | reconciler
#     property: NoDoubleEffect | NoNonceGap | NoLostHash | NoFalseFailure |
#               CancelHonest | PollingNeverStops | EventuallyFinal
#     chain:    mempool (default) | arbitrum
#     mutation: none (default) | nonAtomicClaim | sendBeforePersist |
#               trustLaggingNode | conflictStopsPolling | noCasOnResubmit
#
# Prints PASS or FAIL; on FAIL the counterexample trace follows.
# Exit code: 0 pass, 1 property violated, 2 setup or model error.
set -euo pipefail

TLA_VERSION=1.7.4
TLA_SHA256=936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88

here="$(cd "$(dirname "$0")" && pwd)"
variant=${1:?variant}; prop=${2:?property}; chain=${3:-mempool}; mutation=${4:-none}

jar=${TLA_JAR:-}
if [[ -z "$jar" ]]; then
  for c in "$HOME/.cache/tla/tla2tools-$TLA_VERSION.jar" \
           "$HOME/.cache/guardian-tla/tla2tools-$TLA_VERSION.jar"; do
    [[ -f "$c" ]] && jar=$c && break
  done
fi
if [[ -z "$jar" || ! -f "$jar" ]]; then
  echo "tla2tools $TLA_VERSION not found. Get it with:" >&2
  echo "  mkdir -p ~/.cache/tla && curl -fsSL -o ~/.cache/tla/tla2tools-$TLA_VERSION.jar \\" >&2
  echo "    https://github.com/tlaplus/tlaplus/releases/download/v$TLA_VERSION/tla2tools.jar" >&2
  exit 2
fi
actual=$(shasum -a 256 "$jar" | cut -d' ' -f1)
if [[ "$actual" != "$TLA_SHA256" ]]; then
  echo "checksum mismatch for $jar (got $actual)" >&2; exit 2
fi

# Bounds. Small on purpose: these races need 2 prepares, 1 resubmit,
# 1 cancel and a few signatures to show up.
MaxSigs=${MAX_SIGS:-3}; ErrBudget=${ERR_BUDGET:-1}; LagBudget=${LAG_BUDGET:-1}
ExpireBudget=${EXPIRE_BUDGET:-1}

work=$(mktemp -d "${TMPDIR:-/tmp}/evmtx.XXXXXX")
trap 'rm -rf "$work"' EXIT
cp "$here/EvmTxLifecycle.tla" "$work/"

if [[ "$prop" == "EventuallyFinal" ]]; then check="PROPERTY $prop"; else check="INVARIANT $prop"; fi
spec=Spec; [[ "$variant" == lease ]] && spec=SpecLease
cat > "$work/MC.cfg" <<EOF
SPECIFICATION $spec
CONSTANTS
  Variant = "$variant"
  Chain = "$chain"
  Mutation = "$mutation"
  MaxSigs = $MaxSigs
  ErrBudget = $ErrBudget
  LagBudget = $LagBudget
  ExpireBudget = $ExpireBudget
  None = None
$check
EOF

set +e
out=$(cd "$work" && java -XX:+UseParallelGC -Xss16m -cp "$jar" tlc2.TLC \
        -deadlock -workers auto -cleanup -lncheck final -metadir "$work/states" \
        -config MC.cfg EvmTxLifecycle.tla 2>&1)
set -e

if grep -q "Model checking completed. No error has been found" <<<"$out"; then
  states=$(grep -Eo '[0-9,]+ distinct states found' <<<"$out" | tail -1)
  echo "PASS  $variant $prop $chain mutation=$mutation  ($states)"
  exit 0
elif grep -qE "is violated|Temporal properties were violated" <<<"$out"; then
  echo "FAIL  $variant $prop $chain mutation=$mutation"
  sed -n '/^Error: /,/^[0-9]* states generated/p' <<<"$out"
  exit 1
else
  echo "ERROR $variant $prop $chain mutation=$mutation" >&2
  echo "$out" >&2
  exit 2
fi
