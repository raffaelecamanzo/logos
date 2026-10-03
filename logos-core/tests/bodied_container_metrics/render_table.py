#!/usr/bin/env python3
"""Render the S-502 estate results (measure_estate.py's results.jsonl) as the
markdown before/after table: one row per member, old → new per column.

Usage: render_table.py <results.jsonl>             (per-member table)
       render_table.py --summary <results.jsonl>   (aggregate figures)
Markdown on stdout. Exit codes: 0 rendered; 2 bad arguments; 3 no rows.
"""

import json
import statistics
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


def moved(scored, key):
    """(rose, fell, unchanged) for one dimension over the scored rows."""
    up = sum(1 for r in scored if r["new"][key] is not None and r["old"][key] is not None
             and r["new"][key] > r["old"][key] + 1e-12)
    down = sum(1 for r in scored if r["new"][key] is not None and r["old"][key] is not None
               and r["new"][key] < r["old"][key] - 1e-12)
    return up, down, len(scored) - up - down


def summary(rows) -> None:
    """The aggregate figures the narrative quotes, computed — never typed."""
    errors = [r for r in rows if "error" in r["old"] or "error" in r["new"]]
    ok = [r for r in rows if r not in errors]
    scored = [r for r in ok if r["old"]["signal"] is not None and r["new"]["signal"] is not None]
    deltas = sorted(r["new"]["signal"] - r["old"]["signal"] for r in scored)
    top = max(scored, key=lambda r: r["new"]["signal"] - r["old"]["signal"])
    print(f"- Members: {len(rows)}; scored under both binaries: {len(scored)}; "
          f"empty-graph n/a: {len(ok) - len(scored)}; error rows: {len(errors)}; "
          f"with `src/main/java`: {sum(r['java'] for r in rows)}.")
    print(f"- Signal: rose on {sum(d > 0 for d in deltas)}, unchanged on "
          f"{sum(d == 0 for d in deltas)}, fell on {sum(d < 0 for d in deltas)}; "
          f"median change {statistics.median(deltas):+g}, mean {statistics.mean(deltas):+.0f}, "
          f"largest {deltas[-1]:+d} (`{top['member']}`).")
    for key in ("redundancy", "cohesion", "focus", "uniqueness"):
        up, down, same = moved(scored, key)
        print(f"- {key.capitalize()}: rose on {up}, fell on {down}, unchanged on {same}.")
    print(f"- Production `is_duplicate`: {sum(r['old']['is_duplicate'] for r in ok)} → "
          f"{sum(r['new']['is_duplicate'] for r in ok)}.")
    print(f"- God containers: {sum(r['old']['god_containers'] for r in ok)} → "
          f"{sum(r['new']['god_containers'] for r in ok)}.")
    bodyless = sum(r["new"].get("bodyless", 0) for r in ok)
    recorded = sum(r["new"].get("has_body_recorded", 0) for r in ok)
    print(f"- Bodyless production callables: {bodyless} of {recorded} "
          f"({100 * bodyless / recorded:.1f}%).")
    changed = sum(r["old"]["top_cohesion"] != r["new"]["top_cohesion"] for r in scored)
    print(f"- Top Cohesion offender changed on {changed} members.")


def main() -> int:
    args = sys.argv[1:]
    want_summary = args[:1] == ["--summary"]
    if want_summary:
        args = args[1:]
    if len(args) != 1:
        print(__doc__, file=sys.stderr)
        return 2
    rows = [json.loads(l) for l in open(args[0]) if l.strip()]
    if not rows:
        return 3
    if want_summary:
        summary(rows)
        return 0
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
