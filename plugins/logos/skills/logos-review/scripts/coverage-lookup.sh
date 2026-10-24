#!/usr/bin/env bash
#
# coverage-lookup.sh — for each entry of the delta that names a symbol, ask the
# graph whether a test reaches it (FR-IN-15, FR-GV-13).
#
#   coverage-lookup.sh --delta FILE --out FILE [--depth N] [--project DIR]
#
# FILE is the output of review-delta.sh (branch mode) or debt-list.sh (debt mode).
# Per entry the answer is one of:
#
#   covered     a test-marked file depends on the entry's file (`logos affected
#               --tests-only`) AND a caller of the symbol, within N hops (default
#               8, `logos impact`), lives in one of those files
#   file-only   tests depend on the file; none reaches this symbol
#   uncovered   no test file depends on the file
#   unknown     the graph does not know the file or the symbol, or a bare name
#               resolved to another file (no test is ever credited to it)
#   no-symbol   the entry names no single symbol (a chain, a cycle, a file rule)
#
# The symbol is asked by its `node_id` first, then by the bare name it ends in. A
# debt unit whose id is an integer or null is known by its name only: when two
# symbols of that name sit in its file nothing is credited. A group (`clone_group:`,
# `duplicate_group:`) names no single symbol and reads `no-symbol`.
#
# Reach is by test-marked FILES. A unit test inside the offender's own file is not
# seen, so such an entry reads file-only or uncovered, and the gate sends it to
# `test-first`: the safe side.
#
# Exit: 0 looked up (whatever the answers) · 2 usage · 3 environment

set -euo pipefail

case "${BASH_SOURCE[0]}" in
    */*) SCRIPT_DIR="${BASH_SOURCE[0]%/*}" ;;
    *) SCRIPT_DIR=. ;;
esac
# shellcheck source=SCRIPTDIR/lib/common.sh
. "$SCRIPT_DIR/lib/common.sh"

DELTA="" OUT="" DEPTH=8
PROJECT="$(rv_project)"
while [ $# -gt 0 ]; do
    case "$1" in
        --delta) [ $# -ge 2 ] || rv_die 2 "--delta needs a file"; DELTA="$2"; shift 2 ;;
        --out) [ $# -ge 2 ] || rv_die 2 "--out needs a file"; OUT="$2"; shift 2 ;;
        --depth) [ $# -ge 2 ] || rv_die 2 "--depth needs a number"; DEPTH="$2"; shift 2 ;;
        --project) [ $# -ge 2 ] || rv_die 2 "--project needs a directory"; PROJECT="$2"; shift 2 ;;
        *) rv_die 2 "unknown argument '$1' (usage: coverage-lookup.sh --delta FILE --out FILE [--depth N] [--project DIR])" ;;
    esac
done
[ -n "$DELTA" ] && [ -n "$OUT" ] || rv_die 2 "--delta FILE and --out FILE are required"
case "$DEPTH" in '' | *[!0-9]*) rv_die 2 "--depth must be a whole number" ;; esac

rv_need_python
rv_need_logos "$PROJECT"

rv_py coverage --delta "$DELTA" --logos "$LOGOS_BIN" --project "$PROJECT" --depth "$DEPTH" --out "$OUT"
