#!/usr/bin/env python3
"""Turn a TLC counterexample into a short list of steps in the relayer's terms.

Reads TLC output on stdin.  For each state prints the action (with the code
it models) and only what changed: the tx record, the nonce counter, the
chain, and which workers are busy.  A payload prints as nonce/kind#id.
"""
import re
import sys

STEP_CODE = {
    "PrepStart": "prepare job reads tx",
    "PrepAlloc": "prepare: take nonce (evm_transaction.rs:797)",
    "PrepPresign": "prepare: save nonce (evm_transaction.rs:816)",
    "PrepSign": "prepare: sign (evm_transaction.rs:829)",
    "PrepPost": "prepare: write Sent + hashes, queue submit (evm_transaction.rs:851)",
    "SubStart": "submit job reads tx",
    "SubSend": "submit: send_raw_transaction (evm_transaction.rs:925)",
    "SubMark": "submit: write Submitted (evm_transaction.rs:1025)",
    "ResStart": "resubmit job reads tx",
    "ResSign": "resubmit: reprice + sign (evm_transaction.rs:1106-1135)",
    "ResPersist": "resubmit: save attempt before send (reconciler)",
    "ResSend": "resubmit: send_raw_transaction (evm_transaction.rs:1145)",
    "ResWrite": "resubmit: write network_data + hashes (evm_transaction.rs:1234)",
    "CanStart": "API: cancel called",
    "CanRead": "cancel: read tx",
    "CanPend": "cancel: Pending -> Canceled (evm_transaction.rs:1282)",
    "CanNoop": "cancel: write NOOP + is_canceled, queue resubmit (evm_transaction.rs:1305)",
    "TStart": "status job reads tx",
    "TAct": "status job acts (status.rs:1062)",
    "NStart": "nonce-recovery check reads tx",
    "NAct": "nonce-recovery verdict (status.rs:827)",
    "FillGap": "nonce health fills a gap with a filler NOOP",
    "Mine": "chain mines a payload",
    "Expire": "lease TTL expires while holder still runs",
}


# ---- minimal parser for TLC value syntax ----------------------------------
TOK = re.compile(r'\s*(<<|>>|\|->|:>|@@|[\[\]{}(),]|"[^"]*"|-?\d+|[A-Za-z_][A-Za-z0-9_]*)')


def tokenize(s):
    pos, out = 0, []
    while pos < len(s):
        m = TOK.match(s, pos)
        if not m:
            if s[pos:].strip() == "":
                break
            raise ValueError(f"cannot parse at: {s[pos:pos+40]!r}")
        out.append(m.group(1))
        pos = m.end()
    return out


def parse(tokens, i=0):
    t = tokens[i]
    if t == "[":
        rec, i = {}, i + 1
        while tokens[i] != "]":
            key = tokens[i]
            assert tokens[i + 1] == "|->"
            val, i = parse(tokens, i + 2)
            rec[key] = val
            if tokens[i] == ",":
                i += 1
        return rec, i + 1
    if t in ("{", "<<"):
        close = "}" if t == "{" else ">>"
        items, i = [], i + 1
        while tokens[i] != close:
            val, i = parse(tokens, i)
            items.append(val)
            if tokens[i] == ",":
                i += 1
        return (frozenset(map(freeze, items)) if t == "{" else tuple(items)), i + 1
    if t == "(":  # function: (a :> x @@ b :> y)
        fn, i = {}, i + 1
        while tokens[i] != ")":
            key, i = parse(tokens, i)
            assert tokens[i] == ":>"
            val, i = parse(tokens, i + 1)
            fn[key] = val
            if tokens[i] == "@@":
                i += 1
        return fn, i + 1
    if t.startswith('"'):
        return t[1:-1], i + 1
    if re.fullmatch(r"-?\d+", t):
        return int(t), i + 1
    if t in ("TRUE", "FALSE"):
        return t == "TRUE", i + 1
    return t, i + 1  # model value, e.g. None


def freeze(v):
    if isinstance(v, dict):
        return tuple(sorted((k, freeze(x)) for k, x in v.items()))
    return v


# ---- formatting ------------------------------------------------------------
def pv(p):
    if p in ("None", None):
        return "-"
    d = dict(p) if isinstance(p, tuple) else p
    return f"n{d['n']}/{d['k']}#{d['id']}"


def pset(s):
    return "{" + ", ".join(sorted(pv(x) for x in s)) + "}"


def summary(st):
    rec = st["rec"]
    tx = (f"tx[{rec['st']} nonce={rec['nonce']} ver={rec['ver']} kind={rec['kind']}"
          f"{' canceled' if rec['canceled'] else ''} cur={pv(rec['cur'])} "
          f"hashes={pset(rec['hashes'])}]")
    chain = f"chain[nonce={st['chainNonce']} pool={pset(st['pool'])} mined={pset(st['mined'])}]"
    busy = []
    for slot, ws in sorted(st["w"].items()):
        if ws["pc"] != "idle":
            extra = f" nonce={ws['nn']}" if ws["nn"] != "None" else ""
            extra += f" payload={pv(ws['sig'])}" if ws["sig"] != "None" else ""
            busy.append(f"{slot}:{ws['pc']}{extra}")
    queued = sorted(k for k, v in st["q"].items() if v)
    other = (f"counter={st['counter']} statusJob={'alive' if st['tAlive'] else 'STOPPED'}"
             f"{' cancelAccepted' if st['cancelAccepted'] else ''}"
             f"{' lock=' + st['lock'] if st['lock'] != 'None' else ''}")
    return {"tx": tx, "chain": chain, "workers": "busy " + (", ".join(busy) or "none")
            + (f"; queued {','.join(queued)}" if queued else ""), "other": other}


def main():
    text = sys.stdin.read()
    head = re.search(r"^Error: (Invariant \w+ is violated|Temporal properties were violated)\.?",
                     text, re.M)
    print(head.group(1) if head else "Violation")
    blocks = re.split(r"^State (\d+): <(Initial predicate|\w+)[^>]*>\s*$", text, flags=re.M)
    prev = None
    for k in range(1, len(blocks) - 2, 3):
        num, action, body = blocks[k], blocks[k + 1], blocks[k + 2]
        body = re.split(r"\n\s*\n|\nError:|\nBack to state|\n\d[\d,]* states generated", body)[0]
        vars_ = {}
        for m in re.finditer(r"^/\\ (\w+) = (.*?)(?=^/\\ |\Z)", body, re.M | re.S):
            vars_[m.group(1)] = parse(tokenize(m.group(2)))[0]
        if not vars_:
            continue
        cur = summary(vars_)
        label = "start" if action == "Initial predicate" else STEP_CODE.get(action, action)
        changed = [v for key, v in cur.items() if prev is None or prev[key] != v]
        print(f"{num:>3}. {label}")
        for c in changed:
            print(f"       {c}")
        prev = cur
    loop = re.search(r"^Back to state (\d+)", text, re.M)
    if loop:
        print(f"     ... then loops back to state {loop.group(1)} forever (no final status)")
    elif "Stuttering" in text:
        print("     ... then nothing else happens (stuttering): never reaches a final status")


if __name__ == "__main__":
    main()
