#!/usr/bin/env bash
#
# debt-list.sh — debt mode: read the "where to start" list instead of the branch
# delta (FR-IN-15, FR-QM-17). On request only; branch mode is the default.
#
#   debt-list.sh --out FILE [--project DIR]
#
# Writes the recorded units (at most ten, ranked by the signal points a fix is
# estimated to recover) in the entry shape coverage-lookup.sh and trust-gate.sh
# read, and prints one summary line. A unit has no trust label of its own, so one
# is derived: `stale` when its file is dirty in the worktree (an edit the graph may
# not hold yet), the cause tag of a language whose calls the graph binds within
# files only, else `resolved`. The points are an estimate; the verdict never rests
# on them and a fix is never proven by them (see remeasure.sh).
#
# A list that is `recorded: false` is "no scan yet", not "clean": exit 1.
#
# Exit: 0 listed · 1 nothing recorded · 2 usage · 3 environment (no python3, no
#       logos, not enrolled, a logos without `improvements`)

set -euo pipefail

case "${BASH_SOURCE[0]}" in
    */*) SCRIPT_DIR="${BASH_SOURCE[0]%/*}" ;;
    *) SCRIPT_DIR=. ;;
esac
# shellcheck source=SCRIPTDIR/lib/common.sh
. "$SCRIPT_DIR/lib/common.sh"

OUT=""
PROJECT="$(rv_project)"
while [ $# -gt 0 ]; do
    case "$1" in
        --out) [ $# -ge 2 ] || rv_die 2 "--out needs a file"; OUT="$2"; shift 2 ;;
        --project) [ $# -ge 2 ] || rv_die 2 "--project needs a directory"; PROJECT="$2"; shift 2 ;;
        *) rv_die 2 "unknown argument '$1' (usage: debt-list.sh --out FILE [--project DIR])" ;;
    esac
done
[ -n "$OUT" ] || rv_die 2 "--out FILE is required"

rv_need_python
rv_need_logos "$PROJECT"

rv_py debt --logos "$LOGOS_BIN" --project "$PROJECT" --out "$OUT"
