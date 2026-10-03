#!/usr/bin/env python3
"""S-502 estate harness: score each estate member with two logos binaries on
identical copies and record the per-member before/after figures.

The estate itself is never touched: every member is exported with
`git -C <member> archive HEAD` (a read of the clone) and extracted twice into
the work directory, once per binary, so neither binary ever runs in the estate
and neither store can be migrated by the other. Members run one at a time
(no parallelism: memory pressure). Results append to `results.jsonl` in the
output directory, one JSON object per member, so a run can be resumed.

Usage:
  measure_estate.py --old-bin logos --new-bin target/release/logos \\
      --estate ~/source/pec-services --work <scratch> --out <dir> \\
      [--members a,b,...] [--limit N]

Exit codes: 0 every selected member measured (some may record an error row);
2 bad arguments; 3 an estate path is not a directory.
"""

import argparse
import json
import os
import shutil
import sqlite3
import subprocess
import sys
import time
from pathlib import Path

FUNCTION, METHOD = 7, 8
TIMEOUT_S = 1800


def members_in_order(estate: Path, only: list[str] | None) -> list[str]:
    """mailbox-manager first, then the Java (src/main/java) members, then the
    rest — each group alphabetical — so the members the CR is about are
    measured first if a run is cut short."""
    names = sorted(
        d.name
        for d in estate.iterdir()
        if d.is_dir() and (d / ".git").exists()
    )
    if only:
        missing = [m for m in only if m not in names]
        if missing:
            sys.exit(f"unknown members: {missing}")
        names = [m for m in names if m in only]

    def key(m: str) -> tuple[int, str]:
        if m == "mailbox-manager":
            return (0, m)
        if (estate / m / "src" / "main" / "java").is_dir():
            return (1, m)
        return (2, m)

    return sorted(names, key=key)


def run(cmd: list[str], cwd: Path) -> tuple[int, str, str, float]:
    t0 = time.time()
    try:
        p = subprocess.run(
            cmd, cwd=cwd, capture_output=True, text=True, timeout=TIMEOUT_S
        )
        return p.returncode, p.stdout, p.stderr, time.time() - t0
    except subprocess.TimeoutExpired:
        return 124, "", f"timeout after {TIMEOUT_S}s", time.time() - t0


def store_facts(db: Path) -> dict:
    """Production is_duplicate count and bodyless share, read-only."""
    conn = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
    try:
        cols = {r[1] for r in conn.execute("PRAGMA table_info(nodes)")}
        prod = (
            f"kind IN ({FUNCTION},{METHOD}) AND derived = 0 "
            "AND COALESCE(is_test, 0) = 0"
        )
        dup = conn.execute(
            f"SELECT COUNT(*) FROM nodes WHERE {prod} AND is_duplicate = 1"
        ).fetchone()[0]
        callables = conn.execute(f"SELECT COUNT(*) FROM nodes WHERE {prod}").fetchone()[0]
        facts = {"is_duplicate": dup, "production_callables": callables}
        if "has_body" in cols:
            bodyless, recorded = conn.execute(
                f"SELECT COALESCE(SUM(has_body = 0), 0), COUNT(has_body) "
                f"FROM nodes WHERE {prod}"
            ).fetchone()
            facts["bodyless"] = bodyless
            facts["has_body_recorded"] = recorded
        return facts
    finally:
        conn.close()


def dim(metrics: dict, name: str):
    v = metrics.get(name)
    if isinstance(v, dict):
        return v.get("normalized")
    return v


