#!/usr/bin/env bash
# Run every (variant, property) pair on one chain and print a PASS/FAIL table.
# Counterexamples are summarised into results/<chain>/<variant>.<property>.txt
# (raw TLC output next to them as .raw).
#
#   ./matrix.sh [chain] [variant ...]
#
# Bounds: main/version/lease use MAX_SIGS=2 (their state spaces are 16-35M
# states; short counterexamples show up in seconds, but a property that
# passes needs the full search).  reconciler uses MAX_SIGS=3.
# EventuallyFinal (liveness) runs on reconciler only: on the other variants
# the full graph is too large, and PollingNeverStops covers the liveness bug
# that matters (the status job ending while the tx is not final).  It runs
# with ERR_BUDGET=0 LAG_BUDGET=0 (no send errors, no lagging node) and is
# cut off after LIVE_TIMEOUT seconds (default 2400): with the fault budgets
# on, TLC's final liveness pass over the graph did not finish in 2 hours.
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
chain=${1:-mempool}; shift || true
if [[ $# -gt 0 ]]; then variants=("$@"); else variants=(main version lease reconciler); fi
props=(NoDoubleEffect NoNonceGap NoLostHash NoFalseFailure CancelHonest PollingNeverStops EventuallyFinal)

out="$here/results/$chain"; mkdir -p "$out"
printf '%-11s' variant; printf ' %-14s' "${props[@]}"; echo
for v in "${variants[@]}"; do
  sigs=2; [[ $v == reconciler ]] && sigs=3
  printf '%-11s' "$v"
  for p in "${props[@]}"; do
    if [[ $p == EventuallyFinal && $v != reconciler ]]; then printf ' %-14s' "n/a"; continue; fi
    raw="$out/$v.$p.raw"
    if [[ $p == EventuallyFinal ]]; then
      ERR_BUDGET=0 LAG_BUDGET=0 MAX_SIGS=$sigs timeout "${LIVE_TIMEOUT:-2400}" \
        "$here/run.sh" "$v" "$p" "$chain" > "$raw" 2>&1
    else
      MAX_SIGS=$sigs "$here/run.sh" "$v" "$p" "$chain" > "$raw" 2>&1
    fi
    case $? in
      0) r=pass; rm -f "$raw" "$out/$v.$p.txt" ;;
      1) r=FAIL; python3 "$here/trace.py" < "$raw" > "$out/$v.$p.txt" ;;
      124) r=timeout; pkill -f "tlc2.TLC.*-metadir" || true ;;
      *) r=error ;;
    esac
    printf ' %-14s' "$r"
  done
  echo
done
