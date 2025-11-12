#!/usr/bin/env python3
"""

De-duplicates repeated bullet entries across CHANGELOG versions
Keeps the earliest (lowest) version that mentions a given entry

Usage:
  python3 dedupe_changelog.py ../CHANGELOG.md --in-place
"""

import argparse
import re
import sys
from pathlib import Path

VERSION_REGEX = re.compile(r"^\s*(?:##\s*)?(?:\[(v?\d+\.\d+\.\d+)\]\([^)]+\)|(v?\d+\.\d+\.\d+))(?:\s*\(\d{4}-\d{2}-\d{2}\))?\s*$")

SECTION_REGEX = re.compile(r"^\s*###\s+")
BULLET_REGEX = re.compile(r"^\s*[\*\-]\s+")
TRAILING_PAREN_GROUPS_REGEX = re.compile(r"(?:\s*\((?:#[0-9]+|[0-9a-fA-F]{7,})\))+\s*$")

def parse_semver_tuple(ver: str):
    v = ver.lstrip("vV").strip()
    parts = v.split(".")
    a = []
    for p in parts[:3]:
        m = re.match(r"^(\d+)", p)
        a.append(int(m.group(1)) if m else 0)
    while len(a) < 3:
        a.append(0)
    return tuple(a)

def normalize_bullet(line: str) -> str:
    s = line.strip()
    s = BULLET_REGEX.sub("", s)
    s = TRAILING_PAREN_GROUPS_REGEX.sub("", s)
    s = re.sub(r"\s+", " ", s)
    return s.strip()

def find_version_blocks(lines):
    heads = []
    for i, ln in enumerate(lines):
        m = VERSION_REGEX.match(ln)
        if m:
            ver = m.group(1) or m.group(2)
            heads.append((i, ver))
    blocks = []
    for idx, (start, ver) in enumerate(heads):
        end = heads[idx + 1][0] if idx + 1 < len(heads) else len(lines)
        blocks.append((ver, start, end))
    return blocks

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("path", type=Path)
    ap.add_argument("--in-place", action="store_true", help="Write back to file")
    ap.add_argument("--verbose", action="store_true", help="Print debug info to stderr")
    args = ap.parse_args()

    raw = args.path.read_text(encoding="utf-8")
    lines = raw.splitlines()

    blocks = find_version_blocks(lines)
    if not blocks:
        if args.verbose:
            print("No version headers matched; writing back only if --in-place.", file=sys.stderr)
        if args.in_place:
            args.path.write_text(raw, encoding="utf-8")
        else:
            sys.stdout.write(raw)
        return

    ordered = sorted(blocks, key=lambda b: parse_semver_tuple(b[0]))
    seen = set()
    to_drop = set()

    for ver, start, end in ordered:
        local_seen = set()
        i = start + 1
        while i < end:
            line = lines[i]
            if not line.strip() or SECTION_REGEX.match(line) or VERSION_REGEX.match(line):
                i += 1
                continue
            if BULLET_REGEX.match(line):
                key = normalize_bullet(line)
                if key and (key in seen or key in local_seen):
                    to_drop.add(i)
                else:
                    local_seen.add(key)
            i += 1
        seen |= local_seen

    out_lines = [ln for idx, ln in enumerate(lines) if idx not in to_drop]
    out = "\n".join(out_lines) + ("\n" if raw.endswith("\n") else "")

    if args.in_place:
        args.path.write_text(out, encoding="utf-8")
    else:
        sys.stdout.write(out)

    if args.verbose:
        print(f"Versions detected: {[v for v,_,_ in ordered]}", file=sys.stderr)
        print(f"Removed {len(to_drop)} duplicate bullet(s).", file=sys.stderr)

if __name__ == "__main__":
    main()
