#!/usr/bin/env python3
"""Render the S-502 estate results (measure_estate.py's results.jsonl) as the
markdown before/after table: one row per member, old → new per column.

Usage: render_table.py <results.jsonl>   (markdown on stdout)
Exit codes: 0 rendered; 2 bad arguments; 3 no rows.
"""

import json
import sys


def num(v, places=3):
    if v is None:
        return "n/a"
    if isinstance(v, float):
        return f"{v:.{places}f}"
    return str(v)


def pair(o, n, key, places=3):
    a, b = num(o.get(key), places), num(n.get(key), places)
    return a if a == b else f"{a} → {b}"


def share(side):
    recorded = side.get("has_body_recorded")
    if not recorded:
        return "n/a"
    return f"{100 * side['bodyless'] / recorded:.1f}% ({side['bodyless']}/{recorded})"


def cell(text):
    return (text or "—").replace("|", "\\|")


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__, file=sys.stderr)
        return 2
    rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
    if not rows:
        return 3
    print("| Member | Java | Signal | Redundancy | Cohesion | Focus | `is_duplicate` | "
          "Bodyless share | Top Cohesion offender (old → new) |")
    print("|---|---|---|---|---|---|---|---|---|")
    for r in rows:
        o, n = r["old"], r["new"]
        if "error" in o or "error" in n:
            err = o.get("error") or n.get("error")
            print(f"| {r['member']} | {'yes' if r['java'] else 'no'} | ERROR: {cell(err)} "
                  "| | | | | | |")
            continue
        top_o, top_n = o.get("top_cohesion"), n.get("top_cohesion")
        top = cell(top_o) if top_o == top_n else f"{cell(top_o)} → {cell(top_n)}"
        print(
            f"| {r['member']} | {'yes' if r['java'] else 'no'} "
            f"| {pair(o, n, 'signal')} | {pair(o, n, 'redundancy')} "
            f"| {pair(o, n, 'cohesion')} | {pair(o, n, 'focus')} "
            f"| {pair(o, n, 'is_duplicate')} | {share(n)} | {top} |"
        )
    return 0


if __name__ == "__main__":
    sys.exit(main())
