#!/usr/bin/env bash
#
# trust-gate.sh — the deterministic half of every verdict: trust first, then what
# the branch already did, then test reach (FR-IN-15).
#
#   trust-gate.sh --delta FILE --coverage FILE --out FILE
#
# Reads the delta (review-delta.sh or debt-list.sh) and the coverage answers
# (coverage-lookup.sh). Writes the gated entries as JSON to --out and prints one
# row per entry, tab separated:
#
#   id  proposed  status  dimension/rule  name  file:line  coverage  trust  reason
#
# In order, first match wins:
#
#   trust is not `resolved`             untrusted            (stale, same-file-only,
#                                                             no-resolved-edges, anything else)
#   status resolved or improved         no-action-warranted  (the branch already did it)
#   the entry names no single symbol    no-action-warranted
#   coverage is not `covered`           test-first
#   otherwise                           fix                  (shown `fix?`)
#
# Only `fix?` is open: the model may downgrade it to no-action-warranted with a
# reason (below threshold, cold file, cost exceeds benefit, intentional). It may
# never turn any other row into a fix. A row with no coverage answer reads unknown.
#
# Exit: 0 gated · 2 usage · 3 unreadable input

set -euo pipefail

case "${BASH_SOURCE[0]}" in
    */*) SCRIPT_DIR="${BASH_SOURCE[0]%/*}" ;;
    *) SCRIPT_DIR=. ;;
esac
# shellcheck source=SCRIPTDIR/lib/common.sh
. "$SCRIPT_DIR/lib/common.sh"

DELTA="" COVERAGE="" OUT=""
while [ $# -gt 0 ]; do
    case "$1" in
        --delta) [ $# -ge 2 ] || rv_die 2 "--delta needs a file"; DELTA="$2"; shift 2 ;;
        --coverage) [ $# -ge 2 ] || rv_die 2 "--coverage needs a file"; COVERAGE="$2"; shift 2 ;;
        --out) [ $# -ge 2 ] || rv_die 2 "--out needs a file"; OUT="$2"; shift 2 ;;
        *) rv_die 2 "unknown argument '$1' (usage: trust-gate.sh --delta FILE --coverage FILE --out FILE)" ;;
    esac
done
[ -n "$DELTA" ] && [ -n "$COVERAGE" ] && [ -n "$OUT" ] || rv_die 2 "--delta, --coverage and --out are required"

rv_need_python
rv_py triage --delta "$DELTA" --coverage "$COVERAGE" --out "$OUT"
