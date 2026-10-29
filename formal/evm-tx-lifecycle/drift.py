#!/usr/bin/env python3
"""Tell whether the Rust code that EvmTxLifecycle.tla models has changed.

Each anchor is a function the model represents.  `check` hashes each
function's current source and compares with anchors.lock; any difference
means the model may no longer match the code and needs another look.
`update` rewrites anchors.lock after the model has been brought in step.

    python3 drift.py check     # exit 1 if any modeled function changed
    python3 drift.py update    # record the current fingerprints
"""
import hashlib
import json
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
LOCK = HERE / "anchors.lock"

# (file, function name, which model actions depend on it)
ANCHORS = [
    ("src/domain/transaction/evm/evm_transaction.rs", "prepare_transaction",
     "PrepStart PrepAlloc PrepPresign PrepSign PrepPost"),
    ("src/domain/transaction/evm/evm_transaction.rs", "submit_transaction",
     "SubStart SubSend SubMark"),
    ("src/domain/transaction/evm/evm_transaction.rs", "resubmit_transaction",
     "ResStart ResSign ResSend ResWrite"),
    ("src/domain/transaction/evm/evm_transaction.rs", "cancel_transaction",
     "CanRead CanPend CanNoop"),
    ("src/domain/transaction/evm/evm_transaction.rs", "update_transaction_status",
     "CanPend, status writes"),
    ("src/domain/transaction/evm/status.rs", "handle_status_impl", "TAct"),
    ("src/domain/transaction/evm/status.rs", "check_transaction_status", "TAct (receipt)"),
    ("src/domain/transaction/evm/status.rs", "handle_pending_state", "TAct (pending)"),
    ("src/domain/transaction/evm/status.rs", "handle_sent_state", "TAct (sent)"),
    ("src/domain/transaction/evm/status.rs", "handle_submitted_state", "TAct (submitted)"),
    ("src/domain/transaction/evm/status.rs", "detect_nonce_gap_ahead", "FillGap"),
    ("src/domain/transaction/evm/status.rs", "reconcile_tx_nonce_state", "NAct"),
    ("src/repositories/transaction/transaction_redis.rs", "partial_update",
     "NewRec / Rejected (Lua merge + final guard)"),
    ("src/jobs/handlers/transaction_status_handler.rs", "handle_result",
     "StatusWrite conflict -> tAlive"),
]


def function_source(path: Path, name: str) -> str:
    text = path.read_text()
    m = re.search(rf"^\s*(pub(\([^)]*\))?\s+)?(async\s+)?fn\s+{re.escape(name)}\b", text, re.M)
    if not m:
        raise LookupError(f"fn {name} not found in {path.relative_to(REPO)}")
    start = text.index("{", m.end())
    depth, i = 0, start
    while i < len(text):
        c = text[i]
        if c == "{":
            depth += 1
        elif c == "}":
            depth -= 1
            if depth == 0:
                return text[m.start():i + 1]
        i += 1
    raise LookupError(f"unbalanced braces in fn {name}")


def fingerprints():
    out = {}
    for rel, name, _ in ANCHORS:
        key = f"{rel}::{name}"
        try:
            src = function_source(REPO / rel, name)
            out[key] = hashlib.sha256(src.encode()).hexdigest()[:16]
        except (LookupError, FileNotFoundError) as e:
            out[key] = f"MISSING ({e})"
    return out


def main():
    cmd = sys.argv[1] if len(sys.argv) > 1 else "check"
    now = fingerprints()
    if cmd == "update":
        LOCK.write_text(json.dumps(now, indent=2, sort_keys=True) + "\n")
        print(f"recorded {len(now)} fingerprints in {LOCK.relative_to(REPO)}")
        return 0
    old = json.loads(LOCK.read_text()) if LOCK.exists() else {}
    actions = {f"{r}::{n}": a for r, n, a in ANCHORS}
    changed = [k for k in now if old.get(k) != now[k]]
    if not changed:
        print("drift: none, model anchors match the code")
        return 0
    print("drift: these modeled functions changed since the model was last checked:")
    for k in changed:
        print(f"  {k}  -> model actions: {actions[k]}")
    return 1


if __name__ == "__main__":
    sys.exit(main())