def measure(binary: str, root: Path) -> dict:
    subprocess.run(["git", "init", "-q", str(root)], check=True)
    code, _, err, t_index = run([binary, "index"], root)
    if code != 0:
        return {"error": f"index exit {code}: {err.strip()[-400:]}"}
    code, out, err, t_scan = run([binary, "--json", "scan"], root)
    try:
        scan = json.loads(out)
    except json.JSONDecodeError:
        return {"error": f"scan exit {code}: {err.strip()[-400:]}"}
    m = scan.get("metrics") or {}
    w = scan.get("worst_offenders") or {}
    coh = w.get("cohesion") or []
    foc = w.get("focus") or []
    row = {
        "scan_exit": code,
        "signal": scan.get("signal"),
        "redundancy": dim(m, "redundancy"),
        "cohesion": dim(m, "cohesion"),
        "focus": dim(m, "focus"),
        "uniqueness": dim(m, "uniqueness"),
        "function_count": m.get("function_count"),
        "top_cohesion": f"{coh[0]['name']} ({coh[0]['detail']})" if coh else None,
        "top_focus": f"{foc[0]['name']} ({foc[0]['detail']})" if foc else None,
        "top_uniqueness": (
            f"{w['uniqueness'][0]['name']} ({w['uniqueness'][0]['detail']})"
            if w.get("uniqueness")
            else None
        ),
        "god_containers": len(foc),
        "index_s": round(t_index, 1),
        "scan_s": round(t_scan, 1),
    }
    row.update(store_facts(root / ".logos" / "logos.db"))
    return row


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--old-bin", required=True)
    ap.add_argument("--new-bin", required=True)
    ap.add_argument("--estate", required=True, type=Path)
    ap.add_argument("--work", required=True, type=Path)
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--members", help="comma-separated subset")
    ap.add_argument("--limit", type=int, help="stop after N members")
    a = ap.parse_args()
    estate = a.estate.expanduser()
    if not estate.is_dir():
        print(f"estate not a directory: {estate}", file=sys.stderr)
        return 3
    a.work.mkdir(parents=True, exist_ok=True)
    a.out.mkdir(parents=True, exist_ok=True)
    results = a.out / "results.jsonl"
    done = set()
    if results.exists():
        done = {json.loads(l)["member"] for l in results.read_text().splitlines() if l}
    versions = {
        side: subprocess.run([b, "--version"], capture_output=True, text=True).stdout.strip()
        for side, b in (("old", a.old_bin), ("new", a.new_bin))
    }
    (a.out / "versions.json").write_text(json.dumps(versions, indent=2) + "\n")

    order = members_in_order(estate, a.members.split(",") if a.members else None)
    measured = 0
    for m in order:
        if m in done:
            continue
        if a.limit is not None and measured >= a.limit:
            break
        tar = a.work / f"{m}.tar"
        rev = subprocess.run(
            ["git", "-C", str(estate / m), "rev-parse", "--verify", "-q", "HEAD"],
            capture_output=True, text=True,
        )
        head = rev.stdout.strip()
        if rev.returncode != 0:
            # An empty clone has no commit to archive: record it, never guess.
            err = {"error": "no HEAD commit (empty clone) — nothing to measure"}
            row = {"member": m, "head": None, "java": False, "old": err, "new": err}
            with open(results, "a") as fh:
                fh.write(json.dumps(row) + "\n")
            measured += 1
            print(f"[{measured}] {m}: no HEAD commit", flush=True)
            continue
        with open(tar, "wb") as fh:
            subprocess.run(
                ["git", "-C", str(estate / m), "archive", "--format=tar", "HEAD"],
                stdout=fh, check=True,
            )
        row = {"member": m, "head": head,
               "java": (estate / m / "src" / "main" / "java").is_dir()}
        for side, binary in (("old", a.old_bin), ("new", a.new_bin)):
            root = a.work / side / m
            shutil.rmtree(root, ignore_errors=True)
            root.mkdir(parents=True)
            subprocess.run(["tar", "-xf", str(tar), "-C", str(root)], check=True)
            row[side] = measure(binary, root)
            shutil.rmtree(root, ignore_errors=True)
        tar.unlink()
        with open(results, "a") as fh:
            fh.write(json.dumps(row) + "\n")
        measured += 1
        print(f"[{measured}] {m}: old={row['old'].get('signal')} new={row['new'].get('signal')}",
              flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
