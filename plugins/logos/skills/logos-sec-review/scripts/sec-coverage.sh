#!/usr/bin/env bash
#
# sec-coverage.sh — the scanners and the OWASP categories the report must state
# (FR-SC-04, FR-SC-19, BR-71).
#
#   sec-coverage.sh --out FILE [--paths A,B] [--project DIR]
#
# Reads `logos security findings --json` and keeps only what the report needs: the
# stored reports (scanner, version) and the categories no enabled scanner covers.
# Prints the report's `SCANNED` and `COVERAGE` lines ready to paste; --paths names
# the paths the report speaks of (default: the whole project). The findings
# themselves are read and dropped; nothing of them is kept.
#
# Exit: 0 the categories were read · 1 the answer held no categories · 2 usage ·
#       3 no python3, no logos, not enrolled, no answer

set -euo pipefail

case "${BASH_SOURCE[0]}" in
    */*) SCRIPT_DIR="${BASH_SOURCE[0]%/*}" ;;
    *) SCRIPT_DIR=. ;;
esac
# shellcheck source=SCRIPTDIR/lib/secrev.sh
. "$SCRIPT_DIR/lib/secrev.sh"

OUT="" PATHS=""
PROJECT="$(rv_project)"
while [ $# -gt 0 ]; do
    case "$1" in
        --out) [ $# -ge 2 ] || sr_die 2 "--out needs a file"; OUT="$2"; shift 2 ;;
        --paths) [ $# -ge 2 ] || sr_die 2 "--paths needs a list"; PATHS="$2"; shift 2 ;;
        --project) [ $# -ge 2 ] || sr_die 2 "--project needs a directory"; PROJECT="$2"; shift 2 ;;
        *) sr_die 2 "unknown argument '$1' (usage: sec-coverage.sh --out FILE [--paths A,B] [--project DIR])" ;;
    esac
done
[ -n "$OUT" ] || sr_die 2 "--out FILE is required"
case "$OUT" in /*) ;; *) OUT="$PWD/$OUT" ;; esac
[ -d "${OUT%/*}" ] || sr_die 3 "the directory of $OUT does not exist"

rv_need_python
rv_need_logos "$PROJECT"

FINDINGS="$OUT.findings.$$"
trap 'rm -f "$FINDINGS"' EXIT
RC=0
(cd "$PROJECT" && "$LOGOS_BIN" security findings --json >"$FINDINGS" 2>/dev/null) || RC=$?
[ "$RC" -eq 0 ] || sr_die 3 "\`logos security findings --json\` failed with exit $RC and no answer"

sr_py coverage --findings "$FINDINGS" --out "$OUT" --paths "$PATHS"
